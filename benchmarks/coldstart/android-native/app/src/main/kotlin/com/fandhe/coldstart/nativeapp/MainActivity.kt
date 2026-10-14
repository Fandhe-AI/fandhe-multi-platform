// パッケージ名は `com.fandhe.coldstart.nativeapp`（`native` ではなく `nativeapp`）とする。
// `native` は Java の予約キーワードであり、AGP が namespace 検証で
// 「有効な Java パッケージ名ではない」として拒否するため
// （本環境で実際に `gradle assembleRelease` 実行時にエラーを確認した）。
package com.fandhe.coldstart.nativeapp

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

/**
 * TASK-5.1（REQ-5 / M4、Issue #236）: Android コールドスタート計測用ネイティブ SDK
 * 比較アプリのエントリ Activity。
 *
 * `fandhe_core::sample_screen()`（`crates/fandhe-core/src/ui/sample.rs`）と同一の
 * 6 要素（見出し・テキスト・テキスト入力・ボタン 2 種・リスト）を Jetpack Compose で
 * 再現する。`scripts/android/measure-cold-start.sh` は本 Activity のコールドスタートを
 * `adb shell am start -W -S` の `TotalTime`（OS 側メトリクス）で計測するため、
 * アプリ内部での時刻計測は行わない（Android 側は iOS 側の `COLD_START_MS` 埋め込み方式
 * とは異なる計測経路になる。手順書「計測方法」節に理由を明記）。
 */
class MainActivity : ComponentActivity() {
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
 * `sample_screen()` の状態（`memo` / `count` / `items`）と対応する UI 構成。
 * fandhe 実装（egui 経由の描画）と要素数・状態遷移の複雑さを揃え、計測の公平性を保つ。
 */
@Composable
private fun SampleScreen() {
    var memo by remember { mutableStateOf("") }
    var count by remember { mutableStateOf(0) }
    var items by remember { mutableStateOf(listOf<String>()) }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(text = "代表 1 画面サンプル")
        OutlinedTextField(
            value = memo,
            onValueChange = { memo = it },
            label = { Text("メモ") },
        )
        Button(onClick = { count += 1 }) {
            Text("カウントを増やす")
        }
        Button(onClick = { items = items + "item-${items.size + 1}" }) {
            Text("項目を追加")
        }
        LazyColumn {
            items(items) { item ->
                Text(text = item)
            }
        }
    }
}
