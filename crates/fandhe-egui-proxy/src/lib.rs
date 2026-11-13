//! fandhe-egui-proxy: 端末機能の配線と各 OS の入口を持つ、v1 既定バックエンドの現行実装。
//!
//! 描画（`Node` → egui の変換）とアプリ起動（eframe・バックエンドの選択）は
//! `fandhe-render-egui` へ移した（Issue #1164）。画面の登録と各 OS の最終成果物
//! （デスクトップ・iOS の実行体、Android・Web の cdylib）は非公開アプリ `fandhe-gallery` へ
//! 出した（Issue #1165）。ここに残るのは端末機能の配線（`device/`）、TV フォーカス・
//! ソフトキーボードの橋渡し、各 OS の入口関数（`ios::run`・`web::start`・`android::run`）、
//! 計測専用アプリで、REQ-21 で `fandhe-platform-*` などへ順に分ける予定（設計は
//! `docs/architecture/consumer-app-structure.md`）。移した項目は従来のパス
//! （`fandhe_egui_proxy::render_screen`・`app::ProxyApp` 等）で再公開しており、#1194 で本クレートごと撤去する。
//!
//! 依存方向は `fandhe-gallery` → 本クレート → `fandhe-render-egui` → `fandhe-runtime` → `fandhe-core` の
//! 一方向（`.claude/rules/coding-rust.md` 準拠）。eframe には直接依存しない。
//!
//! 画面名（環境変数 `FANDHE_PROXY_APP`、[`selected_app_name`]）から画面を作る登録
//! （`generated_screens!`、`fandhe_core::screens::*`）は gallery が持つ。各 OS の入口は
//! 登録の解決関数 [`BuildAppFn`] を引数で受け取る（未登録名・未指定時は既定の
//! [`app::ProxyApp::new`]（sample 画面）へフォールバックする）。
//!
//! `android_main` は固定シンボル名の `#[no_mangle]` で `forbid(unsafe_code)` と両立しないため
//! `fandhe-gallery` が置き、本クレートの `android::run` を呼ぶ。本クレートは rlib のみで、
//! `android_main` と `#[wasm_bindgen(start)]` を持たない。
//!
//! Android ターゲットは、`device/` の JNI 呼び出しが `unsafe` を要するため `forbid` を `deny` へ
//! 緩める（他ターゲットは `forbid` を維持し、`coding-rust.md` の FFI 例外規定に従う）。
//!
//! iOS も同じ理由で `deny` へ緩める（Issue #751）: `device/ios.rs` が UIKit 実装
//! （`ios/fandhe_ios_motion.m` 等、`build.rs` でコンパイルする C ABI）を呼ぶために
//! `unsafe extern "C"` 宣言が必要で、`unsafe` は当該 FFI 呼び出し 1 関数に閉じ
//! `// SAFETY:` を付与する（`android.rs` の Android 分岐と同型）。
//!
//! macOS も Issue #889 で同じ理由により `deny` へ緩める: `device/apple_location.rs`
//! が CoreLocation 実装（`apple/fandhe_apple_location.m`）を呼ぶために
//! `unsafe extern "C"` 宣言が必要（iOS と同一の C ABI・同一ファイルを共用する）。
//! macOS はプラットフォーム統合レイヤ（platforms/apple/）を持たず、この `unsafe` が
//! 唯一の FFI 境界であるため、Android/iOS と同型の緩和で足りる。Issue #897 で
//! `device/apple_share.rs`（`apple/fandhe_apple_share.m` の共有シート実装を呼ぶ）が
//! 同じ緩和へ相乗りした。Issue #906 で `device/apple_biometric.rs`
//! （`apple/fandhe_apple_biometric.m` の生体認証実装を呼ぶ）も同様に相乗りする。
//! Issue #934 の `apple_gamepad`（`GCController` 実装）も同じ緩和へ相乗りしていたが、
//! 機能 crate `fandhe-cap-gamepad` へ移した（Issue #1303）。
//!
//! Windows も Issue #898 で同じ理由により `deny` へ緩める: `device/windows_share.rs`
//! が WinRT `IDataTransferManagerInterop::GetForWindow` / `ShowShareUIForWindow`
//! （`windows` クレートの COM interop、2 呼び出しとも `unsafe fn`）を呼ぶために必要。
//! `unsafe` はこの 2 呼び出しを含む `windows_share::present` 1 関数に閉じ、
//! `// SAFETY:` で HWND の生存根拠を明記する（Android/iOS/macOS と同型の FFI 例外、
//! coding-rust.md 準拠）。Issue #907 で `device/windows_biometric.rs`
//! （`IUserConsentVerifierInterop::RequestVerificationForWindowAsync` 1 呼び出し）も
//! 同じ緩和へ相乗りした。Issue #1033 で `device/windows_notification.rs`
//! （スタートメニューへ `.lnk` を作って AUMID を登録する `IShellLinkW`/
//! `IPropertyStore`/`IPersistFile` 系の呼び出し）も同じ緩和へ相乗りした。
//! Windows の `CoInitializeEx` ガードと HWND の保持は Issue #1184 で `fandhe-platform-windows` へ移した。
//!
// TASK-2.3（#54）計測マーカー: 以下はプロトタイプ描画対象切替（TASK-2.2-S5 / #52）に
// 関するモジュール doc。プラットフォーム別コードとして計上する。
// fandhe-prototype:begin
//! プロトタイプアプリ（`fandhe-prototype`、一覧・詳細・設定の 3 画面＋ナビゲーション、
//! TASK-2.2-S5 / #52）は実描画対象である（[`app::PrototypeApp`]）。
//! デスクトップ相当のエントリ（`main.rs`）は環境変数 `FANDHE_PROXY_APP=prototype` で
//! `sample_screen()`（既定・[`app::ProxyApp`]）とプロトタイプアプリを切り替える。
//! web / iOS / Android のエントリは [`screens`] のレジストリ経由の名前選択に対応する
//! （Issue #715、上記モジュール doc 参照。`prototype`/`tv-focus` は各エントリが
//! 専用の分岐で扱い、レジストリには登録しない）。
// fandhe-prototype:end
#![cfg_attr(
    not(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    )),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        target_os = "windows"
    ),
    deny(unsafe_code)
)]

pub use fandhe_render_egui::{accessibility, font, paint_command, recyclers, text_role};
#[cfg(target_os = "android")]
pub mod android;
pub mod app;
// Phase 8（端末機能、Issue #746/#747/#748/#749）の画面 effect ドライバの置き場。`device`
// feature（既定 on）は wasm32 でも有効になるが `fandhe-device` 自体は非 wasm32 分岐にのみ
// optional dependency として置かれているため、`not(target_arch = "wasm32")` を
// 併せて要る（`device.rs`・`screens.rs` のマクロ doc 参照）。
#[cfg(all(feature = "device", not(target_arch = "wasm32")))]
pub mod device;
// TASK-5.1（REQ-5、Issue #236 / #593）: コールドスタート計測の共通ラッパー。
// iOS（`ios.rs`）・デスクトップ native（`main.rs`）の両エントリから使う
// （`cold_start.rs` モジュール doc 参照）。
#[cfg(feature = "cold-start-probe")]
pub mod cold_start;
// Issue #1103: `ImageWidget` の実画素を egui へ届ける 1 枠の保管所は fandhe-render-egui にある。
// `device/` が従来のパスで使えるよう非公開で再公開する。
#[cfg_attr(
    not(all(feature = "device", not(target_arch = "wasm32"))),
    allow(unused_imports)
)]
pub(crate) use fandhe_render_egui::image_store;
pub(crate) use fandhe_render_egui::rgba_image;
// Issue #796 / #795: ソフトキーボードの状態バッファ（Android `TextInputState` /
// iOS `UITextField.text` + `markedTextRange`）を egui イベント列へ変換する純関数群。
// winit / android-activity / UIKit の型に依存させず `(&str, Option<(usize, usize)>)` の
// 素の型のみを扱うことで、Android（#796）と iOS（#795）の両ポーリング実装から
// そのまま共有できるようにしている（`soft_keyboard.rs` モジュール doc 参照）。
// iOS は計測専用ビルドでは使い手（`ios_text_input`）が無いため外す（Issue #19）。
#[cfg(any(
    target_os = "android",
    all(
        target_os = "ios",
        not(any(feature = "cold-start-probe", feature = "frame-metrics"))
    ),
    test
))]
pub mod soft_keyboard;
// TASK-17.2（REQ-17）: TV フォーカスナビゲーションのプラットフォーム統合レイヤ
// （キー入力ブリッジ・FocusRing 駆動・reveal・ハイライト描画）と検証用サンプル画面。
// Android 限定ではなく全ターゲットでコンパイル対象にする（デスクトップの矢印キーでも
// 同じ経路を駆動できるようにするタスク制約、`tv_focus.rs` モジュール doc 参照）。
pub mod tv_focus;
pub mod tv_sample;
// TASK-5.3（REQ-5 / M4、Issue #238）: 固定 60Hz 環境でのフレームレート計測専用アプリ。
// non-default feature `frame-metrics`（`Cargo.toml` 参照）でのみコンパイル対象になる
// 加法的な追加（`frame_metrics.rs` モジュール doc 参照）。Web の計測は既存の
// `web/index.html` `?measure=1` 計装（TASK-4.5）を使うため wasm32 は対象外にする。
#[cfg(all(feature = "frame-metrics", not(target_arch = "wasm32")))]
pub mod frame_metrics;
#[cfg(target_os = "ios")]
pub mod ios;
// Issue #795: `ios.rs::run` 専用の `TextInputApp` ラッパー（ソフトキーボードの
// 文字入力ブリッジ）。`ios.rs` からのみ使う内部モジュールのため非公開にする。
// FFI 境界（`TextInputApp` 本体・`#[no_mangle]` コールバック）は
// `#[cfg(target_os = "ios")]` でモジュール内側に閉じるが、キュー処理の純関数
// （`drain_pending`）はホストでも単体テストできるよう `test` cfg でも
// コンパイル対象にする（`soft_keyboard` モジュールと同じ形、モジュール doc 参照）。
// 計測専用ビルド（`ios.rs::run` が `TextInputApp` を使わない分岐）では外す（Issue #19）。
// `soft_keyboard` と `build.rs` の `.m` コンパイルも同じ条件に揃えている。
#[cfg(any(
    all(
        target_os = "ios",
        not(any(feature = "cold-start-probe", feature = "frame-metrics"))
    ),
    test
))]
mod ios_text_input;
#[cfg(target_arch = "wasm32")]
pub mod web;

// TASK-2.3（#54）計測マーカー: TASK-2.2-S5（#52）で `PrototypeApp` の再エクスポートを
// 追加した変更行（境界判断で迷う既存行の書き換えのため、安全側でプラットフォーム別に計上）。
// fandhe-prototype:begin
pub use app::{PrototypeApp, ProxyApp};
// fandhe-prototype:end
pub use app::{Completion, CompletionSender, EffectFn, ProxyAppDyn, TvFocusApp};
pub use fandhe_render_egui::{
    dispatch_back_if_requested, render_screen, render_screen_with_recyclers, save_color_image,
    EguiRenderer, VirtualListRecyclers,
};

/// 画面名から実描画アプリを組み立てる登録の解決関数。未登録名は `None`。
///
/// 画面の登録は `fandhe-gallery` が持ち（Issue #1165）、各 OS の入口（[`ios::run`]・
/// [`web::start`]・[`android::run`]）へ引数で渡す。依存の向きを gallery → 本 crate の
/// 一方向に保つための逆転で、本 crate は登録の中身を知らない。
pub type BuildAppFn = fn(&str, &egui::Context) -> Option<Box<dyn ProxyAppDyn>>;

/// 描画対象の画面名を解決する（Issue #715）。
///
/// 実行時 `FANDHE_PROXY_APP` を優先し、未設定（または空文字列）ならビルド時
/// `option_env!` を見る（wasm32 / Android 等、実行時の環境変数注入手段が乏しいターゲット向け。
/// Android はさらに `adb shell setprop` 経由のフォールバック（`debug-proxy-app` feature 有効時のみ）を
/// `android.rs` 側で本関数の後段に持つ）。固定文字列との比較のみに使い、パス・コマンド・クエリへは
/// 展開しない（A03 インジェクション対策、security.md 準拠）。
pub fn selected_app_name() -> Option<String> {
    std::env::var("FANDHE_PROXY_APP")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("FANDHE_PROXY_APP").map(str::to_string))
}

/// 端末リソースの終了・破棄時の解放処理を、描画層（`fandhe-render-egui`）へ登録する。
///
/// `ProxyApp` を使う各入口（`main.rs`・`ios.rs`・`android.rs`）が起動前に 1 回呼ぶ。
/// 描画層は端末機能を知らないため、`device::shutdown`（ウィンドウを閉じる経路）と
/// `device::memory_pressure::stop_shared`（`ProxyApp` の破棄）をここで渡す。
/// `device` feature が無効なビルド・wasm32 では何もしない（`device.rs` モジュール doc 参照）。
pub fn register_teardown_hooks() {
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fandhe_render_egui::app::set_teardown_hooks(
        device::shutdown,
        device::memory_pressure::stop_shared,
    );
}
