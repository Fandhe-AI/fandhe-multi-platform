# scripts/metrics/android-memory

TASK-5.2（REQ-5 / REQ-6 実施後、Issue #237）のメモリ増分実機計測専用 Gradle プロジェクト。

## 位置づけ

配布用アプリ（`platforms/android`、NativeActivity + `fandhe-egui-proxy` の cdylib）とは
**意図的に分離**した検証専用プロジェクト。`platforms/android/settings.gradle.kts` へは
モジュールとして追加しない。debug 署名のみで、`INTERNET` 等の権限は要求しない
（`.claude/rules/security.md` A05 対策）。

## モジュール

| モジュール | applicationId | 役割 |
|-----------|---------------|------|
| `:baseline` | `com.fandhe.metrics.baseline` | 空の `Activity` のみ（Compose・View 階層を使わない）。PoC-4 のベースライン手法を Android 実機向けに再現 |
| `:native-sample` | `com.fandhe.metrics.nativesample` | Jetpack Compose で `crates/fandhe-core/src/ui/sample.rs::sample_screen()` 相当（見出し・テキスト入力・ボタン 2 種・リストの計 6 要素）を再現したネイティブ SDK 比較アプリ |

## ビルド

```bash
source scripts/android/android-env.sh
gradle -p scripts/metrics/android-memory :baseline:assembleDebug :native-sample:assembleDebug
```

## 計測

`scripts/android/measure-idle-memory-device.sh` が本プロジェクトと `platforms/android`
の計 3 アプリをビルド・インストールし、`adb shell dumpsys meminfo` の TotalPSS を
サンプリングする。結果の記入・判定は人間担当が行う（検証記録は非公開）。

## バージョン管理

Kotlin・Compose BOM・activity-compose のバージョンは `scripts/android/versions.env`
（`KOTLIN_VERSION` / `COMPOSE_BOM_VERSION` / `ACTIVITY_COMPOSE_VERSION`）を SSOT とする。
