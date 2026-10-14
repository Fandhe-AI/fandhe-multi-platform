//! サンプル・カタログへのテーマ適用デモをヘッドレスで動作確認する example
//! （TASK-8.4-S23、Issue #179。親 issue #156 の分解 sub-issue 23/24）。
//!
//! `ui::theme_demo`（`demo_resolver` / `themed_sample_lines` /
//! `themed_catalog_lines`）を用い、iOS / Android それぞれでライト → ダーク
//! テーマ切替を実演する。実描画バックエンド（`fandhe-egui-proxy` 等）・
//! `fandhe-prototype` アプリへの配線は個別ウィジェットの実描画対応が揃う後続
//! スコープであり、ここではヘッドレスなテキスト表現の変化により「テーマが
//! サンプル画面・ウィジェットカタログへ実際に適用され、解決値が変わる」ことを
//! 確認する（`examples/widget_gallery.rs` の様式を踏襲）。
//!
//! 実行方法: `cargo run -p fandhe-core --example theme_gallery`
//!
//! 外部入力（引数・環境変数・ネットワーク）は一切受け取らず、すべてコード内
//! リテラルのみで完結する（A03 インジェクション対策・security.md 準拠）。

use fandhe_core::ui::WidgetCatalog;
use fandhe_core::{
    ThemePlatform, ThemeVariant, demo_resolver, themed_catalog_lines, themed_sample_lines,
};

/// 指定プラットフォームでライト/ダームの両テーマを表示し、変化した行数を要約する。
///
/// # Errors
///
/// resolver 構築・テーマ解決のいずれかが失敗した場合に伝播する（プラットフォーム
/// 別既定テーマの内部一貫性の問題であり、通常は起こらない）。
fn run_for_platform(
    platform: ThemePlatform,
    platform_label: &str,
    catalog: &WidgetCatalog,
) -> Result<(), fandhe_core::TokenError> {
    println!("\n########## {platform_label} ##########");

    let light = demo_resolver(platform, ThemeVariant::Light)?;
    let dark = demo_resolver(platform, ThemeVariant::Dark)?;

    println!("\n=== サンプル画面（ライト） ===");
    let sample_light = themed_sample_lines(&light)?;
    for line in &sample_light {
        println!("{line}");
    }

    println!("\n=== サンプル画面（ダーク） ===");
    let sample_dark = themed_sample_lines(&dark)?;
    for line in &sample_dark {
        println!("{line}");
    }

    println!("\n=== ウィジェットカタログ（ライト） ===");
    let catalog_light = themed_catalog_lines(catalog, &light)?;
    for line in &catalog_light {
        println!("{line}");
    }

    println!("\n=== ウィジェットカタログ（ダーク） ===");
    let catalog_dark = themed_catalog_lines(catalog, &dark)?;
    for line in &catalog_dark {
        println!("{line}");
    }

    let sample_changed = sample_light
        .iter()
        .zip(sample_dark.iter())
        .filter(|(a, b)| a != b)
        .count();
    let catalog_changed = catalog_light
        .iter()
        .zip(catalog_dark.iter())
        .filter(|(a, b)| a != b)
        .count();

    println!(
        "\n=== {platform_label} 要約: ライト/ダークで変化した行数 サンプル={sample_changed} カタログ={catalog_changed} ==="
    );

    Ok(())
}

fn main() {
    let catalog = WidgetCatalog::with_builtins();
    println!(
        "=== ウィジェットカタログ登録件数: {} ===",
        catalog.entries().len()
    );

    for (platform, label) in [
        (ThemePlatform::Ios, "iOS"),
        (ThemePlatform::Android, "Android"),
    ] {
        if let Err(err) = run_for_platform(platform, label, &catalog) {
            // demo_resolver / themed_*_lines は panic を契約にしない設計
            // （coding-rust.md）。example では最終的な報告として標準エラーへ
            // 出力し、後続プラットフォームの実行は継続する。
            eprintln!("{label} のテーマ解決に失敗しました: {err}");
        }
    }
}
