// swift-tools-version:5.9
// SPM パッケージマニフェスト（TASK-3.2-S5 / #62、binaryTarget 配線は TASK-3.4-S2 / #72）。
//
// `crates/fandhe-swiftui-bridge`（S2: #59）の `extern "C"` 関数群を Swift の型安全な API
// （`FandheSwiftUIBridge` ターゲット）へ包むためのパッケージ骨格。staticlib
// （`libfandhe_swiftui_bridge.a`）を SPM から実際にリンクする統合は、
// `linkerSettings: .unsafeFlags(...)` ではなく **XCFramework + binaryTarget** 方式を
// 採用した（TASK-3.4-S2 / #72）。`unsafeFlags` は依存パッケージとして参照される側
// （本パッケージは `FandheWatchApp` / `FandheVisionApp` から依存パッケージとして参照される）
// では SPM が使用を許可しない制約があるため、公式のバイナリ配布手段である binaryTarget を
// 選んだ（設計文書 `docs/architecture/swiftui-bridge-architecture.md`
// §3.1 参照）。
//
// 生成手順: `bash scripts/apple/build-bridge-xcframework.sh` が
// `Artifacts/FandheBridge.xcframework`（watchos-arm64 / watchos-arm64-simulator /
// xros-arm64 / xros-arm64-simulator の 4 スライス）を組み立てる。生成物は `.gitignore`
// で除外しコミットしない（ソースからの再生成のみを正とする）ため、本マニフェストは
// `FileManager.fileExists` で XCFramework の有無を実行時に判定し、**存在する場合のみ**
// binaryTarget を targets へ追加する。これにより、生成物未作成の環境（本開発環境の
// Linux CI・型チェックのみ実行する場合）でも `swift package` の依存解決が
// 壊れない。iOS / macOS 向けスライスは含まない（REQ-3 のスコープは watchOS/visionOS のみ）。
//
// プラットフォーム最小バージョンは TASK-3.1（#56, PR #319）が検証したクロスビルド対象
// （watchOS / visionOS）と、SwiftUI バインドの PoC-2 実績（watchOS 9.0 シミュレータ・
// xrOS 2.0 シミュレータでの起動確認）に合わせる。iOS はブリッジ利用側（TASK-3.3 以降）の
// 最小要件に合わせて広めに設定し、macOS はホスト（Linux CI）上での
// `swift build`（将来 macOS ランナー導入時）を見据えて明示する。

import Foundation
import PackageDescription

let package = Package(
    name: "FandheSwiftUIBridge",
    platforms: [
        .iOS(.v17),
        .watchOS(.v10),
        .visionOS(.v1),
        .macOS(.v14),
    ],
    products: [
        .library(
            name: "FandheSwiftUIBridge",
            targets: ["FandheSwiftUIBridge"]
        ),
    ],
    targets: [
        // `fandhe_bridge.h` の C ABI 宣言のみを提供するヘッダオンリーターゲット。
        // 実体（staticlib）は下記 `CFandheBridge` の依存先である `FandheBridgeBinary`
        // （XCFramework 生成時のみ条件付き追加、TASK-3.4-S2）経由でリンクされる。
        .target(
            name: "CFandheBridge"
        ),
        // `ffi` 生関数を型安全な Swift API へ包むラッパ本体。
        .target(
            name: "FandheSwiftUIBridge",
            dependencies: ["CFandheBridge"]
        ),
        // Swift ツールチェーン不在の本開発環境では実行できないプレースホルダ。
        // macOS ランナー導入後、S6（#63）以降で実テストを追加する想定。
        .testTarget(
            name: "FandheSwiftUIBridgeTests",
            dependencies: ["FandheSwiftUIBridge"]
        ),
    ]
)

// TASK-3.4-S2（#72）: staticlib 実体を binaryTarget として条件付きに配線する。
// `Artifacts/FandheBridge.xcframework` が存在しない（未生成・未生成環境）場合は
// targets に追加しない。これにより CI・ローカルの型チェックのみの実行でも
// パッケージグラフの解決自体は壊れない（実リンクを要する `swift build` は
// XCFramework 生成後にのみ成功する）。
// `Context.packageDirectory`（swift-tools-version 5.6 以降で利用可能）はマニフェスト
// 自身のファイルパス（`#filePath`）を手繰るより SwiftPM が提供する意図した解決手段であり、
// マニフェスト評価のサンドボックス環境下でも安定してパッケージルートを指す。
//
// `FileManager.fileExists` はディレクトリの有無しか見ないため、空・生成途中の
// ディレクトリに対しても true を返してしまう（Bugbot 指摘 #336）。
// `scripts/apple/build-bridge-xcframework.sh` は全スライス・Info.plist が揃った
// 完成状態でのみ一時ディレクトリから最終パスへ原子的に mv するよう修正済みだが、
// 本マニフェスト側でも「XCFramework 規約上、組み立て完了の証跡である
// `Info.plist` の存在」まで確認することで、生成手順に依存せず不完全な状態での
// binaryTarget 有効化を防ぐ（多層防御）。
let xcframeworkPath = "Artifacts/FandheBridge.xcframework"
let xcframeworkInfoPlistPath = xcframeworkPath + "/Info.plist"
if FileManager.default.fileExists(
    atPath: Context.packageDirectory + "/" + xcframeworkInfoPlistPath
) {
    package.targets.append(
        .binaryTarget(
            name: "FandheBridgeBinary",
            path: xcframeworkPath
        )
    )
    if let cBridgeTarget = package.targets.first(where: { $0.name == "CFandheBridge" }) {
        cBridgeTarget.dependencies.append(.target(name: "FandheBridgeBinary"))
    }
}
