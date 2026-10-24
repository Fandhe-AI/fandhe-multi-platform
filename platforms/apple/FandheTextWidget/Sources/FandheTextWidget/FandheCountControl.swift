// Issue #719: コントロールセンター / メニューバーに置く「押すと回数が 1 増える」コントロール
// （macOS 26+）。押下は拡張プロセス内の `AppIntent.perform()` で実行され、App Group コンテナの
// 回数ファイルを「読む → +1 → 原子的に書く」。アプリ（Rust の fandhe-egui-proxy の
// apple_quick_settings_tile）は同じファイルを読むだけ。アプリが起動していなくても押した分は
// ファイルに残り、次の読み出しで数に入る。
// 失敗（読めない・内容が壊れている・溢れる・書けない）は throw して数えたことにしない。
// 壊れた内容を 0 に戻して上書きはしない。回数・パスはログに出さない。
import AppIntents
import SwiftUI
import WidgetKit

/// 回数ファイル名。Rust 側 `apple_quick_settings_tile::COUNT_FILE_NAME` と一致させる
/// （key "quick_settings_count" を FsStorage が 16 進符号化した名前）。
private let countFileName = "717569636b5f73657474696e67735f636f756e74"

/// コントロールの表示名。`fandhe_core::screens::quick_settings_tile_show::CONTROL_TITLE` と一致させる。
private let controlTitle = "Fandhe カウント"

/// 押下の保存失敗。理由（パス・OS の文言）は持たない。
enum CountError: Error {
    case containerUnavailable
    case corrupt
    case overflow
}

/// 回数ファイルの読み・増加・保存。拡張プロセス内の排他は `NSLock`。
/// 複数の拡張プロセスが同時に走る場合の排他は守らない範囲（Rust 側 trait doc 参照）。
enum CountStore {
    private static let lock = NSLock()

    /// 現在値を厳格に解釈する。ASCII 数字のみ受理し、空・符号・空白・改行・非 UTF-8 は `corrupt`、
    /// UInt64 を超える値は `overflow`。
    static func parse(_ data: Data) throws -> UInt64 {
        guard !data.isEmpty, data.allSatisfy({ $0 >= 0x30 && $0 <= 0x39 }) else { throw CountError.corrupt }
        guard let text = String(data: data, encoding: .utf8), let value = UInt64(text) else {
            throw CountError.overflow
        }
        return value
    }

    /// 1 回の押下を数える。失敗したら throw し、ファイルは変えない。ファイルが無ければ 0 から数える。
    static func increment() throws {
        lock.lock()
        defer { lock.unlock() }
        guard
            let group = Bundle.main.object(forInfoDictionaryKey: "FandheAppGroupIdentifier") as? String,
            let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: group)
        else {
            throw CountError.containerUnavailable
        }
        let url = dir.appendingPathComponent(countFileName)
        var current: UInt64 = 0
        if FileManager.default.fileExists(atPath: url.path) {
            current = try parse(try Data(contentsOf: url))
        }
        guard current < UInt64.max else { throw CountError.overflow }
        // `.atomic` は一時ファイルへ書いて置換する。失敗時は元のファイルが残る。
        try Data(String(current + 1).utf8).write(to: url, options: .atomic)
    }
}

/// 押下で回数を 1 増やす操作。
struct FandheCountIntent: AppIntent {
    static let title: LocalizedStringResource = "Fandhe カウントを増やす"

    func perform() async throws -> some IntentResult {
        try CountStore.increment()
        return .result()
    }
}

@available(macOS 26.0, *)
struct FandheCountControl: ControlWidget {
    static let kind = "com.fandhe.widget.count-control"

    var body: some ControlWidgetConfiguration {
        StaticControlConfiguration(kind: Self.kind) {
            ControlWidgetButton(action: FandheCountIntent()) {
                Label(controlTitle, systemImage: "plus.circle")
            }
        }
        .displayName("Fandhe カウント")
        .description("押すと回数が 1 増えます。アプリで回数を確認できます。")
    }
}
