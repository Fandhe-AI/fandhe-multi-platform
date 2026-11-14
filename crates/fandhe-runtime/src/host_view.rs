//! 描画層の観測値と操作の口（Issue #1302）。
//!
//! 端末機能の effect ドライバが、描画層（egui 等）から読みたい値（ウィンドウのフォーカス・
//! ファイルのドラッグ状況・ポインタの押下）と、描画層へ渡したい操作（拡大率の差し替え）を
//! レンダラ非依存で表す。機能 crate は egui を知らず、[`crate::CompletionSender::host_view`]
//! 経由でこの trait だけを使う。
//!
//! - 実装はレンダラ実装（`fandhe-render-egui` の `EguiWaker`）が [`crate::RepaintWaker::host_view`]
//!   の上書きで提供する。[`crate::Renderer`] には載せない（`Renderer::render` は UI スレッドで
//!   画面を描く契約で、effect からは見えないため。`RepaintWaker` と同じく別契約として扱う）
//! - 呼んでよい時点は [`crate::EffectFn`] と同じく、描画ループのフレーム末尾（UI スレッド）だけ。
//!   別スレッドから呼ばない
//! - 画面の読み戻し（`request_screenshot` / `tagged_screenshot`）・ウィンドウ操作（`window_state` /
//!   `request_window` / `viewport_size`）・入力欄のヒント（`set_text_input_hint`）も同じ口に載せる（Issue #1326）。
//!   値は egui・eframe の型を使わず、この module の型で受け渡す
//! - 全メソッドに既定の実装がある。描画層が観測できない（テスト・ヘッドレス）場合の既定値は
//!   「観測していない」側に倒してあり、成功の表示や権限の行使を既定値だけで起こさない

use std::path::PathBuf;

/// 画面の読み戻し結果（RGBA8・行優先）。画素は `width * height * 4` バイトが期待で、
/// 長さ・寸法の検査は受け手（印刷の `PrintImage::new` 等）の責務。
#[derive(Clone, PartialEq, Eq)]
pub struct Screenshot {
    /// 横の画素数。
    pub width: usize,
    /// 縦の画素数。
    pub height: usize,
    /// RGBA8 の画素列（行優先）。
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Screenshot {
    /// 画面の内容を漏らさないよう、寸法だけを出す。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Screenshot({}x{})", self.width, self.height)
    }
}

/// ウィンドウの矩形（左上原点・描画層の point）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRect {
    /// 左端。
    pub x: f32,
    /// 上端。
    pub y: f32,
    /// 幅。
    pub width: f32,
    /// 高さ。
    pub height: f32,
}

/// ウィンドウの現在の状態。読めない項目は `None`（既定は全項目 `None`）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct WindowState {
    /// 装飾を含む外側の矩形。
    pub outer: Option<WindowRect>,
    /// 内側（描画領域）の矩形。
    pub inner: Option<WindowRect>,
    /// 全画面表示中か。
    pub fullscreen: Option<bool>,
    /// 最小化中か。
    pub minimized: Option<bool>,
    /// 描画の拡大率。
    pub zoom_factor: Option<f32>,
    /// 1 point あたりの物理画素数。
    pub pixels_per_point: Option<f32>,
}

/// ウィンドウへの操作。値（有限・正）の検査は呼び出し側が OS へ渡す前に済ませる。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WindowCommand {
    /// 外側の左上位置へ移す。
    OuterPosition {
        /// 左端。
        x: f32,
        /// 上端。
        y: f32,
    },
    /// 内側の大きさへ変える。
    InnerSize {
        /// 幅。
        width: f32,
        /// 高さ。
        height: f32,
    },
    /// 最前面に固定する（`true`）か、通常へ戻す（`false`）。
    AlwaysOnTop(bool),
}

/// 入力欄に付けるヒント。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputHint {
    /// 既定（ヒントなし）へ戻す。
    Default,
    /// ワンタイムコード入力用（数字キーボード・候補の提示）。
    OneTimeCode,
}

/// [`HostView::set_text_input_hint`] の失敗。固定の種別だけを運ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputHintError {
    /// 描画層に入力欄の橋渡しが無い。
    Unsupported,
    /// 橋渡しが受け付けなかった。
    Failed,
}

impl std::fmt::Display for TextInputHintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => f.write_str("text input hint is unsupported"),
            Self::Failed => f.write_str("text input hint failed"),
        }
    }
}

impl std::error::Error for TextInputHintError {}

/// 描画層の観測値と操作の口。メソッドはすべて既定の実装を持つ。
pub trait HostView {
    /// このフレームでウィンドウがフォーカスを持っているか。
    ///
    /// 既定は `true`（フォーカスを失ったと観測していない）。Windows の共有シートのキャンセル推定と
    /// 注意喚起の待機が「フォーカスを失った」ことの観測に使うので、既定ではどちらも発火しない。
    fn focused(&self) -> bool {
        true
    }

    /// このフレームのファイルのドラッグ & ドロップ入力。
    ///
    /// 第 1 要素はホバー中のファイルがあるか、第 2 要素はこのフレームでドロップされたパスの一覧
    /// （Web では常に `None`、デスクトップでも OS からパスが渡らない場合は `None`）。ドロップは
    /// 1 フレームだけ現れ、次フレーム以降には現れない。既定は `(false, 空)`。
    fn file_drag(&self) -> (bool, Vec<Option<PathBuf>>) {
        (false, Vec::new())
    }

    /// ポインタの主ボタン（左クリック / タップ）がこのフレームで押下中か。既定は `false`。
    fn pointer_primary_down(&self) -> bool {
        false
    }

    /// 描画の拡大率を差し替え、適用した値をこの描画コンテキスト単位で記録する。
    /// 次フレームの再描画も要求する。既定は何もしない（拡大率は変わらず、記録も残らない）。
    fn set_zoom_factor(&self, _zoom: f32) {}

    /// この描画コンテキストへ最後に [`HostView::set_zoom_factor`] した値。未適用・非対応なら `None`。
    ///
    /// プロセス全体の static ではなく描画コンテキスト単位で持つこと（複数コンテキストで
    /// 適用済みの誤判定が起きないため）。
    fn applied_zoom_factor(&self) -> Option<f32> {
        None
    }

    /// 次のフレームの描画を `tag` 付きで読み戻すよう依頼する。既定は何もしない
    /// （[`HostView::tagged_screenshot`] も `None` のままで、呼び出し側の期限で失敗になる）。
    fn request_screenshot(&self, _tag: u64) {}

    /// このフレームに届いた、`tag` 付きの読み戻し。他のタグ・未到着は `None`。
    fn tagged_screenshot(&self, _tag: u64) -> Option<Screenshot> {
        None
    }

    /// このフレームの描画領域の大きさ（幅・高さ）。読めなければ `None`。
    fn viewport_size(&self) -> Option<(f32, f32)> {
        None
    }

    /// ウィンドウの現在の状態。既定は全項目 `None`（読めない）。
    fn window_state(&self) -> WindowState {
        WindowState::default()
    }

    /// ウィンドウへ操作を依頼する。反映は次フレーム以降で、成否は [`HostView::window_state`] の
    /// 読み戻しで判定する。既定は何もしない。
    fn request_window(&self, _command: WindowCommand) {}

    /// 入力欄のヒントを描画層へ渡す。既定は [`TextInputHintError::Unsupported`]。
    fn set_text_input_hint(&self, _hint: TextInputHint) -> Result<(), TextInputHintError> {
        Err(TextInputHintError::Unsupported)
    }
}

/// 描画層が口を提供しないときの [`HostView`]（既定値だけを返す）。
pub(crate) struct NoHostView;

impl HostView for NoHostView {}
