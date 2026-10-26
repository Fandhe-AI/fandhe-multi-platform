//! ファイル・動画のプレビューとサムネイルの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/file-preview.json`、親 #182、Issue #917）。
//!
//! 扱う範囲は固定のファイル 2 つ（[`FileKind`]: サンプル画像の PNG、種類不明の 64 バイト）と
//! 固定の 3 操作（サンプルのサムネイル・種類不明ファイルのサムネイル・サンプルのプレビュー）だけ。
//! 外から任意のパスを渡す経路は作らない。動画・PDF・複数ファイルは範囲外。対象は macOS / iOS
//! （QuickLook）、Windows（サムネイルのみ）、Android（API 29+ のサムネイルと他アプリへの委譲）。
//! Web は対象外。サムネイルは同期 API で、呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! ワーカースレッドから呼ぶ。プレビューは OS 側が非同期に確定するため trait を通さず、
//! ドライバが各 OS のモジュールを直接呼ぶ。
//!
//! # 契約
//!
//! - **サムネイルの結果**: `Ok(`[`ThumbnailResult::Thumbnail`]`)`（[`check_thumbnail`] を通ったものだけ）、
//!   `Ok(`[`ThumbnailResult::NoThumbnail`]`)`（OS が「サムネイルを作れない」と答えた。失敗にしない）、
//!   `Err(Unsupported("file_preview"))`、期限切れ → `Internal(`[`TIMEOUT`]`)`、
//!   寸法・長さの不正 → `Internal(`[`SIZE`]`)`、未知の結果コード → `Internal(`[`OUTCOME`]`)`、
//!   固定ファイルの書き出し失敗 → `Internal(`[`WRITE`]`)`
//! - **プレビューの結果**: [`PreviewResult`]（閉じた / 他アプリへ渡した / 表示アプリなし /
//!   この OS では対象外）か `Err`。いずれも成功表示に畳まない。`HandedOff` は「渡した」であって
//!   「表示した」ではない
//! - **OS を正にする**: 事前判定（`canPreviewItem:`・拡張子での種類判定）をしない。アイコンを
//!   サムネイルとして成功に混ぜない（QuickLook には `.thumbnail` だけを頼み、返った表現の型も確かめる）。
//!   プレビューを閉じたことは OS の通知だけで知り、推測しない
//! - **結果の三値**（契約表 10）: 閉じた・サムネイルなし・表示アプリなしは失敗の色にしない
//! - **期限**（契約表 3）: サムネイルだけ [`THUMBNAIL_DEADLINE`]（10 秒）。起点はドライバが要求を観測した
//!   単調時計 `Instant`。取り消せる OS は期限で取り消してから戻る。プレビューに期限は無い
//! - **並行性**（契約表 2）: 世代は画面側が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//!
//! # OS 別の到達点
//!
//! | OS | サムネイル | プレビュー |
//! | --- | --- | --- |
//! | macOS | `QLThumbnailGenerator`（#917） | 自前の窓の `QLPreviewView`（#917） |
//! | iOS | 同上を共用（b-ios） | `QLPreviewController`（b-ios） |
//! | Windows | `IShellItemImageFactory`（windows の子） | `TargetUnavailable` |
//! | Android | `ThumbnailUtils`（API 29+。b-android） | `ACTION_VIEW` で他アプリへ（`HandedOff` / `NoHandler`） |
//! | その他 | `Unsupported` | `Unsupported` |
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、OS 資源・一時ファイルの解放。
//! 守らない範囲: サムネイルの色・アルファの正確さと所要時間、期限後も OS 側の生成が走り切ること
//! （Windows は取り消せない）、プレビューを閉じた通知が届かなかったときの復旧、受け手アプリが
//! 実際に表示したか、複数プロセス・複数窓、OS の形式対応の差。パス・画素・OS のエラー文は
//! ログへ出さない。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{DeviceError, Image};

/// 一時ファイルを置くディレクトリ名（`temp_dir()` 配下に固定）。
pub const DIR_NAME: &str = "fandhe-file-preview";
/// サムネイルの長辺の上限（ピクセル）。`fandhe_core::RGBA_MAX_SIDE` 以下に保つ。
pub const THUMBNAIL_MAX_SIDE: u32 = 256;
/// サムネイルの期限（Rust のドライバだけが持つ）。
pub const THUMBNAIL_DEADLINE: Duration = Duration::from_secs(10);

/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "file_preview timeout";
/// 出力の寸法・長さ・引数が不正なときの `Internal` の識別子（SSOT）。
pub const SIZE: &str = "file_preview size";
/// 未知の結果コード・OS の想定外の結果のときの `Internal` の識別子（SSOT）。
pub const OUTCOME: &str = "file_preview outcome";
/// 固定ファイルの書き出しに失敗したときの `Internal` の識別子（SSOT）。
pub const WRITE: &str = "file_preview write";

/// 種類不明のファイルの中身（64 バイト固定）。PNG・JPEG・PDF などのマジックで始まらない。
pub const UNKNOWN_BYTES: [u8; 64] = {
    let mut b = [0u8; 64];
    let mut i = 0;
    while i < 64 {
        b[i] = 0xA0 + (i as u8 % 16);
        i += 1;
    }
    b
};

/// 扱う固定ファイル。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// サンプル画像（PNG）。
    SampleImage,
    /// 種類不明の 64 バイト（`.bin`）。
    Unknown,
}

impl FileKind {
    /// 一時ディレクトリに書くファイル名（固定）。
    pub fn file_name(self) -> &'static str {
        match self {
            FileKind::SampleImage => "fandhe-sample.png",
            FileKind::Unknown => "fandhe-unknown.bin",
        }
    }
}

/// サムネイルの結果。
#[derive(Clone)]
pub enum ThumbnailResult {
    /// [`check_thumbnail`] を通った画像。
    Thumbnail(Image),
    /// OS が「サムネイルを作れない」と答えた（失敗ではない）。
    NoThumbnail,
}

impl std::fmt::Debug for ThumbnailResult {
    /// 画素を出さない（寸法だけ）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThumbnailResult::Thumbnail(i) => write!(f, "Thumbnail({}x{})", i.width, i.height),
            ThumbnailResult::NoThumbnail => f.write_str("NoThumbnail"),
        }
    }
}

/// プレビューの結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewResult {
    /// 利用者がプレビューを閉じた（OS の通知）。
    Closed,
    /// 他アプリへ渡した。表示されたかは分からない。
    HandedOff,
    /// 表示できるアプリが無い。
    NoHandler,
    /// この OS ではプレビューを表示しない（Windows）。
    TargetUnavailable,
}

/// サムネイル生成の契約（モジュール doc の「契約」節）。
pub trait FileThumbnailer: Send + Sync {
    /// `kind` の固定ファイルのサムネイルを長辺 `max_side` 以内で作る。同期でブロックしてよい。
    /// `timeout` は残り時間で、取り消せる OS は期限で取り消してから戻る。
    fn thumbnail(
        &self,
        kind: FileKind,
        bytes: &[u8],
        max_side: u32,
        timeout: Duration,
    ) -> Result<ThumbnailResult, DeviceError>;
}

/// 未実装 OS・対象外 OS のフォールバック。成功へは倒さない。
pub struct UnsupportedFileThumbnailer;

impl FileThumbnailer for UnsupportedFileThumbnailer {
    fn thumbnail(
        &self,
        _: FileKind,
        _: &[u8],
        _: u32,
        _: Duration,
    ) -> Result<ThumbnailResult, DeviceError> {
        Err(DeviceError::Unsupported("file_preview"))
    }
}

/// サンプル画像（640x480 の決定的な RGBA）。四分割の 4 色と対角線で、文字と写真は入れない。
pub fn sample_image() -> Image {
    const W: u32 = 640;
    const H: u32 = 480;
    let mut rgba = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let diagonal = x * H / W == y;
            let px: [u8; 4] = if diagonal {
                [255, 255, 255, 255]
            } else {
                match (x < W / 2, y < H / 2) {
                    (true, true) => [220, 60, 60, 255],
                    (false, true) => [60, 180, 90, 255],
                    (true, false) => [60, 100, 220, 255],
                    (false, false) => [240, 200, 60, 255],
                }
            };
            rgba.extend_from_slice(&px);
        }
    }
    Image {
        width: W,
        height: H,
        rgba,
    }
}

/// `dir` を作り、呼び出しごとに一意な名前（`<pid>-<連番>-<name>`）で排他的に（`create_new`、O_EXCL 相当）
/// 固定ファイルを置く（Apple・Windows で共用）。既存のファイル・シンボリックリンクは追跡も上書きもしない。
/// 置いたファイルの削除は呼び出し側の責務（OS が参照しなくなった時点で `remove_file` する）。
/// 失敗した経路では書きかけを消し `Internal(`[`WRITE`]`)` を返す。
pub fn write_fixed(dir: &Path, kind: FileKind, bytes: &[u8]) -> Result<PathBuf, DeviceError> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let fail = |_: std::io::Error| DeviceError::Internal(WRITE);
    std::fs::create_dir_all(dir).map_err(fail)?;
    let name = format!(
        "{}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        kind.file_name()
    );
    let dest = dir.join(name);
    let result = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&dest)
        .and_then(|mut f| f.write_all(bytes));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&dest);
        return Err(fail(e));
    }
    Ok(dest)
}

/// 成功を判定する唯一の関数。寸法が `1..=`[`THUMBNAIL_MAX_SIDE`] で `rgba.len() == w * h * 4`。
pub fn check_thumbnail(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Result<ThumbnailResult, DeviceError> {
    let in_range = |v: u32| (1..=THUMBNAIL_MAX_SIDE).contains(&v);
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4));
    if !in_range(width) || !in_range(height) || expected != Some(rgba.len()) {
        return Err(DeviceError::Internal(SIZE));
    }
    Ok(ThumbnailResult::Thumbnail(Image {
        width,
        height,
        rgba,
    }))
}

/// Apple（`.m`）のサムネイル結果コードを写す。`Ok(true)` はサムネイルあり（続けて
/// [`check_thumbnail`]）、`Ok(false)` はサムネイルなし。コード表の SSOT:
/// 0 サムネイルあり / 1 非対応（OS が古い）/ 2 サムネイルなし（`GenerationFailed`、または返った表現が
/// `.thumbnail` ではない）/ 3 期限切れ（取り消し済み）/ 4 引数・寸法の不正 /
/// 5 その他の QuickLook エラー・結果なし。未知の値は失敗（嘘の成功の禁止）。
pub fn thumbnail_from_apple_code(code: i32) -> Result<bool, DeviceError> {
    match code {
        0 => Ok(true),
        1 => Err(DeviceError::Unsupported("file_preview")),
        2 => Ok(false),
        3 => Err(DeviceError::Internal(TIMEOUT)),
        4 => Err(DeviceError::Internal(SIZE)),
        _ => Err(DeviceError::Internal(OUTCOME)),
    }
}

/// macOS のプレビュー結果コードを写す。0 = 窓が閉じた。1 = 提示できなかった（`NSApp` が無い・
/// URL や view を作れない）。未知の値は失敗。
pub fn preview_from_macos_code(code: i32) -> Result<PreviewResult, DeviceError> {
    match code {
        0 => Ok(PreviewResult::Closed),
        1 => Err(DeviceError::Internal("file_preview present")),
        _ => Err(DeviceError::Internal(OUTCOME)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_image_is_deterministic_and_four_colored() {
        let a = sample_image();
        assert_eq!(a.rgba, sample_image().rgba);
        assert_eq!((a.width, a.height), (640, 480));
        let px = |x: usize, y: usize| {
            let i = (y * 640 + x) * 4;
            a.rgba[i..i + 4].to_vec()
        };
        assert_eq!(px(5, 200), vec![220, 60, 60, 255]);
        assert_eq!(px(635, 5), vec![60, 180, 90, 255]);
        assert_eq!(px(5, 475), vec![60, 100, 220, 255]);
        assert_eq!(px(635, 475), vec![240, 200, 60, 255]);
    }

    #[test]
    fn unknown_bytes_have_no_known_magic() {
        assert_eq!(UNKNOWN_BYTES.len(), 64);
        for magic in [&b"\x89PNG"[..], b"%PDF", b"\xFF\xD8", b"GIF8", b"PK"] {
            assert!(!UNKNOWN_BYTES.starts_with(magic));
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fandhe-fp-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn write_fixed_is_unique_exclusive_and_ignores_planted_links() {
        let dir = scratch("ok");
        let p1 = write_fixed(&dir, FileKind::Unknown, b"abc").unwrap();
        let p2 = write_fixed(&dir, FileKind::Unknown, b"xyz").unwrap();
        assert_ne!(p1, p2);
        assert_eq!(std::fs::read(&p1).unwrap(), b"abc");
        assert_eq!(std::fs::read(&p2).unwrap(), b"xyz");
        // 旧実装の固定名 tmp にリンクを仕込んでも、リンク先は書き換わらない。
        #[cfg(unix)]
        {
            let victim = dir.join("victim");
            std::fs::write(&victim, b"keep").unwrap();
            let old_tmp = dir.join(format!("{}.tmp", FileKind::Unknown.file_name()));
            std::os::unix::fs::symlink(&victim, &old_tmp).unwrap();
            write_fixed(&dir, FileKind::Unknown, b"zzz").unwrap();
            assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_fixed_failure_is_internal_write() {
        let dir = scratch("fail");
        std::fs::create_dir_all(&dir).unwrap();
        // dir がファイルのとき。
        let file = dir.join("plain");
        std::fs::write(&file, b"x").unwrap();
        let r = write_fixed(&file, FileKind::Unknown, b"abc");
        assert!(matches!(r, Err(DeviceError::Internal(WRITE))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_thumbnail_boundaries() {
        let ok = |w: u32, h: u32, len: usize| check_thumbnail(w, h, vec![0; len]);
        assert!(ok(1, 1, 4).is_ok());
        assert!(ok(256, 256, 256 * 256 * 4).is_ok());
        for r in [
            ok(0, 1, 0),
            ok(257, 1, 257 * 4),
            ok(1, 257, 257 * 4),
            ok(1, 1, 3),
            ok(1, 1, 5),
        ] {
            assert!(matches!(r, Err(DeviceError::Internal(SIZE))), "{r:?}");
        }
        let r = check_thumbnail(u32::MAX, u32::MAX, vec![]);
        assert!(matches!(r, Err(DeviceError::Internal(SIZE))));
    }

    #[test]
    fn apple_code_table() {
        assert!(matches!(thumbnail_from_apple_code(0), Ok(true)));
        assert!(matches!(
            thumbnail_from_apple_code(1),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(thumbnail_from_apple_code(2), Ok(false)));
        assert!(matches!(
            thumbnail_from_apple_code(3),
            Err(DeviceError::Internal(TIMEOUT))
        ));
        assert!(matches!(
            thumbnail_from_apple_code(4),
            Err(DeviceError::Internal(SIZE))
        ));
        for c in [5, 6, 99, -1, i32::MIN] {
            assert!(matches!(
                thumbnail_from_apple_code(c),
                Err(DeviceError::Internal(OUTCOME))
            ));
        }
    }

    #[test]
    fn only_zero_is_closed() {
        assert_eq!(preview_from_macos_code(0).unwrap(), PreviewResult::Closed);
        for c in [1, 2, -1, 100] {
            assert!(matches!(
                preview_from_macos_code(c),
                Err(DeviceError::Internal(_))
            ));
        }
    }

    #[test]
    fn unsupported_thumbnailer_never_succeeds() {
        let r = UnsupportedFileThumbnailer.thumbnail(
            FileKind::SampleImage,
            &[1],
            THUMBNAIL_MAX_SIDE,
            THUMBNAIL_DEADLINE,
        );
        assert!(matches!(r, Err(DeviceError::Unsupported(_))));
    }

    #[test]
    fn debug_hides_pixels() {
        let t = ThumbnailResult::Thumbnail(Image {
            width: 1,
            height: 1,
            rgba: vec![7, 7, 7, 7],
        });
        assert_eq!(format!("{t:?}"), "Thumbnail(1x1)");
    }
}
