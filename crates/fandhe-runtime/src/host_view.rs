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
//! - 全メソッドに既定の実装がある。描画層が観測できない（テスト・ヘッドレス）場合の既定値は
//!   「観測していない」側に倒してあり、成功の表示や権限の行使を既定値だけで起こさない

use std::path::PathBuf;

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
}

/// 描画層が口を提供しないときの [`HostView`]（既定値だけを返す）。
pub(crate) struct NoHostView;

impl HostView for NoHostView {}
