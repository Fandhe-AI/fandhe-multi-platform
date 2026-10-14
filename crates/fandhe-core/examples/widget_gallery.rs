//! TASK-8.3-S6 のウィジェットカタログ・サンプルギャラリーをヘッドレスで動作確認する
//! example（親 issue #95 の分解 sub-issue 6/60、Issue #101）。
//!
//! 実描画バックエンド（`fandhe-egui-proxy` 配線等）は個別ウィジェットが揃う S7 以降の
//! スコープであるため、ここでは組み込みカタログ（Text / TextField / Button / List の
//! 4 エントリ）のギャラリー画面を `render_headless` した一覧と、各エントリの
//! `preview` 関数の出力を標準出力へ書き出すことで、カタログ登録・列挙・ギャラリー画面
//! 構築が一巡してエンドツーエンドで動作することを確認する。
//!
//! 実行方法: `cargo run -p fandhe-core --example widget_gallery`
//!
//! 外部入力（引数・環境変数・ネットワーク）は一切受け取らず、すべてコード内リテラル
//! のみで完結する（A03 インジェクション対策・security.md 準拠）。

use fandhe_core::render_headless;
use fandhe_core::ui::WidgetCatalog;

fn main() {
    let catalog = WidgetCatalog::with_builtins();

    println!(
        "=== ウィジェットカタログ登録件数: {} ===",
        catalog.entries().len()
    );

    let gallery_state = catalog.gallery_state();
    let gallery_screen = catalog.gallery_screen();

    println!("\n=== サンプルギャラリー（render_headless） ===");
    for line in render_headless(&gallery_screen, &gallery_state) {
        println!("{line}");
    }

    println!("\n=== 各エントリの preview 出力 ===");
    for entry in catalog.entries() {
        println!("--- {} ---", entry.name);
        for line in (entry.preview)() {
            println!("{line}");
        }
    }
}
