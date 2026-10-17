//! fandhe-egui-proxy: TASK-1.2（REQ-1 / M2）の実描画検証専用プロキシクレート。
//!
//! `fandhe-core` の `Screen<S>` / `Node<S>` 宣言的 UI 記述を egui ウィジェットへ
//! マッピングし、デスクトップ相当（native / glow）・Web（wasm32 / WebGL2）の
//! 2 環境で実際にレンダリングして「視覚的要素欠落 0 件」を確認するために存在する。
//!
//! **本クレートは正式なレンダラ（wgpu / taffy / cosmic-text、REQ-8 系）ではない。**
//! `fandhe-core` は依存ゼロ・`forbid(unsafe_code)` の共通コアであり、egui/eframe への
//! 依存はこのプロキシ側にのみ閉じる（依存方向はプロキシ → コアの一方向、
//! `.claude/rules/coding-rust.md` 準拠）。呼び出し元は `main.rs`（デスクトップ相当の
//! 自動起動・自動撮影）と `web.rs`（wasm エントリ）、`ios.rs`（iOS 向け wgpu/Metal エントリ、
//! TASK-1.3-S2）、`android.rs`（Android `android_main` エントリ、TASK-1.5-S2）、
//! および `tests/desktop_render.rs`（egui_kittest による要素欠落チェック）。
//!
//! iOS のみ glow（glutin）バックエンドが非対応なため wgpu（Metal）へ切り替える
//! （PoC-1、TASK-1.3 系。`Cargo.toml` のターゲット条件分岐と対になる）。
//!
//! [`screens`]（`generated_screens!` マクロ）は `fandhe-core` の `screens/` に置いた
//! 画面（`fandhe_core::screens::*`）を、端末アプリが実行時に名前
//! （環境変数 `FANDHE_PROXY_APP`）で選んで描画するためのレジストリ（Issue #715）。
//! web / iOS / Android のエントリも [`build_app`] を経由して名前選択に対応する
//! （未登録名・未指定時は既定の [`app::ProxyApp::new`]（sample 画面）へフォールバックする）。
//!
//! Android 向け `android_main`（`android.rs`）は `android-activity` の
//! NativeActivity グルーが要求する固定シンボル名を `#[no_mangle]` で公開する必要があり、
//! `#![forbid(unsafe_code)]` と両立できない（`#[no_mangle]` は `unsafe_code` lint 対象）。
//! Android ターゲットのみ `forbid` を `deny` へ緩め、`android.rs` 側の当該関数 1 箇所に
//! `#[allow(unsafe_code)]` を最小範囲で付与する（他ターゲットは `forbid` を維持し、
//! `coding-rust.md` の FFI 例外規定に従う）。
//!
//! iOS も同じ理由で `deny` へ緩める（Issue #751）: `device/ios.rs` が UIKit 実装
//! （`ios/fandhe_ios_camera.m`、`build.rs` でコンパイルする C ABI 1 点）を呼ぶために
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
//! Issue #934 で `device/apple_gamepad.rs`（`apple/fandhe_apple_gamepad.m` の
//! `GCController` 実装を呼ぶ）も同じ緩和へ相乗りした。
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
//! `IPropertyStore`/`IPersistFile`/`CoInitializeEx` 系の呼び出し）も同じ緩和へ
//! 相乗りした。
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

use fandhe_core::{
    CanvasWidget, Checkbox, DialogWidget, Event, ImageWidget, Measurement, Node, PaintCommand,
    RadioButton, Screen, Slider, StateStore, ToggleSwitch, Widget, WidgetValue,
};

pub mod accessibility;
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
pub mod font;
// Issue #1103: `ImageWidget` の実画素を egui へ届ける 1 枠の保管所（モジュール doc 参照）。
mod image_store;
pub mod recyclers;
mod rgba_image;
// Issue #715: 生成画面を名前で選ぶレジストリ（モジュール冒頭 doc 参照）。
pub mod screens;
// Issue #813（親 #811 の 2/3）: `Node::StyledText` の役割（`TextRole`）を
// `TokenResolver` 経由の色へ解決するモジュール。`text_role_color` を結合テスト
// （`tests/styled_text_render.rs`）から呼べるよう公開する（`pub(crate)` 不可）。
pub mod text_role;
// Issue #828: `PaintCommand::Circle` → egui `Shape` への純粋変換（`Node::Widget`
// 分岐への実配線は #830 が担う。モジュール冒頭 doc 参照）。
pub mod paint_command;
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
// Issue #27: UIScene 化シムのリンク用アンカー（`main` から呼ぶ）。
#[cfg(target_os = "ios")]
pub mod ios_scene;
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
pub use recyclers::VirtualListRecyclers;
pub use screens::build_app;

/// `screen` を `store` の現在状態で 1 フレーム分描画し、発生した UI イベントを
/// `store.dispatch` へ配線する。
///
/// 状態への書き込みは `fandhe_core::Event`（`ButtonClickedAt` / `TextFieldChanged`）を
/// 経由した `StateStore::dispatch` のみに限定し、`&mut S` をこの関数の外へ
/// 直接公開しない契約を維持する（`state::store` モジュールの設計方針を踏襲）。
/// `Node::Widget(DialogWidget)` のパネル外タップ dismiss（Issue #739、`render_node` の
/// 該当アーム参照）も同じ `Event::ButtonClickedAt` を `ctx.pending_events` へ積むだけで、
/// この契約から逸脱しない。
/// `Node::Button` / `Node::IndexedButton` はいずれも位置解決
/// （`Event::ButtonClickedAt` / `ui::click_button_at`）で dispatch する
/// （Issue #491。動的生成子内のボタンをラベル一致の非展開走査では解決できない
/// 制約の解消。`render_node` の該当アーム参照）。
///
/// 走査中は `store.state()`（共有参照）のみを借用し、発火したイベントは走査完了後に
/// まとめて `dispatch` する（走査中に `store` を可変借用しないための実装上の制約）。
///
/// TASK-5.5 の残課題（#83 クローズコメント、Issue #487）: `Node::List` は
/// `fandhe_core::ui::Screen::visit_expanded` と同じ「都度全展開」の意味論を保つため
/// 自前で再帰展開するが、`Node::VirtualList` のみ [`render_virtual_list`]
/// （`egui::ScrollArea::show_viewport` + `compute_visible_range` による可視範囲限定
/// 描画）へ分岐させる必要がある。`Screen::visit_expanded` は `List`/`VirtualList` を
/// 区別せず一律に全展開する 1 本の走査 API であり、このクロージャ内から
/// 「`VirtualList` だけ全展開させない」制御はできない（`visit_node_expanded` は
/// `f(node)` の後に無条件で子を展開する、screen.rs 参照）。そのため本関数は
/// `Screen::visit_expanded` を使わず、[`render_node`] による自前の事前順（pre-order）
/// 再帰走査へ置き換える。走査順序・「`List` 自身は行を描かない」という契約は
/// `visit_node_expanded` と同一に保つ（TASK-1.2 の要素欠落 0 件テストが依存する契約）。
/// なお fandhe-core 側の窓掛け走査 `Screen::visit_windowed` も本呼び出し形状には
/// 使えない: 可視範囲 `range` を呼び出し前に確定させる契約（本関数は
/// `ScrollArea::show_viewport` が返すビューポートから range を求める）・
/// 単一 range を全 `VirtualList` へ一律適用する設計（ネストごとの独立スクロールと
/// 非両立）・コールバックへ絶対 index を渡さない（`id_salt` に必須）の 3 点による。
///
/// `S: 'static` 境界は [`render_node`] 内の `DialogWidget<S>` への
/// `downcast_ref`（`Any` の要件）に必要（Issue #739）。アプリ状態は通常所有型で
/// 借用を含まないため、実務上の制約にはならない。
pub fn render_screen<S: 'static>(ui: &mut egui::Ui, screen: &Screen<S>, store: &mut StateStore<S>) {
    // `render_screen` は「フレーム跨ぎの再利用は不要な呼び出し元向けの互換ラッパー」
    // という位置付けにする（#490）。関数ローカルの一時プールで委譲するため、
    // 挙動は毎フレーム可視分を全件新規構築するのみに保たれる。
    // フレーム間再利用が必要な呼び出し元は `render_screen_with_recyclers` を使う契約
    // （`recyclers` モジュール doc・`VirtualListRecyclers` の設計判断を参照）。
    let mut recyclers = VirtualListRecyclers::new();
    render_screen_with_recyclers(ui, screen, store, &mut recyclers);
}

/// [`render_screen`] のフレーム跨ぎ再利用版（#490）。`recyclers` を呼び出し側
/// （[`app::ProxyApp`] / [`app::PrototypeApp`] / 結合テストハーネス）がフィールドとして
/// フレームをまたいで保持し、本関数へ毎フレーム渡す契約にする（`recyclers` モジュール doc
/// 参照。egui 側の永続領域には保持しない設計判断）。
///
/// 可視範囲内の `Node::VirtualList` 項目ノードは常に
/// `VirtualListRecyclers::take`/`ListRecycler::acquire`/`VirtualListRecyclers::restore`
/// を経由して取得し、新規構築を「新たに可視域へ入った項目」または
/// `StateStore::version` が進んだ（＝状態変化があった）ときのみに限定する
/// （REQ-5 受け入れ基準「immediate mode に起因する性能劣化がないこと」）。
///
/// `S: 'static` 境界は [`render_screen`] と同じ理由（`DialogWidget<S>` への
/// `downcast_ref`、Issue #739）で必要。
pub fn render_screen_with_recyclers<S: 'static>(
    ui: &mut egui::Ui,
    screen: &Screen<S>,
    store: &mut StateStore<S>,
    recyclers: &mut VirtualListRecyclers<S>,
) {
    // Issue #616 症状 B / #663: Linux（fcitx5 既定の XIM `PreeditNothing`）では
    // egui-winit が `Preedit` を一切合成せず `Ime(Commit(s))` のみが届き、egui 側の
    // `ime_cursor_range`（既定値のまま）とカーソルが不一致で確定文字列が破棄される。
    // 描画開始前にイベント列を正規化し、この破棄を経路ごと消す
    // （`inject_preedit_before_ime_commit` doc 参照）。
    ui.ctx()
        .input_mut(|input| inject_preedit_before_ime_commit(&mut input.events));

    // version はキャッシュ無効化キー（`ListRecycler::acquire` 参照）。走査開始前の
    // 一時点で確定させ、走査中の状態変化（本関数は `dispatch` を走査完了後にまとめて
    // 行うため発生しない）と混同しない。
    let version = store.version();

    // 前フレームで確定したプール一式を「前フレーム分」へ退避し、今フレームの
    // 確定プール集合を空から開始する（`VirtualListRecyclers::begin_frame` 参照。
    // 描画されなくなった `path` のプールをここで解放し、メモリを有界に保つ）。
    recyclers.begin_frame();
    rgba_image::begin_frame(ui.ctx());

    let mut ctx = RenderContext {
        version,
        pending_events: Vec::new(),
        recyclers,
    };

    ui.heading(screen.title());
    ui.separator();

    let state = store.state();
    // 見出しは固定せず本文と一緒にスクロールさせる（既存の見た目を保つ最小の変更）。
    // VirtualList を含む画面も包む。VirtualList 側は見えている高さを上限にした内側
    // スクロールなので、仮想化は有限の viewport で効いたまま、ボタン群を越えて外側で
    // 一覧まで届く（Issue #77）。
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        // 画面ごとに位置を分ける（共通の id だと、画面を移ったとき前の画面の位置を引き継ぐ）。
        // ponytail: 見出しで分けるので、同じ見出しの画面どうしは位置を共有する。
        .id_salt(("fandhe_screen_root_scroll", screen.title()))
        .show(ui, |ui| {
            for (index, node) in screen.iter().enumerate() {
                render_node(ui, node, state, &[index], &mut ctx);
            }
        });

    for event in ctx.pending_events {
        store.dispatch(screen, &event);
    }
}

/// 縦 `VirtualList` の内側スクロールの高さ上限。外側（画面ルート）スクロールで
/// 見えている高さを使い、窓が極端に低くても数行分は確保する（Issue #77）。
fn virtual_list_max_height(ui: &egui::Ui, item_extent: f32) -> f32 {
    ui.clip_rect().height().max(item_extent * 3.0)
}

/// OS の戻る操作を検知して `Event::BackRequested` を dispatch する（Issue #726）。
///
/// [`app::ProxyApp`]（`ScreenSource::FromState`）が毎フレーム [`render_screen_with_recyclers`]
/// の後に呼ぶ想定の窓口で、kittest ベースの結合テスト（`tests/screen_stack_render.rs`）からも
/// 同じ実装を共有する（描画と戻る検知で別実装を持たない、二重管理を避ける設計判断）。
///
/// - `egui::Key::Escape`: macOS/Linux/Windows のデスクトップ入力
/// - `egui::Key::BrowserBack`: Android の KEYCODE_BACK。winit 0.30 系が
///   `NamedKey::BrowserBack` へ正規化し、egui-winit が `Key::BrowserBack` へ写像する
///   （JNI 側の追加配線は不要）
///
/// 発火した場合は `true` を返す（呼び出し元は戻り値を無視してよい）。
///
/// `Escape` はテキスト入力欄へフォーカス中（IME 変換のキャンセル・編集の取り消し目的の
/// 押下を含む）は戻る操作へ変換しない。フォーカス中の `Id` が `TextEdit` の状態として
/// 保存されているか（`egui::text_edit::TextEditState::load`）で判定する。単に
/// `ctx.memory(|m| m.focused())` の有無だけを見ると、Button 等 TextEdit 以外の
/// ウィジェットへ Tab フォーカスした場合も Esc が除外されてしまう
/// （Issue #726 レビュー指摘 P1）。`BrowserBack`
/// （Android の物理戻るキー）はテキスト入力中でも常に戻る操作として扱う。
///
/// `Escape` は `ctx.input_mut(|i| i.consume_key(...))` で消費状態を見て判定する
/// （`i.key_pressed` だけを見ると、Esc で閉じるオーバーレイ（`egui::Modal` の
/// `ModalResponse::should_close` 等）が同フレーム内でその Esc を消費して閉じたあとでも、
/// 同じ Esc が二重に画面スタックの pop へ渡ってしまう。PR #733 レビュー指摘 P1）。
/// この関数は描画（[`render_screen_with_recyclers`]）の後に呼ぶ契約のため、描画中に
/// オーバーレイ側が `consume_key` 済みなら、ここでの `consume_key` は false を返す。
/// なお `egui::Popup`（コンボボックス等）の Esc クローズは egui 側が `consume_key` を
/// 呼ばず `key_pressed` のみで判定しているため、この仕組みでも救えない
/// （upstream の既知の制約。`egui::Modal` 等 `consume_key` を呼ぶオーバーレイのみ対象）。
///
/// `Escape` / `BrowserBack` を長押しした場合、OS はキーリピートとして毎フレーム
/// `Event::Key { pressed: true, repeat: true, .. }` を送り続ける（egui 0.33.3
/// `input_state/mod.rs` の `count_and_consume_key` を実査済み。`pressed: true` のみで
/// 一致判定しており `repeat` の値は見ない）。この関数はキーリピートのフレームでは
/// 発火させない（[`consume_key_ignoring_repeat`] 参照）。ただし repeat のみのフレームでも
/// 一致したイベントは consume する（consume しないまま放置すると、同じキー押下が
/// 別のフレーム・別の判定ロジックへ漏れて二重処理される恐れがあるため、
/// 上記コメントの「同フレーム内の二重消費を避ける」契約は保つ）。
pub fn dispatch_back_if_requested<S>(
    ctx: &egui::Context,
    screen: &Screen<S>,
    store: &mut StateStore<S>,
) -> bool {
    // Cursor Bugbot 指摘（PR #733）: `egui::Memory::set_focus_lock_filter`（`escape: true`
    // をテキスト入力へ適用する箇所、`Node::TextField` 描画部）は
    // `had_focus_last_frame(id) && has_focus(id)` の両方を満たすフレームでしか
    // filter を書き換えない（egui 0.33.3 `memory/mod.rs` L860 実査済み）。フォーカス
    // 獲得直後の 1 フレーム（`had_focus_last_frame` が false）は素通りし、
    // 続く 1 フレームも `Focus::begin_pass`（同 L525〜）が Esc を「filter 未設定」として
    // 先に消費してフォーカスを剥奪してしまうため、結局 filter が有効になるまでの間に
    // 最短で 1 フレーム、`escape: true` を適用できない隙間ができる。このフレームでは
    // `ctx.memory(|m| m.focused())` が既に `None` を返すため、直後の判定だけでは
    // 検知できない。そこで「直前にこの関数を呼んだフレームでテキスト入力が
    // フォーカスしていたか」を `text_input_focused_last_frame_id` の一時領域に
    // 記録し、フォールバックとして OR する（剥奪される直前のフレームの情報を補う）。
    //
    // codex-review 指摘 P2（PR #733）: この関数は [`app::ProxyApp::update`] から
    // 毎フレーム呼ばれる契約のため（同関数のコメント参照）、一時領域は
    // 「今フレームの生の判定結果」で必ず毎回上書きする。「読み出し → 今フレームの
    // 生の判定 → 書き込み」の順で必ず両方行うことで、フォールバックの寿命を
    // 厳密に 1 フレームへ限定する（この順序を崩すと、Tab/クリックで Button 等へ
    // フォーカスを移した直後のフレームでも古い `true` が読まれ、Esc が誤って
    // 無視される）。
    let focused_now = ctx.memory(|m| m.focused());
    let text_input_focused_now =
        focused_now.is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some());
    // 「読み出し → 今フレームの生の判定で上書き」を必ず両方行う（P2 是正）。
    let focused_last_frame = ctx.data_mut(|d| {
        let id = text_input_focused_last_frame_id();
        let previous = d.get_temp::<bool>(id).unwrap_or(false);
        d.insert_temp(id, text_input_focused_now);
        previous
    });
    // フォールバックは「フォーカスを剥奪された直後」の埋め合わせに使う情報のため、
    // 今フレームが実際に何もフォーカスしていない場合に限って参照する
    // （Button 等へ正常にフォーカスが移っていれば `focused_now` が `Some` になり
    // 対象から外れる）。
    let text_input_focused =
        text_input_focused_now || (focused_last_frame && focused_now.is_none());
    let browser_back_pressed = ctx.input_mut(|i| {
        consume_key_ignoring_repeat(i, egui::Modifiers::NONE, egui::Key::BrowserBack)
    });
    let escape_requested = !text_input_focused
        && ctx.input_mut(|i| {
            consume_key_ignoring_repeat(i, egui::Modifiers::NONE, egui::Key::Escape)
        });
    if !escape_requested && !browser_back_pressed {
        return false;
    }
    store.dispatch(screen, &Event::BackRequested)
}

/// [`dispatch_back_if_requested`] が「直前フレームでテキスト入力がフォーカスしていたか」を
/// 一時記憶するための `egui::Id`。`Node::TextField` 描画部（本ファイル）が書き込み、
/// `dispatch_back_if_requested` が読んで消費する（Cursor Bugbot 指摘、PR #733）。
fn text_input_focused_last_frame_id() -> egui::Id {
    egui::Id::new("fandhe_egui_proxy_text_input_focused_last_frame")
}

/// `egui::InputState::consume_key` 相当だが、キーリピートのみのフレームでは `false` を
/// 返す。一致したイベント（repeat 含む）は `consume_key` と同様にすべて消費するため、
/// 呼び出し元・他の判定ロジックへ同じキー押下が漏れて二重処理される事態は防ぐ
/// （[`dispatch_back_if_requested`] 参照）。
fn consume_key_ignoring_repeat(
    input: &mut egui::InputState,
    modifiers: egui::Modifiers,
    logical_key: egui::Key,
) -> bool {
    let mut non_repeat_match = false;
    input.events.retain(|event| {
        let egui::Event::Key {
            key: ev_key,
            modifiers: ev_mods,
            pressed: true,
            repeat,
            ..
        } = event
        else {
            return true;
        };
        if *ev_key != logical_key || !ev_mods.matches_logically(modifiers) {
            return true;
        }
        if !*repeat {
            non_repeat_match = true;
        }
        false
    });
    non_repeat_match
}

/// IME の `Commit` 直前に同一文字列の `Preedit` を合成注入し、カーソルが先頭以外の
/// ときに確定文字列が無言で破棄される問題を防ぐ（Issue #616 症状 B / #663）。
/// [`render_screen_with_recyclers`] の描画開始前にイベント列へ適用する。
///
/// ## 機序（egui-winit 0.33.3 / egui 0.33.3 のソース実査で確認済み）
///
/// egui-winit は Linux では winit の `Ime::Enabled` を無視し、`Preedit(_, Some)`
/// 受信時にのみ `egui::Event::Ime(ImeEvent::Enabled)` を合成する。XIM の既定スタイル
/// （`PreeditNothing`、fcitx5 の既定設定）では `Preedit` が一切届かず、
/// `Ime(Commit(s))` のみが egui へ渡る。
///
/// egui の `TextEdit` は `Commit` を
/// `cursor_range.secondary.index == state.ime_cursor_range.secondary.index`
/// のときにしか挿入しない。`ime_cursor_range` は `Enabled`/`Preedit` 受信時にのみ
/// 更新される既定値（index 0）のまま残るため、カーソルが 0 以外だと不一致になり
/// 確定文字列が破棄される。`TextEditState` の当該フィールドは `pub(crate)` のため
/// proxy 側から直接同期することはできない。
///
/// `Commit` の直前に同一文字列の `Preedit` を挿入すると、egui の `Preedit` ハンドラが
/// `ime_cursor_range` を現在カーソルへ同期してから `Commit` を処理する
/// （Windows/macOS で実際に流れる正規シーケンス `Preedit → Commit` と同型）。
///
/// ## 冪等性・既存挙動の非破壊
///
/// - 直前が同一文字列の `Preedit` であれば注入しない（Windows/macOS 側の正規シーケンス・
///   `UseOnTheSpot=True` 設定の fcitx5 で二重注入しない）
/// - `Commit("")`・`Commit("\n")`・`Commit("\r")` は egui 側で no-op 扱いのため対象外にする
///   （`Preedit("")` は選択削除の副作用を持つため、注入すると既存挙動を変えてしまう）
fn inject_preedit_before_ime_commit(events: &mut Vec<egui::Event>) {
    let mut index = 0;
    while index < events.len() {
        if let egui::Event::Ime(egui::ImeEvent::Commit(text)) = &events[index] {
            let text = text.clone();
            let is_noop_commit = text.is_empty() || text == "\n" || text == "\r";
            let already_synced_by_preceding_preedit = index > 0
                && matches!(
                    &events[index - 1],
                    egui::Event::Ime(egui::ImeEvent::Preedit(preedit)) if *preedit == text
                );
            if !is_noop_commit && !already_synced_by_preceding_preedit {
                events.insert(index, egui::Event::Ime(egui::ImeEvent::Preedit(text)));
                index += 1; // 挿入した Preedit をスキップし、直後の Commit 側へ進める
            }
        }
        index += 1;
    }
}

/// [`render_node`] / [`render_virtual_list`] の再帰全体で共有する可変状態をまとめた
/// コンテキスト。引数増加による clippy `too_many_arguments` を避けつつ、
/// 「走査完了後にまとめて dispatch する」契約（[`render_screen_with_recyclers`] doc
/// 参照）と「`VirtualListRecyclers` はフレーム跨ぎで呼び出し側が保持する」契約
/// （`recyclers` モジュール doc 参照）を 1 箇所へ集約する。
struct RenderContext<'a, S> {
    /// [`fandhe_core::StateStore::version`]。`ListRecycler::acquire` の
    /// キャッシュ無効化キーとして `render_virtual_list` からのみ参照する。
    version: u64,
    /// 走査中に発火したイベント。走査完了後に [`render_screen_with_recyclers`] が
    /// まとめて `store.dispatch` する（`&mut S` を走査中に公開しない契約）。
    pending_events: Vec<Event>,
    /// 呼び出し側がフレームをまたいで保持するプール（`recyclers` モジュール doc 参照）。
    recyclers: &'a mut VirtualListRecyclers<S>,
}

/// [`render_screen_with_recyclers`] の再帰本体。`node` 1 件を描画し、`List` は子ノードを
/// 展開して再帰的に描画する（`fandhe_core::ui::Screen::visit_expanded` と同じ事前順走査・
/// 「`List` 自身は行を描かない」契約）。`VirtualList` のみ
/// [`render_virtual_list`] へ委譲し、可視範囲限定・フレーム間再利用の描画経路を通す。
///
/// `path` は画面ツリー上の位置（兄弟内の挿入順 index の列）を表し、
/// `Node::VirtualList` の `egui::ScrollArea` に安定な `id_salt` を与えるためと、
/// `VirtualListRecyclers` のプール識別キーの双方に使う（呼び出しごとに毎回異なる
/// 位置情報になるとスクロール状態・再利用プールの双方が毎フレームリセットされるため、
/// ツリー構造が変わらない限り不変な値が必要）。
fn render_node<S: 'static>(
    ui: &mut egui::Ui,
    node: &Node<S>,
    state: &S,
    path: &[usize],
    ctx: &mut RenderContext<S>,
) {
    match node {
        Node::Text(text) => {
            ui.label(text);
        }
        Node::DynamicText { get } => {
            // `Node::Text` と同じラベル描画。都度 `get(state)` を評価するため
            // 状態変化にフレームごと追随する（Issue #581、node.rs 参照）。
            ui.label(get(state));
        }
        Node::StyledText { role, get } => {
            // `role` に応じた色解決（`TokenResolver` 経由、macOS/Android では live
            // region 公開も行う）は `text_role.rs` に委譲する（#813 で解決済み）。
            text_role::styled_label(ui, *role, get(state));
        }
        Node::TextField { label, get, .. } => {
            let mut buffer = get(state);
            ui.horizontal(|ui| {
                let label_response = ui.label(label);
                // `labelled_by` で隣接ラベルをアクセシブルネームとして関連付ける
                // （AccessKit ツリー上でラベル文字列から入力欄自体を検索できるようにする。
                // egui_kittest の `get_by_label` は本関連付けに依存するため必須）。
                // Issue #614: 既定の `text_edit_singleline` は Enter を
                // `TextEdit::return_key`（既定 Some(Enter)）にマッチさせて
                // `surrender_focus` する（egui 0.33.3 builder.rs L1006-1018）。
                // IME の確定操作も Enter キーのため、日本語入力中は一語確定
                // するたびに入力欄からフォーカスが外れ連続入力できなくなる。
                // `return_key(None)` で当該分岐自体を無効化し、単一行
                // TextField 上の Enter を no-op（フォーカス維持・改行なし）
                // にする。フォーカス離脱は Tab / クリックのみが担う。
                let field_response = ui
                    .add(egui::TextEdit::singleline(&mut buffer).return_key(None))
                    .labelled_by(label_response.id);
                // Issue #726 codex-review 指摘: egui は `TextEdit` の既定設定
                // （`EventFilter::escape == false`）では Esc 押下時に
                // `Memory::begin_pass` がユーザーコードより先にフォーカスを
                // 剥奪してしまう（IME 変換・編集キャンセル目的の Esc と区別できない）。
                // このため `dispatch_back_if_requested`（同ファイル下部）が
                // 「Esc 時にテキスト入力がフォーカス中か」を見ようとしても、
                // 見た時点で既にフォーカスが外れており判定できない。
                // フォーカス確定後（前フレームと今フレームの双方でフォーカス中）に
                // `escape: true` を明示指定し、Esc を「消費されたが no-op」として
                // TextField 側に留め、egui 自身の自動デフォーカスを止める
                // （`set_focus_lock_filter` は初回フォーカス直後の 1 フレームは
                // 適用されない egui 側の設計のため、以降のフレームで確実に効く）。
                ui.memory_mut(|memory| {
                    memory.set_focus_lock_filter(
                        field_response.id,
                        egui::EventFilter {
                            escape: true,
                            ..Default::default()
                        },
                    );
                });
                // Cursor Bugbot 指摘（PR #733）: 上の `set_focus_lock_filter` の
                // ガード（`had_focus_last_frame && has_focus`）が満たされず filter が
                // まだ効いていないフレームでは、`egui::Memory::begin_pass` が
                // ここへ到達する前に Esc でフォーカスを剥奪済みになる場合がある。
                // このフレームでは `field_response.has_focus()` も既に `false` を返すため、
                // 剥奪される直前のフレームの情報は `dispatch_back_if_requested` 側が
                // 自前で 1 フレーム分だけ持ち越して補う（codex-review 指摘 P2、
                // detail は同関数のコメント参照。記録・参照とも同関数へ一本化しており、
                // ここでは一時領域への書き込みを行わない）。
                if field_response.changed() {
                    ctx.pending_events.push(Event::TextFieldChanged {
                        label: label.clone(),
                        value: buffer,
                    });
                }
            });
        }
        Node::Button { label, .. } => {
            // Issue #491: 動的生成子（`List`/`VirtualList`）内のボタンをラベル一致の
            // 非展開走査（`Event::ButtonClicked`）では解決できないため、位置解決
            // （`Event::ButtonClickedAt` / `ui::click_button_at`）へ統一する。トップレベル
            // に置かれた既存のボタンも同じ経路（`path` 長 1）を通るが、`click_button_at`
            // はラベル一致の整合性ガードを最終的に必ず通すため、既存の「ラベル一致で
            // 発火する」挙動自体は変わらない（`path` はレンダラが構築する一意な位置を
            // 使うだけで、二重解決や意味論変更を持ち込まない）。
            if ui.button(label).clicked() {
                ctx.pending_events.push(Event::ButtonClickedAt {
                    label: label.clone(),
                    path: path.to_vec(),
                });
            }
        }
        Node::IndexedButton { label, .. } => {
            // 動的生成子が埋め込んだ index はノード自身が保持しているため、
            // dispatch 側は `path` のみで対象ノードへ再到達すればよい
            // （`click_button_at` が葉ノードから `index` を取り出して `on_click` へ渡す）。
            if ui.button(label).clicked() {
                ctx.pending_events.push(Event::ButtonClickedAt {
                    label: label.clone(),
                    path: path.to_vec(),
                });
            }
        }
        Node::List { expand, .. } => {
            // `List` 自身は行を描かない。子ノードを都度全展開し、事前順で再帰的に
            // 描画する（`Screen::visit_node_expanded` と同じ契約、node.rs 参照）。
            // `child_path` はループ外で 1 度だけ確保し、末尾要素の書き換えで
            // 使い回す（子ごとの再確保を避ける。深いネスト・大量要素での無駄な
            // アロケーション削減）。
            let mut child_path = path.to_vec();
            child_path.push(0);
            for (child_index, child) in expand(state).into_iter().enumerate() {
                if let Some(last) = child_path.last_mut() {
                    *last = child_index;
                }
                render_node(ui, &child, state, &child_path, ctx);
            }
        }
        Node::VirtualList {
            len,
            item,
            item_extent,
            axis,
            ..
        } => {
            render_virtual_list(ui, path, *len, *item, *item_extent, *axis, state, ctx);
        }
        Node::RgbaImage { pixels, alt } => rgba_image::show(ui, pixels, alt, path),
        Node::Widget(widget) => {
            // `Widget<S>` トレイトオブジェクトの実描画配線は、`DialogWidget`（S47、
            // Issue #739）と `Checkbox` / `ToggleSwitch` / `Slider` / `RadioButton`
            // （Issue #806・親 #804）を egui 標準部品へ配線済み（下記ブロック参照）。
            // それ以外の具象型（レイアウトプロトコル S4 等が未確定）は引き続き
            // AX ノード公開のみに留める（TASK-8.3-S2、Issue #97、
            // out-of-scope-tracking.md 対象）。
            //
            // AX ツリーへの公開は TASK-9.1-S2（REQ-9、Issue #215）で配線した。
            // `accessibility_role()` / `accessibility_label()` / `interaction_state()`
            // は `&self` 経由のみで済むため、上記の未配線契約に抵触しない。
            // S1（#214）で macOS 通常ビルドの eframe "accesskit" feature を有効化した
            // （`Cargo.toml` の `cfg(target_os = "macos")` 専用 target 分岐参照）。
            // Linux CI は `target_os = "macos"` に一致しないため本ブロックは
            // コンパイル対象外となり、CI を壊さない（macOS 実機での VoiceOver 読み上げ
            // 確認は人間担当、S3 レポート §5・Issue #216 の reopen 運用で引き継ぐ、
            // out-of-scope-tracking.md 対象）。
            //
            // TASK-9.3-S2（REQ-9 / M4、Issue #243）: Android（GameActivity 方式、#242）
            // 向けにも同じ AX ノード公開経路を展開する。`Cargo.toml` の
            // `cfg(target_os = "android")` 分岐で eframe "accesskit" +
            // accesskit_winit "accesskit_android" を有効化済みで、
            // `accesskit_node_builder` 呼び出しは macOS と同一の `&self` 経由のみのため
            // 追加配線は不要。
            // TalkBack 実機での読み上げ確認は S3（#244、人間担当）へ引き継ぐ。
            //
            // Issue #1001（REQ-9）: Windows（ナレーター）向けにも同じ AX ノード公開経路を
            // 展開する。`Cargo.toml` の `cfg(target_os = "windows")` 分岐で eframe
            // "accesskit" を有効化済みで、`accesskit_node_builder` 呼び出しは
            // macOS / Android と同一の `&self` 経由のみのため追加配線は不要。ナレーター
            // 実機での読み上げ確認は人間担当（実機・msvc 環境がないため未確認、
            // out-of-scope-tracking.md 対象）。
            // 呼び出しは `dyn Widget<S>` トレイトオブジェクト経由のメソッド解決であり、
            // ジェネリック境界（`T: Widget<S>`）と異なりトレイト自体のスコープ内 `use` を
            // 要求しない（Rust の trait object メソッド解決の仕様）。そのため
            // `use fandhe_core::Widget as _;` は不要（unused import 警告を検証で確認済み）。
            //
            // Issue #806: `Checkbox` / `ToggleSwitch` / `Slider` / `RadioButton` は
            // egui 標準部品（下記ブロック）が自前で正しい accesskit ノード（role・label）
            // を生成する。この 4 型に対して以下の合成 AX ノードも重ねて登録すると、
            // 同一ラベル・同一 role のノードが 2 つ存在することになり
            // `egui_kittest::get_by_label` 等が一意に解決できなくなる（実測で確認済み）。
            // `as_any` は型判定のみに使い、クローン前の `widget` から直接呼べる
            // （`sync_from_state` 前でも型自体は変わらない）。
            #[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
            let is_natively_rendered = widget.as_any().is_some_and(|any| {
                any.is::<Checkbox<S>>()
                    || any.is::<ToggleSwitch<S>>()
                    || any.is::<Slider<S>>()
                    || any.is::<RadioButton<S>>()
            });
            #[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
            if !is_natively_rendered {
                let ax_id = egui::Id::new(("fandhe-widget-ax", widget.id().raw()));
                let role = widget.accessibility_role();
                let label = widget.accessibility_label();
                let interaction = widget.interaction_state();
                ui.ctx().accesskit_node_builder(ax_id, |node| {
                    *node =
                        crate::accessibility::widget_ax_node(role, label.as_deref(), interaction);
                });
            }

            // `DialogWidget`（S47）・`Checkbox` / `ToggleSwitch` / `Slider` /
            // `RadioButton`（Issue #806）を実描画する。`Checkbox` とは別系統の自己完結型
            // `CheckboxWidget`（`checkbox.rs`）は未配線のまま（#806 のスコープ外）。reconcile 契約
            // （`Widget::sync_from_state` は `&mut self` を要求し `&Node<S>` の
            // 共有参照走査からは呼べない、`widget.rs` doc 参照）を満たすため
            // `clone_box` したクローンへ `sync_from_state` を適用してから
            // `as_any().downcast_ref` で具象型を取り出す。`DialogWidget::children`
            // 自体は `open_source` から直接開閉を判定するため sync 前の `widget` からでも
            // 正しく子を返すが（`dialog_widget.rs` doc 参照）、他フィールド
            // （title/body/dismiss_label のキャッシュ値）は sync 済みクローンで
            // 読むという契約に統一する。
            let mut synced = widget.clone_box();
            synced.sync_from_state(state);
            if let Some(dialog) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<DialogWidget<S>>())
            {
                if dialog.is_open() {
                    // `id()` はトレイトメソッドのため、具象型 `DialogWidget<S>` へ
                    // downcast 済みの `dialog` からではなく、trait object 経由の
                    // メソッド解決が効く `widget`（`&Box<dyn Widget<S>>`）から呼ぶ
                    // （AX ブロックと同じ理由、モジュール冒頭のコメント参照）。
                    let modal_id = egui::Id::new(("fandhe-dialog", widget.id().raw()));
                    let title = dialog.title().to_string();
                    let body = dialog.body().to_string();
                    let dismiss_label = dialog.dismiss_label().to_string();
                    let is_modal = dialog.is_modal();
                    let disabled = dialog.interaction_state().disabled;
                    // 表示は `content_nodes` 経由（`disabled` を見ず開状態なら常に
                    // 返す、`dialog_widget.rs` の PR #792 codex-review 指摘 A 対応）。
                    // `Widget::children` は disabled 中に空を返す契約に変わったため
                    // （同ファイル参照）、表示用途では使えない。
                    let children = dialog.content_nodes(state);

                    // パネル外タップ（backdrop クリック）を、パネル内の
                    // `dismiss_label` と同名 `Node::Button` の path へ変換する
                    // （既存の legacy button bridge 契約、`dialog_widget.rs` モジュール
                    // doc 参照）。同名ボタンが無い場合は `Event::ButtonClicked`
                    // （非展開走査 `click_button`）へフォールバックしない（security-auditor
                    // 指摘・A08）: `ButtonClicked` はラベル一致のみで木を走査し任意位置の
                    // 同名ボタンを発火させ得るため、path + label で対象を一意に絞る
                    // `ButtonClickedAt` 経路に限定する。外側タップで閉じたい画面は
                    // `with_content` の子に `dismiss_label` と同名の `Button` を置く契約とする。
                    // `content_nodes` の列挙 index で探すが、`resolve_node_at`
                    // （`click_button_at` の内部実装）が実際に辿るのは
                    // `Widget::children` の列挙であり、非 disabled 時は `children`
                    // （disabled 中は空を返す）と `content_nodes` の列挙順・要素は
                    // 一致する（`dialog_widget.rs` の `children` 実装が非 disabled 時に
                    // `content_nodes` へ委譲するため）。よってこの index は非 disabled
                    // 時のみ実際に解決可能な path になる。
                    let dismiss_event =
                        children
                            .iter()
                            .enumerate()
                            .find_map(|(child_index, child)| match child {
                                Node::Button { label, .. } if label == &dismiss_label => {
                                    let mut child_path = path.to_vec();
                                    child_path.push(child_index);
                                    Some(Event::ButtonClickedAt {
                                        label: dismiss_label.clone(),
                                        path: child_path,
                                    })
                                }
                                _ => None,
                            });

                    // パネル本体の内容（見出し・本文・子）を描画する。子だけを
                    // `add_enabled_ui(!disabled, ..)` で包み、disabled 中はクリックが
                    // egui 側で発火しないようにする（見た目は維持、PR #792
                    // codex-review 指摘 A）。モーダル・非モーダル両経路で共有する。
                    let panel_content = |ui: &mut egui::Ui, ctx: &mut RenderContext<S>| {
                        ui.heading(&title);
                        if !body.is_empty() {
                            ui.label(&body);
                        }
                        ui.add_enabled_ui(!disabled, |ui| {
                            let mut child_path = path.to_vec();
                            child_path.push(0);
                            for (child_index, child) in children.into_iter().enumerate() {
                                if let Some(last) = child_path.last_mut() {
                                    *last = child_index;
                                }
                                render_node(ui, &child, state, &child_path, ctx);
                            }
                        });
                    };

                    if is_modal {
                        let modal =
                            egui::Modal::new(modal_id).show(ui.ctx(), |ui| panel_content(ui, ctx));

                        // backdrop dismiss は disabled 中は積まない（入力遮断、
                        // PR #792 codex-review 指摘 A）。
                        if !disabled && modal.backdrop_response.clicked() {
                            if let Some(dismiss_event) = dismiss_event {
                                ctx.pending_events.push(dismiss_event);
                            }
                        }
                    } else {
                        // 非モーダル: `egui::Modal` は常に backdrop（クリックブロック
                        // 用の透明スクリム）を持つため使えない（PR #792 codex-review
                        // 指摘 B）。`egui::Area` + `Frame::popup` で背景入力を遮断
                        // しないパネルとして描く（backdrop なし・dismiss なし）。
                        egui::Area::new(modal_id)
                            .order(egui::Order::Foreground)
                            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                            .show(ui.ctx(), |ui| {
                                egui::Frame::popup(ui.style())
                                    .show(ui, |ui| panel_content(ui, ctx));
                            });
                    }

                    // Esc は `Modal::should_close()` を使わず、既存の
                    // `dispatch_back_if_requested` → `Event::BackRequested` →
                    // `.on_back()` 経路に一本化する（`should_close()` も内部で
                    // `consume_key(Escape)` するため、両方呼ぶと Esc 1 回の押下が
                    // どちらの経路で消費されるか競合し、二重発火・取りこぼしの
                    // 原因になる）。非モーダル経路は `egui::Modal` を使わないため
                    // 元々この競合と無縁。
                }
            } else if let Some(cb) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<Checkbox<S>>())
            {
                // `disabled` は trait object `widget` 経由で読む（AX ブロックと同じ理由）。
                // `DialogWidget` 分岐は downcast 後の `dialog` から読むが、`sync_from_state`
                // は `interaction` を更新しないため参照元の違いによる値の差は無い。
                // 以下 3 型も同じ。
                let disabled = widget.interaction_state().disabled;
                let mut checked = cb.is_checked();
                ui.add_enabled_ui(!disabled, |ui| {
                    if ui.checkbox(&mut checked, cb.label_text()).changed() {
                        ctx.pending_events.push(Event::WidgetValueChanged {
                            id: widget.id(),
                            path: path.to_vec(),
                            value: WidgetValue::Bool(checked),
                        });
                    }
                });
            } else if let Some(ts) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<ToggleSwitch<S>>())
            {
                let disabled = widget.interaction_state().disabled;
                let mut on = ts.is_on();
                ui.add_enabled_ui(!disabled, |ui| {
                    ui.horizontal(|ui| {
                        // `TextField` の `labelled_by` と同じ関連付け（ファイル冒頭
                        // 参照）。トグル自体は空文字のアクセシブルネームしか持たない
                        // （`toggle_switch` 関数参照）ため、隣接ラベルを紐付けないと
                        // `egui_kittest::get_by_label` でこの部品を一意に特定できない。
                        // ラベルは `Sense::click()` を付けて、`Checkbox` / `RadioButton`
                        // と同じく文字を押しても切り替わるようにする（Bugbot 指摘、
                        // PR #809）。
                        let label_response =
                            ui.add(egui::Label::new(ts.label_text()).sense(egui::Sense::click()));
                        let toggle_response =
                            toggle_switch(ui, &mut on).labelled_by(label_response.id);
                        let clicked = toggle_response.clicked() || label_response.clicked();
                        if label_response.clicked() {
                            on = !on;
                        }
                        if clicked {
                            ctx.pending_events.push(Event::WidgetValueChanged {
                                id: widget.id(),
                                path: path.to_vec(),
                                value: WidgetValue::Bool(on),
                            });
                        }
                    });
                });
            } else if let Some(sl) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<Slider<S>>())
            {
                let disabled = widget.interaction_state().disabled;
                let mut value = sl.value();
                ui.add_enabled_ui(!disabled, |ui| {
                    let mut slider = egui::Slider::new(&mut value, sl.min()..=sl.max())
                        .text(sl.label_text())
                        // 既定の `SliderClamping::Always` は描画するだけで現在値を
                        // step 境界へ丸め直し `changed()` を立てる（Bugbot 指摘 P1、
                        // PR #809）。ユーザー編集時のみ丸め・clamp する `Edits` にし、
                        // 「表示するだけで値が書き変わる」事故を防ぐ。
                        .clamping(egui::SliderClamping::Edits);
                    if let Some(step) = sl.step() {
                        slider = slider.step_by(step as f64);
                    }
                    // ドラッグ中は毎フレーム dispatch される。次フレームで
                    // `sync_from_state` 経由の状態値へ再読込されるため、
                    // 連続ドラッグでも取りこぼしなく反映される（Issue #806）。
                    if ui.add(slider).changed() {
                        ctx.pending_events.push(Event::WidgetValueChanged {
                            id: widget.id(),
                            path: path.to_vec(),
                            value: WidgetValue::Number(value),
                        });
                    }
                });
            } else if let Some(rb) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<RadioButton<S>>())
            {
                let disabled = widget.interaction_state().disabled;
                ui.add_enabled_ui(!disabled, |ui| {
                    if ui.radio(rb.is_selected(), rb.label_text()).clicked() {
                        ctx.pending_events.push(Event::WidgetValueChanged {
                            id: widget.id(),
                            path: path.to_vec(),
                            value: WidgetValue::Index(rb.value()),
                        });
                    }
                });
            } else if let Some(img) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<ImageWidget<S>>())
            {
                // Issue #1103: 識別子が `image_store` の画像と一致するときだけ実画素を描く
                // （復号確認済みの画像のみが置かれる）。無ければ何も描かない。AX は上の
                // 合成ノード（role Image + alt）を使うため `is_natively_rendered` には足さない。
                if let Some(tex) = crate::image_store::texture(ui.ctx(), img.source_text()) {
                    ui.add(
                        egui::Image::new(&tex)
                            .max_width(ui.available_width())
                            .max_height(320.0),
                    );
                }
            } else if let Some(canvas) = synced
                .as_any()
                .and_then(|any| any.downcast_ref::<CanvasWidget<S>>())
            {
                // Issue #830: `PaintCommand`（Issue #828）を実描画へ届ける唯一の経路。
                // `measure` は `CanvasWidget` 側で固定サイズをクランプ済みのため、
                // ここでは無限の `available` を渡してそのまま確保する
                // （レイアウトプロトコル未確定、S4 のスコープ）。
                let measured = canvas.measure(Measurement {
                    width: f32::INFINITY,
                    height: f32::INFINITY,
                });
                // Issue #355: ペン描画キャンバス（`captures_drag`）だけ drag を拾い、ルートの
                // `ScrollArea` のドラッグスクロールを止める。他画面は hover のまま（スクロール維持）。
                // 押下中の位置（キャンバス左上原点）は `CompletionSender::canvas_pointer` が渡す。
                let captures = canvas.captures_drag();
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(measured.width, measured.height),
                    if captures {
                        egui::Sense::drag()
                    } else {
                        egui::Sense::hover()
                    },
                );
                if captures {
                    let pointer = if response.is_pointer_button_down_on() {
                        response
                            .interact_pointer_pos()
                            .map(|p| (p.x - rect.min.x, p.y - rect.min.y))
                    } else {
                        None
                    };
                    // 同じフレームに「離す → 押す」が続いても、新しい接触だと分かるように押下も渡す
                    // （前のストロークとつながらないようにする）。
                    let pressed = pointer.is_some() && ui.input(|i| i.pointer.primary_pressed());
                    // このフレームでキャンバス上の接触が終わった位置。押下から解放までが 1 フレームに
                    // 収まる短いタップでも、最後の 1 点を描いてから線を終えられるようにする。
                    let released =
                        if pointer.is_none() && (response.clicked() || response.drag_stopped()) {
                            response
                                .interact_pointer_pos()
                                .or_else(|| ui.input(|i| i.pointer.latest_pos()))
                                .map(|p| (p.x - rect.min.x, p.y - rect.min.y))
                        } else {
                            None
                        };
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(crate::app::canvas_pointer_id(widget.id().raw()), pointer);
                        d.insert_temp(crate::app::canvas_pressed_id(widget.id().raw()), pressed);
                        d.insert_temp(crate::app::canvas_released_id(widget.id().raw()), released);
                    });
                }
                // `paint` は配置矩形の左上 `origin` 分だけ平行移動済みの座標を返す
                // （`ui/canvas.rs` の `Widget::paint` 契約）。
                // 端のサンプルで円がキャンバス外へはみ出さないよう、キャンバス矩形と
                // 既存 clip_rect の交差でクリップした painter で描く。
                let painter = ui.painter_at(rect.intersect(ui.clip_rect()));
                for command in canvas.paint((rect.min.x, rect.min.y), measured) {
                    match command {
                        // `StyledRect` はキャンバス全面を暗黙の境界とする契約
                        // （`ui/canvas.rs` の `translate` doc 参照）。
                        PaintCommand::StyledRect { fill } => {
                            painter.rect_filled(
                                rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(
                                    fill.r, fill.g, fill.b, fill.a,
                                ),
                            );
                        }
                        PaintCommand::Circle { .. } => {
                            crate::paint_command::paint_circle(&painter, &command);
                        }
                        // `Text`/`Rect`/`StyledText`/`CompositionText` はキャンバス経路で
                        // 使わないため素通しする（`ProgressBar` 等の他ウィジェットが
                        // 生成する値のため、ここへは通常届かない）。
                        _ => {}
                    }
                }
            }
        }
    }
}

/// egui 公式デモ（`egui_demo_lib::demo::toggle_switch`）と同型のトグルスイッチ描画。
/// `ui.toggle_value` は選択ボタン風の見た目になりスイッチ形状にならないため使わない
/// （Issue #806、ユーザー合意済み）。`Node::Widget` の `ToggleSwitch` 分岐から呼ぶ。
///
/// 既知の差分: egui 0.33 の `WidgetType` にスイッチ相当のバリアントが無く
/// `widget_info` 経由では role が `CheckBox` になる。コアの
/// `ToggleSwitch::accessibility_role()`（`AccessibilityRole::Switch`）と一致させるため、
/// `WidgetInfo` 登録後に `accesskit_node_builder` で同じノードの role を `Switch` へ
/// 直接補正する（macOS/Android/Windows のみ、下記参照。ノードを新規追加するわけではない。
/// Issue #1001 で Windows を追加）。
fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let desired_size = ui.spacing().interact_size.y * egui::vec2(2.0, 1.0);
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    // `label` を明示的に `None` へ上書きする（`WidgetInfo::selected` は空文字でも
    // `Some(String::new())` を積み、accesskit 側の `write_label_direct` がそれを
    // 「明示ラベルあり」と見なして `labelled_by` 経由の算出ラベルへフォールバック
    // しなくなる。呼び出し元（`Node::Widget` の `ToggleSwitch` 分岐）が隣接ラベルへ
    // `Response::labelled_by` で関連付ける契約のため、ここでは意図的に自前ラベルを
    // 持たない。実測で `label: None` にしないと `egui_kittest::get_by_label` が
    // 隣接ラベル文字列でこの部品を解決できないことを確認済み）。
    response.widget_info(|| egui::WidgetInfo {
        label: None,
        ..egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *on, "")
    });
    // `WidgetInfo` 経由では role が `CheckBox` になる（関数 doc の既知の差分）ため、
    // 同じノード（`response.id`）へ後追いで role のみ `Switch` へ補正する（P1、
    // PR #809 レビュー指摘）。合成 AX ノードの重複回避ガード（`Node::Widget` の
    // `is_natively_rendered` 参照）と同じ理由で macOS/Android/Windows のみで有効化する。
    #[cfg(any(target_os = "macos", target_os = "android", target_os = "windows"))]
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(crate::accessibility::to_accesskit_role(
            fandhe_core::AccessibilityRole::Switch,
        ));
    });

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let visuals = ui.style().interact_selectable(&response, *on);
        let rect = rect.expand(visuals.expansion);
        let radius = 0.5 * rect.height();
        ui.painter().rect(
            rect,
            radius,
            visuals.bg_fill,
            visuals.bg_stroke,
            egui::StrokeKind::Inside,
        );
        let circle_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(circle_x, rect.center().y);
        ui.painter()
            .circle(center, 0.75 * radius, visuals.bg_fill, visuals.fg_stroke);
    }

    response
}

/// スクロール位置に応じた前後バッファ（overscan）項目数。スクロール中に新規構築が
/// 必要になる項目を減らす目的で [`fandhe_core::compute_visible_range`] へ渡す
/// （S1 設計文書 `docs/architecture/list-virtualization-architecture.md` §3.2 の
/// overscan 引数の契約）。値そのものは実測に基づく調整余地があるため定数化し、
/// 呼び出し箇所を 1 つに保つ。
const VIRTUAL_LIST_OVERSCAN: usize = 3;

/// `Node::VirtualList` を `egui::ScrollArea::show_viewport` で可視範囲のみ描画する
/// （TASK-5.5 残課題、Issue #487・親 issue #83）。
///
/// `fandhe_core::ui::scroll`（`total_content_extent` / `clamp_scroll_offset`）と
/// `fandhe_core::ui::virtual_list::compute_visible_range` を組み合わせて呼ぶという
/// `virtual_list.rs` モジュール doc が定める契約どおりに実装する:
/// 1. [`fandhe_core::total_content_extent`] で総項目数から `ScrollArea` へ渡す
///    総コンテンツ高さを求める（スクロールバーの長さ・位置を実項目数と整合させる）。
/// 2. `show_viewport` が返す `viewport`（現在の可視領域、コンテンツ座標系）の
///    `viewport.min.y` を [`fandhe_core::clamp_scroll_offset`] で安全側にクランプする
///    （`virtual_list.rs` モジュール doc が要求する「クランプ済みオフセットを渡す」
///    呼び出し順序）。
/// 3. [`fandhe_core::compute_visible_range`] で可視 index 範囲を求め、その範囲のみ
///    `item(state, index)` を呼んで描画する（可視範囲外は `item` を一切呼ばない。
///    「可視範囲外ノードが描画されない」という Issue #487 の受け入れ条件そのもの）。
///
/// **widget id の安定化**: 各項目は `ui.push_id(("fandhe-virtual-item", index), ...)`
/// で **絶対 index**（ウィンドウ内の相対位置ではない）から決定的に `egui::Id` を
/// 生成する。ウィンドウ相対位置（`0..range.len()` の列挙位置）から id を作ると、
/// スクロールに伴いウィンドウが 1 つずれるだけで同じ index の項目の id が変わり、
/// egui 側のウィジェット状態（フォーカス・アニメーション等）が別項目へ混線する
/// （Issue #487 受け入れ条件の該当項目）。`ScrollArea` 自体の `id_salt` にも
/// `path`（画面ツリー上の位置）を与え、同一画面内に複数の `VirtualList` が
/// 存在してもスクロール状態が衝突しないようにする。
///
/// **レイアウト上の制約**: 各可視項目は `item_extent` ちょうどの高さへ拘束して
/// 描画する（`allocate_exact_size` による固定行高。項目内容が `item_extent` を
/// 超える場合は行内に収まらずはみ出すが、レイアウト位置は崩れない）。これにより
/// `compute_visible_range` が前提とする「1 項目 = `item_extent` px」の幾何モデルと
/// 実描画レイアウトが一致し、スクロールバーの長さ・位置が実項目数と整合する。
/// また `ScrollArea` は `auto_shrink([false, true])` とし、総コンテンツ高さが
/// 利用可能領域より小さい場合は縮む（後続の兄弟ノードを不要に圧迫しない）。
/// 総コンテンツ高さが利用可能領域を超える場合は残りの垂直領域全体を占有するため、
/// 常に可視であるべき兄弟ノードは `VirtualList` より前に配置すること
/// （`Node::VirtualList` に高さ指定パラメータを追加する API 拡張は別途検討）。
/// なお非有限・非正の `item_extent` は入口で `0.0` へ正規化され、可視範囲が
/// `0..0` となるためリストは空表示になる（`scroll.rs` と同じ安全側フォールバック）。
///
/// 可視範囲内の項目ノードは [`fandhe_core::ui::ListRecycler`] 経由で取得する（#490）。
/// `path` に対応するプールを `ctx.recyclers`（[`VirtualListRecyclers`]。呼び出し側が
/// フレームをまたいで保持する）から
/// [`VirtualListRecyclers::take`] で取り出し、`ListRecycler::acquire` で
/// `ctx.version`（[`fandhe_core::StateStore::version`]）をキャッシュ無効化キーとして
/// 渡し、描画後に [`VirtualListRecyclers::restore`] でプールへ戻す（`recyclers`
/// モジュール doc の take/restore パターン参照）。`acquire` が返す `Vec<&Node<S>>` は
/// 取り出した `recycler`（`ctx.recyclers` とは別のローカル変数）を借用するだけなので、
/// 項目の再帰描画中に `ctx.recyclers` へネストした `VirtualList` からアクセスしても
/// 借用は競合しない。
// Issue #540 で `axis` 引数を追加し 8 引数になった（`tv_focus.rs::render_virtual_list`
// と同型の分岐関数）。既存の `RenderContext` 集約方針はそれ以外の可変状態
// （version・pending_events・recyclers）向けであり、`axis` は 1 回の呼び出しで
// 完結する分岐条件のため無理に構造体化せず、同ファイル内の他分岐関数と同じ
// `#[allow(clippy::too_many_arguments)]` を付ける（tv_focus.rs の同名関数と同じ判断）。
#[allow(clippy::too_many_arguments)]
fn render_virtual_list<S: 'static>(
    ui: &mut egui::Ui,
    path: &[usize],
    len: fandhe_core::ReadFn<S, usize>,
    item: fandhe_core::ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    axis: fandhe_core::Axis,
    state: &S,
    ctx: &mut RenderContext<S>,
) {
    let total_len = len(state);

    // `item_extent` は DSL 側（アプリ）から供給され得る値のため、`scroll.rs` /
    // `virtual_list.rs` の防御的正規化と同じ契約（非有限・非正は安全側へ潰す）を
    // 本関数のスペーサー計算にも適用する。`compute_visible_range` 等は内部で
    // 同じガードを持つが、下記の `add_space` / `allocate_exact_size` の乗算は
    // ガードの外にあり、`NaN` がそのまま egui のレイアウトへ伝播してしまうため
    // 入口で一度だけ正規化する。
    let item_extent = if item_extent.is_finite() && item_extent > 0.0 {
        item_extent
    } else {
        0.0
    };

    // Issue #540/#551: `axis` は `Node::VirtualList` の宣言的な配置意味情報
    // （`fandhe_core::ui::node::Axis` doc 参照）。`Horizontal` は専用の横描画経路
    // （[`render_virtual_list_horizontal`]）、`Grid { columns }` は専用の複数列描画経路
    // （[`render_virtual_list_grid`]）へ分岐させる。`Vertical` のみ以下の既存の縦描画・
    // 縦仮想化のまま挙動を変えない。
    if matches!(axis, fandhe_core::Axis::Horizontal) {
        render_virtual_list_horizontal(ui, path, total_len, item, item_extent, state, ctx);
        return;
    }
    if let fandhe_core::Axis::Grid { columns } = axis {
        render_virtual_list_grid(ui, path, total_len, item, item_extent, columns, state, ctx);
        return;
    }

    let max_height = virtual_list_max_height(ui, item_extent);
    egui::ScrollArea::vertical()
        .id_salt(("fandhe-virtual-list", path.to_vec()))
        .max_height(max_height)
        // 幅は利用可能領域いっぱいに固定し、高さは総コンテンツが小さい場合に
        // 縮ませる（`[false, true]`）。高さも `false` にすると項目数が少なくても
        // 残りの垂直領域を全占有し、後続の兄弟ノードを不要に圧迫するため
        // （関数 doc の「レイアウト上の制約」参照）。
        .auto_shrink([false, true])
        .show_viewport(ui, |ui, viewport| {
            let total_extent = fandhe_core::total_content_extent(total_len, item_extent);
            let offset =
                fandhe_core::clamp_scroll_offset(viewport.min.y, total_extent, viewport.height());
            let range = fandhe_core::compute_visible_range(
                viewport.height(),
                offset,
                item_extent,
                total_len,
                VIRTUAL_LIST_OVERSCAN,
            );

            // 「1 項目 = `item_extent` px」の幾何モデルを実レイアウトへ強制するため、
            // egui 既定の項目間スペーシングをゼロにする（スペーシングが残ると
            // 実際の行送りが `item_extent + spacing` になり、`compute_visible_range`
            // の算出範囲と実際に画面へ収まる項目数がずれる）。
            ui.spacing_mut().item_spacing.y = 0.0;

            // 総コンテンツ高さを明示し、可視範囲のみ描画してもスクロールバーの
            // 長さ・位置が実項目数と整合するようにする（scroll.rs モジュール doc）。
            ui.set_height(total_extent);

            // 可視範囲より手前の項目はスペーサーで高さのみ確保し、`item` を呼ばない
            // （受け入れ条件「可視範囲外ノードが描画されない」の前半分）。
            ui.add_space(range.start as f32 * item_extent);

            // `path` に対応するプールを取り出す（`ctx.recyclers` からは消える。
            // ネストした `VirtualList` の再帰から `ctx.recyclers` へ触れても
            // このローカル `recycler` とは別物のため借用が競合しない、関数 doc 参照）。
            let mut recycler = ctx.recyclers.take(path);
            let nodes = recycler.acquire(range.clone(), ctx.version, state, item);

            // 項目 id は絶対 index から決定的に生成する（関数 doc 参照）。ツリー上の
            // 位置（`path`）は `render_node` へ引き継ぐ必要がある: ネストした
            // `VirtualList`（`item` が `Node::VirtualList` を返す構成）の
            // `ScrollArea` id_salt・`VirtualListRecyclers` のプールキーはこの `path`
            // から作られるため、親の `path` をリセットすると別の親配下の同 index
            // リストとスクロール状態・再利用プールの双方が衝突する
            // （`Node::List` アームの `child_path` 構築と同じ一貫性を保つ）。
            let mut child_path = path.to_vec();
            child_path.push(0);
            for (child, index) in nodes.into_iter().zip(range.clone()) {
                if let Some(last) = child_path.last_mut() {
                    *last = index;
                }
                ui.push_id(("fandhe-virtual-item", index), |ui| {
                    // 各項目を `item_extent` ちょうどの高さの行へ拘束する。
                    // 項目内容の実高さに依存せずカーソルが正確に `item_extent` ずつ
                    // 進むため、スペーサー計算・スクロール幾何モデルと実レイアウトが
                    // 一致する（レビュー指摘: 幾何モデルと実描画高さの乖離の解消）。
                    let (row_rect, _response) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), item_extent),
                        egui::Sense::hover(),
                    );
                    let mut row_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(row_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    // 固定高の行で文字列が折り返すと次の行へ重なるため、行の範囲内で
                    // 折り返さず末尾を「…」で省略する（この行 `Ui` にだけ適用）。
                    row_ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    render_node(&mut row_ui, child, state, &child_path, ctx);
                });
            }

            // 描画を終えたプールをマップへ戻す（戻さないと当該フレーム限りで失われ、
            // 次フレームで全件再構築になる。`VirtualListRecyclers::restore` 参照）。
            ctx.recyclers.restore(path, recycler);

            // 可視範囲より後ろの項目も同様にスペーサーのみで済ませる
            // （受け入れ条件「可視範囲外ノードが描画されない」の後半分）。
            let trailing_len = total_len.saturating_sub(range.end);
            ui.add_space(trailing_len as f32 * item_extent);
        });
}

/// `Axis::Horizontal` の非 TV 描画経路（[`render_virtual_list`] からの委譲、Issue #540）。
/// `render_virtual_list` の縦版と対を成し、`item_extent` を項目「幅」として
/// `ScrollArea::horizontal()` + 横仮想化で描画する。可視範囲計算は
/// [`fandhe_core::total_content_extent`] / [`fandhe_core::clamp_scroll_offset`] /
/// [`fandhe_core::compute_visible_range`]（いずれも軸非依存の純関数）を
/// `extent = item_extent`・座標系を `viewport.width()` / `viewport.min.x` に
/// 差し替えて流用する。id_salt・[`VirtualListRecyclers`] の扱いは縦版と同一規約
/// （絶対 index 由来の id、`path` キーのプールをフレームをまたいで再利用）を維持する。
///
/// TV 側 `tv_focus::render_virtual_list_horizontal` を移植した実装だが、能動的 reveal
/// （フォーカス追従スクロール）は TV 専用機構のため持ち込まない（本関数は常に
/// `ScrollArea` 既定のスクロール位置のみに従う）。
fn render_virtual_list_horizontal<S: 'static>(
    ui: &mut egui::Ui,
    path: &[usize],
    total_len: usize,
    item: fandhe_core::ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    state: &S,
    ctx: &mut RenderContext<S>,
) {
    egui::ScrollArea::horizontal()
        .id_salt(("fandhe-virtual-list", path.to_vec()))
        // 縦版の `auto_shrink([false, true])`（主軸=高さ false・交差軸=幅 true）と
        // 対称なポリシー: 主軸（幅 = index 0）は false で shelf の横幅いっぱいに
        // 広がり、交差軸（高さ = index 1）は true で 1 行分の高さに収まる。
        // `[true, false]` にすると交差軸（高さ）が `ui.available_height()` を
        // 埋めてしまい、横リストを縦に積むレイアウトで親パネルの全高を専有する
        // （tv_focus.rs の同ポリシーと同じ理由、Cursor Bugbot 指摘 Medium 相当）。
        .auto_shrink([false, true])
        .show_viewport(ui, |ui, viewport| {
            let total_extent = fandhe_core::total_content_extent(total_len, item_extent);
            let offset =
                fandhe_core::clamp_scroll_offset(viewport.min.x, total_extent, viewport.width());
            let range = fandhe_core::compute_visible_range(
                viewport.width(),
                offset,
                item_extent,
                total_len,
                VIRTUAL_LIST_OVERSCAN,
            );

            let mut recycler = ctx.recyclers.take(path);
            let nodes = recycler.acquire(range.clone(), ctx.version, state, item);

            let mut child_path = path.to_vec();
            child_path.push(0);
            ui.horizontal(|ui| {
                // 「1 項目 = `item_extent` px」の幾何モデルを実レイアウトへ強制する
                // （縦版の `item_spacing.y = 0.0` と同じ理由、軸を x に差し替え）。
                ui.spacing_mut().item_spacing.x = 0.0;
                // 総コンテンツ幅を明示し、可視範囲のみ描画してもスクロールバーの
                // 長さ・位置が実項目数と整合するようにする（縦版の `set_height` 相当）。
                ui.set_width(total_extent);
                // 可視範囲より手前の項目はスペーサーで幅のみ確保し、`item` を呼ばない。
                ui.add_space(range.start as f32 * item_extent);

                for (child, index) in nodes.into_iter().zip(range.clone()) {
                    if let Some(last) = child_path.last_mut() {
                        *last = index;
                    }

                    // 幅は仮想化の前提（`item_extent` = 列幅）どおり厳密固定し、高さは
                    // 内容依存にする（縦版の「主軸を固定・交差軸を内容依存にする」設計と
                    // 対称）。`allocate_ui`/`allocate_ui_with_layout` は desired_size を
                    // 「ヒント」としてしか使わず、内容がそれを超えると実際に使った幅を
                    // そのまま親のカーソル前進量に使ってしまう（egui `Ui::allocate_ui`
                    // doc「If the contents overflow, more space will be allocated」）。
                    // そのため `item_extent` より広い内容（長いラベル等）があると、
                    // この項目だけでなく後続のスペーサー・仮想化計算が実レイアウトと
                    // ずれる。回避のため子 `Ui` を `new_child` で自前に作り、実際の
                    // 使用幅を無視して `advance_cursor_after_rect` で常に `item_extent`
                    // 幅として親のカーソルを進める（tv_focus.rs と同じ理由）。
                    let cell_rect = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(item_extent, ui.available_height().max(0.0)),
                    );
                    let mut cell_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(cell_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Min)),
                    );
                    // `new_child` の `max_rect` はレイアウト制約のヒントであり描画・
                    // hit-testing をクリップしない。`item_extent` を超える内容が隣の
                    // セルへ視覚的・操作的にはみ出さないよう、セル矩形（親のクリップとの
                    // 交差）で明示的にクリップする（tv_focus.rs と同じ理由）。
                    cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                    cell_ui.push_id(("fandhe-virtual-item", index), |ui| {
                        render_node(ui, child, state, &child_path, ctx);
                    });
                    ui.advance_cursor_after_rect(egui::Rect::from_min_size(
                        cell_rect.min,
                        egui::vec2(item_extent, cell_ui.min_rect().height()),
                    ));
                }

                // 可視範囲より後ろの項目も同様にスペーサーのみで済ませる。
                let trailing_len = total_len.saturating_sub(range.end);
                ui.add_space(trailing_len as f32 * item_extent);
            });

            // 描画を終えたプールをマップへ戻す（縦版と同じ理由、`VirtualListRecyclers::restore` 参照）。
            ctx.recyclers.restore(path, recycler);
        });
}

/// `Axis::Grid { columns }` の非 TV 描画経路（[`render_virtual_list`] からの委譲、Issue #551）。
/// `total_len` 件の項目を `columns` 列の行に折り返し、行単位で縦仮想化する
/// （`item_extent` は 1 行の高さ）。可視範囲計算は縦版と同じ純関数
/// （[`fandhe_core::total_content_extent`] / [`fandhe_core::clamp_scroll_offset`] /
/// [`fandhe_core::compute_visible_range`]）を「項目数」ではなく「行数」に適用し、
/// 可視行の範囲を `* columns` で項目 index 範囲へ変換する。
///
/// TV 側 `tv_focus::render_virtual_list_vertical_or_grid` の Grid 幾何ロジックを
/// 移植した実装だが、能動的 reveal（フォーカス追従スクロール）・`TvRenderCtx`・
/// hints（`ctx.new_hints`）は TV 専用機構のため持ち込まない（本関数は常に
/// `ScrollArea` 既定のスクロール位置のみに従う）。
#[allow(clippy::too_many_arguments)]
fn render_virtual_list_grid<S: 'static>(
    ui: &mut egui::Ui,
    path: &[usize],
    total_len: usize,
    item: fandhe_core::ReadIndexedFn<S, Node<S>>,
    item_extent: f32,
    columns: usize,
    state: &S,
    ctx: &mut RenderContext<S>,
) {
    // `columns = 0`（DSL 側の誤設定）は 1 列扱いにする（TV 側と同じガード）。
    let columns = columns.max(1);
    let rows = total_len.div_ceil(columns);

    let max_height = virtual_list_max_height(ui, item_extent);
    egui::ScrollArea::vertical()
        .id_salt(("fandhe-virtual-list", path.to_vec()))
        .max_height(max_height)
        .auto_shrink([false, true])
        .show_viewport(ui, |ui, viewport| {
            // 仮想化の単位は「項目」ではなく「行」（`item_extent` = 行高）。
            let total_extent = fandhe_core::total_content_extent(rows, item_extent);
            let offset =
                fandhe_core::clamp_scroll_offset(viewport.min.y, total_extent, viewport.height());
            let row_range = fandhe_core::compute_visible_range(
                viewport.height(),
                offset,
                item_extent,
                rows,
                VIRTUAL_LIST_OVERSCAN,
            );
            // 可視行の範囲を項目 index 範囲へ変換する（overflow・クランプの理由は
            // `fandhe_core::grid_item_range` の doc 参照、Issue #557 で TV 側と共通化）。
            let item_range = fandhe_core::grid_item_range(&row_range, columns, total_len);

            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_height(total_extent);
            ui.add_space(row_range.start as f32 * item_extent);

            let mut recycler = ctx.recyclers.take(path);
            let nodes = recycler.acquire(item_range.clone(), ctx.version, state, item);
            let mut nodes_iter = nodes.into_iter().zip(item_range.clone());

            let mut child_path = path.to_vec();
            child_path.push(0);
            for _row in row_range.clone() {
                // 行を `item_extent` ちょうどの高さへ拘束する（縦版の 1 項目分の拘束と
                // 同じ理由）。
                let (row_rect, _response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), item_extent),
                    egui::Sense::hover(),
                );
                // 列は `row_rect.width() / columns` の均等セル幅で固定する。
                // `push_id` 内で自然幅のまま `left_to_right` へ積むと、前の列の内容幅
                // （項目内容依存）で後続列の x が決まってしまい、内容が長い列があると
                // 列が揃わず後続セルへ重なる（codex 指摘）。`render_virtual_list_horizontal`
                // の「セル矩形を自前で確保し `new_child` + `set_clip_rect` で幅を固定・
                // クリップする」設計（同関数 doc 参照）を列方向にも適用する。
                let cell_width = row_rect.width() / columns as f32;
                for col in 0..columns {
                    let Some((child, index)) = nodes_iter.next() else {
                        break;
                    };
                    if let Some(last) = child_path.last_mut() {
                        *last = index;
                    }
                    let cell_rect = egui::Rect::from_min_size(
                        row_rect.min + egui::vec2(col as f32 * cell_width, 0.0),
                        egui::vec2(cell_width, item_extent),
                    );
                    let mut cell_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(cell_rect)
                            .layout(egui::Layout::left_to_right(egui::Align::Min)),
                    );
                    // `new_child` の `max_rect` はレイアウト制約のヒントであり描画・
                    // hit-testing をクリップしないため、`item_extent` を超える内容が
                    // 隣列へ視覚的・操作的にはみ出さないよう明示的にクリップする
                    // （`render_virtual_list_horizontal` と同じ理由）。
                    cell_ui.set_clip_rect(cell_rect.intersect(ui.clip_rect()));
                    cell_ui.push_id(("fandhe-virtual-item", index), |ui| {
                        render_node(ui, child, state, &child_path, ctx);
                    });
                }
            }

            // 描画を終えたプールをマップへ戻す（縦版と同じ理由）。
            ctx.recyclers.restore(path, recycler);

            let trailing_rows = rows.saturating_sub(row_range.end);
            ui.add_space(trailing_rows as f32 * item_extent);
        });
}

/// `egui::ColorImage`（ネイティブ `ViewportCommand::Screenshot` の応答画像）を
/// PNG として `path` へ保存する。
///
/// native 自動撮影（`main.rs`）から利用する。`image` クレートへ生の RGBA バイト列を
/// そのまま渡すだけの薄いラッパーであり、外部入力（`path` 以外）は取らない
/// （A03 インジェクション対策・security.md 準拠。書き込み対象は呼び出し元が指定した
/// 1 ファイルに限定する）。
pub fn save_color_image(
    image: &egui::ColorImage,
    path: &std::path::Path,
) -> Result<(), image::ImageError> {
    image::save_buffer(
        path,
        image.as_raw(),
        image.width() as u32,
        image.height() as u32,
        image::ColorType::Rgba8,
    )
}

/// [`inject_preedit_before_ime_commit`] の純関数ユニットテスト（GPU 不要）。
/// 実描画を通した結合確認は `tests/ime_commit_render.rs`（egui_kittest）が担う。
#[cfg(test)]
mod ime_commit_injection_tests {
    use super::inject_preedit_before_ime_commit;
    use egui::{Event, ImeEvent};

    #[test]
    fn injects_preedit_before_bare_commit() {
        let mut events = vec![Event::Ime(ImeEvent::Commit("日本語".into()))];
        inject_preedit_before_ime_commit(&mut events);
        assert_eq!(
            events,
            vec![
                Event::Ime(ImeEvent::Preedit("日本語".into())),
                Event::Ime(ImeEvent::Commit("日本語".into())),
            ]
        );
    }

    #[test]
    fn does_not_duplicate_when_preedit_already_precedes_commit() {
        let mut events = vec![
            Event::Ime(ImeEvent::Preedit("日本語".into())),
            Event::Ime(ImeEvent::Commit("日本語".into())),
        ];
        let expected = events.clone();
        inject_preedit_before_ime_commit(&mut events);
        assert_eq!(events, expected);
    }

    #[test]
    fn injects_when_preceding_preedit_text_differs_from_commit() {
        let mut events = vec![
            Event::Ime(ImeEvent::Preedit("日本語".into())),
            Event::Ime(ImeEvent::Commit("日本語で".into())),
        ];
        inject_preedit_before_ime_commit(&mut events);
        assert_eq!(
            events,
            vec![
                Event::Ime(ImeEvent::Preedit("日本語".into())),
                Event::Ime(ImeEvent::Preedit("日本語で".into())),
                Event::Ime(ImeEvent::Commit("日本語で".into())),
            ]
        );
    }

    #[test]
    fn does_not_inject_for_empty_or_newline_commit() {
        let mut events = vec![
            Event::Ime(ImeEvent::Commit(String::new())),
            Event::Ime(ImeEvent::Commit("\n".into())),
            Event::Ime(ImeEvent::Commit("\r".into())),
        ];
        let expected = events.clone();
        inject_preedit_before_ime_commit(&mut events);
        assert_eq!(events, expected);
    }

    #[test]
    fn applying_twice_is_idempotent() {
        let mut events = vec![Event::Ime(ImeEvent::Commit("日本語".into()))];
        inject_preedit_before_ime_commit(&mut events);
        let once = events.clone();
        inject_preedit_before_ime_commit(&mut events);
        assert_eq!(events, once);
    }
}

/// app_badge_set 画面に入力欄が描画されること（#74: `text_input` は未描画だった）の回帰チェック。
#[cfg(test)]
mod app_badge_set_input_tests {
    use egui_kittest::kittest::Queryable;
    use fandhe_core::screens::app_badge_set::{current_screen, AppBadgeSetState};
    use fandhe_core::StateStore;

    #[test]
    fn app_badge_set_renders_text_input() {
        let mut harness = egui_kittest::Harness::builder().build_ui_state(
            |ui, store: &mut StateStore<AppBadgeSetState>| {
                let screen = current_screen(store.state());
                super::render_screen(ui, &screen, store);
            },
            StateStore::new(AppBadgeSetState::default()),
        );
        harness.run();
        harness.get_by_role(egui::accesskit::Role::TextInput);
    }
}

/// [`dispatch_back_if_requested`] のフォーカス除外分岐（Issue #726 codex-review 指摘）の
/// 回帰チェック。`egui_kittest` の既定レンダラ（`LazyRenderer`）は `.render()`（画像化）
/// を呼ばない限り wgpu アダプタを要求しないため、`tests/screen_stack_render.rs` と異なり
/// GPU アダプタ欠如環境でもスキップなしに実行できる。
#[cfg(test)]
mod back_dispatch_focus_tests {
    use egui_kittest::kittest::Queryable;
    use fandhe_core::{Screen, StateStore, WidgetId};

    #[derive(Default)]
    struct State {
        text: String,
        popped: bool,
    }

    fn build_screen() -> Screen<State> {
        Screen::new("detail")
            .text_field(
                "field",
                |s: &State| s.text.clone(),
                |s: &mut State, v: String| s.text = v,
            )
            // Tab でテキスト入力からフォーカスを移す先として使う（P2 回帰テスト参照）。
            .button(WidgetId::from_raw(0), "noop", |_s: &mut State| {})
            .on_back(|s: &mut State| s.popped = true)
    }

    fn build_harness() -> egui_kittest::Harness<'static, StateStore<State>> {
        egui_kittest::Harness::builder().build_ui_state(
            move |ui, store: &mut StateStore<State>| {
                let screen = build_screen();
                super::render_screen(ui, &screen, store);
                super::dispatch_back_if_requested(ui.ctx(), &screen, store);
            },
            StateStore::new(State::default()),
        )
    }

    #[test]
    fn escape_does_not_pop_while_text_field_focused() {
        let mut harness = build_harness();
        harness.run();

        // ラベルでの要素解決は関連付け（`labelled_by`）を経由するため `TextEdit` 自体を選ぶ。
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .focus();
        harness.run();

        harness.key_press(egui::Key::Escape);
        harness.run();

        assert!(
            !harness.state().state().popped,
            "テキスト入力フォーカス中の Esc は戻る操作へ変換しない"
        );
    }

    /// codex-review 指摘 P2（PR #733）の回帰チェック。`text_input_focused_last_frame_id`
    /// の一時フラグは「フォーカスを剥奪された直後」限定のフォールバックであり、
    /// Tab/クリックで正常にフォーカスが Button 等へ移った場合は対象外にする必要がある。
    /// 短絡評価に任せて `remove_temp` を呼ばない実装だと、TextField フォーカス中に
    /// 立てたフラグが消費されず残留し、Button へ移った直後のフレームの Esc まで
    /// 誤って戻る操作から除外されてしまう。
    /// `Harness::run` は内部で複数フレーム分の `step` を実行して安定化させるため、
    /// フォーカスが続いた実フレーム数（残留フラグが蓄積する条件）を厳密に作れず
    /// 検証にならない（`escape_does_not_pop_immediately_after_text_field_gains_focus`
    /// と同じ理由）。`step` で 1 フレームずつ進める。
    #[test]
    fn escape_pops_after_tab_moves_focus_away_from_text_field() {
        let mut harness = build_harness();
        harness.step();

        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .focus();
        harness.step();

        // フォーカスが継続する実フレームを複数はさみ、`set_focus_lock_filter` を
        // 安定させつつ「テキスト入力にフォーカスしたまま推移するフレーム」を
        // 複数回作る（残留バグの再現条件）。
        harness.step();
        harness.step();

        // Tab でテキスト入力から次のウィジェット（Button）へフォーカスを移す。
        harness.event(egui::Event::Key {
            key: egui::Key::Tab,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
            physical_key: None,
        });
        harness.step();

        harness.event(egui::Event::Key {
            key: egui::Key::Escape,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
            physical_key: None,
        });
        harness.step();

        assert!(
            harness.state().state().popped,
            "Button へフォーカスが移った直後のフレームの Esc は戻る操作へ変換する"
        );
    }

    /// Cursor Bugbot 指摘（PR #733）の回帰チェック。テキスト入力がフォーカスを得た
    /// 「直後の 1 フレーム」に届いた Esc は `BackRequested` へ変換してはいけない
    /// （`set_focus_lock_filter` のガードが `had_focus_last_frame(id) && has_focus(id)`
    /// を要求するため、フォーカス獲得直後の 1 フレームは `escape: true` の filter が
    /// まだ効かず、`egui::Memory::begin_pass` が先にフォーカスを剥奪してしまう。
    /// `dispatch_back_if_requested` 冒頭のコメント参照）。
    ///
    /// `Harness::run` は内部で複数フレーム分の `step` を実行して安定化させるため、
    /// この 1 フレームだけの隙間を素通りしてしまい検証にならない
    /// （`egui_kittest::Harness::step` の実装を実査し確認済み: `run` は
    /// レイアウトが安定するまで複数回 `step` する一方、`step` は
    /// キュー済みイベント 1 件ごとに 1 フレームを実行する）。
    /// そのため `run` ではなく `step` で 1 フレームずつ厳密に進める。
    #[test]
    fn escape_does_not_pop_immediately_after_text_field_gains_focus() {
        let mut harness = build_harness();
        harness.step();

        // フォーカス要求イベントを 1 件キューし、1 フレームだけ進める
        // （= フォーカス獲得フレーム。このフレームでは filter がまだ効かない）。
        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .focus();
        harness.step();

        // フォーカス獲得の直後のフレームに Esc の press を 1 件だけ届ける
        // （= 上記コメントの脆弱な 1 フレーム）。
        harness.event(egui::Event::Key {
            key: egui::Key::Escape,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
            physical_key: None,
        });
        harness.step();

        assert!(
            !harness.state().state().popped,
            "フォーカス獲得直後のフレームの Esc も戻る操作へ変換しない"
        );
    }

    #[test]
    fn browser_back_pops_even_while_text_field_focused() {
        let mut harness = build_harness();
        harness.run();

        harness
            .get_by_role(egui::accesskit::Role::TextInput)
            .focus();
        harness.run();

        harness.key_press(egui::Key::BrowserBack);
        harness.run();

        assert!(
            harness.state().state().popped,
            "BrowserBack はテキスト入力フォーカス中でも戻る操作へ変換する"
        );
    }

    /// Cursor Bugbot 指摘（PR #733）の回帰チェック。`consume_key` は `pressed: true` の
    /// みで一致判定し `repeat` を見ないため、長押しキーリピートのみのフレーム
    /// （最初の press イベントを含まない）でも毎フレーム発火してしまっていた。
    /// [`consume_key_ignoring_repeat`] が repeat のみのフレームを弾くことを確認する。
    ///
    /// `Event::Key.repeat` は送り手が指定した値をそのまま使えるわけではなく、
    /// egui 側の `InputState::begin_pass`（`input_state/mod.rs`）がその場で
    /// 上書きする（同じキーが前フレームから `keys_down` に残っていれば `repeat: true`
    /// に強制される）。そのため、最初の press（`keys_down` へ挿入・repeat: false で
    /// 消費・pop 発生）を経ずにいきなり repeat イベントだけを注入しても、
    /// egui が `first_press = true` と判定して `repeat` を `false` に戻してしまい
    /// 検証にならない。実機の「押しっぱなし」を模すには、まず素の press を送って
    /// 消費させたうえで、2 フレーム目に同じキーの press イベントをもう一度送る
    /// （このとき `keys_down` に既に入っているため egui が `repeat: true` へ補正する）。
    #[test]
    fn escape_repeat_only_frame_does_not_dispatch_back() {
        let mut harness = build_harness();
        harness.run();

        let escape_key_down = || egui::Event::Key {
            key: egui::Key::Escape,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
            physical_key: None,
        };

        // 1 フレーム目: 素の press。egui が repeat: false と判定し通常どおり pop する。
        harness.event(escape_key_down());
        harness.run();
        assert!(
            harness.state().state().popped,
            "最初の Esc press は通常どおり戻る操作へ変換する"
        );
        let version_after_first_press = harness.state().version();

        // `StateStore::state_mut` は公開していない（`dispatch` 経由のみが状態変更の窓口、
        // `state/store.rs` 冒頭のコメント参照）ため、popped フラグを直接リセットできない。
        // 代わりに `dispatch` が発火したときだけ進む `version()` の変化なしを確認する
        // （repeat フレームで `BackRequested` が再発火していれば version が進む）。
        // 同じキーが押されたままの状態を再現するため、同じ Escape の press イベントを
        // もう一度送る。egui は `keys_down` に既に Escape が入っているため、これを
        // key-repeat（repeat: true）へ補正する。
        harness.event(escape_key_down());
        harness.run();

        assert_eq!(
            harness.state().version(),
            version_after_first_press,
            "repeat のみのフレームでは BackRequested を発火しない（dispatch されず version が進まない）"
        );
    }
}
