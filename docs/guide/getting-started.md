# はじめてのアプリ

- 対応: #1204、REQ-21 / TASK-21.5

fandhe でアプリを新しく作り、端末機能を足し、各 OS で動かして配布物を作るまでの手順です。
本書のコマンドとコードは、リポジトリの外の新しいディレクトリで本書どおりに写して確かめました（13 節）。

v1 前のため API は変わることがあります。crates.io には未公開で、依存は git か path で書きます。
iOS の `cargo fandhe run`・`build` はシミュレータだけで、実機は未対応です（8.5 節）。

## 1. 前提

| 対象 | 要るもの |
| --- | --- |
| 共通 | rustup・cargo。本書は Rust 1.99.1（リポジトリと同じ版）で確かめた |
| macOS | Xcode（コマンドラインツールを含む） |
| iOS | Xcode（コマンドラインツールだけでは足りない。`xcode-select` で選ぶ）・iOS のシミュレータ、Rust の iOS シミュレータ target |
| Linux | `build-essential`・`pkg-config`。実行には `libxcursor1`・`libxi6`・`libxkbcommon-x11-0`（Ubuntu/Debian の例） |
| Web | target `wasm32-unknown-unknown`、`wasm-bindgen` CLI（版はアプリの `Cargo.lock` と同じ）。`--release` は `wasm-opt`（binaryen） |
| Android | Android SDK（`ANDROID_HOME`）・NDK・`cargo-ndk`・`gradle`（PATH。wrapper は使わない）・JDK 17 以上・`adb`、Rust の Android target |

Android の Gradle・SDK platform・build-tools・NDK の版は、リポジトリの `scripts/android/versions.env` に合わせます（生成する Gradle プロジェクトがその版を前提にします）。

足りないものは `cargo fandhe run`・`build` が実行前に全件を 1 行ずつ、直し方つきで出します。環境は変えません。

Android の SDK・NDK・Gradle は、リポジトリを clone して `scripts/android/setup-android-env.sh` で揃えることもできます
（`docs/environment/android-ndk-gradle-setup.md`）。

## 2. `cargo fandhe` を入れる

```sh
cargo install --git https://github.com/Fandhe-AI/fandhe-multi-platform cargo-fandhe --locked
```

`cargo fandhe check`・`gen`・`run`・`build`・`verify-scope` が使えるようになります。
リポジトリを clone してあれば `cargo install --path <clone 先>/crates/cargo-fandhe --locked` でも入ります。

## 3. 新しいアプリを作る

```sh
cargo new --lib my-app
cd my-app
touch src/main.rs
```

`rust-toolchain.toml` を置いて版を揃えます。

```toml
[toolchain]
channel = "1.99.1"
```

`Cargo.toml` です。

```toml
[package]
name = "my-app"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
fandhe = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform" }

[package.metadata.fandhe-app.ios]
bundle-id = "com.example.myapp"

[package.metadata.fandhe-app.android]
application-id = "com.example.myapp"
```

- `cdylib` は Android と Web が読み込む形です。`rlib` は `src/main.rs`（デスクトップ・iOS の実行体）から使うためです
- `version` は数字とドットだけにします（iOS の版になるため、`1.0.0-rc.1` は `gen` が拒否します）
- `bundle-id` は iOS の Info.plist に要ります。iOS を出さないアプリでも、`gen` は無ければ止まります
- `application-id` は `cargo fandhe run|build --os android` に要ります
- clone したリポジトリを使うなら `fandhe = { path = "<clone 先>/crates/fandhe" }` と書きます

`.gitignore` に `gen/`（`cargo fandhe gen` の出力。毎回作り直します）と、署名鍵（9.1 節）の取り違えを防ぐ `*.jks`・`*.keystore` を足します。

```text
/target
/gen
*.jks
*.keystore
```

## 4. 画面を作る

`src/lib.rs` です。状態の型・状態から画面を作る関数・画面を返す入口の 3 つを書きます。

```rust
// Android は生成される `android_main` が `no_mangle` を伴うため deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use fandhe::app::{ProxyApp, ProxyAppDyn};
use fandhe::{Screen, WidgetId, egui};

/// 画面の状態。
#[derive(Default)]
pub struct Counter {
    count: u32,
}

fn increment(s: &mut Counter) {
    s.count += 1;
}

/// 状態から画面を作る。ボタンの処理は状態を書き換える関数で渡す。
fn screen(_: &Counter) -> Screen<Counter> {
    Screen::new("カウンター")
        .dynamic_text(|s: &Counter| format!("押した回数: {}", s.count))
        .button(WidgetId::from_raw(1), "押す", increment)
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    Box::new(ProxyApp::with_screen_fn(ctx, screen, Counter::default()))
}
```

`src/main.rs` です。lib 名は package 名の `-` を `_` にしたものです。

```rust
fn main() -> Result<(), fandhe::app::LaunchError> {
    my_app::main()
}
```

- `#[fandhe::main]` は、デスクトップと iOS の `main`、Android の `android_main`、Web の `start` を生成します。
  付ける関数の名前は `main` 以外にします
- `dynamic_text` の閉包には引数の型（`&Counter`）を書きます。画面の先頭の部品だと型を推論できず、コンパイルエラーになります
- 複数の画面は `fandhe::app::Menu` で組みます（`docs/architecture/consumer-app-structure.md` 3.3 節）

デスクトップで起動します。

```sh
cargo fandhe run
```

`cargo run` でも同じです。ウィンドウに「カウンター」の画面が出ます。

## 5. 端末機能を足す

端末機能は `fandhe` の feature で選びます。feature 名は対応表の id（`docs/device/capabilities/<id>.json` のファイル名）で、
一覧は `crates/fandhe/Cargo.toml` の `[features]` にあります。選んだ機能は `fandhe::<id の - を _ にした名前>` で使えます。

接続状態（`network-status`）を足す例です。`Cargo.toml` の依存を次にします。

```toml
fandhe = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform", features = ["network-status"] }
```

`src/lib.rs` を次にします。端末機能は effect（毎フレーム呼ばれる関数）から呼び、状態は `tx.send` で積んだ処理でだけ変えます。

```rust
// Android は生成される `android_main` が `no_mangle` を伴うため deny へ緩める。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use fandhe::app::{CompletionSender, ProxyApp, ProxyAppDyn};
use fandhe::network_status::{NetworkSnapshot, READ_INTERVAL, platform_network_status};
use fandhe::{Screen, WidgetId, egui};

/// 画面の状態。
#[derive(Default)]
pub struct Counter {
    count: u32,
    network: String,
}

fn increment(s: &mut Counter) {
    s.count += 1;
}

/// 状態から画面を作る。ボタンの処理は状態を書き換える関数で渡す。
fn screen(_: &Counter) -> Screen<Counter> {
    Screen::new("カウンター")
        .dynamic_text(|s: &Counter| format!("押した回数: {}", s.count))
        .button(WidgetId::from_raw(1), "押す", increment)
        .dynamic_text(|s: &Counter| format!("接続: {}", s.network))
}

/// 毎フレーム呼ばれる effect。端末機能を読み、状態は `tx.send` で積んだ処理でだけ変える。
fn effect(s: &Counter, tx: &CompletionSender<Counter>) {
    let network = match platform_network_status().read() {
        Ok(NetworkSnapshot::Online { kind, .. }) => format!("あり（{kind:?}）"),
        Ok(other) => format!("{other:?}"),
        Err(e) => e.to_string(),
    };
    if network != s.network {
        let _ = tx.send(Box::new(move |s: &mut Counter| s.network = network));
    }
    // 値が変わらないフレームでも、次の読み直しを予約する。
    tx.request_repaint_after(READ_INTERVAL);
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    Box::new(ProxyApp::with_screen_fn(ctx, screen, Counter::default()).with_effect(effect))
}
```

- 選ばなかった機能は依存に入らず、呼ぶとコンパイルエラーです。その機能の権限も成果物に入りません
- 機能ごとの呼び方（同期の入口 `platform_<名前>()`、結果が後から届く機能の `begin_*`・`poll_*`・`discard_*`）は、
  各機能 crate（`crates/fandhe-cap-<id>`）の doc を見ます
- 機能の crate（`fandhe-cap-<id>`）を直接依存に書いても選べますが、起動前の登録・終了処理・Android の再生成時の破棄が要る機能は、
  直接依存では走りません（#1453）。標準機能は feature で選んでください。見分け方と第三者の機能の作り方は
  `docs/architecture/capability-crate-contract.md` 5 節です
- Web では、実装が DOM 出力側（`fandhe-web-dom`）に残っている機能は、`#[fandhe::main]` のアプリから `Unsupported` を返します（#1456）。
  network-status はその 1 つで、Web では「このプラットフォームでは未対応です」から始まる文言が出ます。
  該当する機能はまだ洗い出しておらず、対応表で Web が done の機能でも `Unsupported` になりえます

## 6. 権限の説明文を書く

iOS・macOS で利用者の許可が要る機能は、許可ダイアログに出す説明文（Info.plist の `*UsageDescription`）を要求します。
機能 crate はキーだけを宣言し、文言はアプリが書きます。既定の文言はありません。既定の文言のまま審査に出る事故を避けるためです。
Android の権限（`uses-permission`）は機能の宣言から自動で入り、アプリが書くものはありません。

### 6.1 書かないと `gen` が止まる

位置情報（`location`）を足します。

```toml
fandhe = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform", features = ["network-status", "location"] }
```

`cargo fandhe gen` は、足りないキーを OS ごとに 1 行ずつ出して終了コード 1 で止まります。

```text
エラー: ios: 機能 [location] が要求する NSLocationWhenInUseUsageDescription の文言がありません。[package.metadata.fandhe-app.ios.usage-descriptions] に書いてください
エラー: macos: 機能 [location] が要求する NSLocationUsageDescription の文言がありません。[package.metadata.fandhe-app.macos.usage-descriptions] に書いてください
```

### 6.2 書き方

エラーが示したテーブルに、キーと文言を足します。iOS と macOS でキーが違うことがあるので、エラーのキーをそのまま写します。

```toml
[package.metadata.fandhe-app.ios.usage-descriptions]
NSLocationWhenInUseUsageDescription = "近くの店舗を地図に出すために、現在地を使います"

[package.metadata.fandhe-app.macos.usage-descriptions]
NSLocationUsageDescription = "近くの店舗を地図に出すために、現在地を使います"
```

`cargo fandhe gen` が通り、`gen/ios/Info.plist`・`gen/macos/Info.plist` に文言が、`gen/android/AndroidManifest.xml` に
`android.permission.ACCESS_COARSE_LOCATION` が入ります。

文言は、何のために・何を使うかを利用者の言葉で 1 文にします。App Store の審査は、目的が分からない説明文を差し戻します。

| 良い例 | 悪い例 |
| --- | --- |
| 近くの店舗を地図に出すために、現在地を使います | 位置情報を使用します |
| 撮った写真を投稿に添付するために、カメラを使います | カメラへのアクセスが必要です |
| 音声メモを録音するために、マイクを使います | アプリの機能向上のため |

- 文言は空白だけにできず、制御文字も入れられません
- 1 つのキーに 1 つの文言です。言語ごとの文言は `gen` では出せません
- 文言を書いても、選んだ機能が要求しないキーは Info.plist に出ません（エラーにもなりません）。足したいときは
  `[package.metadata.fandhe-app.ios.info-plist]` に書きます（`docs/architecture/capability-declaration.md` 7 節）
- どのキーが要るかは、機能 crate の `Cargo.toml` の `[package.metadata.fandhe.ios]`・`[package.metadata.fandhe.macos]` の
  `usage-descriptions` でも分かります

## 7. `check` と `gen`

| コマンド | すること |
| --- | --- |
| `cargo fandhe check` | 依存にある機能の宣言（`[package.metadata.fandhe]`）を検証する。id の重複などで終了コード 1 |
| `cargo fandhe gen` | 選んだ機能の宣言とアプリの設定から、各 OS の設定（Info.plist・entitlements・AndroidManifest の overlay・Gradle の断片）を `gen/` に作る |
| `cargo fandhe gen --os web` | Web App Manifest だけを作る（`[package.metadata.fandhe-app.web] manifest` が要る） |
| `cargo fandhe gen --eject <パス>` | 生成した設定ファイルを `eject/` へ書き出し、以後は生成の代わりに使う（消せば生成へ戻る） |

`run`・`build` は必要な分の `gen` を自分で走らせます。生成値を変えたいときは、まずアプリの `[package.metadata.fandhe-app.*]` への追記で
足りるかを見ます（`docs/architecture/capability-declaration.md` 7 節）。

## 8. OS ごとのビルドと実行

```sh
cargo fandhe run|build [--os macos|windows|linux|web|android|ios] [--release] [--device] [--manifest-path <Cargo.toml>]
```

`--os` を省くとホストのデスクトップ OS です。手順の詳細は `docs/architecture/run-build-cli.md` です。

### 8.1 macOS・Windows・Linux

```sh
cargo fandhe run      # cargo run と同じ
cargo fandhe build    # cargo build と同じ
```

ホストと違う OS はビルドできません（`--os windows はホスト（macos）と違うため、クロスビルドの実行はできません`）。
各 OS の上で実行します。

### 8.2 Web

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version <Cargo.lock の wasm-bindgen の版> --locked
cargo fandhe build --os web
cargo fandhe run --os web
```

- 成果物は `target/fandhe/my-app/web/`（`index.html`・`.js`・`.wasm`・フォント）です
- `run` は 127.0.0.1 の空きポートで配り、URL を出します（Ctrl-C で終了）
- CLI と `Cargo.lock` の版が違うと、`wasm-bindgen CLI の版 … が Cargo.lock の … と一致しません` と入れる版つきで止まります
- アプリに `web/` ディレクトリがあれば丸ごと写します。`web/index.html` を置けば既定の `index.html` の代わりに使います

### 8.3 Android

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk --locked
cargo fandhe build --os android   # 3 ABI を含む debug APK
cargo fandhe run --os android     # adb の端末 1 台の ABI だけ作り、入れて起動する
```

- 成果物は `target/fandhe/my-app/android/app/build/outputs/apk/debug/app-debug.apk` です
- Gradle プロジェクト（殻）は `target/fandhe/my-app/android/` に毎回作ります。`MainActivity` を書く必要はありません
- `run` は端末が 0 台・複数台なら止まります。複数なら `ANDROID_SERIAL` で選びます。エミュレータは自動で起動しません
- アイコン・`google-services.json`・`versionCode` などの殻のファイルは、`eject/android/shell/` に同じ相対パスで置いて置き換えます
  （`docs/architecture/run-build-cli.md` の「殻の重ね置き」）

### 8.4 Windows の配布物

signtool の署名・パッケージ ID（#1208）、MSIX とストア提出（#3）は未対応です。

### 8.5 iOS

```sh
rustup target add aarch64-apple-ios-sim
cargo fandhe build --os ios   # シミュレータ向けの .app
cargo fandhe run --os ios     # 起動中のシミュレータへ入れて起動する
```

- 先にシミュレータを起動しておきます（Simulator.app か `xcrun simctl boot "<端末名>"`。端末名は `xcrun simctl list devices available`）。
  自動では起動しません。起動中の iOS シミュレータが 0 台・複数台なら止まります（複数なら使わないものを `xcrun simctl shutdown` で止めます）
- Intel Mac では target が `x86_64-apple-ios` です（`rustup target add x86_64-apple-ios`）。cargo-fandhe がホストの CPU から選びます
- 成果物は `target/fandhe/my-app/ios/my-app.app` です。ad-hoc 署名（`codesign --sign -`）で、シミュレータ専用です
- Xcode プロジェクトは作りません。cargo で実行体を作り、`gen` の Info.plist と合わせて `.app` を組みます
- entitlements は Xcode のシミュレータ向けと同じく実行体に埋め込み、署名には入れません（`application-identifier` は bundle id で足します）
- `eject/ios/Info.plist` を置いたときは、`CFBundleIdentifier`・`CFBundleExecutable`・`MinimumOSVersion` を `bundle-id`・bin 名・17.0 にそろえます（違えば止まります）
- 拡張（ウィジェット・ライブアクティビティ）は `.app` に入りません。拡張を要る機能はシミュレータでも動きません
- 実機（`--device`）は未対応です（#1425）

## 9. 配布物を作る

| OS | 作り方 | 状態 |
| --- | --- | --- |
| Android | `cargo fandhe build --os android --release` で署名済みの `.aab` | 使える。Play への提出は手で行う |
| Web | `cargo fandhe build --os web --release`（`wasm-opt -Os` を通す）。`target/fandhe/my-app/web/` を静的ホスティングに置く | 使える（本書の手順では未確認、13 節） |
| iOS・macOS | `cargo fandhe` での署名・提出は未対応。`build --os ios` の `.app` はシミュレータ専用 | #1425。証明書とプロファイルの用意は `docs/environment/apple-developer-setup.md` |
| Windows | 未対応 | signtool の署名・パッケージ ID は #1208、MSIX とストア提出は #3 |

### 9.1 Android の署名

`--release` は 3 つの環境変数がそろわないと止まります（無署名では成功させません）。
keystore はアプリのリポジトリの外に作ります。パスワードは keytool と `read` の対話入力だけで渡し、コマンドライン・`export` の行・スクリプトに書きません
（シェルの履歴とプロセス一覧に残さないため。`docs/operations/android-aab-signing.md` 2 節）。

```sh
mkdir -p ~/.config/my-app
keytool -genkeypair -keystore ~/.config/my-app/upload.jks -storetype PKCS12 -alias upload \
  -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=my-app"
export FANDHE_ANDROID_KEYSTORE=~/.config/my-app/upload.jks
read -rs FANDHE_ANDROID_KEYSTORE_PASSWORD && export FANDHE_ANDROID_KEYSTORE_PASSWORD
export FANDHE_ANDROID_KEY_ALIAS=upload
cargo fandhe build --os android --release
```

keytool はパスワードを 2 回、`read` は 1 回、入力を求めます（`read` は入力を画面に出しません）。

- 成果物は `target/fandhe/my-app/android/app/build/outputs/bundle/release/app-release.aab` です
- 鍵のパスワードが keystore と違うときだけ `FANDHE_ANDROID_KEY_PASSWORD` を足します
- `run --os android --release` は署名済みの `.apk` を入れて起動します
- keystore とパスワードはリポジトリに入れません。2 回目以降の提出は `versionCode` を上げます（8.3 節の殻の置き換え）

## 10. 既知の落とし穴

### 10.1 Swift 薄層・弱リンクを持つ機能（iOS・macOS）

alarm・image-generation・live-activity・on-device-llm・screen-capture・translation の 6 機能は、Swift の薄層か弱リンクを持ちます。
そのリンク指定は機能 crate の `build.rs` から最終成果物へ届かないため、feature で選ぶだけでは iOS・macOS のリンクが
`Undefined symbols` で失敗します。アプリが指定を受け取って出し直します（`docs/architecture/capability-crate-contract.md` 5.4 節）。

1. 機能 crate を直接依存にも書き、`fandhe-platform-apple`（feature `build`）を build 依存にします。
   リンク指定は直接の依存元の `build.rs` にしか渡らないため、feature で選んだ機能でも直接依存が要ります

   ```toml
   [dependencies]
   fandhe = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform", features = ["alarm"] }
   fandhe-cap-alarm = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform" }

   [build-dependencies]
   fandhe-platform-apple = { git = "https://github.com/Fandhe-AI/fandhe-multi-platform", features = ["build"] }
   ```

   `fandhe` に `rev`・`tag`・`branch` を付けるときは、機能 crate と `fandhe-platform-apple` にも同じ値を付けます。
   別のコミットを指すと同じ機能 crate が 2 つ入り、`links` の重複で cargo がビルドを拒否します

2. アプリの直下に `build.rs` を置き、機能ごとに `import_link_args` を呼びます。引数は機能 crate の `Cargo.toml` の `links` の値
   （`fandhe_cap_<id の - を _ にした名前>`）です。Apple 以外のターゲットでは何もしません

   ```rust
   fn main() {
       fandhe_platform_apple::build::import_link_args("fandhe_cap_alarm");
   }
   ```

3. Swift 薄層を持つ 5 機能（screen-capture 以外）を `cargo build`・`cargo check` で直接 iOS 向けにビルドするときは、
   `iOS の最小 OS（IPHONEOS_DEPLOYMENT_TARGET）が 17.0 以上でない` で止まります。アプリの `.cargo/config.toml` に次を書きます。
   cargo の `[env]` は依存先から引き継がれないため、アプリ側に要ります（#1398）。
   `cargo fandhe run|build --os ios` は同じ値を cargo へ渡すため、この設定が無くても止まりません

   ```toml
   [env]
   IPHONEOS_DEPLOYMENT_TARGET = { value = "17.0", force = false }
   ```

### 10.2 そのほか

- **macOS の実行体に Info.plist が入らない**: `cargo fandhe run`・`build` の macOS の実行体には、`gen/macos/Info.plist` が埋め込まれません（#1454）。
  説明文を要する機能（位置情報・カメラ・マイク等）は、macOS で許可ダイアログが出ないことがあります
- **Android では `forbid(unsafe_code)` にできない**: `android_main` が `no_mangle` を伴うため、4 節のように Android だけ `deny` にします
- **wasm-bindgen の版**: `fandhe` が `wasm-bindgen` を `=` で固定しているため、CLI は `Cargo.lock` の版に合わせます（8.2 節）
- **終了処理を持つ機能の直接依存**: 5 節。feature で選びます（#1453）
- **Web で `Unsupported` になる機能**: 5 節。対応表で Web が done でも、`#[fandhe::main]` のアプリでは未対応のことがあります（#1456）
- **`gen/` に手を入れない**: 毎回作り直します。変えたいときは追記か `--eject` を使います（7 節）

## 11. AI に画面と機能を書かせる

AI（Claude Code など）に、アプリのリポジトリで画面を作らせ、端末機能を足させるためのスキル `fandhe-app` があります。
手順（画面を作る・端末機能を足す・`check` と `gen` を通す）の `SKILL.md` と、AI 向けの資料 `reference.md` からなります。

依存に入れた fandhe と同じ版のスキルを、cargo が取得した fandhe のソースから写します。アプリの直下で実行します（POSIX シェル）。

```sh
FANDHE_SRC=$(cargo metadata --format-version 1 \
  | grep -o '"manifest_path":"[^"]*/crates/fandhe/Cargo.toml"' | head -n 1 \
  | sed -e 's/^"manifest_path":"//' -e 's#/crates/fandhe/Cargo.toml"$##')
if [ -d "$FANDHE_SRC/skills/fandhe-app" ]; then
  mkdir -p .claude/skills
  cp -R "$FANDHE_SRC/skills/fandhe-app" .claude/skills/
else
  echo "fandhe のソースが見つかりません" >&2
fi
```

「見つかりません」と出たら、`cargo metadata` のエラーと、`Cargo.toml` に `fandhe` の依存があるかを確かめます。

- Claude Code は `.claude/skills/fandhe-app/SKILL.md` を読み、画面や端末機能を頼むとスキルを使います
- clone したリポジトリを使うなら `cp -R <clone 先>/skills/fandhe-app .claude/skills/` でも同じです
- fandhe の版を上げたら、同じ手順で写し直します。スキルの資料は写した時点の API に合わせてあります
- スキルは、AI に fandhe のソース（`FANDHE_SRC`。git 依存なら `~/.cargo/git/checkouts/` の下）の `docs/ai/SCHEMA.md`・
  機能 crate の doc・対応表を読ませます。Claude Code は、`claude --add-dir "$FANDHE_SRC"` で起動するか、
  起動後に `/add-dir <FANDHE_SRC のパス>` で足して、アプリの外のそのディレクトリを読めるようにします
- スキルを使わない AI ツールには、`reference.md` を最初に読ませます

## 12. 関連文書

| 文書 | 内容 |
| --- | --- |
| `docs/architecture/consumer-app-structure.md` | 入口・宣言・crate 構成の設計 |
| `docs/architecture/capability-crate-contract.md` | 機能 crate の契約、第三者の機能の作り方 |
| `docs/architecture/capability-declaration.md` | 宣言の欄・検証・生成・eject |
| `docs/architecture/run-build-cli.md` | `cargo fandhe run`・`build` の手順と前提の検査 |
| `docs/operations/android-aab-signing.md` | Android の署名鍵の扱い |

## 13. 確かめた範囲

macOS（Apple silicon）で、リポジトリの外の新しいディレクトリに本書どおりに作り、依存は git（`main`）で確かめました。

| 手順 | 結果 |
| --- | --- |
| 2 節の `cargo install --git` | 入った |
| 4 節・5 節の `cargo fandhe run` | 画面が出て、接続状態（Wi-Fi）が出る |
| 6.1 節の説明文なしの `cargo fandhe gen` | 6.1 節の 2 行で終了コード 1 |
| 6.2 節の説明文ありの `cargo fandhe check`・`gen` | 終了コード 0。Info.plist に文言、Manifest に権限 |
| 8.2 節の `build --os web`・`run --os web` | Chrome で画面が出る（接続状態は未対応の表示、#1456） |
| 8.3 節の `build --os android`・`run --os android` | エミュレータ（API 34）で起動し、ボタンで回数が増える |
| 9.1 節の keystore の作成と `build --os android --release` | 署名済みの `.aab` ができる（jarsigner の検証が通る） |
| 10.1 節の 6 機能の組み込み | screen-capture は、`build.rs` が無いと macOS のリンクが `Undefined symbols` で失敗し、手順どおりに足すと通って起動する。alarm は、`.cargo/config.toml` が無いと 10.1 節の文言で止まり、`build.rs` が無いと iOS シミュレータ向けのリンクが失敗し、両方を足すと通る |
| 8.5 節の `run --os ios`・`build --os ios --release`（4 節のアプリ、依存は path） | iOS シミュレータ（iOS 26.5）で起動し、「カウンター」の画面が出る。`--release` の `.app` もできる |
| 10.1 節の alarm を `.cargo/config.toml` なしで `run --os ios`（依存は path） | `build.rs` があれば通り、シミュレータで起動する |
| 11 節のスキルの導入（依存は path） | `.claude/skills/fandhe-app/` に写せた。経緯を知らない AI が、スキルだけを頼りに位置情報（`location`）の画面を足し、`cargo fandhe check`・`gen`・macOS の `cargo build`・`cargo test` が通った |

Windows・Linux の `run`・`build`、Web の `--release`、実機での起動は、この手順では確かめていません。
11 節を git（`main`）依存で入れる手順は、`skills/` が `main` に入るまで確かめられないため、未確認です。
