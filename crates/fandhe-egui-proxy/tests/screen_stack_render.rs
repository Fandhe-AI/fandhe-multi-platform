//! egui_kittest による `fandhe_core::screens::list_detail`（Issue #726 / #730）の
//! 実描画検証。
//!
//! `prototype_render.rs` と同様、毎フレーム `current_screen` を引き直すループを
//! `Harness` 上で再現し、AccessKit ツリー経由で 1 行目（商品 01）のタップ・
//! 「戻る」ボタン・`dispatch_back_if_requested`（Esc/BrowserBack）の 3 経路すべてが
//! 実描画レベルで一覧⇔詳細画面を往復させることを検証する。GPU アダプタ欠如環境での
//! スキップ判定は `prototype_render.rs` の `is_adapter_unavailable_panic` パターンを
//! 踏襲する。

use egui_kittest::kittest::Queryable;
use fandhe_core::screens::list_detail::{current_screen, row_label, ListDetailState};
use fandhe_core::StateStore;

/// 一覧 1 行目（商品 01、初期状態は未お気に入り）の表示ラベル。`row_label` の書式に
/// 追随させ、リテラルの二重管理を避ける（Issue #730、`list_detail.rs` 参照）。
fn first_row_label() -> String {
    row_label(&ListDetailState::default().products[0])
}

/// GPU アダプタの取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`prototype_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`fandhe_egui_proxy::app::ScreenSource::FromState`]（`ProxyApp::with_screen_fn`）と
/// 同型のループで `Harness` を組み立てる。描画（`render_screen`）でボタン処理により
/// `store` の画面スタックが変わりうるため、`dispatch_back_if_requested` へ渡す画面は
/// 描画後に `current_screen` で `store` から引き直したものを使う（描画に使った画面を
/// そのまま使い回さない。`app.rs::update` と同じ実装、PR #733 レビュー是正）。
fn build_harness() -> egui_kittest::Harness<'static, StateStore<ListDetailState>> {
    egui_kittest::Harness::builder()
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<ListDetailState>| {
                let screen = current_screen(store.state());
                fandhe_egui_proxy::render_screen(ui, &screen, store);
                let screen_after_render = current_screen(store.state());
                fandhe_egui_proxy::dispatch_back_if_requested(
                    ui.ctx(),
                    &screen_after_render,
                    store,
                );
            },
            StateStore::new(ListDetailState::default()),
        )
}

/// アダプタ欠如由来の panic メッセージにのみ現れる部分文字列
/// （`prototype_render.rs` の判定基準と同一）。
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

/// `build_harness` を呼び出し、wgpu アダプタ欠如による構築時 panic のみを捕捉して
/// `None` を返す（テストをスキップさせる。`prototype_render.rs` と同方針）。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<ListDetailState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、画面遷移スタック実描画テストをスキップします（既知・別テストで確認済み）"
        );
        return None;
    }

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build_harness)) {
        Ok(harness) => Some(harness),
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }

            GPU_UNAVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、画面遷移スタック実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 1 行目（商品 01）タップ→「戻る」ボタンの実クリックで詳細⇔一覧を往復できることを検証する。
#[test]
fn detail_button_and_back_button_round_trip() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("一覧");
    harness.get_by_label(&first_row_label()).click();
    harness.run();

    harness.get_by_label("詳細");
    harness.get_by_label("戻る").click();
    harness.run();

    harness.get_by_label("一覧");
}

/// Esc キー押下（`dispatch_back_if_requested`）で詳細画面から一覧へ戻ることを検証する。
#[test]
fn escape_key_pops_detail_screen() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label(&first_row_label()).click();
    harness.run();
    harness.get_by_label("詳細");

    harness.key_press(egui::Key::Escape);
    harness.run();

    harness.get_by_label("一覧");
}

/// Esc キー押下時、フォーカスが TextEdit 以外のウィジェット（Button 等）にある場合は
/// 除外対象ではなく通常どおり戻る操作として扱われることを検証する
/// （Issue #726 レビュー指摘 P1: `ctx.memory(|m| m.focused().is_some())` だけでは
/// フォーカス種別を判別できず、Button フォーカス中の Esc も誤って無効化されていた）。
#[test]
fn escape_key_pops_detail_screen_when_button_focused() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label(&first_row_label()).click();
    harness.run();
    harness.get_by_label("詳細");

    // 「戻る」ボタン（TextEdit ではない）へキーボードフォーカスを移す。
    harness.get_by_label("戻る").focus();
    harness.run();

    harness.key_press(egui::Key::Escape);
    harness.run();

    harness.get_by_label("一覧");
}

/// `BrowserBack`（Android バックキーが winit/egui-winit で正規化される先、Issue #726）
/// でも同様に詳細画面から一覧へ戻ることを検証する。
#[test]
fn browser_back_key_pops_detail_screen() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label(&first_row_label()).click();
    harness.run();
    harness.get_by_label("詳細");

    harness.key_press(egui::Key::BrowserBack);
    harness.run();

    harness.get_by_label("一覧");
}

/// Esc を消費して閉じるオーバーレイ（`egui::Modal`）が開いている状態で Esc を押すと、
/// そのフレームでモーダルが閉じるだけで画面スタックは pop されないことを検証する
/// （PR #733 codex-review P1 指摘: `key_pressed` だけの判定だと、モーダルが消費した
/// はずの Esc がそのまま `BackRequested` へも二重に渡り、詳細画面まで pop されていた）。
#[test]
fn escape_key_consumed_by_modal_does_not_pop_detail_screen() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // 詳細画面へ遷移する前からモーダルを開くと、背景クリックがモーダルの backdrop に
    // 吸収され 1 行目タップが届かなくなる。詳細画面へ遷移してからモーダルを開く。
    let modal_open = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let modal_open_for_ui = modal_open.clone();

    let build = || {
        let modal_open = modal_open_for_ui.clone();
        egui_kittest::Harness::builder()
            .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
            .build_ui_state(
                move |ui, store: &mut StateStore<ListDetailState>| {
                    let screen = current_screen(store.state());
                    fandhe_egui_proxy::render_screen(ui, &screen, store);

                    if modal_open.load(std::sync::atomic::Ordering::Relaxed) {
                        let response = egui::Modal::new(egui::Id::new("test-modal"))
                            .show(ui.ctx(), |ui| ui.label("modal"));
                        if response.should_close() {
                            modal_open.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                    }

                    let screen_after_render = current_screen(store.state());
                    fandhe_egui_proxy::dispatch_back_if_requested(
                        ui.ctx(),
                        &screen_after_render,
                        store,
                    );
                },
                StateStore::new(ListDetailState::default()),
            )
    };

    let mut harness = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(harness) => harness,
        Err(payload) => {
            if !is_adapter_unavailable_panic(payload.as_ref()) {
                std::panic::resume_unwind(payload);
            }
            eprintln!(
                "notice: wgpu アダプタが取得できない環境のため、モーダル Esc 消費テストをスキップします"
            );
            return;
        }
    };
    harness.run();

    harness.get_by_label(&first_row_label()).click();
    harness.run();
    harness.get_by_label("詳細");

    // 詳細画面へ遷移してからモーダルを開き、開いた状態を 1 フレーム反映させる。
    modal_open.store(true, std::sync::atomic::Ordering::Relaxed);
    harness.run();
    harness.key_press(egui::Key::Escape);
    harness.run();

    // モーダルが閉じているだけで、詳細画面のまま（一覧へ pop されていない）ことを確認する。
    assert!(!modal_open.load(std::sync::atomic::Ordering::Relaxed));
    harness.get_by_label("詳細");
}

/// 一覧（root）画面で 1 行目タップと `BrowserBack` を同一フレームへ投入した
/// 場合、戻る処理は描画で遷移した後の詳細画面（`on_back` を持つ）へ dispatch され、
/// 一覧へ戻ることを検証する（PR #733 レビュー是正: 描画前に構築した古い root 画面
/// （`on_back` なし）へ戻る操作が送られると、戻る操作が無視されて詳細画面に
/// 取り残される回帰があった）。
#[test]
fn combined_open_detail_and_back_in_same_frame_uses_post_render_screen() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();
    harness.get_by_label("一覧");

    // 同一フレームへ 1 行目タップと BrowserBack を積んでから run() する。
    harness.get_by_label(&first_row_label()).click();
    harness.key_press(egui::Key::BrowserBack);
    harness.run();

    harness.get_by_label("一覧");
}
