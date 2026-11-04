# 機能の宣言の形式と検証

状態: 形式と検証を実装済み。生成（`cargo fandhe gen`）は #1176、現行設定との突き合わせは #1161。
対応: REQ-21 / TASK-21.2、Issue #1160。上位の方針は `consumer-app-structure.md` の 4 節。

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

検証ツールを Rust にした理由は、生成（#1176）も Rust で実装するため、規則を 2 言語で持つとずれるからです。

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
permissions = [{ name = "android.permission.NFC" }]  # 任意で max-sdk
features = [{ name = "android.hardware.nfc.hce", required = false }]
gradle = ["com.google.android.gms:play-services-mlkit-document-scanner:16.0.0"]
java = ["android/java"]
res = ["android/res"]
main-activity-intent-filters = [
  { actions = ["android.intent.action.SEND"], categories = ["android.intent.category.DEFAULT"], data = [{ mimeType = "text/plain" }] },
]

[[package.metadata.fandhe.android.components]]
kind = "service"              # activity / service / receiver / provider
name = "com.example.NfcCardEmulationService"
exported = true
attributes = { permission = "android.permission.BIND_NFC_SERVICE" }
intent-filters = [{ actions = ["android.nfc.cardemulation.action.HOST_APDU_SERVICE"] }]
meta-data = [{ name = "android.nfc.cardemulation.host_apdu_service", resource = "@xml/apduservice" }]

[package.metadata.fandhe.windows]
identity = true
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
| `extensions` | Apple の拡張ターゲット | `sources` は crate からの相対パス |
| `permissions` | `<uses-permission>` | `max-sdk` は `maxSdkVersion` |
| `features` | `<uses-feature>` | `required` は省略不可 |
| `gradle` | 依存の宣言 | `group:artifact:version` の固定版 |
| `java`・`res` | ソースとリソースのディレクトリ | crate からの相対パス |
| `main-activity-intent-filters` | 既存の MainActivity への追記 | |
| `components` | `<activity>`・`<service>`・`<receiver>`・`<provider>` | `exported` は省略不可。`attributes` は他の `android:*` 属性 |
| `windows.identity` | パッケージ ID | 生成は #1208 |

選んだ理由は次のとおりです。

- `usage-descriptions` と `info-plist` を分けるのは、説明文の既定値を持たない方針（`consumer-app-structure.md` の 4 節）を型で守るためです
- `app-groups` が記号名なのは、正式 ID が iOS と macOS で形が違い、Team ID を宣言に混ぜないためです
- `permissions` を文字列でなくテーブルにしたのは、`maxSdkVersion` 付きの権限が実在し、後から形を変えると破壊的変更になるためです
- `required`・`exported` を省略不可にするのは、Android の暗黙の既定値による端末の除外や意図しない公開を防ぐためです

## 3. 検証規則

問題は 1 件で止めず、欄のパスつきで全件を返します。

- `id` は kebab-case です。`fandhe-cap-*` の crate は名前と `id` が一致し、宣言が必須です。`id` は重複できません
- `usage-descriptions` の各キーは `UsageDescription` で終わります。`info-plist` に同名のキーを置くと、`-iphoneos`・`~ipad` 等の修飾子つき（`NSCameraUsageDescription~ipad`）も含めて拒否します（拡張も同じ）
- `entitlements` に `com.apple.security.application-groups` を直接書くと拒否します
- `app-groups` は記号名（小文字英数字とハイフン）です。`group.`・`$`・`.` を含む正式 ID は拒否します
- 拡張は `NSExtension` が必須で、`deployment-target` は `数字.数字`、`sources` は 1 件以上です
- 権限・feature・コンポーネントの `name` はドット区切りの識別子です。先頭 `.` の相対名は、利用者の applicationId で解決されて壊れるため拒否します
- `gradle` は 3 部で、版は数字始まりの固定版です（`+`・`latest`・範囲・変数・`-SNAPSHOT` は拒否）
- `attributes` のキーは英字のみで、`name`・`exported` は置けません
- `meta-data` は `resource`（`@` 始まり）か `value` のどちらか一方です
- パスは相対のみです（空・絶対・`..`・`\`・`:` は拒否）。実在と、symlink 解決後に crate 内に収まることも確かめます
- 文字列と plist の値に制御文字は置けません

宣言の値は後で XML・plist・Gradle へ展開されます。エスケープは生成側（#1176）の責務で、本書はそのための入力制約を定めます。

## 4. 統合規則

機能を id 順に処理し、結果を決定的にします。
`cargo fandhe check` は解決済みの全 crate の宣言を検証するだけで、統合はしません。統合規則は純粋関数（`merge`）で、検証済みの宣言の一覧を受け取ります。どの機能を統合に入れるか（到達判定）は 5 節のとおり #1176 の担当です。
競合は、競合した葉のパスと、その葉を最初に書いた機能を報告します。

| 欄 | 規則 |
|----|------|
| `usage-descriptions`・`frameworks`・`app-groups`・`gradle`（完全一致） | 和集合 |
| `gradle`（同じ成果物で版違い） | 両方を出力し、Gradle の解決に任せる（高い版が勝つ） |
| `permissions` | `name` で統合。`max-sdk` は大きい方。片方が無指定なら無指定 |
| `features` | `name` で統合。`required` はどちらかが true なら true |
| `info-plist`・`entitlements` | 同じ値は 1 つ。配列は和集合（出現順）。オブジェクトは再帰。それ以外の不一致はエラー |
| `components`・`extensions` | `name` で統合。内容が同じなら 1 つ、違えばエラー |
| `main-activity-intent-filters` | 内容が同じなら 1 つ |
| `java`・`res`・`sources` | 統合しない。crate ごとに集める。同名リソースの衝突は Gradle のマージに任せる（#1176 で確かめる） |
| `windows.identity` | どれかが true なら true |

エラーには欄と、食い違った 2 機能の id を含めます。

## 5. 入れなかったもの

| 項目 | 理由・足す条件 |
|------|--------------|
| `web` の欄 | 本 issue の範囲外。Web の共通層の分離（#1185）で決める |
| `windows.capabilities` | #1208 で必要になれば足す |
| 形式の版番号 | 未知キーの拒否で足りる。要否は #1224 で判断する |
| 到達判定（対象アプリが使う機能だけを生成に入れる） | `cargo fandhe gen`（#1176）で扱う。実装は次を満たす: `cargo tree` の文字列解析に頼らず、`cargo metadata` の `resolve` の package ID で対応付ける / feature の統合が他の workspace member から混ざらない / 対象 OS 限定の依存（`[target.'cfg(...)'.dependencies]`）を含める / 取得元違いの同名同版を区別する / default-members に左右されない |
| Gradle の版の正本 | `versions.env` との二重管理は #1194 までの併存。移行は #1161 以降 |
