# 機能の宣言の形式と検証

状態: 形式・検証・到達判定・生成（`cargo fandhe gen`）を実装済み。先行 5 機能の宣言は現行設定と突き合わせ済み（§6）。
対応: REQ-21 / TASK-21.2・21.3・21.4、Issue #1160・#1176・#1328。上位の方針は `consumer-app-structure.md` の 4 節。

## 1. 概要

各機能 crate の `[package.metadata.fandhe]` が、その機能に必要な OS ごとの設定の正本です。
`cargo metadata` で解決した依存の宣言だけを集めるので、選ばなかった機能の権限は成果物に出ません。

検証と統合は `crates/cargo-fandhe` が持ちます。`cargo metadata` の JSON を入力にするので、TOML の解釈は cargo に任せ、
新しい依存は足していません。

```bash
cargo run -p cargo-fandhe -- fandhe check [--manifest-path <Cargo.toml>]
```

終了コードは 0（正常）、1（宣言の不正）、2（未知の引数）です。`cargo test --workspace` でも
同じ検査が実 workspace に対して走るため、CI に専用のジョブは要りません。

生成も同じ crate の `gen` サブコマンドです（§7）。

```bash
cargo run -p cargo-fandhe -- fandhe gen [--manifest-path <アプリの Cargo.toml>]
```

検証ツールを Rust にした理由は、生成も Rust で実装するため、規則を 2 言語で持つとずれるからです。

## 2. 形式

キーは kebab-case です。Android の `<data>` の属性名だけは Android の原語（`mimeType` 等）です。
未知のキーは拒否します（綴り間違いと未対応の欄を黙って捨てないため）。

```toml
[package.metadata.fandhe]
id = "home-widget"            # 対応表の id。crate 名 fandhe-cap-<id> と一致させる

[package.metadata.fandhe.ios]  # macos も同じ型
usage-descriptions = ["NSCameraUsageDescription"]
frameworks = ["VisionKit"]
entitlements = { "com.apple.security.device.camera" = true }
app-groups = ["widget"]
info-plist = { NSServices = [] }

[[package.metadata.fandhe.ios.extensions]]
name = "FandheTextWidget"
bundle-suffix = "text"
deployment-target = "14.0"
sources = ["apple/widget"]
frameworks = ["WidgetKit", "SwiftUI"]
app-groups = ["widget"]
entitlements = {}
info-plist = { NSExtension = { NSExtensionPointIdentifier = "com.apple.widgetkit-extension" } }

[package.metadata.fandhe.android]
permissions = [{ name = "android.permission.NFC" }]  # 任意で max-sdk・flags（例 flags = ["neverForLocation"]）
main-activity-attributes = { supportsPictureInPicture = "true" }  # 書けるキーは許可リストのみ
application-meta-data = [{ name = "com.google.android.gms.wallet.api.enabled", value = "true" }]  # resource = "@xml/..." も可
features = [{ name = "android.hardware.nfc.hce", required = false }]
gradle = ["com.google.android.gms:play-services-mlkit-document-scanner:16.0.0"]
java = ["android/java"]
res = ["android/res"]
main-activity-intent-filters = [
  { actions = ["android.intent.action.SEND"], categories = ["android.intent.category.DEFAULT"], data = [{ mimeType = "text/plain" }] },
]

[[package.metadata.fandhe.android.components]]
kind = "service"              # activity / activity-alias / service / receiver / provider
name = "com.example.NfcCardEmulationService"
exported = true
attributes = { permission = "android.permission.BIND_NFC_SERVICE" }
intent-filters = [{ actions = ["android.nfc.cardemulation.action.HOST_APDU_SERVICE"] }]
meta-data = [{ name = "android.nfc.cardemulation.host_apdu_service", resource = "@xml/apduservice" }]

[[package.metadata.fandhe.android.components]]
kind = "activity"
name = "com.example.RationaleActivity"
exported = false

# activity-alias は target-activity が必須。対象は同じ宣言の kind = "activity" の name（alias より前に書く）
[[package.metadata.fandhe.android.components]]
kind = "activity-alias"
name = "com.example.ViewPermissionUsageActivity"
exported = true
target-activity = "com.example.RationaleActivity"
attributes = { permission = "android.permission.START_VIEW_PERMISSION_USAGE" }
intent-filters = [{ actions = ["android.intent.action.VIEW_PERMISSION_USAGE"] }]

[package.metadata.fandhe.windows]
identity = true

[package.metadata.fandhe.web.manifest.share_target]  # Web App Manifest のメンバー（許可リストのみ）
action = "./"
method = "GET"
params = { text = "share_text", url = "share_url" }
```

先行 5 機能の例は `crates/cargo-fandhe/tests/fixtures/` にあります。

### 2.1 欄の意味

| 欄 | 生成先 | 備考 |
|----|-------|------|
| `usage-descriptions` | Info.plist の `*UsageDescription` | キーだけ。文言は利用者が書く |
| `info-plist` | Info.plist | 説明文以外の任意キー |
| `frameworks` | リンクする framework | |
| `entitlements` | entitlements | App Group は書かず `app-groups` を使う |
| `app-groups` | `application-groups` entitlement | 記号名。OS ごとの正式 ID へ生成時に展開する |
| `extensions` | Apple の拡張ターゲット | `sources` は crate からの相対パス。`usage-descriptions` も書ける（本体と同じくキーだけで、文言の生成は #1176） |
| `permissions` | `<uses-permission>` | `max-sdk` は `maxSdkVersion`、`flags` は `usesPermissionFlags`（値は Android の原語。いまは `neverForLocation` だけ） |
| `features` | `<uses-feature>` | `required` は省略不可 |
| `gradle` | 依存の宣言 | `group:artifact:version` の固定版 |
| `java`・`res` | ソースとリソースのディレクトリ | crate からの相対パス |
| `main-activity-intent-filters` | 既存の MainActivity への追記 | |
| `main-activity-attributes` | MainActivity の開始タグの属性 | 書けるのは `supportsPictureInPicture` だけ（許可リスト） |
| `application-meta-data` | `<application>` 直下の `<meta-data>` | `resource`（`@` 始まり）か `value` の一方。コンポーネントの後に `name` 順で出す |
| `components` | `<activity>`・`<activity-alias>`・`<service>`・`<receiver>`・`<provider>` | `exported` は省略不可。`attributes` は他の `android:*` 属性。`activity-alias` は `target-activity` が必須で、alias は対象の後に出す |
| `windows.identity` | パッケージ ID | 生成は #1208 |
| `web.manifest` | Web App Manifest のメンバー | 書けるのは `share_target` だけ（許可リスト）。基礎 manifest へ足される（7 節、#1238） |

選んだ理由は次のとおりです。

- `usage-descriptions` と `info-plist` を分けるのは、説明文の既定値を持たない方針（`consumer-app-structure.md` の 4 節）を型で守るためです
- `app-groups` が記号名なのは、正式 ID が iOS と macOS で形が違い、Team ID を宣言に混ぜないためです
- `permissions` を文字列でなくテーブルにしたのは、`maxSdkVersion` 付きの権限が実在し、後から形を変えると破壊的変更になるためです
- `required`・`exported` を省略不可にするのは、Android の暗黙の既定値による端末の除外や意図しない公開を防ぐためです
- `flags` を enum にするのは、綴り間違いを `check` で落とすためです。Android がフラグを足し、使う機能が出たら足します
- `main-activity-attributes` を許可リストにするのは、MainActivity がアプリ本体の入口で、`exported`・`launchMode`・`taskAffinity`・`permission` などを依存 crate が書き換えると、起動不能やタスク乗っ取りにつながるためです。`web.manifest` と同じ形で、要る機能が出たら足します
- alias の対象を同じ宣言内に限るのは、別機能の Activity を指すと、その機能を選ばないアプリで Manifest が壊れるためです。`target-activity` を専用の欄にしたのは、必須・参照先の検査・出力順の制御に使うためです

## 3. 検証規則

問題は 1 件で止めず、欄のパスつきで全件を返します。

- `id` は kebab-case です。`fandhe-cap-*` の crate は名前と `id` が一致し、宣言が必須です。`id` は重複できません
- `usage-descriptions` の各キーは `UsageDescription` で終わります（拡張の `usage-descriptions` も同じ）。`info-plist` に同名のキーを置くと、`-iphoneos`・`~ipad` 等の修飾子つき（`NSCameraUsageDescription~ipad`）も含めて拒否します（拡張も同じ）
- `entitlements` に `com.apple.security.application-groups` を直接書くと拒否します
- `app-groups` は記号名（小文字英数字とハイフン）です。`group.`・`$`・`.` を含む正式 ID は拒否します
- 拡張は `NSExtension` が必須で、`deployment-target` は `数字.数字`、`sources` は 1 件以上です
- 権限・feature・コンポーネントの `name` はドット区切りの識別子です。先頭 `.` の相対名は、利用者の applicationId で解決されて壊れるため拒否します
- `gradle` は 3 部で、版は数字始まりの固定版です（`+`・`latest`・範囲・変数・`-SNAPSHOT` は拒否）
- `attributes` のキーは英字のみで、`name`・`exported`・`targetActivity` は置けません
- `meta-data`（コンポーネントの中も `application-meta-data` も）は `resource`（`@` 始まり）か `value` のどちらか一方です。`application-meta-data` の `name` は宣言内で重複できません
- `activity-alias` は `target-activity` が必須で、同じ宣言の `kind = "activity"` の `name` を指します。alias 以外に `target-activity` は書けません
- `main-activity-attributes` のキーは許可リスト（`supportsPictureInPicture`）だけで、値は属性ごとの値域（`supportsPictureInPicture` は `"true"` / `"false"` の完全一致）を check で検査します。`flags` は `neverForLocation` のみです
- パスは相対のみです（空・絶対・`..`・`\`・`:` は拒否）。実在と、symlink 解決後に crate 内に収まることも確かめます
- 文字列と plist の値に制御文字は置けません
- `web.manifest` のキーは許可リスト（`share_target`）だけで、値はオブジェクトです。`start_url`・`scope`・`name`・`icons` などアプリの同一性に関わる値を、依存 crate が書き換えられないようにするためです。別のメンバーを機能へ移すときは許可リストに足します

宣言の値は後で XML・plist・Gradle へ展開されます。エスケープは生成側（`gen`、§7）の責務で、本書はそのための入力制約を定めます。

## 4. 統合規則

機能を id 順に処理し、結果を決定的にします。
`cargo fandhe check` は解決済みの全 crate の宣言を検証するだけで、統合はしません。統合規則は純粋関数（`merge`）で、検証済みの宣言の一覧を受け取ります。どの機能を統合に入れるか（到達判定）は `gen` が決めます（5 節）。
競合は、競合した葉のパスと、その葉を最初に書いた機能を報告します。

| 欄 | 規則 |
|----|------|
| `usage-descriptions`・`frameworks`・`app-groups`・`gradle`（完全一致） | 和集合 |
| `gradle`（同じ成果物で版違い） | 両方を出力し、Gradle の解決に任せる（高い版が勝つ） |
| `permissions` | `name` で統合。`max-sdk` は大きい方。片方が無指定なら無指定。`flags` が違えばエラー（`neverForLocation` を外すと位置を求めない機能の結果が届かなくなり、付けると位置を導く機能の結果が間引かれる。どちらへ寄せても片方が黙って壊れる） |
| `features` | `name` で統合。`required` はどちらかが true なら true |
| `info-plist`・`entitlements` | 同じ値は 1 つ。配列は和集合（出現順）。オブジェクトは再帰。それ以外の不一致はエラー |
| `components`・`extensions`・`application-meta-data` | `name` で統合。内容が同じなら 1 つ、違えばエラー |
| `main-activity-attributes` | キーごとに統合。同値なら 1 つ、違えばエラー |
| `main-activity-intent-filters` | 内容が同じなら 1 つ |
| `java`・`res`・`sources` | 統合しない。crate ごとに集める。同名リソースの衝突は Gradle のマージに任せる |
| `windows.identity` | どれかが true なら true |
| `web.manifest` | `info-plist` と同じ（同じ値は 1 つ、配列は和集合、オブジェクトは再帰、不一致はエラー）。`share_target` は manifest に 1 つしか置けないため、2 機能が違う値を書くとエラー |

エラーには欄と、食い違った 2 機能の id を含めます。

## 5. 入れなかったもの

| 項目 | 理由・足す条件 |
|------|--------------|
| `windows.capabilities` | #1208 で必要になれば足す |
| 形式の版番号 | 未知キーの拒否で足りる。要否は #1224 で判断する |
| 到達判定（対象アプリが使う機能だけを生成に入れる） | 実装済み（`crates/cargo-fandhe/src/reach.rs`、#1176）。次を満たす: `cargo tree` の文字列解析に頼らず、`cargo metadata` の `resolve` の package ID で対応付ける / feature の統合が他の workspace member から混ざらない（`resolve.nodes[].features` は使わず、アプリの `default` から feature を自前で活性化する。辺は通常依存のみ） / 対象 OS 限定の依存（`[target.'cfg(...)'.dependencies]`）を含める / 取得元違いの同名同版を区別する / default-members に左右されない |
| MainActivity を指す alias（ランチャーアイコンの切り替え等） | 使う機能が無い。必要になった issue で足す |
| `<application>` の属性（`localeConfig` 等） | アプリ全体の値で、機能の宣言に書かない |
| `neverForLocation` 以外の権限フラグ | 使う機能が出たとき enum に足す |
| 統合エラー（flags・meta-data・属性の食い違い）を利用者が上書きする手段 | 追記・`--eject`（#1202）の範囲 |
| Gradle の版の正本 | #1194 までは `scripts/android/versions.env` が手書きビルドの正本で、宣言の版は突き合わせテストで一致を保証する |

## 6. 現行設定との突き合わせ

先行 5 機能の宣言を（機能 crate へ移した機能は `crates/fandhe-cap-<id>/Cargo.toml`、それ以外は `crates/cargo-fandhe/tests/fixtures/*.toml`。camera-capture は #1170、document-scan は #1171、nfc-card-emulation は #1172、share-receive は #1173、home-widget は #1174 で crate へ移した）、現行の設定と突き合わせました（#1161、REQ-21 / TASK-21.2）。`crates/cargo-fandhe/tests/gen.rs`（生成物との比較）と `tests/declarations.rs`（生成しない項目）が、次を毎回の `cargo test` で確かめます。

| 宣言 | 比較先 |
|------|-------|
| Android の権限・feature・コンポーネント・MainActivity の intent-filter（生成した overlay と比較） | `platforms/android/app/src/main/AndroidManifest.xml`（属性・filter・meta-data の過不足を含む）。camera の feature 群、HCE の feature 群、`ACTION_SEND` の filter は逆方向も確かめる |
| Gradle の依存の座標（生成した `fandhe.gradle`） | `build.gradle.kts` の `androidVersion("KEY")` を辿り、`scripts/android/versions.env` の値と一致 |
| iOS の `usage-descriptions`・`frameworks` | `FandheIosApp/Resources/Info.plist` のキー、機能 crate の `build.rs`（無ければ `fandhe-egui-proxy/build.rs`）のリンク指定 |
| Android の `java`・`res`（機能 crate のディレクトリ。生成した srcDirs と比較） | `build.gradle.kts` の srcDirs の `crates/fandhe-cap-<id>/<dir>` と両方向で一致 |
| macOS の `NSServices`（生成した Info.plist） | `scripts/apple/bundle-macos-url-scheme.sh` の JSON |
| macOS の Widget 拡張（生成した Info.plist・entitlements） | 拡張の `Info.plist` と entitlements。`project.yml` と Swift の `import` は `declarations.rs` が確かめる |
| iOS の `NSCameraUsageDescription`（生成した Info.plist） | `FandheIosApp/Resources/Info.plist` の文言 |

規則: 宣言は、その機能がいま実際に必要とするものだけを書きます。未実装の OS の欄（iOS の共有拡張 #1211・Widget 拡張 #1212、Windows のパッケージ ID #1208）は、実装する issue で足します。残すと生成で実体のない拡張と権限が出て、最小権限が崩れるためです。形式の確認は JSON を使うテストで保ちます。

### 差分として残すもの

| 項目 | 宣言しない・値が違う理由 | 扱う issue |
|------|----------------------|-----------|
| `android.hardware.nfc` | `nfc`（タグ読み取り）の宣言で、HCE の宣言ではない | 対象外 |
| `androidx.activity`・`androidx.core` | MainActivity が直接 import する共通依存。`fandhe-platform-android` の宣言（`gradle`）で決着。`fandhe-cap-*` 以外の共通層 crate も宣言を持てる（id は対応表の外） | 決着（#1259） |
| `GmsAvailability.java` | document-scan 以外の GMS 機能（行動認識・決済）も使う共有コード。`platforms/android/app` に共有のまま残す。共通層（`fandhe-platform-android`）へ移さないのは、GMS を使わない機能だけを選んだ利用者に Play services の依存を乗せないため。document-scan は同じ判定を `DocumentScanHelper` の private メソッドに持つ（#1257）。残る 2 機能を crate へ移すときに同じ形にする | #1183 |
| `RequestTracker.java`・`FeatureLifecycleProvider.java` | 全機能が使う Java の共有クラス。共通層 `fandhe-platform-android` が `android/java` として持ち、その crate の宣言（`id = "platform-android"`。`fandhe-cap-*` 以外は id と crate 名の一致を求めない）が srcDirs に載る。機能 crate は Android の依存に共通層を持つので到達判定に入る（#1257） | 決着（#1257） |
| UIKit・Foundation・CoreGraphics | 全機能の基盤リンクで機能固有ではない | #1168 |
| `NSPortName` | 宣言は `$(EXECUTABLE_NAME)`、スクリプトは実行体名へ置換。生成はビルド設定変数のまま出す（Xcode が展開する）。iOS 本体の `CFBundle*` は確定値（下の行）だが、macOS は宣言が変数に依存するため変数のまま | 決着（#1176） |
| `FandheWidgetHost` | 素の実行体に拡張を載せるホストアプリ。生成では利用者の `.app` が親になる | #1212 |
| `FandheCountControl.swift`・`AppIntents` | quick-settings-tile の分。ソースは home-widget から分離済み（#1174）。束ねる入口 `FandheWidgets.swift` は platforms 側に残る（Xcode ターゲットの生成は #1212） | #1212 |
| `FandheAppGroupIdentifier` | 現行どおり `$(TeamIdentifierPrefix)com.fandhe.widget` を literal で宣言。記号名との一本化は未決。生成も literal のまま出す | 未決 |
| `NSMenuItem.default` | 現行どおり literal。利用者が差し替える手段は未決。生成も literal のまま出す | 未決 |
| `CFBundle*` 等の定型キー | 生成側のテンプレートの責務。iOS 本体は確定値（実行体名・bundle id・版・`MinimumOSVersion`・`UIDeviceFamily`・`UILaunchScreen`）、macOS と拡張はビルド設定変数 7 キーを出す | 決着（#1176・#1259） |
| Windows の `webcam` | 形式に欄がない。カメラの Windows 実装はダミー | #1208 |
| 対応表 JSON の macOS カメラの `perm` 文 | 計画情報で設定の実体ではない | #1195 |

突き合わせテストの寿命: 生成物との比較（`tests/gen.rs`）は、設定を生成へ切り替える #1194 で比べる相手ごと撤去します。`declarations.rs` に残す項目（framework のリンク・Gradle の版と `versions.env`・Xcode の拡張ターゲット・Swift の `import`）は、#1194 と #1212 で撤去します。

## 7. 生成（`cargo fandhe gen`）

アプリが依存として解決した機能 crate の宣言だけから、各 OS のネイティブ設定を `<アプリの crate>/gen/` に作ります（gitignore する）。
`gen/` の中身は毎回作り直します。

### 入力

- 到達判定: OS ごとに `cargo metadata --filter-platform <triple>` を実行し（android は `aarch64-linux-android`、ios は `aarch64-apple-ios`、macos は `aarch64-apple-darwin`）、アプリの `default` から feature を活性化して辿ります。開始する feature は `default` に固定します
- アプリ側の `[package.metadata.fandhe-app]`（`fandhe` の下に置くと機能宣言として解釈されるため別キー）

```toml
[package.metadata.fandhe-app]
app-group-prefix = "com.example"            # app-groups を使う機能があるときだけ必須
[package.metadata.fandhe-app.ios.usage-descriptions]
NSCameraUsageDescription = "撮影に使います"  # 有効な機能が要求するキーは必須。既定値は持たない
[package.metadata.fandhe-app.macos.usage-descriptions]
```

```toml
[package.metadata.fandhe-app.ios]
bundle-id = "com.example.app"               # iOS の Info.plist を出すので必須（macOS はまだ使えずエラー）
```

殻に要る値は、アプリの crate からも取ります。Android は `cdylib` を含む lib target が要り（無ければエラー）、iOS は bin target がちょうど 1 つと、数字とドットだけの version が要ります。

これらは全 OS の殻を出す前提の必須条件です。`cargo fandhe gen` は Android と iOS の両方を常に出すため、片方の OS だけを対象にする絞り込みは範囲外です（`cdylib`・bin・`ios.bundle-id` のどれが欠けても gen 全体がエラーになります）。

Web は `cargo fandhe gen --os web` で Web App Manifest だけを出します（triple は `wasm32-unknown-unknown`）。bin や iOS 設定を持たない Web 専用のアプリでも、Android・iOS の殻の必須条件に阻まれないよう、Web に限って絞り込みを認めます。

```toml
[package.metadata.fandhe-app.web]
manifest = "web/manifest.webmanifest"       # アプリの crate からの相対パス。名前・アイコン・start_url 等の基礎
```

基礎 manifest に機能のメンバー（`share_target`）を書くとエラーです。`fandhe-app.web` が無いアプリは `--os web` がエラーで、全 OS の `gen` は `web/` を出しません（manifest を配らないアプリは共有先にならないため、嘘の成功にはなりません）。

説明文が欠けると、OS・機能 id・キーを示してエラーにします。`usage-descriptions` は本体と拡張で共有します。

### 出力

| パス（`gen/` 配下） | 内容 |
|------|------|
| `android/AndroidManifest.xml` | manifest merger 用の overlay（権限と `usesPermissionFlags`・feature・コンポーネント・`<application>` 直下の meta-data、MainActivity の宣言＝`exported`・`launchMode=singleTask`・`configChanges`・機能の許可済み属性・`android.app.lib_name`・MAIN/LAUNCHER と機能の intent-filter） |
| `android/fandhe.gradle` | `apply from:` 用。常に出す。gen の `android/java` と機能 crate の `java`・`res` を srcDirs に足し、gen の Manifest を全 variant の overlay として登録し（`androidComponents`）、`gradle` の依存（基盤を含む）を `implementation` で足す |
| `android/java/com/fandhe/platform/FandheLibrary.java` | `NAME` に cdylib 名（`[lib] name`）を持つ。殻は `System.loadLibrary(FandheLibrary.NAME)` と書き、ライブラリ名の正本を `[lib] name` 1 か所にする |
| `ios/Info.plist` | 確定値の定型キー（上記）、説明文、`info-plist`。機能が無くても出す |
| `macos/Info.plist` | ビルド設定変数の定型 7 キー、説明文、`info-plist` |
| `ios/App.entitlements`・`macos/App.entitlements` | `entitlements` と、展開した App Group（ios は `group.<prefix>.<名>`、macos は `$(TeamIdentifierPrefix)<prefix>.<名>`） |
| `<os>/extensions/<name>/Info.plist`・`<name>.entitlements` | 拡張ごとの plist と entitlements |
| `web/manifest.webmanifest` | `--os web` のときだけ。基礎 manifest に、wasm32 で到達した機能の `web.manifest` を足した JSON。選ばなかった機能のメンバーは入らない |
| `.fandhe-gen` | 生成物の印 |

値は出力先の文法（XML・plist・Groovy の単一引用符）でエスケープします。plist の null はエラーです。

### 書き出しの安全策

- `gen/` が symlink なら拒否します
- `gen/` があって `.fandhe-gen` が無ければ、利用者のディレクトリとみなして拒否します
- 印があれば消して作り直します（選ばなくなった機能の拡張が残らない）

### 範囲外

`CFBundleSupportedPlatforms`（シミュレータか実機かは組み立て時に決まる、#1201）、applicationId・compileSdk・`<application>` の属性、macOS・拡張の Info.plist の確定値化、framework のリンクと Xcode のターゲット（#1212）、Windows の identity（#1208）、手書き設定の置き換え（#1194）、`run`/`build` への組み込み（#1201）、追記と `--eject`（#1202）、`verify-scope` の Web 対応。

## 8. 選ばなかった機能の検査（`cargo fandhe verify-scope`）

成果物に、選ばなかった機能の権限・entitlement・framework・Gradle 依存・コンポーネントが混ざっていないことを確かめます（#1178）。

```sh
cargo fandhe verify-scope --catalog <全機能を含む Cargo.toml> \
  [--manifest-path <アプリの Cargo.toml>] --os android|ios|macos <観測ファイル>...
```

- 禁止集合は「カタログの宣言 − アプリが到達した宣言」から作ります。到達判定は `gen` と同じです
- 到達した機能と同じ値を宣言する未選択機能の値は、正当に現れるため禁止にしません
- 観測ファイルは成果物から外部ツールで取り出したテキストです。いずれかの禁止値が現れたら、機能 id・種別・値を出して終了コード 1 で終わります
- 照合はトークン境界つきです。`android.hardware.camera` は `android.hardware.camera.autofocus` に、`group:artifact` は `group:artifact-ktx` に一致しません
- 観測テキストが空なら失敗します（ツールの失敗の見逃し防止）。未選択の機能が 0 件なら検査対象なしで成功します
- 対象は次の値です
  - Android: 権限名、`uses-feature` 名、コンポーネント名（`activity-alias` を含む）、`application-meta-data` の名前、Gradle の `group:artifact`。`application-meta-data` の名前が、選んだ機能の依存 library の Manifest にも現れると誤検知しうる（実例が出たら `GENERIC_KEYS` と同じ除外で扱う）
  - iOS・macOS: entitlement キー（App Group は `com.apple.security.application-groups`）、framework 名、`*UsageDescription` キー、`info-plist` のキー、拡張（`<name>.appex`）
- 観測ファイルの集め方は `scripts/audit/verify-capability-scope.sh` が担います（aapt2・codesign・otool・plutil。Gradle の依存ツリーは任意）
- 判定ロジックは `cargo test -p cargo-fandhe` で守ります。実成果物への実行は CI の必須経路には載せていません

範囲外: 権限の `flags`（権限名が対象で、フラグ名は他の権限にも正当に現れる）、`main-activity-attributes`、`main-activity-intent-filters`（他 library の manifest にも正当に現れるため。漏れは機能のコンポーネント名で検出します）、Windows の identity（#1208）、AAB の直接検査。
