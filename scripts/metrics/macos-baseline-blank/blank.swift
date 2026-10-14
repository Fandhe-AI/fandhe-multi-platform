// TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）macOS プロキシ再計測用のベースラインアプリ。
//
// PoC-4（docs/spec/03-poc/performance/README.md、サブモジュール未初期化環境では参照不可）の
// `baseline-blank/blank.swift`（空の NSWindow のみの Cocoa アプリ）を、本リポジトリ管理下に
// 再作成したもの（spec サブモジュールは変更しない方針のため、本ファイルは独立した複製）。
// `scripts/metrics/baseline-blank`（Linux 向け winit 版）の macOS 対応物であり、
// `fandhe-egui-proxy-desktop` の TASK-6.3（#93）feature 絞り込み前後の macOS 実測再計測に使う。
//
// AppKit のみで構成し、SwiftUI・Rust コア（FandheSwiftUIBridge）には依存しない
// （「ウィンドウ生成のみ」の最小構成というベースラインの前提を崩さないため）。
//
// ビルド手順（macOS 上、Xcode Command Line Tools のみで足りる）:
//   swiftc scripts/metrics/macos-baseline-blank/blank.swift -o /tmp/fandhe-macos-blank
//   scripts/apple/measure-idle-memory-macos.sh /tmp/fandhe-macos-blank
import AppKit

let app = NSApplication.shared
app.setActivationPolicy(.regular)

let window = NSWindow(
    contentRect: NSRect(x: 0, y: 0, width: 400, height: 300),
    styleMask: [.titled, .closable],
    backing: .buffered,
    defer: false
)
window.title = "fandhe-macos-blank"
window.makeKeyAndOrderFront(nil)

app.run()
