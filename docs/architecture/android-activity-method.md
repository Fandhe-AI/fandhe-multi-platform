# Android Activity 方式の決定記録: GameActivity への移行

- 対応: REQ-9 / TASK-9.3-S1（M4）、Issue #242
- 親: TASK-9.3 系列（Issue #241）/ 関連 sub-issue: TASK-9.3-S2（`accesskit_android` 整合確認、
  Issue #243）・TASK-9.3-S3（TalkBack 実機検証、Issue #244、人間担当）
- 関連設計文書: `docs/architecture/ime-android-input-connection.md`（§3.3・§4.1、TASK-8.5-S18）

## 1. 背景

`android-activity`（`fandhe-egui-proxy` / `fandhe-winit-ime` が Android ターゲットで依存する
crate）は `NativeActivity` と `GameActivity` の 2 方式をサポートし、workspace 全体で
片方のみを選択する必要がある（同時有効化は `compile_error!` で拒否される）。従来は
`NativeActivity` 方式（Java/Kotlin ソースなし・`android:hasCode="false"`）を採用していたが、
以下 2 点が `NativeActivity` のままでは解決できない:

1. **REQ-9（本 issue の直接目的）**: `accesskit_android`（TalkBack 等のスクリーンリーダー
   連携）は GameActivity 限定。eframe 0.33.3 は `target_os = "android"` かつ
   `android-native-activity` feature 有効時に `accesskit` feature の同時有効化を
   `compile_error!` で拒否する（`crates/fandhe-egui-proxy/Cargo.toml` 既存コメント）。
2. **REQ-8 / REQ-10（IME・CJK）**: `docs/architecture/ime-android-input-connection.md` §4.1 が
   「独自レンダラ側の Android IME 経路（S19、TASK-8.5-S19）は GameActivity + GameTextInput を
   前提とする」と既に設計確定している。

## 2. 候補比較

| 観点 | NativeActivity | GameActivity |
|------|----------------|--------------|
| Java/Kotlin コード | 不要（OS 同梱グルーコードのみ） | **必要**（`GameActivity` サブクラス。後述§4） |
| Gradle / AAR 依存 | 不要 | **必要**（`androidx.games:games-activity:4.4.0`） |
| IME サポート | 物理キーイベントの ASCII 入力のみ。onscreen keyboard の高度な入力は非対応 | `GameTextInput` 経由でサポート（IME span 更新・IME オプション変更が可能） |
| accesskit 連携 | 非対応（`accesskit_android` は GameActivity 限定） | 対応可能 |
| 基底クラス | なし（OS 直接） | `AppCompatActivity`（Jetpack 標準機能を利用可能） |

一次情報:
`~/.cargo/registry/.../android-activity-0.6.1/README.md`（"Should I use NativeActivity or
GameActivity?" 節）、`~/.cargo/registry/.../winit-0.30.13/src/platform/android.rs`
（バックエンド対応表コメント）。

## 3. 決定: GameActivity 方式を採用

根拠:

1. REQ-9（`accesskit_android`）・S2（#243）・S3（#244）・親 #241・M4 完了基準（TalkBack
   読み上げ確認）は GameActivity でなければ成立しない。
2. `ime-android-input-connection.md` の IME 設計（TASK-8.5-S19）が GameActivity を
   前提としており、方式統一によって IME 設計との矛盾が解消される。
3. android-activity 公式 README も「GameActivity is likely to be the better longer-term
   choice, due to being based on `AppCompatActivity` and having built in support for input
   methods」と長期的な推奨としている。

トレードオフ: Gradle への AAR 依存（`androidx.games:games-activity`・
`androidx.appcompat:appcompat`）が新規に発生し、「Java/Kotlin コードなし・
`hasCode="false"`」構成は放棄する。追加コードは最小の `GameActivity` サブクラス
1 ファイル（§4）に閉じる。

**スコープ境界**: 本 issue（S1）は「方式の確定＝決定記録の作成 + Activity 方式切替の実装 +
ビルド成立確認」まで。`accesskit` feature の有効化・`accesskit_android` の動作整合は
S2（#243）、TalkBack 実機検証は S3（#244、人間担当）へ委ねる。**S1 では accesskit を
有効化しない**（切替のみでビルドが通ることを確認する。§6 参照）。

## 4. GameActivity 導入に伴う実装事項（一次情報で確認済み）

- **サブクラス化が必須**: `com.google.androidgamesdk.GameActivity` は NativeActivity と
  異なり自動ロードを行わないため、Java サブクラスで `System.loadLibrary(...)` を
  static 初期化子に書く必要がある（
  <https://developer.android.com/games/agdk/game-activity/get-started>
  「Implement GameActivity in your game」節、取得日 2026-08-01）。
  → `platforms/android/app/src/main/java/com/fandhe/androidapp/MainActivity.java` を追加。
- **`android.app.lib_name` meta-data は維持**: GameActivity でも同じ規約で解決される
  （同ガイド「Add your native library to AndroidManifest.xml if your library name is not
  the default name」節）。ただし GameActivity 自体は自動ロードに使わないため、
  値は `MainActivity.java` の `System.loadLibrary` 引数と一致させる SSOT として維持する。
- **GameActivity は `AppCompatActivity` のサブクラス**: `android:theme` に
  `Theme.AppCompat`（またはその子孫）を指定しないと起動時に
  `IllegalStateException`（"You need to use a Theme.AppCompat theme (or descendant) with
  this activity"）で落ちる。`platforms/android/app/src/main/res/values/themes.xml` に
  `Theme.AppCompat.NoActionBar` 継承の `AppTheme` を追加し、Manifest の
  `<application android:theme="@style/AppTheme">` で指定した。
- **appcompat 依存は明示追加が必要**: `androidx.games:games-activity:4.4.0` の AAR は POM に
  `androidx.appcompat:appcompat` への依存を含まない（`dl.google.com` の
  `games-activity-4.4.0.pom` を直接取得し確認、dependencies セクションなし）。そのため
  `platforms/android/app/build.gradle.kts` に `androidx.appcompat:appcompat:1.7.1`
  （選定根拠: `dl.google.com/.../appcompat/maven-metadata.xml` の `<versions>` 一覧末尾から
  プレリリースを除いた最新安定版、取得日 2026-08-01）を明示的に追加する。
- **prefab（C++ glue）は有効化しない**: android-activity README が明記する注意事項
  （"Don't compile and link the upstream GameActivity 'prefab' (C++ glue) layer")。
  android-activity crate 自身が独自のネイティブグルーを持つため、
  `buildFeatures { prefab = true }` は追加していない。

## 5. バージョン選定

- `androidx.games:games-activity`: **4.4.0**。android-activity 0.6.1 の README
  「currently supports the GameActivity 4.4.0 Jetpack library」に対応させた
  （`scripts/android/versions.env` の `GAMES_ACTIVITY_VERSION`）。
- `androidx.appcompat:appcompat`: **1.7.1**（`scripts/android/versions.env` の
  `APPCOMPAT_VERSION`）。§4 参照。
- `androidx.core:core`: **1.13.0**（`scripts/android/versions.env` の
  `ANDROIDX_CORE_VERSION`）。`MainActivity` の WindowInsets リスナー
  （`ViewCompat` / `WindowInsetsCompat` / `Insets`、Issue #579）が直接 import
  するため、appcompat の推移依存に頼らず明示依存とする。バージョンは
  appcompat 1.7.1 が要求する androidx.core:core と同一値（二重解決の回避）。

各バージョンは `platforms/android/app/build.gradle.kts` が `versions.env` を
パースして参照する（ハードコード複製によるドリフト防止、キー欠落時はビルド失敗の
fail-closed）。AGP のみ plugins DSL の制約で注釈による手動同期。

## 6. 影響ファイル・実装範囲

| パス | 変更内容 |
|------|---------|
| `crates/fandhe-egui-proxy/Cargo.toml` | Android 分岐の eframe/winit feature を `android-native-activity` → `android-game-activity` へ切替。accesskit は本 issue では有効化しない |
| `crates/fandhe-winit-ime/Cargo.toml` | Android 分岐の winit feature を `android-game-activity` へ切替（workspace 統一） |
| `platforms/android/app/build.gradle.kts` | `androidx.games:games-activity` / `androidx.appcompat:appcompat` / `androidx.core:core` 依存追加（バージョンは `versions.env` を SSOT としてパース参照）、Java compileOptions 追加 |
| `platforms/android/app/src/main/AndroidManifest.xml` | `android:name` を `com.fandhe.androidapp.MainActivity` へ、`android:theme` 追加、`hasCode="false"` 撤回 |
| `platforms/android/app/src/main/java/com/fandhe/androidapp/MainActivity.java`（新規） | `GameActivity` を継承し `System.loadLibrary("fandhe_egui_proxy")` を行うサブクラス。WindowInsets の JNI 中継（Issue #579）もここに置く |
| `platforms/android/app/src/main/res/values/themes.xml`（新規） | `Theme.AppCompat.NoActionBar` を継承する `AppTheme` |
| `scripts/android/versions.env` | `GAMES_ACTIVITY_VERSION` / `APPCOMPAT_VERSION` 追加 |

## 7. 既知の制約・S2/S3 への申し送り

- **ソフトキーボード挙動の変化**: winit 0.30.13 は `InputEvent::TextEvent`（GameTextInput
  経由のコンポジション更新イベント）を処理しないため、GameActivity 切替で
  `fandhe-egui-proxy` のソフトキーボード入力挙動が NativeActivity 時と変わる可能性がある。
  IME の本経路は `fandhe-android-ime`（TASK-8.5-S19、GameActivity 前提の設計）側にあるため、
  `fandhe-egui-proxy` 側の挙動差はここに既知制約として明記し、実機での影響確認は S3
  （#244）へ申し送る。
- **`egui_kittest` の Android 除外は変更なし**: `fandhe-egui-proxy/Cargo.toml` の
  `[target.'cfg(not(target_os = "android"))'.dev-dependencies]` セクション（`egui_kittest`
  除外、E0027 回避）は Activity 方式に依存しない理由（`egui_kittest` が無条件に
  `egui/accesskit` feature を有効化する挙動由来）のため、GameActivity 切替後も
  変更不要である。S2 でこの前提を再検証する必要はない。
- **accesskit 有効化・動作整合**: `eframe` の `accesskit` feature に加え、egui-winit が
  有効化しない `accesskit_winit` の `accesskit_android` feature（Android 専用 optional
  target 依存）を `fandhe-egui-proxy` から明示的に有効化する（TASK-9.3-S2・#243）。
- **TalkBack 実機検証**: TASK-9.3-S3（#244、人間担当）のスコープ。
- **CI（`.github/workflows/ci.yml`）への組み込み**: TASK-11.2-S4（#228）系列の管轄であり
  本 issue では変更しない。
