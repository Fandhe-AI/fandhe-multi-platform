// Issue #1065: アプリが保存した 1 行テキストを表示する WidgetKit 拡張（macOS / iOS 共用）。
// 保存は Rust 側（fandhe-egui-proxy の apple_home_widget）が App Group コンテナのファイル
// へ原子的に書く。ここはそれを読んで表示するだけで、中身を解釈・実行しない。
// 更新は Rust の再描画要求（ホストアプリ経由の reloadAllTimelines）に任せ、
// 自前の定期更新は持たない（policy は .never）。
// iOS 追加（#1066）時にソースを共有できるよう `#if os(...)` 分岐は置かない。
import SwiftUI
import WidgetKit

/// 保存先ファイル名。Rust 側 `apple_home_widget::WIDGET_FILE_NAME` と一致させる
/// （key "widget_text" を FsStorage が 16 進符号化した名前）。
private let widgetFileName = "7769646765745f74657874"

/// 表示状態。未設定と読み出し失敗を区別する。
enum WidgetText {
    case unset
    case failed
    case text(String)

    /// App Group コンテナのファイルを読む。App Group ID は Info.plist の
    /// `FandheAppGroupIdentifier`（`$(TeamIdentifierPrefix)com.fandhe.widget`）から取る。
    static func load() -> WidgetText {
        guard
            let group = Bundle.main.object(forInfoDictionaryKey: "FandheAppGroupIdentifier") as? String,
            let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else {
            return .failed
        }
        let url = dir.appendingPathComponent(widgetFileName)
        guard FileManager.default.fileExists(atPath: url.path) else { return .unset }
        guard let data = try? Data(contentsOf: url), let text = String(data: data, encoding: .utf8) else {
            return .failed
        }
        return .text(text)
    }

    var display: String {
        switch self {
        case .unset: return "未設定"
        case .failed: return "読み出せません"
        case .text(let t): return t
        }
    }
}

struct TextEntry: TimelineEntry {
    let date: Date
    let content: WidgetText
}

struct TextProvider: TimelineProvider {
    func placeholder(in context: Context) -> TextEntry {
        TextEntry(date: Date(), content: .text("おはよう"))
    }

    func getSnapshot(in context: Context, completion: @escaping (TextEntry) -> Void) {
        completion(TextEntry(date: Date(), content: WidgetText.load()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<TextEntry>) -> Void) {
        completion(Timeline(entries: [TextEntry(date: Date(), content: WidgetText.load())], policy: .never))
    }
}

struct TextWidgetView: View {
    let entry: TextEntry

    var body: some View {
        Text(entry.content.display)
            .font(.title3)
            .multilineTextAlignment(.center)
            .minimumScaleFactor(0.5)
            .containerBackground(.fill.tertiary, for: .widget)
    }
}

@main
struct FandheTextWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "FandheTextWidget", provider: TextProvider()) { entry in
            TextWidgetView(entry: entry)
        }
        .configurationDisplayName("Fandhe テキスト")
        .description("アプリで保存した 1 行の文字を表示します。")
        .supportedFamilies([.systemSmall])
    }
}
