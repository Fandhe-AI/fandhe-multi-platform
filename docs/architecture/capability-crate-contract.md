# 機能 crate の契約

- 状態: 採用
- 対応: #1203、REQ-21 / TASK-21.5。上位の方針は `consumer-app-structure.md` の 3.2 節・8 節

## 1. 位置づけ

本書は、端末機能を 1 つの crate として足すときに守る契約（crate の構成・公開 API・宣言・アプリへの組み込み）を定めます。
標準機能（`crates/fandhe-cap-<id>`）も第三者の機能も同じ契約に従います。

- 配布・登録の仕組み（レジストリ・機能の一覧・検索）は作りません。第三者の機能は普通の crate として配り、
  利用者は `Cargo.toml` の依存に足して選びます
- 標準機能も、入口 crate `fandhe` の feature を経由せず依存の追加で選べます（5.1 節）。
  ただし起動前の登録・終了処理・Activity 再生成時の破棄は feature を選んだときだけ走ります（5.3 節）
- 宣言の欄と検証・統合の規則は `capability-declaration.md`、標準機能を移す手順は
  `docs/operations/capability-crate-migration.md` が正です。本書はそれらのうち第三者にも効く部分をまとめます

## 2. crate の構成

- 1 機能 1 crate にします。中身は契約（trait・型）、失敗の種類、OS ごとの実装、宣言（`Cargo.toml`）です。
  Java・ObjC・Swift の薄層と `build.rs` も同じ crate に置きます
- 名前は自由です。`cargo fandhe` は crate 名ではなく `[package.metadata.fandhe]` の有無で機能を見分けます
- `fandhe-cap-` 接頭辞は標準機能が使います。この接頭辞の crate には、宣言があることと、crate 名が `fandhe-cap-<id>` と
  一致することを `cargo fandhe check` が求めます。第三者は自分の接頭辞（例: `acme-cap-hello`）を使います
- `#![forbid(unsafe_code)]` を付けます。FFI で `unsafe` が要る OS だけ `cfg_attr` で `deny` へ緩め、各呼び出しに
  `// SAFETY:` を付けます（`.claude/rules/coding-rust.md`）

### 2.1 依存してよい crate

| crate | 第三者の機能 | 標準機能 | 用途 |
| --- | --- | --- | --- |
| `fandhe-runtime` | 可 | 可 | 完了通知 `CompletionSender`・要求の追跡 `request_slot`・再描画 |
| `fandhe-platform-<os>` | 可 | 可 | OS 層の定型（JNI・main thread・共通の provider 等）。Apple の `build.rs` の helper は `fandhe-platform-apple` の feature `build` を build 依存にして使う |
| 他の機能 crate | 可 | 可 | 他機能の OS 機能を実際に使うとき。依存先の宣言も一緒に生成へ入る（`consumer-app-structure.md` 8 節） |
| `fandhe-device` | 可（安定しない） | 可 | OS に触れない共有の型・検証・定数。標準機能が共有する置き場で、0.x の間は契約として安定させない |
| `fandhe-core` | 不要 | 主に `driver` | カタログの画面の状態型（`fandhe_core::screens`）とつなぐ effect ドライバ用（`consumer-app-structure.md` 8.1 節）。例外は日付計算の `fandhe_core::utc_date`（background-task の Windows） |

- egui・`fandhe-render-egui`・入口 crate `fandhe` には依存しません。レンダラを差し替えても機能 crate を変えずに済ませるためです
  （`consumer-app-structure.md` 7 節）。標準機能では、`fandhe` が機能 crate に依存する側なので循環もします
- `fandhe-runtime`・`fandhe-platform-*` は、アプリの `fandhe` と同じ版に解決されるよう版の指定をそろえます。
  版がずれると runtime が 2 つ解決され、起床が届かず型も一致しません（0.x の間は全 crate 同版、#1224）

## 3. 公開 API

crate ルートから出すのは、契約と、画面の状態型に依存しない入口だけです。画面の状態型を引数に取る effect は
crate ルートから出しません。利用者は自分の状態型で effect を書き、その中から入口を呼びます（5.2 節）。

### 3.1 同期の入口

- `pub fn platform_<stem>()` が trait の実装を返します。`<stem>` は id の `-` を `_` にした名前です
- 実機能に届かない OS では `Unsupported` を返す実装を返します。成功を偽装しません
- 標準機能では、この入口の公開を `node scripts/device/migrate-capability.mjs sync --check` が検査します。
  第三者の crate は検査の対象外ですが、同じ形にすると利用者が迷いません

### 3.2 begin / poll / discard の入口

OS の結果がコールバックで非同期に届き、同期の入口では実機能に届かない OS がある機能は、その OS 向けに
begin / poll / discard 型の入口も出します。同期の入口が OS の完了まで待つ機能も同じ入口を出し、待ちを打ち切れるようにします。
満たす条件は `consumer-app-structure.md` 8.2 節が正です。要点は次のとおりです。

- 入口の名前は `begin_<stem>` / `poll_<stem>` / `discard_<stem>` にそろえます（`PublicEntry` の doc と同じ）
- 結果は要求ごとに 1 回だけ届けます。届かないまま失われた要求は、失敗として 1 回だけ知らせます
- discard は Rust 側の記録だけを捨てます。遅れて届いた結果は id で照合して捨てます
- OS 側でまだ進行中なら、次の begin は busy で断ります
- 部品は `fandhe_runtime::request_slot` の `RequestSlot`（要求 1 件の追跡）・`SlotOwner`（画面と公開入口の排他）・
  `PublicEntry`（公開入口の poll・discard・同期の待ち）です。同じ仕組みを新しく書きません

### 3.3 起動・終了の口

起動前の登録や終了処理が要る機能は、crate ルートから関数を出します。次の表の名前は、標準機能の足し忘れを
`crates/fandhe/tests/features.rs` が検査する名前です。

| 名前 | 呼ぶ時点 | 例 |
| --- | --- | --- |
| `register` | イベントループの開始前（main thread） | URL scheme の受信、背景転送の完了通知 |
| `shutdown`・`stop_shared_*` | アプリの終了（ウィンドウを閉じる経路）。`ProxyApp` の破棄でも呼ぶ機能がある（memory-pressure） | 子プロセス・OS のセッションの停止 |
| `reset_*` | Android の Activity 再生成の前 | 保留中の要求と処理済み世代の破棄 |

- `register` は引数を取ることがあります（`fandhe_cap_spotlight_index::register(URL_PREFIX)`）
- 表の外にも、入口が起動前に呼ぶ Windows の処理があります。タスク スケジューラによる再起動の判定と実行
  （`fandhe_cap_background_task::parse_scheduled_args`・`run_scheduled`）、ファイル起動の経路の登録
  （`fandhe_cap_file_association::windows::register_route`）、単一インスタンスの調停
  （`fandhe_cap_url_scheme::windows::win::startup`）です。`tests/features.rs` はこれらを検査しません

標準機能の分は入口 crate `fandhe` が feature に応じて呼びます（`crates/fandhe/src/entry/lifecycle.rs`）。
第三者の機能の分を呼ぶ口は無く、利用者が呼びます（5.3 節）。

## 4. 宣言（`[package.metadata.fandhe]`）

各 OS の設定（権限・Info.plist のキー・entitlement・framework・Android のコンポーネント・Gradle 依存など）の正本は、
機能 crate の `Cargo.toml` の `[package.metadata.fandhe]` です。欄と検証規則は `capability-declaration.md` の 2 節・3 節です。

- `id` は kebab-case で、`cargo metadata` で解決した全 crate の中で一意です。重複は `cargo fandhe check` が拒否します。
  第三者は、後から足される標準機能の id と衝突しないよう組織名などを前に付けます（例: `acme-hello`）
- 宣言が生成に入るのは、アプリから通常依存で到達するときだけです。dev 依存・build 依存は数えず、
  `[target.'cfg(…)'.dependencies]` はその OS の生成にだけ入ります（`capability-declaration.md` 5 節・7 節「入力」）
- 権限の説明文（`usage-descriptions`）はキーだけを宣言し、文言は利用者が `[package.metadata.fandhe-app.<os>.usage-descriptions]`
  に書きます。文言が無いと `cargo fandhe gen` が失敗します
- 依存 crate がアプリの同一性やバックアップ範囲を書き換えられないよう、一部の欄は許可リストです
  （`main-activity-attributes`・`web.manifest` など）。機能の `info-plist` が殻の確定値（bundle id・scene 構成・実行体名など）と
  同じキーを書くと、`cargo fandhe gen` がエラーにします。第三者の機能にも同じ制限がかかります
- Android の `java`・`res` に書いたディレクトリは、生成される `fandhe.gradle` の `srcDirs` に入ります。
  Activity のライフサイクルに参加する機能は、共通層 `fandhe-platform-android` の `FeatureLifecycleProvider` を継承した
  非公開 provider を `components` に宣言します（`consumer-app-structure.md` 4 節）

## 5. アプリへの組み込み

### 5.1 依存の追加

第三者の機能も標準機能も、アプリの `Cargo.toml` の `[dependencies]` に足すだけで選べます。標準機能は、入口 crate `fandhe` の
feature（対応表の id）で選ぶこともできます。

| 選び方 | 契約の呼び方 | 宣言 | 起動・終了の口（3.3 節） |
| --- | --- | --- | --- |
| `fandhe` の feature | `fandhe::<stem>::…` | 生成に入る | 入口が呼ぶ |
| 標準機能の crate を直接依存 | `fandhe_cap_<stem>::…` | 生成に入る | 呼ばれない（下記） |
| 第三者の crate を依存 | その crate のパス | 生成に入る | 呼ばれない（下記） |

起動・終了の口を持たない標準機能（`fandhe-cap-network-status` など）は、直接依存だけで feature と同じに動きます。
口を持つ標準機能を直接依存だけで選ぶと、起動前の登録・終了処理・再生成時の破棄が走りません。その機能は feature で選びます。
口を持つかどうかは、3.3 節の表の名前が crate ルートにあるか、表の外の Windows の処理の対象（background-task・file-association・
url-scheme）かで分かります。

### 5.2 画面の effect

画面の状態と effect は利用者が書きます。effect は毎フレーム `effect(&state, &sender)` の形で呼ばれ、
状態は `CompletionSender::send`（時間をおくなら `after`）で積んだ完了クロージャでだけ変えます（`fandhe_runtime::EffectFn`）。
次は 8 節のアプリ（`hello-app`）の `src/lib.rs` から、確認用の撮影の部分を除いたものです。

```rust
// Android は生成される `android_main` が `no_mangle` を伴うため `deny` へ緩める（consumer-app-structure.md 3.3 節）。
#![cfg_attr(not(target_os = "android"), forbid(unsafe_code))]
#![cfg_attr(target_os = "android", deny(unsafe_code))]

use fandhe::Screen;
use fandhe::app::{CompletionSender, ProxyApp, ProxyAppDyn};
use fandhe::egui;
use fandhe_cap_network_status::{NetworkSnapshot, READ_INTERVAL};

/// 画面の状態。挨拶はボタンで 1 回、接続状態は読み直しのたびに更新する。
#[derive(Default)]
pub struct HelloState {
    requested: bool,
    greeting: String,
    network: String,
}

fn request(s: &mut HelloState) {
    s.requested = true;
}

fn screen(_: &HelloState) -> Screen<HelloState> {
    Screen::new("hello")
        .button(fandhe::WidgetId::from_raw(1), "挨拶する", request)
        .dynamic_text(|s| format!("挨拶: {}", s.greeting))
        .dynamic_text(|s| format!("接続: {}", s.network))
}

/// 画面の effect（毎フレーム呼ばれる）。状態型はこのアプリのもので、機能 crate の契約だけを呼ぶ。
/// 状態は `tx.send` の完了経由でだけ変える。
fn effect(s: &HelloState, tx: &CompletionSender<HelloState>) {
    if s.requested {
        let greeting = match acme_cap_hello::platform_hello().greet("fandhe") {
            Ok(g) => g,
            Err(e) => e.to_string(),
        };
        let _ = tx.send(Box::new(move |s: &mut HelloState| {
            s.requested = false;
            s.greeting = greeting;
        }));
    }
    // 接続状態は pull 型。値が変わったときだけ送り、変わらなくても次の読み直しを予約する。
    let network = match fandhe_cap_network_status::platform_network_status().read() {
        Ok(NetworkSnapshot::Online { kind, .. }) => format!("あり（{kind:?}）"),
        Ok(other) => format!("{other:?}"),
        Err(e) => e.to_string(),
    };
    if network != s.network {
        let _ = tx.send(Box::new(move |s: &mut HelloState| s.network = network));
    }
    tx.request_repaint_after(READ_INTERVAL);
}

#[fandhe::main]
fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    let state = HelloState {
        requested: true,
        ..Default::default()
    };
    Box::new(ProxyApp::with_screen_fn(ctx, screen, state).with_effect(effect))
}
```

- 値が繰り返し変わる pull 型の機能は、送る値が変わらないフレームでも `tx.request_repaint_after(間隔)` で次の読み直しを予約します
- `fandhe::app::Menu` で複数画面を組むときも、画面ごとに同じ形で `ProxyApp` を作ります
- 確かめた実験のアプリは、これに加えて `FANDHE_PROXY_AUTOSHOT` があれば `ProxyAppDyn::set_autoshot` で撮影を仕掛けます
  （確認用。返す前に `Box<dyn ProxyAppDyn>` を `mut` で受けて呼ぶ）

### 5.3 起動前の登録・終了処理・再生成時の破棄

第三者の機能と、直接依存で選んだ標準機能の口は、入口（`#[fandhe::main]`）からは呼ばれません。

- 起動前の登録（`register`）: デスクトップと iOS では、利用者の `src/main.rs` で入口より前に呼びます

  ```rust
  fn main() -> Result<(), fandhe::app::LaunchError> {
      acme_cap_hello::register();
      hello_app::main()
  }
  ```

- Android の `android_main` と Web の `start` は生成されるため、前に処理を挟めません。Android で起動時に要る処理は、
  宣言した provider（4 節）がプロセス開始時に行います
- 終了処理（`shutdown`）と Android の再生成時の破棄（`reset_*`）を利用者が登録する口はありません（#1453）

### 5.4 Swift 薄層・弱リンクを持つ機能（`links` の出し直し）

`cargo:rustc-link-arg` は出した package 自身の成果物にしか効きません。Swift の薄層（`compile_swift`）や弱リンク
（`weak_link_framework`）を使う機能 crate は、指定を Cargo の `links` メタデータで公開し、最終成果物を作るアプリが出し直します
（`consumer-app-structure.md` 9 節）。

- 機能 crate: `Cargo.toml` に `links = "<名前>"` を書き、`build.rs` の最後で
  `fandhe_platform_apple::build::export_link_args()` を呼びます
- アプリ: その機能 crate を直接依存に書き、`fandhe-platform-apple`（feature `build`）を build 依存にして、`build.rs` で
  `fandhe_platform_apple::build::import_link_args("<名前>")` を呼びます。`links` のメタデータ（`DEP_<名前>_*`）は
  直接の依存元の build script にしか渡らないため、`fandhe` の feature で選んだ標準機能でも直接依存が要ります

## 6. 標準機能と第三者の機能

| 項目 | 標準機能（`fandhe-cap-<id>`） | 第三者の機能 |
| --- | --- | --- |
| 宣言の収集・検証（`check`）・生成（`gen`）・到達判定 | 同じ | 同じ |
| `verify-scope` の到達した宣言 | 同じ | 同じ |
| 名前と id の一致・宣言の必須 | `check` が求める | 求めない |
| `fandhe` の feature と `fandhe::<stem>` の再公開 | ある | ない |
| 起動・終了の口の呼び出し | feature で選べば入口が呼ぶ | 利用者が呼ぶ（5.3 節） |
| 公開入口の検査（`sync --check`）・対応表・カタログ（`fandhe-gallery`） | 対象 | 対象外 |
| 画面の状態型（`fandhe_core::screens`）と effect ドライバ（`driver`） | カタログ用に持つ | 持たない（利用者が書く） |

## 7. 守らない範囲

- 配布・発見・登録の仕組み。第三者の crate の品質・権限の妥当性は、利用者が依存を足すときに判断します
- 第三者の機能の終了処理と Android の再生成時の破棄を呼ぶ口。直接依存で選んだ標準機能の起動・終了の口も呼びません（#1453）
- 版の互換。0.x の間は `fandhe-runtime`・`fandhe-platform-*`・`fandhe-device` の契約が変わりえます（#1224）
- `verify-scope` は、カタログ（`--catalog` の workspace）に無い機能の混入を検出しません。
  第三者の機能が選ばれていない成果物を検査するときは、その crate を含む workspace をカタログに渡します
- 利用者アプリの macOS の実行体には、`cargo fandhe gen` の Info.plist がまだ届きません（#1454）
- `fandhe::__private`（`#[fandhe::main]` の生成コード用）は互換を保証しません

## 8. 最小例（確かめた手順）

リポジトリの外に、第三者の機能 crate と、それと標準機能 1 つを使うアプリを作り、path 依存でリポジトリの crate を使って確かめました。
標準機能は `fandhe` の feature を使わず `fandhe-cap-network-status` を直接依存に足しました。
toolchain はリポジトリと同じ 1.99.1 です（`rust-toolchain.toml` を置く）。どちらの crate も `[workspace]` を持つ独立の workspace です。

```text
ext/
├── rust-toolchain.toml  channel = "1.99.1"
├── acme-cap-hello/      第三者の機能（fandhe-cap- 接頭辞なし。依存は std だけ）
└── hello-app/           利用者のアプリ（#[fandhe::main]）
```

第三者の機能 crate の `Cargo.toml` です。

```toml
[package]
name = "acme-cap-hello"
version = "0.1.0"
edition = "2024"

[workspace]

[package.metadata.fandhe]
id = "acme-hello"

[package.metadata.fandhe.ios]
usage-descriptions = ["NSSpeechRecognitionUsageDescription"]
info-plist = { AcmeHelloGreeting = "hello" }

[package.metadata.fandhe.macos]
info-plist = { AcmeHelloGreeting = "hello" }

[package.metadata.fandhe.android]
permissions = [{ name = "android.permission.VIBRATE" }]
```

`src/lib.rs` は契約の trait `Greeter`・失敗の種類 `HelloError`・入口 `platform_hello()`・起動前の登録 `register()` を出します
（デスクトップは fake、他の OS は `HelloError::Unsupported`）。

アプリの `Cargo.toml` です（`src/lib.rs` は 5.2 節、`src/main.rs` は 5.3 節）。

```toml
[package]
name = "hello-app"
version = "0.1.0"
edition = "2024"

[lib]
name = "hello_app"
crate-type = ["cdylib", "rlib"]

[[bin]]
name = "hello-app"
path = "src/main.rs"

[workspace]

[package.metadata.fandhe-app.ios]
bundle-id = "com.example.hello"

[package.metadata.fandhe-app.ios.usage-descriptions]
NSSpeechRecognitionUsageDescription = "挨拶の確認に使います"

[package.metadata.fandhe-app.macos]
bundle-id = "com.example.hello"

[dependencies]
fandhe = { path = "<repo>/crates/fandhe" }                                       # feature は選ばない
fandhe-cap-network-status = { path = "<repo>/crates/fandhe-cap-network-status" } # 標準機能を直接依存
acme-cap-hello = { path = "../acme-cap-hello" }                                  # 第三者の機能
```

アプリのディレクトリで実行した結果です。

| コマンド | 結果 |
| --- | --- |
| `cargo fandhe check` | 終了コード 0 |
| `cargo fandhe gen` | `gen/` に 6 ファイル。iOS の Info.plist に `AcmeHelloGreeting` と `NSSpeechRecognitionUsageDescription`（アプリの文言）、macOS の Info.plist に `AcmeHelloGreeting`、AndroidManifest に `ACCESS_NETWORK_STATE`（標準機能）と `VIBRATE`（第三者）。`fandhe.gradle` の `srcDirs` に `fandhe-cap-network-status/android/java` |
| `cargo fandhe verify-scope --catalog <repo>/Cargo.toml --os ios gen/ios/Info.plist` | 選ばなかった 142 機能の宣言 75 値が無いことを確認（macOS は 59 値） |
| 同上で、生成した Info.plist に `<key>NSCameraUsageDescription</key>` を足したテキストを渡す | iOS は ``選ばなかった機能 ar の説明文キー `NSCameraUsageDescription` が成果物に含まれています`` を含むエラー、macOS は同じ文言で機能が `camera-preview-stream`。どちらも終了コード 1 |
| アプリの説明文を消して `cargo fandhe gen` | `[acme-hello] が要求する NSSpeechRecognitionUsageDescription の文言がありません` を含むエラーで終了コード 1 |
| 第三者の id を `network-status` にして `cargo fandhe check` | ``id `network-status` が`` と ``と重複しています`` を含むエラーで終了コード 1 |
| `cargo fandhe build --os macos`・`cargo run` | 起動し、画面に第三者の機能の結果（起動前の登録が済んだ状態）と接続状態（Wi-Fi）が出る |
| `cargo check --target aarch64-apple-ios-sim`・`--target wasm32-unknown-unknown` | 成功 |

Android の `verify-scope` は未確認です。観測に aapt2 の出力（`aapt2 dump xmltree`）を取り、生成した Manifest をそのまま渡すと
MainActivity の構造照合だけで失敗するため、禁止値の検出を確かめられません。Gradle・Xcode のビルドと実機の確認はこの手順に含めていません。
第三者の機能が Java（`FeatureLifecycleProvider` の継承など）や ObjC を持つ場合も、この手順では確かめていません。
