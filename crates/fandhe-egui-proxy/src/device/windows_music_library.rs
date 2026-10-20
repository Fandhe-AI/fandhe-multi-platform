//! Windows 向け音楽ライブラリ実装（Issue #483、親 #108、#482 が骨格）。
//! `device/music_library.rs::implementation` の Windows 経路から呼ばれ、ワーカースレッド上で
//! [`MusicLibrary::list_tracks`] が走る。契約（0 曲と失敗の区別・期限・曲名を出さない）の SSOT は
//! `fandhe_device::music_library`。
//!
//! # 方式
//!
//! `SHGetKnownFolderPath(FOLDERID_Music)`（[`super::windows_photo_library::known_folder`]）で
//! Music フォルダを得て、`std::fs` でパスの名前順に深さ優先で列挙し、音声ファイルの
//! `PKEY_Title` / `PKEY_Music_Artist` をプロパティストア（`SHGetPropertyStoreFromParsingName`）で
//! 読む。WinRT の `KnownFolders.MusicLibrary` は capability `musicLibrary` が要り、
//! unpackaged の exe では宣言できないため使わない。
//!
//! # スレッドと COM
//!
//! UI スレッドは STA なので、MTA の `ComInit` は専用の内側スレッドで作る。プロパティハンドラ等の
//! 1 回の COM 呼び出しは中断できないため、呼び出し側は `recv_timeout` で期限を守り、取り残された
//! スレッドは次のエントリで期限を見て自分で終わる（結果は捨てる。契約の守らない範囲）。
//!
//! # unsafe
//!
//! Windows 限定の `read_tags` に閉じ、各呼び出しに `// SAFETY:` を付ける。`PWSTR` と
//! `PROPVARIANT` は全経路で 1 回ずつ解放する。曲名・パス・HRESULT はログにもエラーにも出さない。
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: root を読めない・Music フォルダを解決できない・COM 初期化に失敗・期限切れを 0 曲にしない、
//! クラウドのプレースホルダ（オフライン・再呼び出し属性）を開かず飛ばす、reparse point の
//! ディレクトリへ降りない、配下の読めないディレクトリは飛ばして続ける。
//! 守らない: 深さ 8・5000 エントリより先、タグの無いファイルのファイル名での補完、
//! OneDrive 等へリダイレクトした環境の同期状態。
//!
//! # 実機で確かめる点（#480）
//!
//! Music フォルダの曲が出るか、OneDrive へリダイレクトした環境でダウンロードが走らないか。

use std::fs;
use std::path::Path;
use std::time::Instant;

use fandhe_device::music_library::{tracks_from, MusicLibraryFailure, TrackList, MAX_TRACKS};

#[cfg(target_os = "windows")]
use std::sync::{mpsc, Arc};
#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(target_os = "windows")]
use fandhe_device::music_library::MusicLibrary;

/// 走査の最大深さ（root を 0 とする）。
const MAX_DEPTH: usize = 8;
/// 見る `DirEntry` の総数の上限（ファイルとディレクトリの両方）。
const MAX_ENTRIES: usize = 5000;
/// 対象の拡張子（大文字小文字は区別しない）。
const AUDIO_EXTS: [&str; 8] = ["mp3", "m4a", "aac", "flac", "wav", "wma", "ogg", "opus"];

#[cfg(target_os = "windows")]
const ATTR_REPARSE_POINT: u32 = 0x400;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// 対象の音声拡張子か。
fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// クラウドのプレースホルダ（開くとダウンロードが走る属性）か。
fn is_placeholder(attrs: u32) -> bool {
    attrs & (ATTR_OFFLINE | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS) != 0
}

type Tags = (Option<String>, Option<String>);

/// 走査の状態。上限に当たったら `stop` を立てて再帰を畳む。
struct Scan<'a, S, R> {
    deadline: Instant,
    max_entries: usize,
    seen: usize,
    stop: bool,
    found: Vec<Tags>,
    skip: &'a S,
    read: &'a mut R,
}

impl<S, R> Scan<'_, S, R>
where
    S: Fn(&fs::DirEntry) -> bool,
    R: FnMut(&Path) -> Tags,
{
    fn walk(&mut self, dir: &Path, depth: usize) -> Result<(), MusicLibraryFailure> {
        let rd = fs::read_dir(dir).map_err(|_| MusicLibraryFailure::Failed)?;
        let mut entries: Vec<fs::DirEntry> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if self.stop {
                return Ok(());
            }
            if Instant::now() >= self.deadline {
                return Err(MusicLibraryFailure::Timeout);
            }
            if self.seen >= self.max_entries {
                self.stop = true;
                return Ok(());
            }
            self.seen += 1;
            if (self.skip)(&entry) {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                if depth < MAX_DEPTH {
                    // 配下の読めないディレクトリ（Failed）は飛ばして続ける。期限だけ伝える。
                    if let Err(MusicLibraryFailure::Timeout) = self.walk(&path, depth + 1) {
                        return Err(MusicLibraryFailure::Timeout);
                    }
                }
            } else if ft.is_file() && is_audio(&path) {
                self.found.push((self.read)(&path));
                if self.found.len() > MAX_TRACKS {
                    self.stop = true;
                }
            }
        }
        Ok(())
    }
}

/// `root` を名前順の深さ優先で走査して曲を集める。root の `read_dir` 失敗は `Failed`
/// （0 曲にしない）、`deadline` 超過は `Timeout`。`skip` が真のエントリは中へ降りず、読まない。
fn scan(
    root: &Path,
    deadline: Instant,
    max_entries: usize,
    skip: impl Fn(&fs::DirEntry) -> bool,
    mut read: impl FnMut(&Path) -> Tags,
) -> Result<TrackList, MusicLibraryFailure> {
    let mut s = Scan {
        deadline,
        max_entries,
        seen: 0,
        stop: false,
        found: Vec::new(),
        skip: &skip,
        read: &mut read,
    };
    s.walk(root, 0)?;
    Ok(tracks_from(s.found))
}

/// プレースホルダ、または降りてはいけない reparse point のディレクトリか。
/// `DirEntry::metadata` は列挙データから属性を得るためファイルを開かない（ハイドレーション回避）。
#[cfg(target_os = "windows")]
fn skip_entry(entry: &fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    let Ok(md) = entry.metadata() else {
        return true;
    };
    let attrs = md.file_attributes();
    is_placeholder(attrs) || (md.is_dir() && attrs & ATTR_REPARSE_POINT != 0)
}

/// プロパティストアから (タイトル, アーティスト) を読む。失敗した項目は `None`。
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn read_tags(path: &Path) -> Tags {
    use windows::core::HSTRING;
    use windows::Win32::Storage::EnhancedStorage::{PKEY_Music_Artist, PKEY_Title};
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Com::StructuredStorage::{
        PropVariantClear, PropVariantToStringAlloc,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{
        IPropertyStore, SHGetPropertyStoreFromParsingName, GPS_DEFAULT,
    };

    let name = HSTRING::from(path.as_os_str());
    // SAFETY: `name` は NUL 終端の有効な UTF-16 で、呼び出しの間だけ生きる。
    let Ok(store) = (unsafe {
        SHGetPropertyStoreFromParsingName::<_, _, IPropertyStore>(&name, None, GPS_DEFAULT)
    }) else {
        return (None, None);
    };
    let get = |key| -> Option<String> {
        // SAFETY: `store` は有効な COM 参照で、キーは静的な定数。
        let mut value = unsafe { store.GetValue(key) }.ok()?;
        // SAFETY: `value` は `GetValue` が初期化した有効な `PROPVARIANT`。成功時の `PWSTR` は
        // 呼び出し側の所有で、変換の成否にかかわらず直後に 1 回だけ `CoTaskMemFree` する。
        let text = unsafe {
            let p = PropVariantToStringAlloc(&value).ok();
            let s = p.and_then(|p| p.to_string().ok());
            if let Some(p) = p {
                CoTaskMemFree(Some(p.0 as *const _));
            }
            s
        };
        // SAFETY: `value` は上で初期化済み。ここで 1 度だけ解放する。
        let _ = unsafe { PropVariantClear(&mut value) };
        text
    };
    (get(&PKEY_Title), get(&PKEY_Music_Artist))
}

#[cfg(target_os = "windows")]
struct WindowsMusicLibrary;

#[cfg(target_os = "windows")]
impl MusicLibrary for WindowsMusicLibrary {
    fn list_tracks(&self, timeout: Duration) -> Result<TrackList, MusicLibraryFailure> {
        // 期限の起点は呼び出しの開始（単調時計）。
        let deadline = Instant::now() + timeout;
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("music-library".into())
            .spawn(move || {
                let result = (|| {
                    let _com = super::windows_notification::ComInit::new("music_library")
                        .map_err(|_| MusicLibraryFailure::Failed)?;
                    let root = super::windows_photo_library::known_folder(
                        &windows::Win32::UI::Shell::FOLDERID_Music,
                    )
                    .ok_or(MusicLibraryFailure::Failed)?;
                    scan(&root, deadline, MAX_ENTRIES, skip_entry, read_tags)
                })();
                let _ = tx.send(result);
            })
            .map_err(|_| MusicLibraryFailure::Failed)?;
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(MusicLibraryFailure::Timeout),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(MusicLibraryFailure::Failed),
        }
    }
}

/// Windows の `MusicLibrary` 実装を返す（`device/music_library.rs` の dispatch から呼ばれる）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn MusicLibrary> {
    Arc::new(WindowsMusicLibrary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("fandhe-ml-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, b"").unwrap();
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }
    fn no_skip(_: &fs::DirEntry) -> bool {
        false
    }
    // タイトルにファイル名を入れて並びを検査する。
    fn name_tag(p: &Path) -> Tags {
        (
            Some(p.file_name().unwrap().to_string_lossy().into_owned()),
            None,
        )
    }
    fn titles(l: &TrackList) -> Vec<String> {
        l.tracks.iter().map(|t| t.title.clone().unwrap()).collect()
    }

    #[test]
    fn audio_ext() {
        assert!(is_audio(Path::new("a.MP3")));
        assert!(is_audio(Path::new("a.Flac")));
        assert!(!is_audio(Path::new("a.txt")));
        assert!(!is_audio(Path::new("mp3")));
    }

    #[test]
    fn placeholder_attrs() {
        assert!(is_placeholder(ATTR_OFFLINE));
        assert!(is_placeholder(ATTR_RECALL_ON_OPEN));
        assert!(is_placeholder(ATTR_RECALL_ON_DATA_ACCESS));
        assert!(!is_placeholder(0x20));
    }

    #[test]
    fn sorted_depth_first_and_only_audio() {
        let t = Tmp::new("order");
        for f in ["b.mp3", "a/z.flac", "a/y.txt", "c.wav"] {
            t.touch(f);
        }
        let mut read_calls = 0;
        let l = scan(&t.0, far(), MAX_ENTRIES, no_skip, |p| {
            read_calls += 1;
            name_tag(p)
        })
        .unwrap();
        assert_eq!(titles(&l), ["z.flac", "b.mp3", "c.wav"]);
        assert_eq!(read_calls, 3);
        assert!(!l.truncated);
    }

    #[test]
    fn truncation_boundary() {
        let t = Tmp::new("trunc");
        for i in 0..MAX_TRACKS {
            t.touch(&format!("{i:02}.mp3"));
        }
        let l = scan(&t.0, far(), MAX_ENTRIES, no_skip, name_tag).unwrap();
        assert_eq!((l.tracks.len(), l.truncated), (MAX_TRACKS, false));
        t.touch("99.mp3");
        let l = scan(&t.0, far(), MAX_ENTRIES, no_skip, name_tag).unwrap();
        assert_eq!((l.tracks.len(), l.truncated), (MAX_TRACKS, true));
    }

    #[test]
    fn empty_ok_and_missing_root_fails() {
        let t = Tmp::new("empty");
        let l = scan(&t.0, far(), MAX_ENTRIES, no_skip, name_tag).unwrap();
        assert!(l.tracks.is_empty() && !l.truncated);
        let r = scan(&t.0.join("nope"), far(), MAX_ENTRIES, no_skip, name_tag);
        assert_eq!(r.unwrap_err(), MusicLibraryFailure::Failed);
    }

    #[test]
    fn depth_limit() {
        let t = Tmp::new("depth");
        let ok = ["d"; MAX_DEPTH].join("/");
        let deep = ["d"; MAX_DEPTH + 1].join("/");
        t.touch(&format!("{ok}/in.mp3"));
        t.touch(&format!("{deep}/out.mp3"));
        let l = scan(&t.0, far(), MAX_ENTRIES, no_skip, name_tag).unwrap();
        assert_eq!(titles(&l), ["in.mp3"]);
    }

    #[test]
    fn entry_limit() {
        let t = Tmp::new("entries");
        for f in ["a.mp3", "b.mp3", "c.mp3"] {
            t.touch(f);
        }
        let l = scan(&t.0, far(), 2, no_skip, name_tag).unwrap();
        assert_eq!(titles(&l), ["a.mp3", "b.mp3"]);
    }

    #[test]
    fn past_deadline_is_timeout() {
        let t = Tmp::new("deadline");
        t.touch("a.mp3");
        let r = scan(&t.0, Instant::now(), MAX_ENTRIES, no_skip, name_tag);
        assert_eq!(r.unwrap_err(), MusicLibraryFailure::Timeout);
    }

    #[test]
    fn skipped_entries_are_not_read_or_entered() {
        let t = Tmp::new("skip");
        for f in ["cloud/a.mp3", "b.mp3", "c.mp3"] {
            t.touch(f);
        }
        let skip = |e: &fs::DirEntry| e.file_name() == "cloud" || e.file_name() == "c.mp3";
        let l = scan(&t.0, far(), MAX_ENTRIES, skip, name_tag).unwrap();
        assert_eq!(titles(&l), ["b.mp3"]);
    }
}
