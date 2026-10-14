// REQ-3 / TASK-3.3-S4（Issue #68）: visionOS ウィンドウ（1 ウィンドウ＋1 操作要素）の表示専任ビュー。
//
// 責務分割の意図: 状態の生成・`FandheSwiftUIBridge`（S5・Issue #62）経由でのスナップショット
// 取得・イベントディスパッチは `VisionScreenModel` に閉じ込め、本ビューは受け取った
// `labelText` / `buttonLabel` の表示とタップ操作の委譲のみに徹する（watchOS 側 `ContentView`、
// Issue #65 と同じ分割方針）。S3（Issue #67）の静的プレースホルダ `Text` を、ブリッジ由来の
// テキスト表示＋ボタン（1 操作要素）へ差し替える。
import SwiftUI

struct ContentView: View {
    /// 生成・保持は `FandheVisionApp`（`@main` エントリ）側が担う（1 画面 1 モデルの構成）。
    let model: VisionScreenModel

    var body: some View {
        VStack(spacing: 24) {
            Text(model.labelText)

            // ボタンラベルが確定していない場合（スナップショット未取得・ボタンノード不在）は
            // 無効化表示とし、存在しない操作をユーザーへ提示しない。
            Button(model.buttonLabel ?? "操作不可") {
                model.activateElement()
            }
            .disabled(model.buttonLabel == nil)
        }
        .padding(40)
    }
}
