// Issue #1148: ショートカット App・Siri へ固定アクション 1 つ「Fandhe に表示」を公開する
// App Intents。公開の主体はこのホストアプリ（FandheWidgetHost）で、Xcode ビルドの過程で
// App Intents のメタデータが抽出される。
//
// 呼び出し文脈: perform は固定 URL を LaunchServices で開くだけで、受け口は Rust 側 url-scheme
// （apple_url_scheme → url_scheme_show 画面）。新しい trait・effect は持たない。
// 契約: 成功 = URL を受け手アプリへ渡せた / 失敗 = 渡せなかった（受け手が無い等。
// promptsUserIfNeeded=false のため選択シートは出さず openFailed を返す）。
// キャンセルは無い操作。失敗を成功にしない。OS のエラー文言・URL はログに出さない。
// 守らない範囲: ショートカット App・Siri 側の表示と候補の出方、アプリ未起動の間の状態。
// 定数は Rust 側 fandhe_core::screens::url_scheme_show の INTENT_URL / INTENT_TITLE と一致させる
// （fandhe-egui-proxy の device/url_scheme.rs のテストが検査）。
import AppIntents
import AppKit

private let intentURLString = "fandhe://intent/show"

enum ShowIntentError: Error, CustomLocalizedStringResourceConvertible {
    case openFailed

    var localizedStringResource: LocalizedStringResource { "Fandhe を開けませんでした" }
}

struct FandheShowIntent: AppIntent {
    static let title: LocalizedStringResource = "Fandhe に表示"
    static let openAppWhenRun = false

    @MainActor
    func perform() async throws -> some IntentResult {
        guard let url = URL(string: intentURLString) else { throw ShowIntentError.openFailed }
        do {
            // 受け手が無いときに「アプリケーションを選択」シートを出さず、エラーで返す
            // （任意のアプリを選ばれて嘘の成功になるのを防ぐ）。
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.promptsUserIfNeeded = false
            _ = try await NSWorkspace.shared.open(url, configuration: configuration)
        } catch {
            throw ShowIntentError.openFailed
        }
        return .result()
    }
}

struct FandheAppShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: FandheShowIntent(),
            phrases: ["\(.applicationName) で表示"],
            shortTitle: "Fandhe に表示",
            systemImageName: "text.bubble"
        )
    }
}
