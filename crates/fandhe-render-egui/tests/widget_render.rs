//! `render_node`（`lib.rs`）の `Node::Widget` 分岐が `Checkbox` / `ToggleSwitch` /
//! `RadioButton` を egui 標準部品で実描画し、クリック操作を
//! `Event::WidgetValueChanged` として dispatch することを egui_kittest で検証する
//! （Issue #806・親 #804）。
//!
//! `tests/dialog_render.rs` と同型のハーネス構築
//! （`egui_kittest::wgpu::WgpuTestRenderer`、GPU アダプタ欠如環境でのスキップ判定・
//! `GPU_SERIAL` による直列化）を踏襲する。`Slider` のドラッグ操作（値の書き込み）は
//! kittest 上の座標計算が煩雑なため本テストでは対象外とする（目視・実機確認は
//! 人間担当）。ただし「描画するだけで値が書き変わらないこと」（PR #809 P1 回帰）は
//! ドラッグ不要のため別途検証する。

use egui_kittest::kittest::Queryable;
use fandhe_core::{Node, Screen, Slider, StateStore, WidgetId};

/// チェックボックス・トグル・ラジオの 3 種を束ねた最小状態。
#[derive(Default)]
struct WidgetState {
    agreed: bool,
    notify: bool,
    choice: usize,
}

fn read_agreed(state: &WidgetState) -> bool {
    state.agreed
}
fn write_agreed(state: &mut WidgetState, value: bool) {
    state.agreed = value;
}
fn read_notify(state: &WidgetState) -> bool {
    state.notify
}
fn write_notify(state: &mut WidgetState, value: bool) {
    state.notify = value;
}
fn read_choice(state: &WidgetState) -> usize {
    state.choice
}
fn write_choice(state: &mut WidgetState, value: usize) {
    state.choice = value;
}

const WINDOW_SIZE: egui::Vec2 = egui::Vec2::new(400.0, 400.0);

/// GPU アダプタ取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`dialog_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// アダプタ欠如由来の panic メッセージにのみ現れる部分文字列（`dialog_render.rs` と同一）。
const ADAPTER_UNAVAILABLE_PANIC_MARKERS: [&str; 2] =
    ["CustomNativeAdapterSelectionError", "No adapter found"];

fn is_adapter_unavailable_panic(payload: &(dyn std::any::Any + Send)) -> bool {
    let message: Option<&str> = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));

    match message {
        Some(message) => ADAPTER_UNAVAILABLE_PANIC_MARKERS
            .iter()
            .any(|marker| message.contains(marker)),
        None => false,
    }
}

fn build_screen() -> Screen<WidgetState> {
    Screen::new("test")
        .checkbox(WidgetId::from_raw(1), "同意する", read_agreed, write_agreed)
        .toggle_switch(WidgetId::from_raw(2), "通知", read_notify, write_notify)
        .radio_button(
            WidgetId::from_raw(3),
            "選択肢A",
            0,
            read_choice,
            write_choice,
        )
        .radio_button(
            WidgetId::from_raw(4),
            "選択肢B",
            1,
            read_choice,
            write_choice,
        )
}

fn build_harness() -> egui_kittest::Harness<'static, StateStore<WidgetState>> {
    let screen = build_screen();
    egui_kittest::Harness::builder()
        .with_size(WINDOW_SIZE)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<WidgetState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(WidgetState::default()),
        )
}

/// `build` を呼び出し、wgpu アダプタ欠如による構築時 panic のみを捕捉して
/// `None` を返す（テストをスキップさせる。`dialog_render.rs` と同方針）。
/// 状態型を問わない形にして、Slider 専用ハーネスも同じスキップ判定
/// （`FANDHE_REQUIRE_DESKTOP_RENDER` の強制実行を含む）を通す。
fn harness_or_skip<S: 'static>(
    build: fn() -> egui_kittest::Harness<'static, StateStore<S>>,
) -> Option<egui_kittest::Harness<'static, StateStore<S>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、ウィジェット実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、ウィジェット実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// チェックボックスをラベルでクリックすると `Event::WidgetValueChanged` 経由で
/// `agreed` が `true` になることを検証する。
#[test]
fn clicking_checkbox_toggles_state() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(build_harness) else {
        return;
    };
    harness.run();

    assert!(!harness.state().state().agreed);
    harness.get_by_label("同意する").click();
    harness.run();

    assert!(harness.state().state().agreed);
}

/// トグルスイッチ（隣接ラベルへ `labelled_by` で関連付け済み）をクリックすると
/// `notify` が反転することを検証する。
#[test]
fn clicking_toggle_switch_flips_state() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(build_harness) else {
        return;
    };
    harness.run();

    assert!(!harness.state().state().notify);
    harness.get_by_label("通知").click();
    harness.run();

    assert!(harness.state().state().notify);
}

/// トグルのラベル文字（`ts.label_text()`）をクリックしても、`Checkbox` /
/// `RadioButton` と同じく `notify` が反転することを検証する（Bugbot 指摘、PR #809）。
#[test]
fn clicking_toggle_switch_label_flips_state() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(build_harness) else {
        return;
    };
    harness.run();

    assert!(!harness.state().state().notify);
    // `get_by_label` はトグル本体（`labelled_by` 経由で解決される非ラベルノード）を
    // 返すため、ここではラベル自身の role（`Role::Label`）で区別して探す。
    harness
        .get_by_role_and_label(egui::accesskit::Role::Label, "通知")
        .click();
    harness.run();

    assert!(harness.state().state().notify);
}

/// 2 つ目のラジオボタンをクリックすると `choice == 1` になることを検証する
/// （排他選択、`radio_button.rs` doc 参照）。
#[test]
fn clicking_second_radio_selects_it() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = harness_or_skip(build_harness) else {
        return;
    };
    harness.run();

    assert_eq!(harness.state().state().choice, 0);
    harness.get_by_label("選択肢B").click();
    harness.run();

    assert_eq!(harness.state().state().choice, 1);
}

/// `min=0, max=10, step=2` のスライダーに値 `3`（step 境界外）を持たせた状態で
/// 1 フレーム描画するだけでは値が書き変わらないことを検証する（P1、PR #809）。
/// 既定の `SliderClamping::Always` は描画のたびに step 丸めで `changed()` を立てて
/// `Event::WidgetValueChanged` を発火させてしまうため、`clamping(Edits)` で
/// ユーザー編集時のみ丸めるよう固定した回帰テスト。
#[derive(Default)]
struct SliderState {
    level: f32,
}

fn read_level(state: &SliderState) -> f32 {
    state.level
}
fn write_level(state: &mut SliderState, value: f32) {
    state.level = value;
}

fn build_slider_harness() -> egui_kittest::Harness<'static, StateStore<SliderState>> {
    // `Screen::slider` ビルダーは `step` を受け取れないため、`Slider::bound` +
    // `.with_step` を直接組んで `Node::Widget` として積む（step 境界外の値で
    // 描画するだけで丸められないことを確認するには step 指定が必須）。
    let screen: Screen<SliderState> = Screen::new("test").with_node(Node::Widget(Box::new(
        Slider::bound(
            WidgetId::from_raw(1),
            "レベル",
            0.0,
            10.0,
            read_level,
            write_level,
        )
        .with_step(2.0),
    )));
    egui_kittest::Harness::builder()
        .with_size(WINDOW_SIZE)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<SliderState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(SliderState { level: 3.0 }),
        )
}

#[test]
fn rendering_slider_with_off_step_value_does_not_rewrite_state() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let Some(mut harness) = harness_or_skip(build_slider_harness) else {
        return;
    };

    harness.run();

    assert_eq!(harness.state().state().level, 3.0);
}
