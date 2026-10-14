//! IME 確定文字列の反映を egui_kittest 実描画で検証する回帰テスト
//! （Issue #616 症状 B / #663）。
//!
//! `render_screen_with_recyclers` が呼ぶ
//! [`fandhe_egui_proxy::inject_preedit_before_ime_commit`]（非公開・`lib.rs` の
//! `#[cfg(test)]` ユニットテストで純関数として検証済み）の効果を、実際の
//! `egui::Event` 列注入 → 描画 → 状態反映という結合経路で確認する。
//!
//! GPU アダプタが存在しない環境でのスキップ判定は `desktop_render.rs` の
//! `is_adapter_unavailable_panic` パターンを踏襲する（判定の詳細はそちらの
//! モジュールコメントを参照。本ファイルへの重複コメントは避ける）。

use egui_kittest::kittest::Queryable;
use fandhe_core::{sample_screen, SampleState, StateStore};

/// GPU アダプタの取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`desktop_render.rs` と同じ理由）。
static GPU_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// プロセス内で一度「アダプタなし」と判定したら以降は `Harness` の再構築を省略する。
static GPU_UNAVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn build_harness() -> egui_kittest::Harness<'static, StateStore<SampleState>> {
    let screen = sample_screen();

    egui_kittest::Harness::builder()
        .renderer(egui_kittest::wgpu::WgpuTestRenderer::default())
        .build_ui_state(
            move |ui, store: &mut StateStore<SampleState>| {
                fandhe_egui_proxy::render_screen(ui, &screen, store);
            },
            StateStore::new(SampleState::default()),
        )
}

/// `desktop_render.rs::ADAPTER_UNAVAILABLE_PANIC_MARKERS` と同一の判定基準。
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

fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<SampleState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、IME Commit 実描画テストをスキップします（既知・別テストで確認済み）"
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
                "notice: wgpu アダプタが取得できない環境のため、IME Commit 実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// 受け入れ基準の本体: 「既存テキストの末尾に日本語を追記できる」こと。
///
/// Linux の XIM `PreeditNothing`（fcitx5 既定設定）経路を模倣し、`Enabled`/`Preedit`
/// を一切送らず `Ime(Commit(..))` のみを注入する。カーソルが先頭（index 0）以外の
/// 既存テキスト末尾へ確定文字列が反映されることを確認する（修正前はこのテストが
/// 失敗する: `ime_cursor_range` の既定値とカーソル位置が不一致のため `Commit` が
/// 無言で破棄される）。
#[test]
fn commit_without_preceding_preedit_appends_at_non_zero_cursor() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    // フォーカスを与えたうえで通常の文字入力（IME を介さない `Event::Text` 相当）で
    // 「こんにちは」を先に入力し、カーソルを末尾（index 0 以外）へ進めておく。
    harness.get_by_label("メモ").click();
    harness.run();
    harness.get_by_label("メモ").type_text("こんにちは");
    harness.run();
    assert_eq!(harness.state().state().memo, "こんにちは");

    // Linux XIM PreeditNothing 経路の模倣: Preedit を経由せず Commit のみ届く。
    harness.event(egui::Event::Ime(egui::ImeEvent::Commit("日本語".into())));
    harness.run();

    assert_eq!(harness.state().state().memo, "こんにちは日本語");
    assert_eq!(
        harness.get_by_label("メモ").value().as_deref(),
        Some("こんにちは日本語")
    );
}

/// Windows/macOS の正規シーケンス（`Enabled` → `Preedit` → `Preedit` → `Commit` →
/// `Disabled`）が二重挿入なく正しく動くことの非回帰確認。
#[test]
fn commit_with_preceding_preedit_is_not_duplicated() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("メモ").click();
    harness.run();
    harness.get_by_label("メモ").type_text("こんにちは");
    harness.run();

    harness.event(egui::Event::Ime(egui::ImeEvent::Enabled));
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit("にほんご".into())));
    harness.run();
    harness.event(egui::Event::Ime(egui::ImeEvent::Preedit("日本語で".into())));
    harness.run();
    harness.event(egui::Event::Ime(egui::ImeEvent::Commit("日本語で".into())));
    harness.event(egui::Event::Ime(egui::ImeEvent::Disabled));
    harness.run();

    assert_eq!(harness.state().state().memo, "こんにちは日本語で");
}

/// カーソル index 0（空欄）での確定は既存挙動どおり動くことの確認。
#[test]
fn commit_at_cursor_index_zero_still_inserts() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    harness.get_by_label("メモ").click();
    harness.run();

    harness.event(egui::Event::Ime(egui::ImeEvent::Commit("日本語".into())));
    harness.run();

    assert_eq!(harness.state().state().memo, "日本語");
}
