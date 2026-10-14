/**
 * RN 最小比較アプリのルート画面（#647 S1 / Issue #656）。
 * fandhe 側 `crates/fandhe-core/src/ui/sample.rs::sample_screen()` と同一の 6 要素・
 * 同一順序（見出し → メモ欄 → ボタン×2 → リスト → カウント表示）を素朴な useState
 * のみで再現する。ナビゲーション・状態管理ライブラリは持ち込まない（計測専用フィクスチャ、
 * README.md の対応表参照）。
 *
 * @format
 */

import { useState } from 'react';
import {
  Button,
  FlatList,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import { SafeAreaProvider, SafeAreaView } from 'react-native-safe-area-context';

function App() {
  const [memo, setMemo] = useState('');
  const [count, setCount] = useState(0);
  const [items, setItems] = useState<string[]>([]);

  const addItem = () => {
    setItems(current => [...current, `item-${current.length + 1}`]);
  };

  return (
    <SafeAreaProvider>
      <SafeAreaView style={styles.container}>
        <Text style={styles.heading}>代表 1 画面サンプル</Text>
        <TextInput
          style={styles.textField}
          placeholder="メモ"
          value={memo}
          onChangeText={setMemo}
        />
        <Button title="カウントを増やす" onPress={() => setCount(current => current + 1)} />
        <Button title="項目を追加" onPress={addItem} />
        <FlatList
          data={items}
          keyExtractor={item => item}
          renderItem={({ item }) => <Text style={styles.listItem}>{item}</Text>}
          style={styles.list}
        />
        <Text style={styles.countLabel}>カウント: {count}</Text>
      </SafeAreaView>
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    padding: 16,
  },
  heading: {
    fontSize: 20,
    fontWeight: 'bold',
    marginBottom: 12,
  },
  textField: {
    borderWidth: 1,
    borderColor: '#ccc',
    padding: 8,
    marginBottom: 12,
  },
  list: {
    marginTop: 12,
    flexGrow: 0,
  },
  listItem: {
    paddingVertical: 4,
  },
  countLabel: {
    marginTop: 12,
    fontSize: 16,
  },
});

export default App;
