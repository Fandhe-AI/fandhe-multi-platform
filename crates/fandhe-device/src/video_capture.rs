//! 動画の撮影の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/video-capture.json`、親 #169、Issue #821）。
//!
//! 扱う範囲は「撮影 UI（macOS はアプリが組む撮影パネル）を開き、撮れた動画の長さ・表示の寸法・ファイルサイズを出す」ことまで。
//! 再生・保存・共有・編集は扱わない。長さの上限は [`MAX_DURATION_SECS`] 秒で、OS の撮影 UI へ渡す。
//! 担当 OS は iOS（`UIImagePickerController`、#822）、Android（`ACTION_VIDEO_CAPTURE`、#823）、
//! Web（`<input type="file" capture>`、#824。`fandhe-web-dom` 側。長さの上限は渡せない）、
//! macOS（`AVCaptureMovieFileOutput` と `AVCaptureVideoPreviewLayer` のパネル。音声は録らない。子 issue）。
//! Windows は保留で `Unsupported`（保留を解くときは dispatch の変更を別 issue で扱う）。
//! `fandhe-egui-proxy` の effect ドライバ（`device/video_capture.rs`）が本モジュールの trait を呼び、
//! OS 層が実装する。Web の子は検査 [`VideoClip::from_os`] を再利用する（本モジュールは wasm32 でも
//! ビルドできる純粋なコードに保つ）。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「撮れた」（[`VideoCaptureOutcome::Captured`]）/
//!   「取り消し」（[`VideoCaptureOutcome::Cancelled`]。利用者が撮影 UI を閉じた。失敗にしない）/
//!   失敗（[`VideoCaptureFailure`] の理由別、非対応を含む）
//! - **OS 層から Rust へ渡す数値表**（iOS の `.m` と Android の JNI で共有。[`result_from_code`]）:
//!
//! | kind | 意味 | 返す値 |
//! | --- | --- | --- |
//! | 0 | 撮れた | `Ok(Captured)`（[`VideoClip::from_os`] を通す） |
//! | 1 | 利用者が閉じた | `Ok(Cancelled)` |
//! | 2 | 非対応 | `Unsupported` |
//! | 3 | カメラ権限の拒否・制限 | `PermissionDenied` |
//! | 4 | 撮影アプリがない（Android の `ActivityNotFoundException`） | `NoCameraApp` |
//! | 5 | 前面でない | `NotForeground` |
//! | 6 | 表示前の中断（背景遷移等） | `Interrupted` |
//! | 7 | 0 バイト・動画が書き出されなかった | `Empty` |
//! | 8 | メタデータ（寸法・向き）を読めない | `Unreadable` |
//! | 9・未知のコード | その他 | `Internal` |
//!
//! macOS は 4・5・6 を使わない。2 は「カメラが無い」、9 は録画前の実行時エラー・カメラの取り外し・
//! 一時ファイルを消せない場合を含む
//!
//! - **同時 1 件と世代**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と
//!   画面の世代の両方で捨てる
//! - **成功値の条件**（契約表 6）: `Ok(Captured)` と `Ok(Cancelled)` は表示開始（bridge の
//!   `try_begin_present`）を記録した要求にだけ認める。記録のない要求の成功値は `Internal`。
//!   0 バイト・寸法 0・不正な向きは成功にしない
//! - **期限は持たない**: 撮影は利用者の操作時間に依存するため、Rust 側で期限を決めない
//!   （OS の応答を正とする）。完了通知が届かなければ画面は撮影中のまま（守らない範囲）
//! - **長さ・寸法・サイズの出どころ**: OS のメタデータとファイルのサイズだけから取る。
//!   経過時間（壁時計）から作らない。長さを読めないときは `None`（長さなし）にし、0 を作らない
//! - **OS 層の義務**（契約表 1・5・9）:
//!   - 成功・取り消し・失敗・破棄の全経路で一時ファイルを消す
//!   - `complete_from_os` は 1 要求につき 1 回だけ呼ぶ
//!   - 撮影 UI を閉じてから結果を返す。UI の提示は main queue（Android は UI スレッド）で行う
//!   - 権限の応答時にアプリが前面になければ、UI を出さず `Interrupted` を返す（iOS・Android。macOS は背面でも撮影パネルを出す）
//!   - アプリが自前の置き場に書く OS（macOS）は、置き場を要求ごとに 0o700 で新しく作り、その配下だけを使う。
//!     削除に失敗したら、読めた結果があっても `Internal` にし、置き場を要求より長く生きる場所に記録して次の要求の前に
//!     消し直す。消せなければ次の要求も `Internal`（契約表 9）
//!   - `rotation_deg` は 0..360 に正規化して渡す。結果を共有状態へ入れた直後に再描画を起こす
//!   - 動画の中身・ファイルパス・OS のエラー文を Rust へ渡さない・ログに出さない
//! - **秘密値**（契約表 8）: 動画の中身・パス・OS のエラー文をログ・画面のエラー文・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（非対応・権限拒否・0 バイト・
//! 読めないメタデータを成功にしない）、一時ファイルの全経路での解放。
//! 守らない範囲: 撮影 UI の表示中に Rust 側から閉じること、macOS の撮影パネルの見た目・画質、
//! パネルを開いたまま画面を離れたときのパネル（残る）、OS ごとの映像の違い（コーデック・
//! 手ぶれ補正）、上限秒数を OS が超えて録画した場合の切り詰め、複数プロセス、プロセスが落ちた後の
//! 結果、完了通知が届かない場合の画面表示。

use std::fmt;

/// 長さの上限（秒）。OS の撮影 UI へ渡す値の SSOT。Web の `capture` 属性は上限を渡せない。
pub const MAX_DURATION_SECS: u32 = 30;

/// 撮れた動画の情報。[`VideoClip::from_os`] を通ったものだけが作れる。
/// ファイルパス・中身は持たない。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct VideoClip {
    size_bytes: u64,
    duration_ms: Option<u64>,
    width: u32,
    height: u32,
}

impl VideoClip {
    /// OS から届いた動画の情報を検査する。
    ///
    /// - `size_bytes == 0` は `Empty`
    /// - 幅または高さが 0、`rotation_deg` が 0 / 90 / 180 / 270 以外は `Unreadable`
    /// - 90 / 270 は幅と高さを入れ替えて「表示の向き」にする
    /// - `duration_ms == Some(0)` は長さを読めないものとして `None` に寄せる
    pub fn from_os(
        size_bytes: u64,
        duration_ms: Option<u64>,
        width: u32,
        height: u32,
        rotation_deg: i32,
    ) -> Result<VideoClip, VideoCaptureFailure> {
        if size_bytes == 0 {
            return Err(VideoCaptureFailure::Empty);
        }
        if width == 0 || height == 0 {
            return Err(VideoCaptureFailure::Unreadable);
        }
        let (width, height) = match rotation_deg {
            0 | 180 => (width, height),
            90 | 270 => (height, width),
            _ => return Err(VideoCaptureFailure::Unreadable),
        };
        Ok(VideoClip {
            size_bytes,
            duration_ms: duration_ms.filter(|&d| d > 0),
            width,
            height,
        })
    }

    /// ファイルサイズ（バイト、1 以上）。
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    /// 長さ（ミリ秒）。読めなければ `None`。
    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// 表示の向きでの幅（画素、1 以上）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 表示の向きでの高さ（画素、1 以上）。
    pub fn height(&self) -> u32 {
        self.height
    }
}

impl fmt::Debug for VideoClip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VideoClip")
            .field("size_bytes", &self.size_bytes)
            .field("duration_ms", &self.duration_ms)
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

/// 成功側の結果（失敗ではない 2 値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCaptureOutcome {
    /// 撮れた。
    Captured(VideoClip),
    /// 利用者が撮影 UI を閉じた（失敗にしない）。
    Cancelled,
}

/// 撮れなかった理由（OS のエラー文・パスは持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCaptureFailure {
    /// 非対応 OS・端末。
    Unsupported,
    /// カメラ権限の拒否・制限。
    PermissionDenied,
    /// 撮影できるアプリがない。
    NoCameraApp,
    /// 前面でない。
    NotForeground,
    /// 表示前の中断。
    Interrupted,
    /// 0 バイト・動画が書き出されなかった。
    Empty,
    /// 寸法・向きなどのメタデータを読めない。
    Unreadable,
    /// 未知のコードなど。
    Internal,
}

/// 数値表（モジュール doc）から結果へ写す。iOS の `.m` と Android の JNI が共有する。
///
/// 撮れたとき（kind 0）だけメタデータを使い、[`VideoClip::from_os`] を通す。負の長さは長さなし、
/// 負の寸法は `Unreadable`。取り消し・失敗ではメタデータを無視する。未知のコードは `Internal`
/// （成功へ倒さない）。
pub fn result_from_code(
    kind: i32,
    size_bytes: u64,
    duration_ms: i64,
    width: i32,
    height: i32,
    rotation_deg: i32,
) -> Result<VideoCaptureOutcome, VideoCaptureFailure> {
    match kind {
        0 => {
            if size_bytes == 0 {
                return Err(VideoCaptureFailure::Empty);
            }
            let (Ok(w), Ok(h)) = (u32::try_from(width), u32::try_from(height)) else {
                return Err(VideoCaptureFailure::Unreadable);
            };
            let duration = u64::try_from(duration_ms).ok();
            VideoClip::from_os(size_bytes, duration, w, h, rotation_deg)
                .map(VideoCaptureOutcome::Captured)
        }
        1 => Ok(VideoCaptureOutcome::Cancelled),
        2 => Err(VideoCaptureFailure::Unsupported),
        3 => Err(VideoCaptureFailure::PermissionDenied),
        4 => Err(VideoCaptureFailure::NoCameraApp),
        5 => Err(VideoCaptureFailure::NotForeground),
        6 => Err(VideoCaptureFailure::Interrupted),
        7 => Err(VideoCaptureFailure::Empty),
        8 => Err(VideoCaptureFailure::Unreadable),
        _ => Err(VideoCaptureFailure::Internal),
    }
}

/// 動画撮影の OS 実装。`capture` は「受理」だけを同期で返し、結果は OS 層が
/// `fandhe-egui-proxy` の bridge（`complete_from_os`）へ 1 回だけ非同期に届ける。
pub trait VideoCapturer: Send + Sync {
    /// `request_id` の要求として撮影 UI の表示を OS へ依頼する。`max_duration_secs` は
    /// 録画の長さの上限。表示の直前に bridge の `try_begin_present` を呼び、`false` なら表示しない。
    fn capture(&self, request_id: u64, max_duration_secs: u32) -> Result<(), VideoCaptureFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedVideoCapturer;

impl VideoCapturer for UnsupportedVideoCapturer {
    fn capture(
        &self,
        _request_id: u64,
        _max_duration_secs: u32,
    ) -> Result<(), VideoCaptureFailure> {
        Err(VideoCaptureFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedVideoCapturer.capture(1, MAX_DURATION_SECS),
            Err(VideoCaptureFailure::Unsupported)
        );
    }

    #[test]
    fn zero_bytes_is_empty() {
        assert_eq!(
            VideoClip::from_os(0, Some(1000), 10, 10, 0),
            Err(VideoCaptureFailure::Empty)
        );
    }

    #[test]
    fn zero_dimension_is_unreadable() {
        assert_eq!(
            VideoClip::from_os(5, None, 0, 10, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            VideoClip::from_os(5, None, 10, 0, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
    }

    #[test]
    fn invalid_rotation_is_unreadable() {
        for r in [45, -90, 360, 1, i32::MIN, i32::MAX] {
            assert_eq!(
                VideoClip::from_os(5, None, 10, 20, r),
                Err(VideoCaptureFailure::Unreadable),
                "{r}"
            );
        }
    }

    #[test]
    fn rotation_swaps_dimensions_for_90_and_270() {
        for r in [90, 270] {
            let c = VideoClip::from_os(5, None, 1920, 1080, r).unwrap();
            assert_eq!((c.width(), c.height()), (1080, 1920));
        }
        for r in [0, 180] {
            let c = VideoClip::from_os(5, None, 1920, 1080, r).unwrap();
            assert_eq!((c.width(), c.height()), (1920, 1080));
        }
    }

    #[test]
    fn zero_duration_becomes_none() {
        assert_eq!(
            VideoClip::from_os(5, Some(0), 2, 2, 0)
                .unwrap()
                .duration_ms(),
            None
        );
        assert_eq!(
            VideoClip::from_os(5, Some(1500), 2, 2, 0)
                .unwrap()
                .duration_ms(),
            Some(1500)
        );
    }

    #[test]
    fn code_table_is_exhaustive() {
        use VideoCaptureFailure as F;
        let ok = result_from_code(0, 100, 2000, 1920, 1080, 90).unwrap();
        let VideoCaptureOutcome::Captured(c) = ok else {
            panic!("{ok:?}");
        };
        assert_eq!(
            (c.size_bytes(), c.duration_ms(), c.width(), c.height()),
            (100, Some(2000), 1080, 1920)
        );
        assert_eq!(
            result_from_code(1, 0, 0, 0, 0, 0),
            Ok(VideoCaptureOutcome::Cancelled)
        );
        for (k, f) in [
            (2, F::Unsupported),
            (3, F::PermissionDenied),
            (4, F::NoCameraApp),
            (5, F::NotForeground),
            (6, F::Interrupted),
            (7, F::Empty),
            (8, F::Unreadable),
            (9, F::Internal),
        ] {
            assert_eq!(result_from_code(k, 100, 1, 2, 2, 0), Err(f), "{k}");
        }
    }

    #[test]
    fn unknown_codes_are_internal_not_success() {
        for k in [10, 99, -1, i32::MAX, i32::MIN] {
            assert_eq!(
                result_from_code(k, 100, 1, 2, 2, 0),
                Err(VideoCaptureFailure::Internal),
                "{k}"
            );
        }
    }

    #[test]
    fn captured_with_bad_metadata_fails() {
        assert_eq!(
            result_from_code(0, 0, 1, 2, 2, 0),
            Err(VideoCaptureFailure::Empty)
        );
        assert_eq!(
            result_from_code(0, 5, 1, 0, 0, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(0, 5, 1, -2, 2, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
        assert_eq!(
            result_from_code(0, 5, 1, 2, -2, 0),
            Err(VideoCaptureFailure::Unreadable)
        );
    }

    #[test]
    fn negative_duration_is_unknown_length() {
        let Ok(VideoCaptureOutcome::Captured(c)) = result_from_code(0, 5, -1, 2, 2, 0) else {
            panic!();
        };
        assert_eq!(c.duration_ms(), None);
    }

    #[test]
    fn metadata_is_ignored_for_cancel_and_failure() {
        assert_eq!(
            result_from_code(1, 0, -5, -1, -1, 7),
            Ok(VideoCaptureOutcome::Cancelled)
        );
        assert_eq!(
            result_from_code(3, 0, -5, -1, -1, 7),
            Err(VideoCaptureFailure::PermissionDenied)
        );
    }
}
