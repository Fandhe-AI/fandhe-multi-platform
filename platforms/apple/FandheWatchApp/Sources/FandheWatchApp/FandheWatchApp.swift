import SwiftUI

/// watchOS アプリのエントリポイント（REQ-3 / TASK-3.3-S1、Issue #65）。
///
/// PoC-2（`docs/spec/03-poc/watchos-visionos/watchos-app/Sources/main.swift`）の
/// `@main` App + `WindowGroup` 構成を、本実装（型安全ラッパ経由の `FandheStore` 利用）向けに
/// 踏襲する。PoC-2 は `@_silgen_name` による生 FFI 直接呼び出しだったが、本実装では
/// アプリ層から生ポインタ・生 FFI を一切扱わず、`FandheSwiftUIBridge` の型安全な公開 API
/// のみに依存する（PoC-2 からの意図的差異。所有権規約〔ハンドル解放・バッファ解放一元化〕を
/// ラッパ側へ委ねるため。計画 §3「実装内容の要点」参照）。
///
/// `WatchScreenModel` はアプリ起動時に 1 度だけ生成し、`ContentView` へ渡す
/// （画面遷移が増えても、モデルのライフサイクルはこのエントリに閉じる）。
///
/// `.library` ターゲット内の `@main`（`Package.swift` 参照）は SwiftPM 単体ではビルド対象に
/// ならず、実行体化は `platforms/apple/project.yml` の Xcode ターゲットが担う。
@main
struct FandheWatchApp: App {
    private let model = WatchScreenModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
        }
    }
}
