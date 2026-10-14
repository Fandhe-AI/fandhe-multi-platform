import SwiftUI

/// iOS メモリ増分計測の「ネイティブ SDK 実装比較」アプリのエントリポイント
/// （TASK-5.2、Issue #237）。
///
/// `SampleScreenView`（6 要素構成、ContentView.swift 参照）のみを表示する。
@main
struct FandheMemNativeApp: App {
    var body: some Scene {
        WindowGroup {
            SampleScreenView()
        }
    }
}
