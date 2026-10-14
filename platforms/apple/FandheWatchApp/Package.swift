// swift-tools-version:5.9
//
// FandheWatchApp: watchOS 向け最小画面（1 ラベル＋1 操作要素）の SwiftUI アプリソース一式
// （REQ-3 / TASK-3.3-S1 → S2、Issue #65 → #66。親: TASK-3.3 / Issue #64）。
//
// 型チェック用ビルド単位。アプリバンドル生成は `platforms/apple/project.yml`
// （XcodeGen、TASK-3.4 / Issue #71）が担う。SwiftPM は単体では watchOS アプリバンドル
// （.app・コード署名込み）を生成できないため、本パッケージはソースを型安全に検証する
// ための library ターゲットとして構成する。
//
// ## 依存解決について
//
// `FandheSwiftUIBridge`（Swift 側ラッパ、TASK-3.2-S5 / Issue #62）は
// `platforms/apple/FandheSwiftUIBridge/`（本パッケージから見た相対パス
// `../FandheSwiftUIBridge` で解決可能）に存在する。
import PackageDescription

let package = Package(
    name: "FandheWatchApp",
    platforms: [
        .watchOS(.v10)
    ],
    products: [
        .library(name: "FandheWatchApp", targets: ["FandheWatchApp"])
    ],
    dependencies: [
        // 相対パス依存（上記コメント参照）。
        .package(path: "../FandheSwiftUIBridge")
    ],
    targets: [
        // 注意: `Resources/Info.plist`（このターゲットの `path` 配下ではなくパッケージ
        // ルート直下にある）は、本ターゲットのどの `resources:` からも意図的に参照して
        // いない。SwiftPM の library ターゲットからは参照しない（アプリターゲットの
        // Info.plist は `platforms/apple/project.yml`（XcodeGen）が配線する）。
        .target(
            name: "FandheWatchApp",
            dependencies: [
                .product(name: "FandheSwiftUIBridge", package: "FandheSwiftUIBridge")
            ],
            path: "Sources/FandheWatchApp"
        )
    ]
)
