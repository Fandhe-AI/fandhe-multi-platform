//! `ui::theme_demo` の公開 API 経由の結合テスト（TASK-8.4-S23、Issue #179）。
//!
//! `crates/fandhe-core/tests/platform_theme.rs` の前例に倣い、`fandhe_core::*`
//! の再エクスポートのみを経由して「サンプル・カタログへのテーマ適用デモ」の
//! 公開契約を固定化する。デモ API 表面（`demo_resolver` / `themed_sample_lines` /
//! `themed_catalog_lines`）のみを対象とし、テーマ切替・適用の網羅ユニットテスト
//! （S22 / #178 のスコープ）とは重複しない。

use fandhe_core::ui::WidgetCatalog;
use fandhe_core::{
    ThemePlatform, ThemeVariant, demo_resolver, themed_catalog_lines, themed_sample_lines,
};

#[test]
fn ios_light_and_dark_sample_lines_differ_via_public_api() {
    let light = demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
        .expect("iOS ライト resolver の構築は成功する");
    let dark = demo_resolver(ThemePlatform::Ios, ThemeVariant::Dark)
        .expect("iOS ダーク resolver の構築は成功する");

    let light_lines = themed_sample_lines(&light).expect("ライトのサンプル解決は成功する");
    let dark_lines = themed_sample_lines(&dark).expect("ダークのサンプル解決は成功する");

    assert_ne!(
        light_lines, dark_lines,
        "iOS のサンプル画面はライト/ダークで解決済みスタイル行が異なる必要がある"
    );
}

#[test]
fn android_light_and_dark_catalog_lines_differ_via_public_api() {
    let catalog = WidgetCatalog::with_builtins();
    let light = demo_resolver(ThemePlatform::Android, ThemeVariant::Light)
        .expect("Android ライト resolver の構築は成功する");
    let dark = demo_resolver(ThemePlatform::Android, ThemeVariant::Dark)
        .expect("Android ダーク resolver の構築は成功する");

    let light_lines =
        themed_catalog_lines(&catalog, &light).expect("ライトのカタログ解決は成功する");
    let dark_lines = themed_catalog_lines(&catalog, &dark).expect("ダークのカタログ解決は成功する");

    assert_ne!(
        light_lines, dark_lines,
        "Android のカタログはライト/ダークで解決済みスタイル行が異なる必要がある"
    );
}

#[test]
fn repeated_light_resolver_construction_is_deterministic_via_public_api() {
    let first = demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
        .expect("1 回目の resolver 構築は成功する");
    let second = demo_resolver(ThemePlatform::Ios, ThemeVariant::Light)
        .expect("2 回目の resolver 構築は成功する");

    let first_lines = themed_sample_lines(&first).expect("1 回目のサンプル解決は成功する");
    let second_lines = themed_sample_lines(&second).expect("2 回目のサンプル解決は成功する");

    assert_eq!(
        first_lines, second_lines,
        "同一プラットフォーム・同一テーマの再構築は決定的な出力になる必要がある"
    );
}

#[test]
fn themed_catalog_lines_includes_all_builtin_entries_via_public_api() {
    let catalog = WidgetCatalog::with_builtins();
    let resolver = demo_resolver(ThemePlatform::Android, ThemeVariant::Light)
        .expect("resolver の構築は成功する");

    let lines = themed_catalog_lines(&catalog, &resolver).expect("カタログの解決は成功する");

    assert_eq!(
        catalog.entries().len(),
        22,
        "組み込みカタログは 22 エントリを登録している前提"
    );
    for entry in catalog.entries() {
        assert!(
            lines.iter().any(|line| line.contains(entry.name)),
            "エントリ {} の行がテーマ適用デモの出力から欠けている",
            entry.name
        );
    }
}

#[test]
fn themed_sample_lines_includes_sample_screen_title_via_public_api() {
    let resolver =
        demo_resolver(ThemePlatform::Ios, ThemeVariant::Dark).expect("resolver の構築は成功する");

    let lines = themed_sample_lines(&resolver).expect("サンプル画面の解決は成功する");

    assert!(
        lines.contains(&"# サンプル画面".to_string()),
        "サンプル画面のタイトル行が含まれている必要がある"
    );
}
