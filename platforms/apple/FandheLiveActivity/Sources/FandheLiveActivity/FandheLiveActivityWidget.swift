// ライブアクティビティの表示側（Issue #295、親 #88 の b-ios）。Widget Extension として iOS アプリの
// `PlugIns/` に同梱される。開始・更新・終了はアプリ側の薄層（`crates/fandhe-egui-proxy/ios/
// fandhe_ios_live_activity.swift`）が行い、本拡張は OS が渡す値をロック画面と Dynamic Island に出すだけ。
// 属性型は `fandhe_live_activity_attributes.swift`（project.yml が取り込む共有ファイル）。
import ActivityKit
import SwiftUI
import WidgetKit

@main
struct FandheLiveActivityBundle: WidgetBundle {
    var body: some Widget {
        FandheLiveActivityWidget()
    }
}

struct FandheLiveActivityWidget: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FandheLiveActivityAttributes.self) { context in
            HStack {
                Text("Fandhe")
                Spacer()
                Text("\(context.state.value)").font(.title).monospacedDigit()
            }
            .padding()
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) { Text("Fandhe") }
                DynamicIslandExpandedRegion(.trailing) {
                    Text("\(context.state.value)").monospacedDigit()
                }
            } compactLeading: {
                Text("F")
            } compactTrailing: {
                Text("\(context.state.value)").monospacedDigit()
            } minimal: {
                Text("\(context.state.value)").monospacedDigit()
            }
        }
    }
}
