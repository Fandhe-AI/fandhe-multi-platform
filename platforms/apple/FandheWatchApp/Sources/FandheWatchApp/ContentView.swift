import SwiftUI

/// watchOS 最小画面（1 ラベル＋1 操作要素）の表示専任ビュー
/// （REQ-3 / TASK-3.3-S1 → S2、Issue #65 → #66）。
///
/// 状態の生成・Rust コアからのスナップショット取得・イベントディスパッチは
/// `WatchScreenModel` に閉じ込め、本ビューは受け取った `labelText` を `Text` で表示し、
/// `buttonLabel` があればボタンをタップ可能な形で出すのみに徹する
/// （ロジックをモデル側に置く責務分界を S1 から維持）。
struct ContentView: View {
    /// 生成・保持は `FandheWatchApp`（`@main` エントリ）側が担う（1 画面 1 モデルの構成）。
    let model: WatchScreenModel

    var body: some View {
        VStack {
            Text(model.labelText)
            if let buttonLabel = model.buttonLabel {
                Button(buttonLabel) {
                    model.tapPrimaryButton()
                }
            }
        }
        .padding()
    }
}
