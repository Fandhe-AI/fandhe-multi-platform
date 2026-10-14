import SwiftUI

/// `fandhe_core::sample_screen()` と同一の 6 要素（見出し・テキスト・テキスト入力・
/// ボタン 2 種・リスト）を SwiftUI ネイティブ実装で再現する画面（TASK-5.1 / Issue #236）。
///
/// 状態は `sample_screen()`（`SampleState`: `memo` / `count` / `items`）と対応させ、
/// 見た目の複雑さを揃える（計測の公平性のため、要素数・状態遷移の複雑さを
/// fandhe 側と同程度に保つ）。
struct ContentView: View {
    let startTime: Date

    @State private var memo: String = ""
    @State private var count: Int = 0
    @State private var items: [String] = []
    @State private var hasLoggedColdStart = false

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text("代表 1 画面サンプル")
                TextField("メモ", text: $memo)
                    .textFieldStyle(.roundedBorder)
                Button("カウントを増やす") {
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
            .navigationTitle("サンプル画面")
        }
        .onAppear {
            // 初回表示のみ記録する（NavigationStack の再描画等で複数回 onAppear が
            // 呼ばれうるため、`scripts/ios/measure-cold-start.sh` が抽出する
            // `COLD_START_MS=` 行を 1 起動につき 1 回に限定する契約
            // （fandhe 側の `ColdStartProbe.logged` フラグと対称）。
            guard !hasLoggedColdStart else { return }
            hasLoggedColdStart = true
            let elapsedMs = Int(Date().timeIntervalSince(startTime) * 1000)
            // 末尾 `;` は行終端マーカー（fandhe 側 `ios.rs` の `ColdStartProbe` と対称の
            // 契約）。疑似 TTY 経由での折り返しによる数値断片化を、抽出側の正規表現が
            // `;` 必須で検知できるようにするため（Bugbot 指摘対応、review id 4834058437）。
            print("COLD_START_MS=\(elapsedMs);")
        }
    }
}
