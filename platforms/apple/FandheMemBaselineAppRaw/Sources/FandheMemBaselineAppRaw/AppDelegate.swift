import UIKit

/// iOS メモリ増分計測の「UIKit 版ベースライン」アプリのエントリポイント（TASK-5.2、Issue #237、
/// PR #497）。
///
/// `FandheMemBaselineApp`（SwiftUI `App`/`Color.clear`）は、SwiftUI ネイティブ比較アプリ
/// （`FandheMemNativeApp`）の増分計測では正しく機能する（両者とも SwiftUI ランタイムの
/// コストを負っており、差し引きでそのコストが相殺される）。しかし `FandheIosApp`
/// （`fandhe-egui-proxy`、`winit`/`UIApplicationMain` 経由で起動し SwiftUI には一切
/// 依存しない）から SwiftUI 版ベースラインを差し引くと、`FandheIosApp` が実際には
/// 負っていない SwiftUI フレームワークのコストまで誤って除去してしまい、REQ-5 の
/// 相対・絶対チェックの pass/fail を歪める（PR #497）。
///
/// 本アプリは UIKit のみ（`import SwiftUI` なし）で構成した空の `UIWindow` +
/// `UIViewController`（背景 `.clear`）であり、`FandheIosApp` と同じ
/// `UIApplicationMain` ライフサイクルに揃えた「同種ランタイムのベースライン」。
/// `FandheIosApp` の増分計測ではこちらを差し引くベースラインとして使い、
/// `FandheMemNativeApp`（SwiftUI）の増分計測では引き続き `FandheMemBaselineApp`
/// （SwiftUI 版）を使う。
@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // `UIWindow(frame:)` に `UIScreen.main.bounds` を渡す形式は iOS 16 以降非推奨のため、
        // フレーム省略形（デバイスの画面サイズへ自動追従する）を使う。
        let window = UIWindow()
        let viewController = UIViewController()
        viewController.view.backgroundColor = .clear
        window.rootViewController = viewController
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}
