import SwiftUI

/// TASK-5.1（REQ-5 / M4、Issue #236）: iOS 実機コールドスタート計測のネイティブ SDK
/// 比較アプリ（SwiftUI）のエントリポイント。
///
/// `crates/fandhe-core/src/ui/sample.rs` の `sample_screen()` と同一の 6 要素
/// （見出し・テキスト・テキスト入力・ボタン 2 種・リスト）を SwiftUI で再現し、
/// `scripts/ios/measure-cold-start.sh` が fandhe 実装（wgpu/Metal、
/// `crates/fandhe-egui-proxy/src/ios.rs` の `ColdStartProbe`）と同一条件で
/// 起動時間を比較するための計測専用フィクスチャである。
/// `benchmarks/` 配下に隔離しており、`platforms/` のストア提出パイプラインには
/// 影響しない（本ファイル冒頭 `project.yml` のコメント参照）。
///
/// `t0` の取り方: `static let startTime` は Swift の遅延初期化規則により
/// 最初に参照された時点で評価される。`init()` の冒頭でアクセスすることで、
/// 「アプリの型がインスタンス化された最初の瞬間」（＝ SwiftUI ランタイムが
/// アプリコードへ制御を渡した最速の地点）を `t0` として確保する。
/// fandhe 側（`ios::run()` 冒頭で `Instant::now()` を取得）と厳密に同一の
/// 基準点ではない（Swift の `@main` エントリは UIApplicationMain 相当の
/// 初期化を経てからこの `init()` に到達するのに対し、Rust 側の `fn main` は
/// winit の iOS ブートストラップから直接呼ばれる）近似であることに注意する。
@main
struct FandheColdStartNativeApp: App {
    private static let startTime = Date()

    init() {
        _ = Self.startTime
    }

    var body: some Scene {
        WindowGroup {
            ContentView(startTime: Self.startTime)
        }
    }
}
