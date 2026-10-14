// Flutter 側の性能比較用最小アプリ（#647 S2 / Issue #657）。
//
// fandhe-core の `crates/fandhe-core/src/ui/sample.rs::sample_screen()` と
// 同じ 6 要素・同順序（見出し → メモ入力 → カウントボタン → 項目追加ボタン →
// リスト → カウント表示）を Flutter のネイティブウィジェットのみで再現する。
// `benchmarks/memory/react-native/`（#656 S1）と対になる計測専用フィクスチャで、
// `platforms/` の配布用 Android プロジェクトとは独立している。
// 状態管理・ルーティング・サードパーティ依存は持たない
// （`StatefulWidget` + `setState` のみ。A06 対策）。
import 'package:flutter/material.dart';

void main() {
  runApp(const FlutterSampleApp());
}

/// アプリのルート。`MaterialApp` → `Scaffold` の最小構成。
class FlutterSampleApp extends StatelessWidget {
  const FlutterSampleApp({super.key});

  @override
  Widget build(BuildContext context) {
    return const MaterialApp(
      home: Scaffold(body: SampleScreen()),
    );
  }
}

/// `sample_screen()` 相当の 1 画面。
///
/// カウンタ・項目リストはこのウィジェットのローカル状態として持つ
/// （fandhe 側の `SampleState` に対応。外部ストアには置かない）。
class SampleScreen extends StatefulWidget {
  const SampleScreen({super.key});

  @override
  State<SampleScreen> createState() => _SampleScreenState();
}

class _SampleScreenState extends State<SampleScreen> {
  int _count = 0;
  final List<String> _items = [];
  // メモ入力の内部状態（`SampleState.memo` 相当）。TextField の
  // controller をそのまま保持先とし、setState での二重管理を避ける。
  final TextEditingController _memoController = TextEditingController();

  @override
  void dispose() {
    _memoController.dispose();
    super.dispose();
  }

  /// 「カウントを増やす」ボタンのハンドラ（`increment_count` 相当）。
  void _incrementCount() {
    setState(() => _count++);
  }

  /// 「項目を追加」ボタンのハンドラ（`add_item` 相当）。
  /// 項目名は現在のリスト長から機械的に導出し、外部入力に依存しない
  /// （A03 インジェクション対策・security.md 準拠、sample.rs と同様）。
  void _addItem() {
    setState(() => _items.add('item-${_items.length + 1}'));
  }

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.all(16),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // 1. 見出し（sample.rs の `.text("代表 1 画面サンプル")` 相当）。
            const Text('代表 1 画面サンプル'),
            const SizedBox(height: 8),
            // 2. メモ入力欄（`.text_field("メモ", get_memo, set_memo)` 相当）。
            //    controller で入力値を保持し、SampleState.memo との
            //    状態モデル一致を保つ（codex-review 指摘対応、Issue #657）。
            TextField(
              controller: _memoController,
              decoration: const InputDecoration(hintText: 'メモ'),
            ),
            const SizedBox(height: 8),
            // 3. カウントボタン（`.button(1, "カウントを増やす", ...)` 相当）。
            ElevatedButton(
              onPressed: _incrementCount,
              child: const Text('カウントを増やす'),
            ),
            // 4. 項目追加ボタン（`.button(2, "項目を追加", ...)` 相当）。
            ElevatedButton(
              onPressed: _addItem,
              child: const Text('項目を追加'),
            ),
            const SizedBox(height: 8),
            // 5. リスト表示（`.list(expand_items)` 相当）。
            Expanded(
              child: ListView.builder(
                itemCount: _items.length,
                itemBuilder: (context, index) => Text(_items[index]),
              ),
            ),
            // 6. カウント表示（`.dynamic_text(count_label)` 相当。末尾に配置し
            //    Issue #581 の DynamicText 位置に合わせる）。
            Text('カウント: $_count'),
          ],
        ),
      ),
    );
  }
}
