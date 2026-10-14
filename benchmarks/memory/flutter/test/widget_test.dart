// `SampleScreen` の要素構成・操作を検証する結合テスト（Issue #657）。
// `crates/fandhe-core/tests/sample_screen.rs` の
// `sample_screen_includes_text_button_and_list` に対応する Flutter 側の検証。
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:flutter_sample/main.dart';

void main() {
  testWidgets('sample_screen 相当の 6 要素が表示され、ボタン操作で状態が更新される',
      (WidgetTester tester) async {
    await tester.pumpWidget(const FlutterSampleApp());

    // 1. 見出し、2. メモ入力欄、6. 初期カウント表示。
    expect(find.text('代表 1 画面サンプル'), findsOneWidget);
    expect(find.byType(TextField), findsOneWidget);
    expect(find.text('カウント: 0'), findsOneWidget);

    // 3. カウントボタンを 2 回押下 → カウント表示が更新される。
    final incrementButton = find.widgetWithText(ElevatedButton, 'カウントを増やす');
    expect(incrementButton, findsOneWidget);
    await tester.tap(incrementButton);
    await tester.tap(incrementButton);
    await tester.pump();
    expect(find.text('カウント: 2'), findsOneWidget);

    // 4. 項目追加ボタンを 2 回押下 → 5. リストに項目が増える。
    final addItemButton = find.widgetWithText(ElevatedButton, '項目を追加');
    expect(addItemButton, findsOneWidget);
    await tester.tap(addItemButton);
    await tester.tap(addItemButton);
    await tester.pump();
    expect(find.text('item-1'), findsOneWidget);
    expect(find.text('item-2'), findsOneWidget);
  });

  testWidgets('メモ入力欄への入力が内部状態（_memo）へ反映される',
      (WidgetTester tester) async {
    await tester.pumpWidget(const FlutterSampleApp());

    await tester.enterText(find.byType(TextField), 'テストメモ');
    await tester.pump();

    expect(find.text('テストメモ'), findsOneWidget);
  });
}
