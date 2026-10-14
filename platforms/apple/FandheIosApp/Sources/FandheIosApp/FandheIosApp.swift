import SwiftUI

/// Personal Team（無料 Apple ID）のプロビジョニングプロファイルを自動生成させるための
/// 検証専用ホストアプリ（TASK-1.4 系 / Issue #567）。
///
/// **配布対象の `FandheIosApp` 本体ではない**。実際に実機へ配布・計測される iOS アプリは
/// `scripts/ios/measure-cold-start.sh` 等が `aarch64-apple-ios` 向けにビルドした Rust
/// （winit）バイナリを手組みでバンドルしたものであり、SwiftUI にも Rust コアにも依存しない。
/// 無料 Apple ID は wildcard 型のプロビジョニングプロファイルを発行できず、bundle id
/// `com.fandhe.iosapp` に対応する明示的なプロファイルが必要になる。本ターゲットを
/// `xcodebuild -allowProvisioningUpdates` で実機ビルドさせることで、Xcode に
/// Personal Team 名義のプロファイルを自動生成させ、それを上記の手組みバンドルの実署名に
/// 転用する（bundle id・deployment target は `Resources/Info.plist` と一致させている）。
@main
struct FandheIosApp: App {
    var body: some Scene {
        WindowGroup {
            Text("Provisioning host only")
        }
    }
}
