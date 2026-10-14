import SwiftUI

/// iOS メモリ増分計測の「SwiftUI 版ベースライン」アプリのエントリポイント（TASK-5.2、Issue #237）。
///
/// 空の `View`（`Color.clear`）のみを表示し、Rust コア（FandheSwiftUIBridge）・
/// ネイティブ SDK コンポーネント（List・TextField 等）を一切使わない。
///
/// 差し引き対象は `FandheMemNativeApp`（SwiftUI 比較アプリ）**のみ**とする。
/// `FandheIosApp`（`fandhe-egui-proxy`、winit/UIApplicationMain 経由で SwiftUI には
/// 依存しない）の増分計測に本ベースラインを使うと、`FandheIosApp` が実際には負って
/// いない SwiftUI フレームワークのコストまで誤って除去してしまい、REQ-5 の相対・
/// 絶対チェックの pass/fail を歪める（PR #497）。`FandheIosApp` 側の差し引きには同じランタイム系統の
/// `FandheMemBaselineAppRaw`（UIKit 版）を使うこと。
@main
struct FandheMemBaselineApp: App {
    var body: some Scene {
        WindowGroup {
            Color.clear
        }
    }
}
