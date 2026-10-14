import SwiftUI

/// `crates/fandhe-core/src/ui/sample.rs::sample_screen()` の 6 要素（見出しテキスト・
/// テキスト入力・ボタン 2 種・リスト）を SwiftUI で再現した比較用ビュー
/// （TASK-5.2、Issue #237）。状態は `@State` のみで管理し、Rust コア・
/// FandheSwiftUIBridge には一切依存しない（比較対象を最小構成に保つ）。
struct SampleScreenView: View {
    @State private var memo: String = ""
    @State private var count: Int = 0
    @State private var items: [String] = []

    var body: some View {
        VStack(alignment: .leading) {
            Text("代表 1 画面サンプル")
            TextField("メモ", text: $memo)
                .textFieldStyle(.roundedBorder)
            Button("カウントを増やす（\(count)）") {
                count += 1
            }
            Button("項目を追加") {
                items.append("item-\(items.count + 1)")
            }
            List(items, id: \.self) { item in
                Text(item)
            }
        }
        .padding()
    }
}
