//! 音楽ライブラリ（曲のタイトルとアーティストを先頭 20 件だけ読む）の抽象（Phase 11 の拡充ループ、
//! 対応表 `docs/device/capabilities/music-library.json`、親 #108、Issue #482）。
//!
//! 扱う範囲は「ボタンを押すと端末内の曲のタイトルとアーティストを OS の返す順で先頭
//! [`MAX_TRACKS`] 件だけ読む」ことだけ。再生・アートワーク・アルバム・総件数は扱わない。
//! 対象は macOS（#482）・Windows（#483）・iOS（#484）・Android（#485）。Web は対応表で `no`。
//! `fandhe-egui-proxy` の `device/music_library.rs` がワーカースレッドで
//! [`MusicLibrary::list_tracks`] を呼ぶ（ブロッキングの契約）。全 OS が [`tracks_from`] を通して
//! 整形する。
//!
//! # 契約
//!
//! - 結果は成功（0 曲を含む）か [`MusicLibraryFailure`]。`DeviceError` に variant を足すと既存の
//!   網羅 match が壊れるため機能固有の enum にする（`pedometer::PedometerFailure` と同じ判断）
//! - **0 曲を出すのは、許可済みで OS が空を返したときだけ**。非対応・拒否・期限切れ・失敗を
//!   0 曲へ畳まない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - 期限は [`LIST_TIMEOUT`] が唯一の値で、許可ダイアログを待つ時間を含む。実装は引数で受けた
//!   `timeout` 以内に必ず戻り、`Instant` 等の単調時計で測る（類型 3）
//! - **三値**: 利用者が取り消す UI は無い。許可ダイアログでの拒否は
//!   [`MusicLibraryFailure::PermissionDenied`]、期限切れは [`MusicLibraryFailure::Timeout`] で、
//!   どちらも失敗として表示する（成功にもキャンセルにもしない。類型 10）
//! - 曲名・アーティスト・OS 固有のエラー値をログ・エラー・`Debug` に出さない（類型 8）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内で要求を 1 件ずつ処理すること、0 曲と失敗の区別、嘘の成功を出さないこと、
//! 期限後に資源を残さないこと、曲名・アーティストを件数以外で出力しないこと。
//!
//! 守らない:
//! - OS ごとの並び順の一致（Windows はパスの名前順の深さ優先）
//! - Windows の深さ 8・5000 エントリを超えた先、クラウドのプレースホルダ、タグの無いファイルの補完
//! - iCloud ミュージック・Apple Music 由来の曲が載るか
//! - 読み取った後の変化と複数プロセス
//! - 初回の許可ダイアログに [`LIST_TIMEOUT`] 以内に答えない場合（[`MusicLibraryFailure::Timeout`]）
//! - 期限後も OS 側の処理が走り切ること（結果は捨てる）
//! - macOS での拒否と署名不足の区別（どちらも [`MusicLibraryFailure::Unsupported`]）

use std::time::Duration;

use crate::device_info::clean_text;

/// 読む曲数の上限。
pub const MAX_TRACKS: usize = 20;

/// 一覧取得の期限（Rust 側の唯一の値）。初回の許可ダイアログへの応答時間を含み、
/// 各 OS 実装は別の期限を持たない。
pub const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// 曲 1 件。値は [`clean_text`] 済みで、タグの無い項目は `None`。
#[derive(Clone, PartialEq, Eq)]
pub struct Track {
    /// タイトル。
    pub title: Option<String>,
    /// アーティスト名。
    pub artist: Option<String>,
}

// 曲名・アーティストを出さない。
impl std::fmt::Debug for Track {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Track")
    }
}

/// 読み取った曲の一覧。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct TrackList {
    /// OS の返す順の先頭 [`MAX_TRACKS`] 件まで。
    pub tracks: Vec<Track>,
    /// [`MAX_TRACKS`] を超える曲があって切り捨てたか。
    pub truncated: bool,
}

// 件数と切り捨て有無だけを出す。
impl std::fmt::Debug for TrackList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackList")
            .field("count", &self.tracks.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// 一覧取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MusicLibraryFailure {
    /// 端末に API が無い・OS 版が足りない・署名不足（macOS では拒否と区別できずここに入る）。
    Unsupported,
    /// 利用者または OS 設定がライブラリの読み取りを拒否した。
    PermissionDenied,
    /// 期限内に結果が届かない。0 曲にはしない。
    Timeout,
    /// 上記以外の OS 呼び出し失敗（壊れたバッファを含む）。
    Failed,
}

/// 音楽ライブラリ読み取りの契約（モジュール doc の「契約」節）。
pub trait MusicLibrary: Send + Sync {
    /// 曲を OS の返す順に先頭 [`MAX_TRACKS`] 件まで読む。呼び出し側がワーカースレッドで呼ぶ
    /// ブロッキング呼び出しで、`timeout` 以内に必ず戻る。
    fn list_tracks(&self, timeout: Duration) -> Result<TrackList, MusicLibraryFailure>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedMusicLibrary;

impl MusicLibrary for UnsupportedMusicLibrary {
    fn list_tracks(&self, _timeout: Duration) -> Result<TrackList, MusicLibraryFailure> {
        Err(MusicLibraryFailure::Unsupported)
    }
}

/// OS が返した (タイトル, アーティスト) を整形する。全 OS 実装が通る唯一の経路。
/// 各値は [`clean_text`] を通し、[`MAX_TRACKS`] 件で切り、超過があれば `truncated`。
/// 両方 `None` の行も 1 曲として残す。0 件は空の一覧（正当な結果）。
pub fn tracks_from(raw: impl IntoIterator<Item = (Option<String>, Option<String>)>) -> TrackList {
    let mut tracks = Vec::new();
    let mut truncated = false;
    for (title, artist) in raw {
        if tracks.len() == MAX_TRACKS {
            truncated = true;
            break;
        }
        tracks.push(Track {
            title: title.as_deref().and_then(clean_text),
            artist: artist.as_deref().and_then(clean_text),
        });
    }
    TrackList { tracks, truncated }
}

/// C ABI の戻りコードを失敗へ写す共通表（macOS・iOS の `.m` が共用）。
/// 0 は成功なので `None`、1 = `Unsupported`、2 = `PermissionDenied`、3 = `Timeout`、
/// 4 と未知の値は `Failed`。
pub fn decode_code(code: i32) -> Option<MusicLibraryFailure> {
    match code {
        0 => None,
        1 => Some(MusicLibraryFailure::Unsupported),
        2 => Some(MusicLibraryFailure::PermissionDenied),
        3 => Some(MusicLibraryFailure::Timeout),
        _ => Some(MusicLibraryFailure::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(t: &str, a: &str) -> (Option<String>, Option<String>) {
        (Some(t.to_string()), Some(a.to_string()))
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedMusicLibrary.list_tracks(LIST_TIMEOUT),
            Err(MusicLibraryFailure::Unsupported)
        );
    }

    #[test]
    fn empty_is_ok_and_empty() {
        let l = tracks_from([]);
        assert!(l.tracks.is_empty());
        assert!(!l.truncated);
    }

    #[test]
    fn truncates_after_max() {
        let l = tracks_from((0..MAX_TRACKS).map(|i| row(&format!("t{i}"), "a")));
        assert_eq!(l.tracks.len(), MAX_TRACKS);
        assert!(!l.truncated);
        let l = tracks_from((0..=MAX_TRACKS).map(|i| row(&format!("t{i}"), "a")));
        assert_eq!(l.tracks.len(), MAX_TRACKS);
        assert!(l.truncated);
        assert_eq!(l.tracks[0].title.as_deref(), Some("t0"));
    }

    #[test]
    fn cleans_values_and_keeps_empty_rows() {
        let l = tracks_from([row("  A\u{0}B ", " \t"), (None, None)]);
        assert_eq!(l.tracks[0].title.as_deref(), Some("AB"));
        assert_eq!(l.tracks[0].artist, None);
        assert_eq!(l.tracks.len(), 2);
        assert_eq!(
            l.tracks[1],
            Track {
                title: None,
                artist: None
            }
        );
    }

    #[test]
    fn decode_code_maps_all() {
        assert_eq!(decode_code(0), None);
        assert_eq!(decode_code(1), Some(MusicLibraryFailure::Unsupported));
        assert_eq!(decode_code(2), Some(MusicLibraryFailure::PermissionDenied));
        assert_eq!(decode_code(3), Some(MusicLibraryFailure::Timeout));
        assert_eq!(decode_code(4), Some(MusicLibraryFailure::Failed));
        assert_eq!(decode_code(-1), Some(MusicLibraryFailure::Failed));
        assert_eq!(decode_code(99), Some(MusicLibraryFailure::Failed));
    }

    #[test]
    fn debug_hides_titles() {
        let l = tracks_from([row("SecretSong", "SecretArtist")]);
        let s = format!("{l:?}");
        assert!(!s.contains("Secret"), "{s}");
        assert!(s.contains("count"));
    }
}
