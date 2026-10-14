// swift-tools-version:5.9
// REQ-3 / TASK-3.3-S4（Issue #68）: visionOS 要素とブリッジ接続の SwiftPM パッケージ定義。
//
// 役割・スコープ:
// - 本パッケージは「ソース構成・型チェックのためのビルド単位」に限定する。SwiftPM 単体では
//   visionOS のアプリバンドル（.app・コード署名込み）を生成できないため、実機/シミュレータへ
//   インストール可能な成果物化（.xcodeproj 統合・署名）は TASK-3.4（Issue #70）のスコープとする。
// - watchOS 側（S1、`platforms/apple/FandheWatchApp/`、Issue #65）と同じ配置規約・体裁を踏襲する。
// - ターゲット種別は `FandheVisionApp.swift` の `@main struct FandheVisionApp: App`
//   （実行エントリポイント）と整合させ `.executableTarget` / `.executable` とする。
//   `.library`/`.target` はライブラリ（他パッケージから import される再利用可能コード）向けの種別であり、
//   `@main` エントリポイント合成とは意味的に矛盾するため採用しない（実行ファイル生成の可否は
//   `.app` バンドル・コード署名の可否とは別軸であり、上記の「成果物化しない」方針とは独立に選べる）。
//
// 依存関係の方針（重要）:
// - `FandheSwiftUIBridge`（`crates/fandhe-swiftui-bridge` の Swift 側ラッパ、S5・Issue #62）に
//   依存し、ブリッジ接続・画面要素の配線を担う
//   （`docs/architecture/swiftui-bridge-architecture.md` §3.1 のクレート構成・依存方向を参照）。
// - パスは `../FandheSwiftUIBridge`（`platforms/apple/FandheSwiftUIBridge` への相対パス）。
import PackageDescription

let package = Package(
    name: "FandheVisionApp",
    platforms: [
        .visionOS(.v1)
    ],
    products: [
        // `@main` App 定義（実行エントリポイント）を持つため executable とする。
        // 理由は上記コメントのとおり、アプリバンドル（.app・署名込み）生成の可否とは別軸であり、
        // 「成果物化を本パッケージのスコープ外とする」方針を維持したまま選べる。
        .executable(
            name: "FandheVisionApp",
            targets: ["FandheVisionApp"]
        )
    ],
    dependencies: [
        // S4（Issue #68）: 型安全ラッパ経由で fandhe-core（Rust コア）へ接続するための依存。
        .package(path: "../FandheSwiftUIBridge")
    ],
    targets: [
        .executableTarget(
            name: "FandheVisionApp",
            dependencies: [
                .product(name: "FandheSwiftUIBridge", package: "FandheSwiftUIBridge")
            ]
        )
    ]
)
