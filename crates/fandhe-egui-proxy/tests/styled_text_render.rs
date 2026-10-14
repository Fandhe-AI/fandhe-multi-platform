//! `Node::StyledText` が `color.error` 等のトークン色で実描画されることの結合テスト
//! （Issue #813、親 #811 の 2/3）。`text_role.rs`（`lib.rs` の `render_node` から
//! `StyledText` 分岐が委譲する先）が対象。
//!
//! テスト 1（PNG 画素検証）は `tests/widget_render.rs` と同型のハーネス構築
//! （`WgpuTestRenderer`・GPU 欠如環境でのスキップ判定・`GPU_SERIAL` 直列化）を踏襲する。
//! テスト 2・3（AX ノードの live region 検証）は `tests/ax_tree.rs` と同じく
//! 既定レンダラ（`LazyRenderer`、wgpu 不要）で GPU アダプタに依存しない。

#[cfg(any(target_os = "macos", target_os = "android"))]
use egui_kittest::kittest::{NodeT, Queryable};
use fandhe_core::{Screen, StateStore, TextRole};

/// 対象画面（`error_text`）と対照画面（`dynamic_text`）を同じ文字列で駆動する最小状態。
#[derive(Default)]
struct S {
    message: String,
}

fn get_message(state: &S) -> String {
    state.message.clone()
}

const WINDOW_SIZE: egui::Vec2 = egui::Vec2::new(300.0, 100.0);
const MESSAGE: &str = "エラーが発生しました";

/// GPU アダプタ取得の同時実行を避ける直列化（`widget_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
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

fn error_screen() -> Screen<S> {
    Screen::new("styled").error_text(get_message)
}

fn plain_screen() -> Screen<S> {
    Screen::new("plain").dynamic_text(get_message)
}

fn build_state() -> StateStore<S> {
    StateStore::new(S {
        message: MESSAGE.to_string(),
    })
}

fn build_wgpu_harness(screen: Screen<S>) -> egui_kittest::Harness<'static, StateStore<S>> {
    egui_kittest::Harness::builder()
        .with_size(WINDOW_SIZE)
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<S>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            build_state(),
        )
}

fn harness_or_skip(
    build: impl Fn() -> egui_kittest::Harness<'static, StateStore<S>> + std::panic::RefUnwindSafe,
) -> Option<egui_kittest::Harness<'static, StateStore<S>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、役割付きテキスト描画テストをスキップします"
        );
        return None;
    }

    match std::panic::catch_unwind(&build) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }
            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、役割付きテキスト描画テストをスキップします（強制検証は FANDHE_REQUIRE_DESKTOP_RENDER=1）"
            );
            None
        }
    }
}

/// `error_text` が `color.error`（ライト/ダークいずれかの解決値）の色で描かれ、
/// `dynamic_text`（対照画面）には同じ色の画素が一切現れないことを検証する
/// （受け入れ条件 4）。検証記録用の PNG も残す（受け入れ条件 5）。
#[test]
fn error_text_is_painted_with_color_error_token() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let Some(mut error_harness) = harness_or_skip(|| build_wgpu_harness(error_screen())) else {
        return;
    };
    error_harness.run();
    let error_image = error_harness.render().expect("wgpu レンダリングが失敗した");

    let Some(mut plain_harness) = harness_or_skip(|| build_wgpu_harness(plain_screen())) else {
        return;
    };
    plain_harness.run();
    let plain_image = plain_harness.render().expect("wgpu レンダリングが失敗した");

    // ハーネスの `egui::Visuals` は既定（dark_mode）だが、将来の既定変更に追随できるよう
    // ライト/ダーク両方の解決色を許容する（`text_role::text_role_color` が実装と同じ
    // 解決経路であり、期待色を独自定数として持たない）。
    let expected = [
        fandhe_egui_proxy::text_role::text_role_color(TextRole::Error, false)
            .expect("color.error は解決できる"),
        fandhe_egui_proxy::text_role::text_role_color(TextRole::Error, true)
            .expect("color.error は解決できる"),
    ];
    // グリフのアンチエイリアス（背景とのアルファブレンド）でストローク内部でも
    // 完全な原色までは達しないため、許容差は色チャネルごとに ±40 とする
    // （実測: `color.error` ライト値 #ff3b30 に対し最頻出の非背景画素が #dc3f35
    // 程度に留まる。無色ラベル（白 #ffffff・灰色系）とは離れているため、この
    // 許容差でも「赤系トークン色」と「無色」の判別は成立する）。
    let matches_expected = |px: image::Rgba<u8>| {
        expected.iter().any(|c| {
            (i32::from(px[0]) - i32::from(c.r())).abs() <= 40
                && (i32::from(px[1]) - i32::from(c.g())).abs() <= 40
                && (i32::from(px[2]) - i32::from(c.b())).abs() <= 40
        })
    };

    let error_hits = error_image
        .pixels()
        .filter(|&&px| matches_expected(px))
        .count();
    let plain_hits = plain_image
        .pixels()
        .filter(|&&px| matches_expected(px))
        .count();

    assert!(
        error_hits > 0,
        "error_text にトークン色の画素が見つからない"
    );
    assert_eq!(
        plain_hits, 0,
        "対照画面（無色）にトークン色の画素が現れている"
    );

    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).expect("エビデンス出力ディレクトリの作成に失敗した");
    error_image
        .save(out_dir.join("styled-text-error.png"))
        .expect("PNG 保存に失敗した");
}

/// 既定レンダラ（wgpu 不要、`ax_tree.rs` と同型）で `error_text` の AX ノードを検査する。
/// 呼び出し元（AX ノード検証テスト）が macOS/Android 限定のため、この関数自体も
/// 同じ cfg で絞る（他ターゲットでは呼び出し元が消え dead_code になるため）。
#[cfg(any(target_os = "macos", target_os = "android"))]
fn build_ax_harness(screen: Screen<S>) -> egui_kittest::Harness<'static, StateStore<S>> {
    egui_kittest::Harness::builder().build_ui_state(
        move |ui, store: &mut StateStore<S>| {
            fandhe_egui_proxy::render_screen(ui, &screen, store);
        },
        build_state(),
    )
}

/// macOS/Android では `error_text` の AX ノード（`Role::Label`）が
/// `Live::Polite` として公開される（受け入れ条件 3）。
#[test]
#[cfg(any(target_os = "macos", target_os = "android"))]
fn error_text_is_exposed_as_label_with_polite_live_region() {
    let mut harness = build_ax_harness(error_screen());
    harness.run();

    let node = harness.get_by_role_and_label(egui::accesskit::Role::Label, MESSAGE);
    assert_eq!(node.accesskit_node().live(), egui::accesskit::Live::Polite);
}

/// 対照画面（`Body` 相当、`dynamic_text`）は live region を持たない
/// （`Live::Off`、`Error` 専用であることの確認）。
#[test]
#[cfg(any(target_os = "macos", target_os = "android"))]
fn body_text_is_exposed_without_live_region() {
    let mut harness = build_ax_harness(plain_screen());
    harness.run();

    let node = harness.get_by_role_and_label(egui::accesskit::Role::Label, MESSAGE);
    assert_eq!(node.accesskit_node().live(), egui::accesskit::Live::Off);
}
