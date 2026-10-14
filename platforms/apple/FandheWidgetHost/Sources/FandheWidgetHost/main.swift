// Issue #1065: WidgetKit 拡張（FandheTextWidget）を同梱するための最小ホストアプリ。
// 役割は 1 つだけで、Rust 側（fandhe-egui-proxy の apple_home_widget）が
// `open -n -g -b com.fandhe.widgethost` で起こしたら、`WidgetCenter.reloadAllTimelines()`
// を呼んで自分で終了する。`reloadAllTimelines` は拡張を含むアプリから呼ぶ必要があり、
// 素の実行体（Rust バイナリ）からは呼べないための迂回。
// 保存はここでは行わない（Rust が App Group コンテナのファイルへ直接書く）。
import AppKit
import WidgetKit

/// reload の要求が OS へ渡るまでの猶予（秒）。短すぎると reload 前に終了するため、
/// Mac で調整できるよう定数に出している。
private let exitDelaySeconds: TimeInterval = 1.0

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        WidgetCenter.shared.reloadAllTimelines()
        DispatchQueue.main.asyncAfter(deadline: .now() + exitDelaySeconds) {
            NSApp.terminate(nil)
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
