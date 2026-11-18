//! IME の preedit（未確定文字列）が `TextEdit` へ届きインライン表示・確定される
//! 経路の回帰テスト（#616 症状 A / Issue #662）。
//!
//! 調査で判明した切り分け結果（実装計画 §2 参照）: eframe 分岐は
//! `winit → egui-winit → egui::TextEdit` を直結しており、`fandhe-winit-ime` は
//! 経路上にない（同クレートは `ImeEvent`/`ImeRequest` の純粋変換レイヤで、eframe には
//! 配線されていない）。`crates/fandhe-core/src/ui/text_field.rs` の IME 処理も本経路には
//! 使われない（`fandhe-egui-proxy` の `render_node` は `egui::TextEdit::text_edit_singleline`
//! を直接呼び、`fandhe_core::ui::text_field::TextFieldWidget` には委譲しない）。
//!
//! 症状 A の実体は Linux（X11 + XIM）で IM サーバーが on-the-spot スタイルを広告しない場合、
//! winit が `Ime::Preedit` を一切発行しないことにある（winit-0.30.13 の
//! `x11/ime/input_method.rs` / `context.rs`、fcitx5 は `useOnTheSpot` 既定 false）。
//! この winit ← XIM 区間は本テストの対象外（環境依存であり、egui_kittest の Harness には
//! winit が介在しないため再現できない）。本テストは egui 以降の区間
//! （`egui::Event::Ime` が TextEdit へ渡された後の挙動）を固定し、
//! 「Preedit が届けば正しく機能する」ことを回帰保証する。恒久制約としての文書化は
//! `docs/architecture/ime-known-constraints.md` §3.14 を参照。
//!
//! 併せて、症状 B（カーソルが index 0 以外だと確定文字列が破棄される）が症状 A の解消で
//! 連鎖的に解消することも確認する（egui-0.33.3 の `text_edit/builder.rs` の
//! `ImeEvent::Commit` 処理は `cursor_range` と `state.ime_cursor_range` の一致判定に
//! 依存するが、`ImeEvent::Enabled`/`ImeEvent::Preedit` 受信のたびに `ime_cursor_range` が
//! 現在のカーソル位置へ更新されるため、Preedit が先行していれば後続の Commit はこの
//! 一致判定を通過し、確定文字列が正しくバッファへ反映される）。#663（S2）はこの経路を
//! 出発点に proxy 側の追加防御を検討する。
//!
//! ヘルパは `tests/cjk_render.rs` と同じ枠組みを最小複製する（GPU アダプタ欠如時の
//! スキップ判定の詳細は同ファイルのモジュールコメント参照。重複説明は避ける）。

use egui_kittest::kittest::Queryable;
use fandhe_core::{sample_screen, SampleState, StateStore};

/// GPU アダプタの取得を複数スレッドから同時に行うとハングする既知の制約のため、
/// テスト単位で直列化する（`desktop_render.rs` / `cjk_render.rs` と同じ理由）。
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

fn build_harness_or_skip() -> Option<egui_kittest::Harness<'static, StateStore<SampleState>>> {
    if std::env::var_os("FANDHE_REQUIRE_DESKTOP_RENDER").is_some() {
        return Some(build_harness());
    }

    if GPU_UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "notice: wgpu アダプタが取得できない環境のため、IME preedit 実描画テストをスキップします（既知・別テストで確認済み）"
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
                "notice: wgpu アダプタが取得できない環境のため、IME preedit 実描画テストをスキップします（CI ランナーに GPU/ソフトウェアラスタライザ未整備の既知の制約。強制検証するには FANDHE_REQUIRE_DESKTOP_RENDER=1 を設定する）"
            );
            None
        }
    }
}

/// `egui::Event::Ime` が「メモ」欄の `TextEdit` へ届いた場合に、preedit がインライン
/// 表示され（症状 A の受け入れ）、カーソルが index 0 以外でも確定（Commit）が
/// 正しく反映される（症状 B が連鎖的に解消する経路の固定）ことを検証する。
///
/// 観察事項: preedit 挿入のたびに `response.changed()` が立ち、`TextFieldChanged` が
/// 発火して `SampleState::memo` に未確定文字列が一時的に載る（例: 途中で
/// `"こんにちはにほんご"` になる）。これは egui の仕様であり、
/// 本クレートでは変更しない（IME 未確定文字列が状態へ一時混入する挙動自体の是非は
/// スコープ外・#586 の文脈と合わせて別途検討）。
#[test]
fn preedit_is_displayed_inline_and_commit_applies_after_existing_text() {
    let _gpu_guard = GPU_SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(mut harness) = build_harness_or_skip() else {
        return;
    };
    harness.run();

    // TextEdit はフォーカスを持つ間のみテキストイベントを受理するため、
    // 先にクリックしてフォーカスを与え、通常入力で既存テキスト（カーソル index > 0）を
    // 作る（`desktop_render.rs` の `typing_into_memo_field_updates_state` と同じ手順）。
    harness.get_by_label("メモ").click();
    harness.run();
    harness.get_by_label("メモ").type_text("こんにちは");
    harness.run();
    assert_eq!(harness.state().state().memo, "こんにちは");

    // egui-winit が Linux X11 で preedit 開始時に実際に生成する順序を模す
    // （`preedit_start` 由来の `Ime::Preedit("", None)` 相当）。
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Disabled));
    harness.run();

    // 初回 draw で egui-winit が合成する `ImeEvent::Enabled` と、続く
    // `ImeEvent::Preedit` を同一フレームへ注入する（`virtual_list_render.rs` の
    // イベント注入パターンを踏襲。`Node::event` は非公開のため使わない）。
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Enabled));
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Preedit(
            "にほんご".to_owned(),
        )));
    harness.run();
    assert_eq!(
        harness.get_by_label("メモ").value().as_deref(),
        Some("こんにちはにほんご"),
        "症状 A の受け入れ: preedit が TextEdit に届き、既存テキストの後ろへ\
         インライン表示されるはず"
    );

    // 変換候補の切替（同じ preedit 区間内での再変換）。
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Preedit(
            "日本語".to_owned(),
        )));
    harness.run();
    assert_eq!(
        harness.get_by_label("メモ").value().as_deref(),
        Some("こんにちは日本語")
    );

    // `preedit_done` 由来の確定シーケンス。Commit の確定文字列は直前の Preedit
    // （"日本語"）とあえて異ならせる（"日本語で"）。両者を同一文字列にすると、
    // Commit が cursor_range 不一致等で実際には破棄されても、Preedit が直接
    // バッファへ書き込んだ文字列が既に期待値と一致してしまい、最終 assertion が
    // 偽陽性で通ってしまう（Commit 自体の適用を実証できない）ため。
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Disabled));
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Commit(
            "日本語で".to_owned(),
        )));
    harness
        .input_mut()
        .events
        .push(egui::Event::Ime(egui::ImeEvent::Disabled));
    harness.run();

    assert_eq!(
        harness.state().state().memo,
        "こんにちは日本語で",
        "症状 B の連鎖解消の確認: カーソルが index 0 以外（メモ欄の末尾）でも、\
         直前の Preedit とは異なる Commit 文字列（\"日本語で\"）が正しく反映される\
         はず（Preedit の残留文字列 \"日本語\" ではなく Commit 自体の確定値になる\
         ことを確認する）"
    );
    assert_eq!(
        harness.get_by_label("メモ").value().as_deref(),
        Some("こんにちは日本語で")
    );
}
