# fandhe-multi-platform

Rust を利用したマルチプラットフォーム対応フレームワークです。AI 時代のセキュリティリスクを低減するため、安全性を重視して新規開発します。iOS / Android / Android TV / iPad / watchOS / Vision Pro / macOS / Windows / Linux / Web に共通基盤で一括デプロイでき、ストア公開用ビルドまで生成できることを目指します。

設計思想は次の 3 点です。

1. ネイティブ同等のパフォーマンス
2. Rust の特性活用（メモリ安全性・型安全性）
3. AI 開発への最適化（AI 型安全 DSL）

## 仕様

仕様書（ブレスト〜PoC〜要件定義〜タスク分解〜ロードマップ）は [Fandhe-AI/fandhe-multi-platform-spec](https://github.com/Fandhe-AI/fandhe-multi-platform-spec) （非公開）で管理し、`docs/spec/` にサブモジュールとして取り込んでいます。非公開のため、アクセス権がある人だけが取得できます。ビルド・テストには不要です。

```bash
# アクセス権がある場合のみ
git submodule update --init
```

サブモジュール取得時のみ参照できるドキュメント:

| ドキュメント | 内容 |
|-------------|------|
| [`docs/spec/04-requirements.md`](./docs/spec/04-requirements.md) | MoSCoW 優先度付き要件・受け入れ基準 |
| [`docs/spec/05-tasks.md`](./docs/spec/05-tasks.md) | タスク分解（依存関係・工数） |
| [`docs/spec/06-roadmap.md`](./docs/spec/06-roadmap.md) | マイルストーン M1〜M6・着手判定（Go・2026-07-08） |

## 開発の進め方

`docs/spec/06-roadmap.md` のマイルストーンに従って実装します。

- **M1**: 環境整備完了（M2 と並行実施）
- **M2**: コア基盤確立（宣言的 UI DSL・アーキ分界・メモリ安全性・AI 型安全 DSL）
- **M3**: 3 プラットフォーム統合・基盤クレート新規開発
- **M4**: 実機検証・性能ゲート（Conditional Go 条件の再判定）
- **M5**: ストア提出パイプライン完成（実配布署名・notarization・アップロード）
- **M6**: v1 リリース候補

実装着手の最初のタスクは TASK-1.1（宣言的 UI DSL 最小実装）です。

`scripts/build/build-and-sign.sh` / `scripts/android/build-aab.sh` は開発・検証用のビルド
（シミュレータ ad-hoc 署名・開発署名鍵）にとどまり、本番配布署名・ストア提出は現状未実装で
人間担当です。

## セットアップ

### クローンと初期化

```bash
git clone https://github.com/Fandhe-AI/fandhe-multi-platform.git
```

仕様書サブモジュール（`docs/spec/`）は非公開のため任意です。アクセス権がある場合のみ、次を実行します。

```bash
git submodule update --init
```

### 開発環境の準備

```bash
make setup
```

`make setup` は以下を実行します。

- `git submodule update --init` でサブモジュール（`docs/spec/`）の取得を試みる（非公開のため、取得できなくても警告して続行）
- `rustup target add wasm32-unknown-unknown` で Web（wasm32）ターゲットを追加
- lefthook が導入済みなら `lefthook install` を実行し、未導入なら導入方法を案内

lefthook の導入例:

```bash
# macOS
brew install lefthook

# Linux（Ubuntu/Debian）
sudo apt-get install lefthook

# その他の方法は https://github.com/evilmartians/lefthook を参照
```

ビルド成果物（`target/`）のリンク負荷と容量を抑えるため、ルート `Cargo.toml` で `dev`・`test` プロファイルの debuginfo を絞っています（本リポのクレートは `debug = 1`、依存クレート〈workspace 外〉は `[profile.dev.package."*"]` で行番号のみ。panic 位置・バックトレースの行番号は残ります）。溜まった古い成果物は `cargo install cargo-sweep` のうえ `cargo sweep --time 14`（14 日以上使われていない成果物を削除）や `cargo sweep --installed`（rustup に現存しないツールチェーンでビルドした成果物を削除）で消せます。

## 開発コマンド

`make` を実行するとターゲット一覧が表示されます。主なコマンド:

| ターゲット | 説明 |
|-----------|------|
| `make help` | ターゲット一覧（Make 非依存の一覧は `scripts/help.sh`） |
| `make setup` | サブモジュール取得（任意）・wasm ターゲット追加・lefthook 導入 |
| `make build` | workspace 全体を cargo build |
| `make test` | workspace 全体を cargo test |
| `make fmt` | cargo fmt --all でコード整形 |
| `make lint` | fmt チェック + clippy 検査（警告をエラー扱い） |
| `make wasm` | Web（wasm32）ビルド（scripts/web/build-wasm-proxy.sh へ委譲） |
| `make android` | Android クロスビルド（scripts/android/build-android.sh へ委譲） |
| `make verify-targets` | 全対象プラットフォームのターゲット一括コンパイル確認 |
| `make docker-build` | 開発用 Docker イメージをビルド |
| `make docker-shell` | 開発用 Docker コンテナでシェル起動 |
| `make clean` | cargo clean で生成物を削除 |

## Docker で開発する

Android NDK / Apple（Xcode）を導入せず、共通コア・Linux・Web（wasm32）のみで開発する場合:

```bash
make docker-build    # 初回のみ
make docker-shell    # コンテナシェルを起動
```

コンテナ内では通常の cargo コマンドが使えます。

```bash
cargo build
cargo test
cargo fmt
```

**対象範囲と制限:**

- **対象**: 共通コア（crates/fandhe-core）・Linux ターゲット・Web（wasm32）
- **対象外**: Android NDK（ホスト側で scripts/android/setup-android-env.sh を実行する必要があります）、Apple 系（iOS / watchOS / visionOS / macOS は Xcode 依存のため macOS ホスト上での実行が前提）
- **キャッシュ**: cargo レジストリは named volume でキャッシュされるため、再ビルド時の毎回フルダウンロードを避けられます。ビルド成果物（cargo target ディレクトリ）は `/work/target`（リポジトリの bind mount 内）にそのまま生成されます（`.gitignore` 済みのためコミット対象にはなりません）。scripts/web/build-wasm-proxy.sh が成果物パスを固定参照しているための挙動です。

**Linux ホストでの注意:** `make docker-build` はホストの UID/GID（`id -u`/`id -g`）を自動検出してコンテナ内ユーザーへ反映します。bind mount された `/work/target` へコンテナ内ユーザーが直接書き込むため、UID/GID が一致していないと `cargo build` が Permission denied になります（macOS Docker Desktop はファイル共有層がホスト所有権を透過的に扱うため発生しません）。ホストとコンテナ内で UID/GID がずれる場合は `make docker-build HOST_UID=<uid> HOST_GID=<gid>` で明示指定してください。

## Git hooks

GitHub Desktop や他のツールを使う場合でも、コミット前に自動的に以下の検査が実行されます。

### pre-commit

- **cargo fmt チェック**: Rust ファイルが staged されている場合、`cargo fmt --check` で整形違反をエラーにします。整形方法は `make fmt` または `cargo fmt --all`。
- **.env 混入検査**: `.env.example` を除く `.env` 系ファイル自体の誤コミットをファイル名ベースで防ぎます（ファイル内容のシークレットスキャンは create-commit スキル側で実施）。

### commit-msg

コミットメッセージを Conventional Commits 形式（.claude/rules/conventional-commits.md）で検査します。

許可される type: `feat` / `fix` / `docs` / `style` / `refactor` / `perf` / `test` / `build` / `ci` / `chore`

例:

```
feat(core): 宣言的 UI DSL の Screen/Node を追加
fix(android): cargo-ndk のリンクエラーを修正
chore(skills): スキル定義を更新
```

Merge コミット、Revert コミット、`fixup!` / `squash!` コミットは形式検査の対象外です。

### CI との役割分担

開発速度を落とさないよう、フックには短時間で終わる検査だけを置いています。clippy・test・クロスビルドは CI（`.github/workflows/ci.yml`）で実行し、フックでは実行しません。push 前に手元で確認したい場合は `make lint`（fmt チェック + clippy）と `make test` を実行してください。フックの追加・変更方針は `.claude/rules/git-hooks.md` を参照してください。

### EditorConfig

`.editorconfig` により、エディタ間の改行コード・インデント・末尾空白の揺れを防ぎます。主なエディタは自動的に設定を読み込みますが、設定を有効化する必要がある場合もあります。詳細は https://editorconfig.org を参照してください。

**注**: lefthook がコマンドラインから実行できない場合、手動で `lefthook install` を実行してください。`--no-verify` での強制コミットは禁止です（.claude/rules/conventional-commits.md）。

## リポジトリ構成

| 対象 | 説明 |
|-----|------|
| `crates/` | 8 クレート構成。共通コア fandhe-core と、egui プロキシ・フォント資産・プロトタイプ・SwiftUI ブリッジ・Web DOM・winit IME・端末機能（fandhe-device、Phase 8 / #746、feature `device` で切り離し可能）の各レイヤ |
| `platforms/android/` | Android Gradle プロジェクト（GameActivity ベース） |
| `platforms/apple/` | watchOS / visionOS / iOS Xcode プロジェクト（XcodeGen SSOT） |
| `scripts/` | ビルド・テスト・検証スクリプト（android / apple / ios / build / web / metrics / windows / audit） |
| `benchmarks/` | コールドスタート・メモリ計測用ネイティブ比較アプリ（iOS / Android / React Native / Flutter） |
| `docs/spec/` | 仕様書サブモジュール（非公開・任意。REQ-N / TASK-N.M・マイルストーン M1〜M6） |
| `docs/architecture/` | 設計文書（責務分界・IME・トークン・Web・SwiftUI ブリッジ等） |
| `docs/environment/` | 開発環境のセットアップ手順（Android NDK・Apple Developer） |
| `docs/ai/` | AI 向けメタ情報（SCHEMA.md） |
| `docs/operations/` | 運用手順書（ビルドパイプライン・署名・上流クレート監視） |
| `Makefile` | コマンド入口（setup / build / test / wasm / android 等） |
| `Dockerfile` / `compose.yaml` | 開発用コンテナ定義（共通コア・Linux・wasm32） |
| `.github/workflows/` | CI ワークフロー（GitHub ホスト ubuntu-latest。codex review のみ self-hosted） |
| `.claude/` | Agent 定義・運用ルール・スキル管理 |
| `AGENTS.md` | レビュー観点集（ai-review（provider: codex）の基準） |
| `CLAUDE.md` | 開発ガイドライン・委譲方針・規約・subagent 定義 |

詳細は `CLAUDE.md` のリポジトリ構成セクションを参照。

## 型サンプルの実機確認（http_fetch）

`http_fetch` 型サンプルはループバック（`127.0.0.1:8787`）のダミー API を既定の
取得先にしているため、実機（iPhone・Android 実機）からはそのままでは届きません
（Issue #660）。

1. Mac でダミー API を起動する: `node scripts/web/serve-brotli.mjs
   crates/fandhe-egui-proxy/fixtures/http_fetch 8787`（127.0.0.1 固定）。iPhone から
   直接 LAN 経由で取りに来る場合は `python3 -m http.server 8787 --bind 0.0.0.0
   --directory crates/fandhe-egui-proxy/fixtures/http_fetch` を使う。信頼できる LAN
   でのみ実行し、確認後は停止する
2. Android: `adb reverse tcp:8787 tcp:8787` で端末の `127.0.0.1:8787` を Mac へ
   転送する（コード変更不要）。ビルドは `FANDHE_PROXY_APP=http_fetch`。確認後は
   `adb reverse --remove tcp:8787` で転送を外す
3. iPhone: `FANDHE_PROXY_APP=http_fetch FANDHE_ITEMS_URL=http://<Mac の LAN
   IP>:8787/items.json scripts/ios/verify-device-launch.sh`。`FANDHE_ITEMS_URL`
   指定時のみ `.app` の Info.plist に LAN 限定の ATS 例外
   （`NSAllowsLocalNetworking`）が入る。ビルド後は `unset FANDHE_ITEMS_URL`

## Phase 8（端末機能）の切り離し手順

Phase 8（親 #752）は端末機能 3 型（ローカル保存 → HTTP → カメラ）を画面レジストリ方式で
検証するための土台で、spec 要件化が否決された場合に crate と feature を
消すだけで元に戻せる構成にしています（Issue #746）。

1. `Cargo.toml` の `members` から `crates/fandhe-device` を、`[workspace.dependencies]`
   から `ureq` を削除し、`crates/fandhe-device/` ディレクトリを削除する
2. `crates/fandhe-egui-proxy/Cargo.toml` の `default = ["device"]` / `device = [...]`、
   optional な `fandhe-device` / `serde_json` 依存行、`crates/fandhe-egui-proxy/src/device.rs`
   一式（`lib.rs` の対応する `pub mod device;` 行込み）を削除する
3. `crates/fandhe-core/src/screens/{storage_note,http_fetch,camera_capture}.rs` と
   `screens/mod.rs` の対応する `pub mod` 3 行を削除する（`storage_note` は #747、
   `http_fetch` は #748、`camera_capture` は #749 で追加済み）。
   `crates/fandhe-egui-proxy/src/device.rs`（`http_fetch`/`camera_capture` サブモジュール
   含む）と `lib.rs` の `pub mod device;` 行（`#[cfg(all(feature = "device",
   not(target_arch = "wasm32")))]` 付き）も削除する（マクロの `init`/`effect` 区画自体は
   他画面が使う可能性がある汎用機能のため `screens.rs` に残置してよい。`app.rs` の
   `EffectFn`/`with_effect` は #745 の apply API と同様、Phase 8 に依存しない汎用
   フックのため残置してよい）
4. `crates/fandhe-egui-proxy/src/screens/registry.rs` の
   `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` 付き `init`/`effect` 行
   （`storage_note`/`http_fetch`/`camera_capture`）を削除する
5. Android（#750）:
   - `crates/fandhe-egui-proxy/src/device/android.rs`（`DataDir` 解決・`AndroidCamera`・
     `nativeOnCameraResult` の JNI エクスポート）を削除する
   - `crates/fandhe-egui-proxy/src/device.rs` の `#[cfg(target_os = "android")] pub mod android;`
     行と `storage_note_state` の Android 分岐（`use fandhe_device::storage::FsStorage;` を
     含む `#[cfg(target_os = "android")] { ... }` ブロック）を削除する（iOS 分岐のみ残す）
   - `crates/fandhe-egui-proxy/src/device/camera_capture.rs` の `platform_camera()` を
     `DummyCamera` 直書きへ戻す（Android 分岐 `Arc::new(crate::device::android::AndroidCamera::new())`
     を削除し、`#[cfg(not(target_os = "android"))]` の条件分岐で分けていた
     `DUMMY_WIDTH`/`DUMMY_HEIGHT`/`use ... DummyCamera` の cfg も外す）。`CaptureMailbox`
     はターゲット非依存の汎用機能のため残置してよい
   - `crates/fandhe-egui-proxy/src/android.rs` の `ANDROID_APP` static・
     `set_android_app()`/`android_app()`・`android_main` 内の `set_android_app(app.clone())`
     の呼び出しは Phase 8 固有ではない汎用配線のため残置可（削除しても実害はない）
   - `crates/fandhe-egui-proxy/Cargo.toml` の android target 分岐の `jni` 依存行、
     ルート `Cargo.toml` の `[workspace.dependencies]` の `jni` 行を削除する
   - `platforms/android/app/src/main/java/com/fandhe/androidapp/MainActivity.java` の
     `requestCameraPermission`/`takePicturePreview`（`ActivityResultLauncher` 2 つ）・
     `nativeOnCameraResult` native 宣言・`requestCameraCapture`/`launchCameraCapture`/
     `deliverBitmap` メソッドを削除する
   - `crates/fandhe-egui-proxy/src/device/tilt.rs` の `gravity_to_tilt`（Issue #831）・
     `crates/fandhe-egui-proxy/src/device/android.rs` の `nativeOnMotionChanged` JNI
     エクスポートを削除する（`shared_motion`/`drive` 等ターゲット非依存の部分は残置）
   - `MainActivity.java` の `sensorManager`/`motionSensor` フィールド・`motionListener`・
     `nativeOnMotionChanged` native 宣言・`onResume`/`onPause` 内のセンサ登録/解除を
     削除する（Issue #831）
   - ゲームコントローラ（親 #932、Issue #935）: `crates/fandhe-egui-proxy/src/device/
     android_gamepad.rs`（`device.rs` の `pub mod android_gamepad;` 行込み。JNI
     エクスポート `nativeOnGamepadPresenceScanBegin`/`nativeOnGamepadPresenceScanId`/
     `nativeOnGamepadPresenceScanEnd`/`nativeOnGamepadMotion`/`nativeOnGamepadKey`
     の 5 関数を含む）・`crates/fandhe-egui-proxy/src/device/gamepad.rs` の Android
     分岐（`platform_gamepad()` の `#[cfg(target_os = "android")]` ブロックと
     `dead_code` 許可の `android` 追加分）を削除する。`MainActivity.java` の
     `inputManager` フィールド・`gamepadDeviceListener`・
     `reportGamepadPresence`/`axisFlat`・`nativeOnGamepad*` native 宣言 5 本・
     `dispatchKeyEvent`/`dispatchGenericMotionEvent` オーバーライド・
     `onResume`/`onPause` 内の `InputManager` 登録/解除を削除する
   - `platforms/android/app/src/main/AndroidManifest.xml` の `INTERNET`/`CAMERA`
     の `uses-permission` 2 行と `uses-feature` 3 行（`android.hardware.camera.any`/
     `android.hardware.camera`/`android.hardware.camera.autofocus`）を戻す
   - `platforms/android/app/build.gradle.kts` の `androidx.activity:activity` 依存行、
     `scripts/android/versions.env` の `ANDROIDX_ACTIVITY_VERSION` 行を削除する
   - バックグラウンド処理（親 #1042、Issue #1046）: `platforms/android/app/src/main/
     java/com/fandhe/androidapp/BackgroundTaskWorker.java`・`crates/fandhe-egui-proxy/
     src/device/android_background_task.rs`（`device.rs` の `pub mod
     android_background_task;` 行込み）を削除し、`device/background_task.rs::
     implementation()` の Android 分岐を外す。`MainActivity.java` の
     `BACKGROUND_TASK_WORK_NAME` 定数・`awaitOperation`/`backgroundTaskSchedule`/
     `backgroundTaskCancel` メソッドを削除する。
     `platforms/android/app/build.gradle.kts` の `androidx.work:work-runtime`
     依存行、`scripts/android/versions.env` の `WORK_RUNTIME_VERSION` 行を削除する
   - ホーム画面ウィジェット（親 #1064、Issue #1067）: `platforms/android/app/src/main/
     java/com/fandhe/androidapp/HomeWidgetProvider.java`・`res/layout/home_widget.xml`・
     `res/xml/home_widget_info.xml`・`crates/fandhe-egui-proxy/src/device/
     android_home_widget.rs`（`device.rs` の `pub mod android_home_widget;` 行込み）を削除し、
     `device/home_widget.rs::implementation()` の Android 分岐を外す。`res/values/strings.xml`
     の `home_widget_label`/`home_widget_description`、`AndroidManifest.xml` の
     `<receiver>`、`MainActivity.java` の `homeWidgetSetText`/`homeWidgetCurrentText`
     メソッドを削除する
   iOS（#751）: 以下を削除・復元する
   - `crates/fandhe-egui-proxy/ios/fandhe_ios_camera.m`・`crates/fandhe-egui-proxy/build.rs`
   - `crates/fandhe-egui-proxy/src/device/ios.rs`（`device.rs` の `pub mod ios;` 行込み）
   - ルート `Cargo.toml` の `[workspace.dependencies]` の `cc = "1"`、
     `crates/fandhe-egui-proxy/Cargo.toml` の `[build-dependencies]` の `cc`
   - `platforms/apple/FandheIosApp/Resources/Info.plist` の `NSCameraUsageDescription`
   - `crates/fandhe-egui-proxy/src/lib.rs` の iOS 分岐の `deny(unsafe_code)` 緩和
     （`#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]` を
     Android 単独へ戻す）
   - `crates/fandhe-device/src/lib.rs` の `DeviceError::Cancelled`
     （Android（#750）のキャンセル処理でも使う場合は残す）
   - 実機確認用の `FANDHE_ITEMS_URL`（`crates/fandhe-egui-proxy/src/device/http_fetch.rs`）、
     `scripts/ios/verify-device-launch.sh` の ATS 差し込み処理、
     `crates/fandhe-device/src/http.rs` の私設 IPv4 許可（`is_local_plain_http_host`、Issue #660）
   - 位置情報（親 #887、Issue #889）: `crates/fandhe-egui-proxy/apple/fandhe_apple_location.m`・
     `crates/fandhe-egui-proxy/macos/Info.plist`・`build.rs` の macOS 分岐・`lib.rs` の
     macOS `deny(unsafe_code)` 緩和・iOS `Info.plist` の `NSLocationWhenInUseUsageDescription`
6. `cargo build --workspace && cargo test --workspace --all-features` が通ることを確認する
   （#745 の汎用 apply API のみ残るが、それ自体は Phase 8 に依存しないため残置してよい）

## CI

GitHub Actions の workflow（`.github/workflows/`）は GitHub ホストの ubuntu-latest で実行されます（codex review ジョブのみ self-hosted）。詳細は `.claude/rules/ci.md` を参照してください。

## ライセンス

[MIT](./LICENSE-MIT) または [Apache License 2.0](./LICENSE-APACHE) のデュアルライセンスです。
利用者はどちらかを選べます。

明示しない限り、Apache-2.0 第 5 条のとおり、本リポジトリへの取り込みを意図して提出された
コントリビューションは、追加条件なしで上記のデュアルライセンスとして提供されます。

例外として、`crates/fandhe-fonts/assets/NotoSansJP-Regular.otf` は
SIL Open Font License 1.1 です（全文は `crates/fandhe-fonts/assets/OFL.txt`、
出典と著作権表記は `crates/fandhe-fonts/assets/README.md`）。
