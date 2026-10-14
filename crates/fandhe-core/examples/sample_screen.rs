//! TASK-1.1-S6 の代表 1 画面サンプルをヘッドレスで動作確認する example。
//!
//! 実描画バックエンド（macOS / Web 等、TASK-1.2 以降のスコープ）は存在しないため、
//! ここでは `Screen<S>` の走査・`click_button` によるイベント発火・状態更新後の
//! 再取得値の変化に加え、`render_headless`（TASK-1.1-S2 / #18）によるヘッドレス
//! 描画マッピングの出力を標準出力へ書き出すことで、DSL 一式が 1 画面をエンドツーエンド
//! で記述・動作できることを確認する。
//!
//! 実行方法: `cargo run -p fandhe-core --example sample_screen`
//!
//! 外部入力（引数・環境変数・ネットワーク）は一切受け取らず、すべてコード内リテラル
//! のみで完結する（A03 インジェクション対策・security.md 準拠）。

use fandhe_core::{SampleState, click_button, render_headless, sample_screen};

fn main() {
    let screen = sample_screen();

    println!("=== 画面構成: {} ===", screen.title());
    screen.visit(|node| {
        let kind = match node {
            fandhe_core::Node::Text(text) => format!("Text(\"{text}\")"),
            fandhe_core::Node::DynamicText { .. } => "DynamicText".to_string(),
            fandhe_core::Node::StyledText { role, .. } => format!("StyledText({role:?})"),
            fandhe_core::Node::TextField { label, .. } => format!("TextField(label=\"{label}\")"),
            fandhe_core::Node::Button { label, .. } => format!("Button(label=\"{label}\")"),
            // `sample_screen()` は Issue #491 時点で `IndexedButton` を使わない
            // （動的生成子の対話ノードは未追加）。到達しないはずの分岐だが、
            // 網羅性のために明示する。
            fandhe_core::Node::IndexedButton { label, .. } => {
                format!("IndexedButton(label=\"{label}\")")
            }
            fandhe_core::Node::List { .. } => "List".to_string(),
            // `sample_screen()` は TASK-5.5-S2 時点で `VirtualList` を使わない
            // （TASK-1.1-S6 由来の代表画面のスコープ外）。到達しないはずの分岐だが、
            // 網羅性のために明示する。
            fandhe_core::Node::VirtualList { .. } => "VirtualList".to_string(),
            // `sample_screen()` は TASK-8.3-S2 時点で `Widget` を使わない
            // （個別ウィジェット実装は S7 以降のスコープ）。到達しないはずの分岐だが、
            // 網羅性のために明示する（Issue #97）。
            fandhe_core::Node::Widget(_) => "Widget".to_string(),
        };
        println!("- {kind}");
    });

    let mut state = SampleState::default();
    println!(
        "\n=== イベント発火前: count={}, items={:?} ===",
        state.count, state.items
    );

    let handled_increment = click_button(&screen, "カウントを増やす", &mut state);
    let handled_add_item = click_button(&screen, "項目を追加", &mut state);
    let handled_unknown = click_button(&screen, "存在しないボタン", &mut state);

    println!(
        "\n=== イベント発火後: count={}, items={:?} ===",
        state.count, state.items
    );
    println!(
        "handled: increment={handled_increment}, add_item={handled_add_item}, unknown={handled_unknown}"
    );

    println!("\n=== ヘッドレス描画（render_headless） ===");
    for line in render_headless(&screen, &state) {
        println!("{line}");
    }
}
