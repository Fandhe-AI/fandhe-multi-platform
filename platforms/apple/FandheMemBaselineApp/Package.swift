// swift-tools-version:5.9
//
// FandheMemBaselineApp: TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）のメモリ増分実機計測
// における iOS 側ベースラインアプリのソース一式。
//
// PoC-4（docs/spec/03-poc/performance/README.md）のベースライン手法（空ウィンドウのみ）を
// iOS 実機向けに再現する。空の `View`（`Color.clear`）のみを表示し、Rust コアへの依存
// （FandheSwiftUIBridge）を一切持たない **検証専用**アプリ。配布用 `FandheIosApp`
// （`scripts/ios/build-simulator-app.sh` が生成する NativeActivity 相当のバンドル）とは
// 完全に分離しており、ストア提出パイプラインには接続されない（security.md A05 対策）。
//
// 本パッケージは FandheWatchApp/FandheVisionApp と同様に「ソース構成・型チェックのための
// ビルド単位」として機能させる想定であり、`.xcodeproj` 統合（project.yml の
// `FandheMemBaselineApp` ターゲット）でアプリバンドル化する。本開発環境（Linux）には
// Swift ツールチェーンが存在せず、`swift build` による実ビルド確認は行っていない
// （静的な参照突き合わせによる机上確認、FandheWatchApp/Package.swift と同じ制約）。
import PackageDescription

let package = Package(
    name: "FandheMemBaselineApp",
    platforms: [
        .iOS(.v17)
    ],
    products: [
        .library(name: "FandheMemBaselineApp", targets: ["FandheMemBaselineApp"])
    ],
    targets: [
        .target(
            name: "FandheMemBaselineApp",
            path: "Sources/FandheMemBaselineApp"
        )
    ]
)
