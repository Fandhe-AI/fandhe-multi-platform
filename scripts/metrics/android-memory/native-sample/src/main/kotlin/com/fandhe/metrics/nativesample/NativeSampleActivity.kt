package com.fandhe.metrics.nativesample

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * TASK-5.2（REQ-5、Issue #237）のメモリ増分計測における「ネイティブ SDK 実装比較」アプリ。
 *
 * `crates/fandhe-core/src/ui/sample.rs` の `sample_screen()` が宣言する 6 要素
 * （見出しテキスト・テキスト入力・ボタン 2 種・リスト）を Jetpack Compose で再現し、
 * fandhe（NativeActivity + fandhe-egui-proxy）のアイドル時メモリ増分を同一機能の
 * ネイティブ SDK 実装と比較するための基準点とする。PoC-4（300 項目リスト＋アニメーション）
 * とは異なり、fandhe 側が実機で実際に表示している画面に合わせた構成である
 * （差異は検証記録（非公開）に明記）。
 *
 * 呼び出し元: scripts/android/measure-idle-memory-device.sh が
 * `adb shell am start -n com.fandhe.metrics.nativesample/.NativeSampleActivity` で起動し、
 * `adb shell dumpsys meminfo` で TotalPSS を計測する。
 */
class NativeSampleActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    SampleScreen()
                }
            }
        }
    }
}

/**
 * `sample_screen()` の 6 要素（見出し・テキスト入力・ボタン 2 種・リスト）に対応する
 * Compose 実装。状態は `remember { mutableStateOf(...) }` のみで管理し、
 * ViewModel 等の追加依存は導入しない（比較対象を最小構成に保つ）。
 */
@Composable
private fun SampleScreen() {
    var memo by remember { mutableStateOf("") }
    var count by remember { mutableStateOf(0) }
    val items = remember { mutableStateOf(listOf<String>()) }

    Column(modifier = Modifier.padding(16.dp)) {
        Text("代表 1 画面サンプル")
        TextField(value = memo, onValueChange = { memo = it }, label = { Text("メモ") })
        Button(onClick = { count += 1 }) { Text("カウントを増やす（${count}）") }
        Button(onClick = { items.value = items.value + "item-${items.value.size + 1}" }) {
            Text("項目を追加")
        }
        LazyColumn {
            items(items.value) { item -> Text(item) }
        }
    }
}
