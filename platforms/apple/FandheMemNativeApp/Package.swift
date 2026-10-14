// swift-tools-version:5.9
//
// FandheMemNativeApp: TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）のメモリ増分実機計測
// における iOS 側「ネイティブ SDK 実装比較」アプリのソース一式。
//
// PoC-4 の比較対象言語（SwiftUI）を踏襲しつつ、比較画面は PoC-4 の
// 「300 項目リスト＋アニメーション」ではなく `crates/fandhe-core/src/ui/sample.rs::sample_screen()`
// の 6 要素（見出しテキスト・テキスト入力・ボタン 2 種・リスト）に合わせる（差異は検証記録に明記。非公開）。Rust コア
// （FandheSwiftUIBridge）には依存しない **検証専用**アプリ（配布パイプラインとは分離）。
//
// 本開発環境（Linux）には Swift ツールチェーンが存在せず、`swift build` による実ビルド
// 確認は行っていない（FandheWatchApp/Package.swift と同じ制約）。
import PackageDescription

let package = Package(
    name: "FandheMemNativeApp",
    platforms: [
        .iOS(.v17)
    ],
    products: [
        .library(name: "FandheMemNativeApp", targets: ["FandheMemNativeApp"])
    ],
    targets: [
        .target(
            name: "FandheMemNativeApp",
            path: "Sources/FandheMemNativeApp"
        )
    ]
)
