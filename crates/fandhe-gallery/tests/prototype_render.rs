//! egui_kittest によるプロトタイプアプリ（TASK-2.2-S5 / #52）の実描画検証。
//!
//! `desktop_render.rs`（TASK-1.2）と同様、`fandhe_prototype::current_screen` を
//! `render_screen` へ渡して実描画し、AccessKit ツリー経由で要素の存在・
//! 画面切替を確認する。本ファイル固有の観点は「毎フレーム `current_screen` を
//! 引き直す」実配線（[`fandhe_egui_proxy::PrototypeApp`] と同型のループ）で
//! 一覧画面から設定画面への実描画レベルの切替が機能することの検証である。
//!
//! GPU アダプタが存在しない環境でのスキップ判定は `desktop_render.rs` の
//! `is_adapter_unavailable_panic` パターンを踏襲する（重複コメントは避け、
//! 判定の詳細はそちらのモジュールコメントを参照）。

use egui_kittest::kittest::Queryable;
use fandhe_core::StateStore;
use fandhe_prototype::AppState;

/// GPU アダプタの取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`desktop_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `PrototypeApp` と同型のループ（毎フレーム `current_screen` を引き直して描画）で
/// `Harness` を組み立てる。
fn build_harness() -> egui_kittest::Harness<'static, StateStore<AppState>> {
    egui_kittest::Harness::builder()
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<AppState>| {
                let screen = fandhe_prototype::current_screen(store.state());
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(AppState::default()),
        )
}

/// アダプタ欠如由来の panic メッセージにのみ現れる部分文字列
/// （`desktop_render.rs` の `ADAPTER_UNAVAILABLE_PANIC_MARKERS` と同一の判定基準）。
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
/// `None` を返す（テストをスキップさせる）。判定に一致しない panic は
/// 再送出してテストを失敗させる（fake-green 化を防ぐ、`desktop_render.rs` と同方針）。
fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<AppState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、プロトタイプ実描画テストをスキップします（既知・別テストで確認済み）"
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
                "notice: wgpu アダプタが取得できない環境のため、プロトタイプ実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 一覧画面の要素（見出し・各ボタン）が AccessKit ツリーに存在することを検証する。
#[test]
fn list_screen_elements_are_present_with_no_missing_widgets() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("一覧");
    harness.get_by_label("タスクメモ一覧");
    harness.get_by_label("項目を追加");
    harness.get_by_label("次へ");
    harness.get_by_label("前へ");
    harness.get_by_label("詳細を開く");
    harness.get_by_label("設定");
}

/// 「設定」ボタンのクリックで次フレームから設定画面の要素へ切り替わることを検証する
/// （実描画レベルでの画面切替の検証、`current_screen` の毎フレーム引き直しが
/// 実際に効いていることの回帰検知）。
#[test]
fn clicking_settings_button_switches_to_settings_screen_elements() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("設定").click();
    harness.run();

    harness.get_by_label("ユーザー名");
    harness.get_by_label("一覧へ戻る");
    assert_eq!(
        harness.state().state().route,
        fandhe_prototype::Route::Settings
    );
}
