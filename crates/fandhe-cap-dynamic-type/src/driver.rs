//! Phase 11 の拡充ループ（親 #911）型サンプル「文字サイズ追従」（Issue #912）の
//! effect ドライバ。`fandhe_core::screens::text_scale_preview`（端末依存ゼロ）へ
//! OS 値の読み取り結果を渡しつつ、スライダー値（実描画へ反映する倍率）を
//! `HostView::set_zoom_factor` 経由で毎フレーム実描画へ適用する
//! （`fandhe-gallery` の `screens/registry.rs` の `effect` 区画から `fn(&S, &CompletionSender<S>)`
//! として呼ばれる契約）。
//!
//! # OS 値の読み取りは Windows / iOS / Android で実装（Issue #913 / #914）
//!
//! [`platform_text_scale`] は Windows では [`crate::windows::WindowsTextScale`]
//! （`UISettings.TextScaleFactor` を読む、Issue #913）、iOS/Android では
//! `IosTextScale`/`AndroidTextScale`（`crate::ios`/`crate::android`、
//! Issue #914）を返し、それ以外のターゲットは `UnsupportedTextScale` を返す
//! （macOS は一次情報調査の結果、第三者アプリ向け公開 API が無いため恒久的に
//! `Unsupported`、`fandhe_device::text_scale` モジュール doc 参照）。
//!
//! # zoom の適用元はスライダー値に一本化
//!
//! OS 値が読めた場合も `finish_read` が `state.scale` を上書きするのみで、
//! 実際に `set_zoom_factor` へ渡す値は常にその時点の `state.scale`（スライダー値）
//! を使う。これにより「OS 値 → スライダーで上書き」の順で見た目確認ができる
//! （Issue #912 実装計画 §3.1）。直前に適用した値と異なる値のときだけ
//! `set_zoom_factor` を呼び、描画層のキーボード zoom（egui は既定 `zoom_with_keyboard =
//! true`、Ctrl+/−）を毎フレーム上書きしない（ponytail: キーボード zoom との
//! 競合が要件上問題になった場合は `zoom_with_keyboard` を無効化する設計に上げる）。
//! 「直前に適用した値」は `HostView::applied_zoom_factor` 経由で
//! 描画コンテキスト単位のキャッシュとして持つ（`HostView` 実装側の doc 参照。プロセス
//! グローバルな `static` にすると複数コンテキスト間でキャッシュが混線するため、
//! codex-review 指摘 P2・PR #920 是正で移した）。
//!
//! # ドラッグ中は zoom 適用を保留する
//!
//! スライダーのドラッグ中（ポインタの主ボタンが押下中）は `set_zoom_factor` を呼ばない。
//! ドラッグと同じフレームで `pixels_per_point` を変えると、egui はスライダー自身を含む
//! レイアウトを次パスで別の point 空間の矩形へ再マップし、指/カーソルの追従を失って
//! 倍率が飛ぶ（Cursor Bugbot 指摘 "Slider drag fights live zoom"、PR #920 是正）。
//! ポインタを離した最初のフレームで、その時点の `state.scale`（＝ドラッグ終了時の値）を
//! 一度だけ適用する。`HostView::pointer_primary_down` 参照。

#[cfg(any(
    not(any(target_os = "windows", target_os = "ios", target_os = "android")),
    test
))]
use crate::contract::UnsupportedTextScale;
use crate::contract::{TextScale, read_scale};
use crate::error::DynamicTypeError;
use fandhe_core::screens::text_scale_preview::{
    OsScale, OsScaleError, TextScalePreviewState, finish_read,
};

use fandhe_runtime::CompletionSender;

/// OS の文字サイズ追従実装（iOS 分岐、モジュール doc 参照）。
#[cfg(target_os = "ios")]
pub fn platform_text_scale() -> &'static dyn TextScale {
    &crate::ios::IosTextScale
}

/// OS の文字サイズ追従実装（Android 分岐、モジュール doc 参照）。
#[cfg(target_os = "android")]
pub fn platform_text_scale() -> &'static dyn TextScale {
    &crate::android::AndroidTextScale
}

/// OS の文字サイズ追従実装（iOS/Android 以外の分岐、モジュール doc 参照。Windows は
/// 実装済み、他は `UnsupportedTextScale` 固定）。
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn platform_text_scale() -> &'static dyn TextScale {
    #[cfg(target_os = "windows")]
    {
        &crate::windows::WindowsTextScale
    }
    #[cfg(not(target_os = "windows"))]
    {
        &UnsupportedTextScale
    }
}

/// `crate::error::DynamicTypeError` を
/// `fandhe_core::screens::text_scale_preview::OsScaleError` へ写像する
/// （`device.rs::to_note_storage_error` と同じ判断。内部構造は漏らさず `Display`
/// 文言のみを転送する、security.md A09）。
fn to_os_error(err: DynamicTypeError) -> OsScaleError {
    match err {
        DynamicTypeError::Unsupported => OsScaleError::Unavailable,
        other => OsScaleError::Failed(other.to_string()),
    }
}

/// [`platform_text_scale`] を読み・スライダー値を実描画へ適用する 1 フレーム分の処理。
/// `state`/`sender`/`ts`/`last` をすべて引数で受け取ることでプラットフォーム cfg 抜きで
/// テストできるようにしている（`device/tilt.rs::drive` と同じ構成）。
///
/// `dragging` はスライダーのドラッグ中（ポインタの主ボタン押下中）か
/// （モジュール doc「ドラッグ中は zoom 適用を保留する」参照）。ドラッグ中は zoom を
/// 適用せず `last` も更新しない。離した後の最初の呼び出しで `target` と `last` の差分を
/// 検出し、そのとき一度だけ適用する。
///
/// 戻り値は zoom を適用したか（テスト用）。
fn drive(
    state: &TextScalePreviewState,
    sender: &CompletionSender<TextScalePreviewState>,
    ts: &dyn TextScale,
    last: &mut Option<f32>,
    dragging: bool,
) -> bool {
    // OS 値は未読み取りのときのみ要求する（読み取り後に毎フレーム再送しない、
    // `http_fetch` の Requested/Idle 型状態機械と同じ「一度だけ」の判断）。
    if *state.os_scale() == OsScale::Unread {
        let result = read_scale(ts).map_err(to_os_error);
        // 受信側（ProxyApp）がドロップ済みの送信失敗は無視してよい
        // （coding-rust.md: panic を API 契約にしない、`device/tilt.rs` と同じ判断）。
        let _ = sender.send(Box::new(move |s: &mut TextScalePreviewState| {
            finish_read(s, result);
        }));
    }

    if dragging {
        return false;
    }

    let target = state.scale.clamp(
        fandhe_core::screens::text_scale_preview::SCALE_MIN,
        fandhe_core::screens::text_scale_preview::SCALE_MAX,
    );
    if *last == Some(target) {
        return false;
    }
    sender.host_view().set_zoom_factor(target);
    *last = Some(target);
    true
}

/// `registry.rs` の `effect` 区画から `text_scale_preview` 画面用に配線される effect ドライバ。
pub fn text_scale_effect(
    state: &TextScalePreviewState,
    sender: &CompletionSender<TextScalePreviewState>,
) {
    // 描画コンテキスト単位のキャッシュ（モジュール doc 参照）を読み出し・
    // `drive` に渡し、変わっていれば書き戻す。
    // 適用済みの値は描画層が `set_zoom_factor` のたびに記録する（HostView）。
    let mut last = sender.host_view().applied_zoom_factor();
    let dragging = sender.host_view().pointer_primary_down();
    drive(state, sender, platform_text_scale(), &mut last, dragging);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::text_scale_preview::{SCALE_MAX, SCALE_MIN};

    fn sender_with_channel() -> (
        CompletionSender<TextScalePreviewState>,
        std::sync::mpsc::Receiver<fandhe_runtime::Completion<TextScalePreviewState>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            CompletionSender::new(tx, std::sync::Arc::new(FakeView::default())),
            rx,
        )
    }

    /// 拡大率の記録だけを持つ `HostView` 付きの起床口。描画コンテキスト 1 つ分に相当する。
    #[derive(Default)]
    struct FakeView {
        applied: std::sync::Mutex<Option<f32>>,
    }

    impl fandhe_runtime::RepaintWaker for FakeView {
        fn wake(&self) {}
        fn wake_after(&self, _: std::time::Duration) {}
        fn host_view(&self) -> &dyn fandhe_runtime::HostView {
            self
        }
    }

    impl fandhe_runtime::HostView for FakeView {
        fn set_zoom_factor(&self, zoom: f32) {
            *self.applied.lock().unwrap() = Some(zoom);
        }
        fn applied_zoom_factor(&self) -> Option<f32> {
            *self.applied.lock().unwrap()
        }
    }

    struct MockTextScale(Result<f32, DynamicTypeError>);

    impl TextScale for MockTextScale {
        fn scale_factor(&self) -> Result<f32, DynamicTypeError> {
            match &self.0 {
                Ok(v) => Ok(*v),
                Err(DynamicTypeError::Unsupported) => Err(DynamicTypeError::Unsupported),
                Err(_) => Err(DynamicTypeError::Internal),
            }
        }
    }

    #[test]
    fn zoom_is_applied_only_when_scale_changes() {
        let (sender, _rx) = sender_with_channel();
        let ts = UnsupportedTextScale;
        let mut last = None;
        let mut state = TextScalePreviewState::default();

        assert!(drive(&state, &sender, &ts, &mut last, false));
        assert_eq!(last, Some(state.scale));

        // 同じ状態で再度呼んでも zoom は再適用しない。
        assert!(!drive(&state, &sender, &ts, &mut last, false));

        state.scale = 1.5;
        assert!(drive(&state, &sender, &ts, &mut last, false));
        assert_eq!(last, Some(1.5));
    }

    /// Cursor Bugbot 指摘（PR #920）の回帰チェック: ドラッグ中は zoom を適用せず、
    /// 離した後にドラッグ終了時点の値を一度だけ適用する。
    #[test]
    fn zoom_is_deferred_while_dragging_and_applied_on_release() {
        let (sender, _rx) = sender_with_channel();
        let ts = UnsupportedTextScale;
        let mut last = None;
        let mut state = TextScalePreviewState::default();

        // ドラッグ中に値が動いても zoom は適用しない・last も更新しない。
        state.scale = 1.5;
        assert!(!drive(&state, &sender, &ts, &mut last, true));
        assert_eq!(last, None);
        state.scale = 2.0;
        assert!(!drive(&state, &sender, &ts, &mut last, true));
        assert_eq!(last, None);

        // 離した最初の呼び出しで、その時点の値（ドラッグ終了時の値）を一度だけ適用する。
        assert!(drive(&state, &sender, &ts, &mut last, false));
        assert_eq!(last, Some(2.0));
    }

    #[test]
    fn unsupported_is_reported_as_unavailable_not_read() {
        let (sender, rx) = sender_with_channel();
        let ts = UnsupportedTextScale;
        let mut last = None;
        let state = TextScalePreviewState::default();

        drive(&state, &sender, &ts, &mut last, false);

        let completion = rx.recv().expect("Unread なら完了が送られる");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.os_scale(), &OsScale::Unavailable);
        assert_eq!(applied.scale, 1.0);
    }

    #[test]
    fn nan_from_os_is_clamped_before_reaching_screen() {
        let (sender, rx) = sender_with_channel();
        let ts = MockTextScale(Ok(f32::NAN));
        let mut last = None;
        let state = TextScalePreviewState::default();

        drive(&state, &sender, &ts, &mut last, false);

        let completion = rx.recv().expect("Unread なら完了が送られる");
        let mut applied = state.clone();
        completion(&mut applied);
        assert_eq!(applied.os_scale(), &OsScale::Read(1.0));
    }

    #[test]
    fn read_is_requested_only_while_unread() {
        let (sender, rx) = sender_with_channel();
        let ts = MockTextScale(Ok(1.4));
        let mut last = None;
        let mut state = TextScalePreviewState::default();
        finish_read(&mut state, Ok(1.4));
        assert_ne!(*state.os_scale(), OsScale::Unread);

        drive(&state, &sender, &ts, &mut last, false);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn scale_range_constants_match_device_crate() {
        assert_eq!(SCALE_MIN, crate::contract::SCALE_MIN);
        assert_eq!(SCALE_MAX, crate::contract::SCALE_MAX);
    }

    /// codex-review 指摘 P2（PR #920）の回帰チェック: `text_scale_effect` が使う
    /// 「直近に適用した値」キャッシュは 描画コンテキスト単位に分離されている必要がある。
    /// Context A に倍率を適用したあと、別 Context B の effect が同じ倍率を「適用済み」と
    /// 誤判定して省略しない（= B 側の `set_zoom_factor` が実際に呼ばれる）ことを、
    /// B が返す完了・repaint 要求ではなく `applied_zoom_factor` の値で確認する。
    #[test]
    fn last_applied_cache_is_isolated_per_context() {
        let (sender_a, _rx_a) = sender_with_channel();
        let (sender_b, _rx_b) = sender_with_channel();
        let mut state = TextScalePreviewState::default();
        state.scale = 1.5;

        // Context A へ 1.5 を適用。
        text_scale_effect(&state, &sender_a);
        assert_eq!(sender_a.host_view().applied_zoom_factor(), Some(1.5));

        // Context B は別インスタンスのため、A で適用済みでも未適用のまま。
        assert_eq!(sender_b.host_view().applied_zoom_factor(), None);
        text_scale_effect(&state, &sender_b);
        assert_eq!(sender_b.host_view().applied_zoom_factor(), Some(1.5));
    }

    /// Issue #914 の回帰チェック: iOS/Android 以外のターゲット（このテストが走る
    /// ホスト環境を含む）では [`platform_text_scale`] が引き続き `Unsupported` を
    /// 返す（誤って「読めた」扱いにしていない、モジュール doc 参照）。
    #[cfg(not(any(target_os = "windows", target_os = "ios", target_os = "android")))]
    #[test]
    fn platform_text_scale_is_unsupported_outside_windows_ios_and_android() {
        let err = platform_text_scale().scale_factor().unwrap_err();
        assert!(matches!(err, DynamicTypeError::Unsupported));
    }
}
