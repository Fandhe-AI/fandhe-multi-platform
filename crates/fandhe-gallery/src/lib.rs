//! fandhe-gallery: 全機能の画面を登録し、実機確認・撮影に使う非公開アプリ（`publish = false`）。
//!
//! 位置づけ（`docs/architecture/consumer-app-structure.md` §3、TASK-21.2 / Issue #1165）:
//! 画面の登録（[`screens`]）と、各 OS の最終成果物の入口を持つ。
//!
//! - デスクトップ・iOS の実行体: `main.rs`（bin `fandhe-gallery-desktop`）
//! - Android の cdylib: 本ファイルの `android_main`
//! - Web の wasm cdylib: 本ファイルの `start`（`#[wasm_bindgen(start)]`）
//!
//! 端末機能の配線と各 OS の入口関数は `fandhe-egui-proxy`（機能 crate へ分けるまで、#1194）、
//! 描画と起動は `fandhe-render-egui` が持つ。依存の向きは本 crate → egui-proxy の一方向で、
//! 画面の登録は入口へ引数（`BuildAppFn`）で渡す。`android_main` / `start` のような
//! 最終成果物ごとに 1 つだけの固定シンボルは、重複を避けるため rlib の egui-proxy ではなく本 crate が持つ。
//!
//! Android のみ `unsafe_code` を `deny` へ緩め、`android_main` の `#[no_mangle]` 1 箇所にだけ
//! `#[allow(unsafe_code)]` を付ける（`coding-rust.md` の FFI 例外規定）。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

pub mod screens;
pub use screens::{build_app, registered_names};

/// `device` feature 無効ビルド向けの `nativeOnShareReceived` no-op フォールバック（#1258）。
/// `ShareReceiveLifecycle`（#1257）は feature を知らず `ShareReceiveHelper.deliver` を必ず呼ぶため、実体を持つ
/// `fandhe-cap-share-receive`（`device` 有効時のみ依存）が無いと `UnsatisfiedLinkError` になる。
/// 実体との二重定義を避けるため cfg は `device` 無効のときだけ。署名は Java 宣言・実体と一致
/// （`(String) -> void`）。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` はこの 1 関数に閉じ、
/// シンボル名は JNI 命名規約上の固定名。
#[cfg(all(target_os = "android", not(feature = "device")))]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_platform_ShareReceiveHelper_nativeOnShareReceived(
    _env: *mut core::ffi::c_void,
    _class: *mut core::ffi::c_void,
    _text: *mut core::ffi::c_void,
) {
}

/// Android アプリのエントリポイント（計測専用ビルド）。
///
/// `android-activity`（GameActivity 方式）がライブラリロード後にこのシンボル名を検索して呼ぶ
/// （OS 側グルーコードとの契約で、Rust 側から呼ばれることはない）。`frame-metrics` 有効時は
/// 計測専用アプリへ委譲する。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は Rust の名前修飾を無効化するだけで、
/// シンボル名 `android_main` は `android-activity` の規約上の固定名であり、本 crate は cdylib として
/// 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない（egui-proxy は rlib で
/// 同名シンボルを持たない）。
#[cfg(all(target_os = "android", feature = "frame-metrics"))]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: fandhe_egui_proxy::android::AndroidApp) {
    fandhe_egui_proxy::frame_metrics::run_android(app);
}

/// Android アプリのエントリポイント。
///
/// 実処理は egui-proxy の `android::run` で、画面の登録（[`build_app`]）を渡す。
///
/// # Safety
/// 上の `frame-metrics` 有効時の `android_main` と同じ根拠（この関数自体は `unsafe` 操作を含まない）。
#[cfg(all(target_os = "android", not(feature = "frame-metrics")))]
#[allow(unsafe_code)]
#[no_mangle]
fn android_main(app: fandhe_egui_proxy::android::AndroidApp) {
    // Activity 再生成では画面状態が作り直される（世代 0）。保留中の書類スキャン要求が残ると
    // 次の要求が飲み込まれるため、新しい画面状態を作る前に破棄する（TASK-21.3 / Issue #1171。
    // 以前は egui-proxy の `android::run` が呼んでいた。機能 crate を egui-proxy が知らないようにするため移した）。
    #[cfg(feature = "device")]
    fandhe_cap_document_scan::reset_pending_tracking();
    // 機械学習推論の処理済み世代も破棄する。画面状態は再生成で generation 0 に戻るため、残すと次の
    // 「推論する」が飲み込まれる（`fandhe_cap_ml_inference::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    fandhe_cap_ml_inference::reset_pending_tracking();
    // 通話 UI の記録（処理済み世代）と進行中の通話も破棄する。画面状態は再生成で generation 0 に戻るため、
    // 残すと次の「着信を模擬する」が飲み込まれ、OS 側の着信も残る（`fandhe_cap_call_ui::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    fandhe_cap_call_ui::reset_pending_tracking();
    // ライブアクティビティの保留中の要求も破棄し、表示中の通知を閉じる。再生成後の画面は Idle から始まるため、
    // 残すと次の操作と OS 側の表示が食い違う（`fandhe_cap_live_activity::reset_pending_tracking` doc 参照）。
    #[cfg(feature = "device")]
    fandhe_cap_live_activity::reset_pending_tracking();
    fandhe_egui_proxy::android::run(app, build_app);
}

/// Web のエントリポイント。wasm-bindgen の glue が読み込み時に呼ぶ。実処理は egui-proxy の
/// `web::start` で、画面の登録（[`build_app`]）を渡す。
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() -> Result<(), wasm_bindgen::JsValue> {
    fandhe_egui_proxy::web::start(build_app)
}
