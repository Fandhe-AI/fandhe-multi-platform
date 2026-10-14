# SwiftUI ブリッジ層アーキテクチャ・シリアライズ方式設計

## 1. 目的・位置づけ

本書は REQ-3（`docs/spec/04-requirements.md` L53〜66）の受け入れ基準「Rust コア ⇔
SwiftUI の型安全なブリッジ層（引数・戻り値のシリアライズ方式、エラー伝播）を設計し
文書化する」（L60）を満たすために作成しています。`crates/fandhe-swiftui-bridge` と
`platforms/apple/FandheSwiftUIBridge` の設計 SSOT であり、両者のコード側 doc コメントが
節番号（§1〜§7、§3.1〜§3.4）で本書を参照します。本書を再構成する場合も節番号の
割り当ては変更しないでください。

- **前提タスクの状態**: `fandhe-core` は TASK-3.1（Issue #56）により
  `aarch64-apple-watchos` / `aarch64-apple-watchos-sim` / `aarch64-apple-visionos` /
  `aarch64-apple-visionos-sim` の 4 ターゲットへのクロスコンパイルに成功しており、本書はその先に
  FFI ブリッジ層を新設する前提で設計します。
- **PoC との関係**: `docs/spec/03-poc/watchos-visionos/minimal-lib/src/lib.rs`（PoC-2）は
  `#[no_mangle] extern "C" fn`（`mpf_add` / `mpf_greeting`）＋ `crate-type = ["staticlib", "lib"]`
  ＋ Swift 側 `@_silgen_name` により、プリミティブ引数 1 関数の FFI 呼び出しに成功済みです。
  構造化データのシリアライズ・エラー伝播・所有権規約は本書が設計します。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）のドキュメントとして管理しています（`crate-feature-matrix.md` /
  `responsibility-boundary.md` / `state-access-api-tradeoff.md` / `unsafe-audit.md` と同じ
  置き場・体裁）。spec 自体の変更はこのリポジトリでは行いません。

## 2. 設計判断（結論）

- Rust コア（`fandhe-core`）と SwiftUI 描画層は、新設クレート `fandhe-swiftui-bridge`
  （プラットフォーム最適化レイヤ側、依存方向は `fandhe-swiftui-bridge` → `fandhe-core` の
  一方向）を介して `extern "C"` で接続する。
- ジェネリックな `StateStore<S>` / `Screen<S>` は C ABI を越えられないため、**不透明ハンドル
  （opaque pointer）パターン**で単相化した型のみを公開する。
- 引数・戻り値のシリアライズは、スカラー値（バージョン番号・真偽値・エラーコード）は
  プリミティブ C 型を直渡しし、構造化データ（描画スナップショット・イベント）は **JSON
  （UTF-8 バイト列）** とするハイブリッド方式を採用する。
- エラー伝播は「`i32` エラーコード enum ＋ 詳細メッセージ取得 API」の 2 層方式とし、
  `catch_unwind` により Rust の panic が FFI 境界を越えないようにする。

## 3. 詳細設計

### 3.1 ブリッジ層アーキテクチャ

#### クレート構成

- クレート `crates/fandhe-swiftui-bridge` は `fandhe-` 接頭辞・プラットフォーム最適化
  レイヤ側というクレート命名規約（coding-rust.md）に従う。
- `crate-type = ["staticlib", "lib"]`（`crates/fandhe-swiftui-bridge/Cargo.toml`、
  PoC-2 実証済み構成を踏襲）。`staticlib` は Swift Package Manager / `.xcodeproj` からの
  リンク用、`lib` はホスト（Linux CI）上での `cargo test` 用。
- 依存方向は `fandhe-swiftui-bridge` → `fandhe-core` の一方向（`Cargo.toml` の
  `[dependencies]` に `fandhe-core = { path = "../fandhe-core" }` のみ）。共通コアから
  プラットフォーム固有クレートへは依存しない（coding-rust.md 準拠、既存の
  `crate-feature-matrix.md` の層構造と整合）。
- 依存クレート（`Cargo.toml` の `[dependencies]` および workspace ルートの
  `[workspace.dependencies]`）: `serde`（`derive` feature 付き、1.0 系）・`serde_json`
  （1.0 系）・`thiserror`（2.0 系）。いずれも `rust-version` が 1.88 以下であり、3.4 の
  MSRV フィルタ（1.88 以下）を満たす。

#### 内部モジュール分割

| モジュール | 責務 | `unsafe` |
| --- | --- | --- |
| `codec` | 描画スナップショット・イベントの JSON 相互変換 | なし |
| `error` | `FandheErrorCode`・per-handle / スレッドローカルのエラー詳細・`catch_panic` | なし |
| `ffi` | `extern "C"` エクスポート関数とハンドルレジストリ | 本モジュールのみ許可 |
| `ffi::buffer` | `FandheBuffer`（Rust 確保バイト列の ABI 表現） | 本モジュールのみ許可 |
| `platforms/apple/FandheSwiftUIBridge` | C ヘッダ経由の型安全な Swift ラッパ（`FandheStore` / `FandheABI` / `FandheModels` / `FandheBridgeError` / `FandheBufferBridge` / `FandheBridgeContract`） | — |

#### unsafe 境界の規約

- `fandhe-core` の `#![forbid(unsafe_code)]` は変更しない。ブリッジクレートの
  クレートルートは `#![deny(unsafe_code)]` を付ける。
- `forbid` ではなく `deny` を採用する理由: `forbid` はクレート内のどこからも
  `#[allow(unsafe_code)]` で局所的に上書きできない（コンパイルエラーになる）ため、
  「`ffi` モジュールのみ `unsafe` を許可し他モジュールは禁止する」という本設計の
  境界表現ができない。`deny` は `ffi` モジュール（またはその直下の関数）に限定して
  `#[allow(unsafe_code)]` を付けることで、意図的な例外箇所をコード上に明示しつつ
  それ以外の箇所への `unsafe` 混入を通常の `deny` として検知できる。
- `ffi` モジュール内の全 `unsafe` ブロックに `// SAFETY:` コメントを必須とする
  （coding-rust.md 準拠）。`ffi` モジュール以外（`codec` / `error`）は `unsafe` を
  一切含まない安全コードとする。
- Rust の panic が FFI 境界を越えて Swift 側の未定義動作を引き起こさないよう、
  各エクスポート関数の本体を `std::panic::catch_unwind` で包み、panic 時は
  エラーコードへ変換して返す（3.3「エラー伝播」参照）。
- `unsafe` の登録・検出件数は `docs/architecture/unsafe-audit.md` を台帳とする
  （本書には数値を書かない）。新規追加は security-auditor の必須レビュー対象
  （security.md 準拠）。

#### ジェネリクスの取り扱い（不透明ハンドルパターン）

- `StateStore<S>` / `Screen<S>` の型パラメータ `S` は C ABI を越えられない。初期実装は
  `fandhe_core::sample_screen` / `SampleState`（`crates/fandhe-core/src/ui/sample.rs`）で
  単相化した `FandheStore`（`StateStore<SampleState>` を包む不透明構造体）のみを公開する。
- 任意のアプリ状態 `S` への一般化（マクロ・コード生成による単相化コードの自動生成等）は、
  本書のスコープ外の将来課題とする（「4. 再考条件」参照）。

#### 所有権・寿命規約

| 操作 | 規約 |
| --- | --- |
| ハンドル生成・解放 | `fandhe_*_new` / `fandhe_*_free` を対で提供する。生成された各ハンドルは対応する `free` 関数でのみ解放できる |
| Rust 確保バッファ | Rust 側で確保したバッファ（JSON バイト列等）は必ず Rust 側の解放関数（`fandhe_buffer_free`）で解放する。Swift 側で `free()` 等を直接呼ばない |
| Swift → Rust のポインタ | Swift から渡されるポインタ・長さは呼び出し中のみ有効とみなす（呼び出し完了後に Rust 側が保持してはならない） |
| 二重解放・解放後使用対策 | ハンドル自体（`FandheStore` 等の指す先）に有効性フラグは持たせない（解放済みオブジェクトのメモリを読むこと自体が use-after-free になるため）。代わりに Rust 側がプロセス内で「現在生存しているハンドルのアドレス集合」を別メモリ領域のレジストリとして保持する。ハンドルを受け取る各エクスポート関数は、ハンドルが指す実体を参照する前にこのレジストリへの**メンバーシップ判定のみ**（アドレス値の比較。ハンドル先のメモリは読まない）で有効性を検査し、無効ならエラーコードを返す。`free` は「レジストリからの除去」を実体の解放より**先に**行う（除去と解放の間に他呼び出しが割り込まないよう単一ミューテックスで直列化する）。既知の残存リスクとして、解放済みアドレスが OS/アロケータにより別の新規ハンドルへ再利用された場合、古いハンドル値がレジストリ上「有効」に見えてしまう ABA 問題があるが、除去を解放に先行させることで解放直後の再呼び出し（本設計が主対象とする二重解放・use-after-free）は確実に検出できる（A03/メモリ安全性） |

#### 命名・ABI 規約

- エクスポート関数は `fandhe_` 接頭辞の snake_case（PoC-2 の `mpf_` 接頭辞から本実装向けに
  クレート名に合わせて改めた）。
- `fandhe_abi_version() -> u32` を設け、Swift 側リンク時にビルド生成物の ABI 世代が
  一致するかを実行時に検査できるようにする（A08 整合性の不備対策、3.3 参照）。

#### SPM / Xcode 統合

- SPM 側の統合方式は `linkerSettings: .unsafeFlags(...)` ではなく **XCFramework +
  binaryTarget** 方式を採用する。`unsafeFlags` は依存パッケージとして参照される側
  （`FandheSwiftUIBridge` は `FandheWatchApp` / `FandheVisionApp` から依存パッケージとして
  参照される）では SPM が使用を許可しない制約があるため、公式のバイナリ配布手段である
  binaryTarget を選んだ。
- `scripts/apple/build-bridge-xcframework.sh` が 4 ターゲット
  （watchos-arm64 / watchos-arm64-simulator / xros-arm64 / xros-arm64-simulator）の
  staticlib をクロスビルドし `Artifacts/FandheBridge.xcframework` を組み立てる。
  `platforms/apple/FandheSwiftUIBridge/Package.swift` が `FileManager.fileExists` に
  よる存在確認のうえ条件付きに binaryTarget として取り込む（生成物は `.gitignore` に
  より非コミット）。
- `platforms/apple/project.yml`（XcodeGen SSOT）は `packages:` として本パッケージを
  参照し、`aggregateTargets:` の `preBuildScripts`（`scripts/apple/xcode-prebuild-bridge.sh`
  経由の cargo 再ビルド）で staticlib の鮮度を維持する。
- リンク経路はアプリターゲット → `FandheSwiftUIBridge` → `CFandheBridge` →
  `FandheBridgeBinary`（XCFramework）である。`LIBRARY_SEARCH_PATHS` / `OTHER_LDFLAGS`
  等の直接リンカ設定は binaryTarget 経由と重複するため採用しない。
  XCFramework は非コミットのため、初回のみ `build-bridge-xcframework.sh` の手動実行が
  必要（2 回目以降は `preBuildScripts` が担う）。

### 3.2 シリアライズ方式の比較と選定

#### 案 A: JSON（UTF-8 バイト列で受け渡し）— 採用

- **利点**: Swift 側は標準ライブラリの `Codable`（`Decodable` / `Encodable`）で追加依存
  ゼロのままデコードできる。人間が読めるためデバッグが容易。PoC-2 相当の最小画面規模では
  性能要件（REQ-3 に定量的な性能受け入れ基準はない）を満たすと判断する。
- **欠点**: バイナリ形式に比べてペイロードサイズ・パース速度で不利。
- **Rust 側の実装方針**: `serde`（`derive` feature）+ `serde_json` + `thiserror` を
  `fandhe-swiftui-bridge` の依存として追加する（`fandhe-core` には追加しない。3.1 参照）。
  MSRV フィルタ（1.88 以下）を満たすバージョンを選定済み（3.1・3.4 参照）。`cargo audit`
  によるセキュリティ確認は現時点で CI 未導入のため実施していない（A06、5 節参照）。

#### スキーマ・型（`crates/fandhe-swiftui-bridge/src/codec.rs`）

- `SNAPSHOT_SCHEMA_VERSION: u32 = 1`: スナップショット JSON のスキーマ世代番号
  （A08 対策、`fandhe_abi_version()` と併せて世代不整合を検出する布石）。
- `MAX_EVENT_PAYLOAD_LEN: usize = 64 * 1024`（64 KiB）: Swift から受け取るイベント JSON
  ペイロードの長さ上限。超過分は JSON パースを試みる前に拒否する（A03 対策）。
- `SnapshotNode`（`#[serde(tag = "kind", rename_all = "snake_case")]` の enum）:
  `Text { text }` / `TextField { label, value }` / `Button { label }` /
  `List { children: Vec<SnapshotNode> }` の 4 バリアント。`fandhe_core::Node<S>` を
  関数ポインタを含まない形へ詰め替えたもの。
- `RenderSnapshot`（struct）: `schema_version: u32` / `title: String` /
  `state_version: u64` / `nodes: Vec<SnapshotNode>`。`Screen<S>` の走査結果全体。
- `EventPayload`（非公開 enum、`#[serde(tag = "type", rename_all = "snake_case",
  deny_unknown_fields)]`）: `ButtonClicked { label }` / `TextFieldChanged { label, value }`。
  `From<EventPayload> for fandhe_core::Event` により変換する。
- `CodecError`（`thiserror::Error` 導出）: `PayloadTooLarge { len, max }` /
  `Json(#[from] serde_json::Error)` の 2 バリアント。`unwrap()` / `expect()` を使わず
  `Result` を返す（coding-rust.md 準拠）。
- 公開関数 `encode_snapshot<S>(screen: &Screen<S>, state: &S, state_version: u64) ->
  Result<Vec<u8>, CodecError>` / `decode_event(bytes: &[u8]) -> Result<Event, CodecError>`。
  FFI エラーコード（`i32`）への変換は `error` モジュールが担い、`codec` は `Result` を
  返すところまでを責務とする。

#### 案 B: バイナリ（postcard / bincode 等）— 不採用

- コンパクトだが、Swift 側にデコーダを自作・保守する必要がありコストが高い。
  標準ライブラリでのサポートがなく、フォーマット変更のたびに Swift 側の同期が必要になる。

#### 案 C: フラットな C 構造体のみ — 不採用

- シリアライズ不要という利点はあるが、再帰的な描画ツリー（`Node::List` の子要素展開）を
  可変長・可変深さで表現できずスケールしない。

#### 案 D: バインディング生成ツール（uniffi-rs / swift-bridge / cbindgen）— 本書では不採用（cbindgen は補助採用可）

- uniffi-rs・swift-bridge は依存が重く、watchOS 対応の確度が本書作成時点では未検証。
  本方式選定（JSON か バイナリか）とは直交する関心事のため、採用可否は本書の対象外とする。
- cbindgen は `extern "C"` 関数からヘッダファイルを自動生成する補助ツールであり、
  シリアライズ方式の選定とは独立した判断である。
- 上記ライブラリの正確なバージョン・RUSTSEC 勧告有無の一次情報確認は本書のスコープ外
  （「4. 再考条件」参照）。

#### 選定結論とハイブリッド方式

ホットパスのスカラー値（`fandhe_abi_version` の戻り値、`dispatch_event` の再描画要否・
エラーコード等）はプリミティブ C 型（`u32` / `i32` / `bool` 相当）を直渡しし、構造化データ
（描画スナップショット・イベントペイロード）は JSON（案 A）とするハイブリッド方式を採用する。

**再考条件**: 実機・シミュレータでの性能計測（TASK-3.5、実機検証）で JSON のパース・
シリアライズがフレームタイム・レスポンス性に問題を生じさせることが確認された場合、
バイナリ形式（案 B）への部分的または全面的な移行を検討する。

**スナップショット JSON の実例**（`codec.rs` の `#[cfg(test)]` が検証している構造。
`sample_screen()` を state_version 7 で走査した場合の一部抜粋）:

```json
{
  "schema_version": 1,
  "title": "サンプル画面",
  "state_version": 7,
  "nodes": [
    { "kind": "text", "text": "代表 1 画面サンプル" },
    { "kind": "text_field", "label": "メモ", "value": "こんにちは" },
    { "kind": "button", "label": "カウントを増やす" },
    { "kind": "list", "children": [
      { "kind": "text", "text": "item-1" },
      { "kind": "text", "text": "item-2" }
    ] }
  ]
}
```

**イベント JSON の実例**（`EventPayload`、`codec.rs` の `#[cfg(test)]` が検証）:

```json
{"type":"button_clicked","label":"カウントを増やす"}
{"type":"text_field_changed","label":"メモ","value":"更新後"}
```

### 3.3 インターフェース契約

凍結された C ABI 契約であり、`platforms/apple/FandheSwiftUIBridge/Sources/CFandheBridge/
include/fandhe_bridge.h` と 1 対 1 対応する。設計レベルの草案ではなく、`ffi` モジュール
（`crates/fandhe-swiftui-bridge/src/ffi/mod.rs`）が実装済みの現行シグネチャである。

```text
fandhe_abi_version() -> u32
fandhe_store_new() -> *mut FandheStore
fandhe_store_free(store: *mut FandheStore) -> i32
fandhe_render_snapshot(store: *const FandheStore) -> FandheBuffer
fandhe_dispatch_event(store: *mut FandheStore, event_json: *const u8, len: usize) -> i32
fandhe_buffer_free(buffer: FandheBuffer)
fandhe_last_error_message(store: *const FandheStore) -> FandheBuffer
```

2 つの不透明型を伴う: `FandheStore`（不透明ハンドル）・`FandheBuffer`（Rust 確保
バイト列を Swift 側へ値渡しする `#[repr(C)]` の集約型。バイト列が存在しない場合は
`ptr` が `NULL`・`len` が `0` の空バッファを返す契約）。

現在値・現在契約:

- `fandhe_abi_version()` は `1` を返す（`SNAPSHOT_SCHEMA_VERSION` と同じ世代から開始する）。
  Swift 側（`FandheBridgeContract.expectedAbiVersion`）はリンク時にこの値と期待値の
  一致を検査し、不整合バイナリの誤リンクによる未定義動作を実行時に検出する（A08）。
- `fandhe_dispatch_event` の戻り値は、負値がエラーコード、`0` / `1` が再描画要否
  （`1`: 状態が更新され再描画が必要、`0`: 一致するハンドラがなく状態は変化していない）
  を表す。正値は `ffi` 層のセマンティクス用に予約する。
- `FandheErrorCode` の実値: `Success 0` / `NullPointer -1` / `InvalidHandle -2` /
  `PayloadTooLarge -3` / `JsonError -4` / `PanicCaught -5` / `Internal -6`。
- `MAX_ERROR_MESSAGE_LEN = 4 * 1024`（4 KiB）。エラーメッセージは UTF-8 文字境界で
  切り詰めて格納する（panic ペイロードや内部エラーの `Display` 出力が任意長・任意内容の
  文字列を含み得るため、格納前に切り詰めて肥大・情報過多を防ぐ。A05 対策）。
- `fandhe_last_error_message` は純粋な読み取り専用アクセサであり、`store` が `NULL`
  の場合・無効ハンドルを渡された場合を含め、スレッドローカル最終エラーにも per-handle
  ストレージにも一切書き込まない（自身が「無効ハンドル」等のエラーを新規に書き込むと、
  直前の呼び出しが残した詳細メッセージを取得前に上書き・破壊してしまうため）。
- エラー詳細の書き分け: 有効ハンドル使用時は per-handle ストレージ（`ErrorSlot`）へ、
  ハンドル非依存の失敗（`store` が `NULL`）・無効ハンドル使用時（メンバーシップ検査
  失敗。既に解放済み・元から不正なポインタ）はスレッドローカルへ書き込む。無効ハンドルの
  実体（解放済みかもしれないメモリ）へ書き込まないことで use-after-free を回避する。
  **panic 捕捉時（`catch_panic`）は有効ハンドル使用中でも常にスレッドローカルへ書く**
  （panic 後に per-handle 実体の状態整合を仮定しないための契約）。

以下、各関数の個別契約:

- `fandhe_store_free`: 戻り値 `i32` はハンドルの有効性検査結果（`0` は成功、負値は
  「既に解放済み／無効なハンドル」等のエラーコード）を表す。有効性は「所有権・寿命規約」
  で定義した外部レジストリ（生存中ハンドルのアドレス集合）へのメンバーシップ判定として、
  対象ハンドルが指す実体を解放する**前**に検査する。
  検査・レジストリからの除去・実体の解放はこの順で単一ミューテックスにより直列化し、
  除去後に初めて解放を行う。ハンドル内部（`FandheStore` 自体）に有効性フラグを持たせて
  解放後にそれを読む設計は、読み出し自体が解放済みメモリへのアクセス
  （use-after-free）になり対策として機能しないため採用しない。
  状態に触れる同一関数群（`fandhe_render_snapshot` / `fandhe_dispatch_event`）も
  ハンドル使用前に同じレジストリでメンバーシップを検査し、無効ハンドルであれば実体を
  参照せずエラーを返す（`FandheBuffer` を返す `fandhe_render_snapshot` はヌルポインタ・
  長さ 0 のバッファを返す）。
  無効ハンドルであるという事実自体はエラー詳細として有用なため、この場合の詳細
  メッセージは per-handle ストレージ（実体側、すなわち解放済みかもしれないメモリ）には
  書き込まず、スレッドローカル最終エラー経路にのみ書き込む。
  `fandhe_last_error_message` はこの群に含めない（上記「現在値・現在契約」参照）。
- `fandhe_render_snapshot`: `Screen::iter()` と `Node::expand_children()` による
  **再帰**で走査し、`List` の子要素境界を JSON の**入れ子構造**
  （`SnapshotNode::List { children }`）として保持する（Swift 側が子要素の境界を
  フラットな列の中で復元する必要をなくすための設計）。
  JSON スナップショット型は `fandhe-core` の型ではなく、**`fandhe-swiftui-bridge` の
  `codec` モジュールが新規に定義する、`fandhe-core` に依存しないプレーンなデータ型**
  （`SnapshotNode` / `RenderSnapshot`、3.2 参照）である
  （`Node<S>` は `fn` ポインタフィールド〔`get` / `set` / `on_click` / `expand`〕を持ち
  `Serialize` を導出できないため、また `render_headless` は行単位のテキスト表現しか
  返さずツリー構造を保持しないため、そのままシリアライズ対象にはできない）。`codec` は
  走査結果〔ラベル・現在値・子要素〕を都度この新規型へ詰め替えてから JSON 化する
  （実体は `encode_snapshot`、3.2 参照）。この設計により `fandhe-core` に serde 系の
  依存は一切追加されない（coding-rust.md「共通コアからプラットフォーム固有クレートへ
  依存しない」の対偶として、共通コアが特定バインディング形式の依存を持たないことも
  維持する）。
- `fandhe_dispatch_event`: 受け取った JSON バイト列を `codec::decode_event`
  （長さ上限チェック → JSON パース → 既知タグのみ受理の順で検証する）でデシリアライズし、
  `StateStore::dispatch`（`crates/fandhe-core/src/state/store.rs`）が受け取る
  `Event`（`ButtonClicked` / `TextFieldChanged`）へ変換して呼び出す。戻り値は
  再描画要否・エラーコードを表す `i32`（上記「現在値・現在契約」参照）。
- `fandhe_last_error_message`: 直近のエラーの詳細メッセージを取得する。参照先は次の
  優先順位で決まる。(1) `store` が `NULL` の場合（`fandhe_store_new` 自体が失敗し
  ハンドルがまだ存在しない等）はスレッドローカルの最終エラーを参照する。(2) `store` が
  非 `NULL` でも外部レジストリへのメンバーシップ検査に失敗した場合（無効ハンドル。
  既に `fandhe_store_free` 済み、または元から不正なポインタ）も、実体（解放済みかもしれない
  メモリ）を参照せずスレッドローカルの最終エラーを参照する。無効ハンドルのエラー詳細は
  発生時点で per-handle ストレージではなくこのスレッドローカル経路へ書き込まれているため、
  ここで初めて空バッファ以外の詳細が呼び出し元へ返る。(3) メンバーシップ検査に成功した
  （有効な）ハンドルの場合のみ、そのハンドルに紐づく per-handle の最新エラーメッセージを
  参照する。

#### エラー伝播

2 層方式を採用する。

1. **`i32` エラーコード enum**: 各エクスポート関数の戻り値（または out パラメータ）として
   返す。`0` は成功、負値をエラー種別（不正なポインタ・JSON パース失敗・ハンドル無効・
   panic 捕捉 等）に割り当てる（実値は上記「現在値・現在契約」参照）。
2. **詳細メッセージ取得 API**: `fandhe_last_error_message` により、直前のエラーの
   人間可読な詳細を取得できる。取得元・書き込み先は次の 2 段構えとする。
   - 有効なハンドル使用時: ハンドル（`FandheStore`）ごとに直近のエラーメッセージを
     per-handle ストレージへ保持し、`fandhe_last_error_message` はメンバーシップ検査に
     成功した場合のみここを参照する。
   - ハンドル非依存時（`fandhe_store_new` 自体の失敗、`store` が `NULL`）および
     無効ハンドル使用時（メンバーシップ検査失敗。既に解放済み・元々不正なポインタ）:
     プロセス内スレッドローカルの「最終エラー」に書き込み・参照する。無効ハンドルの
     場合に per-handle ストレージへ書き込まない（＝実体を経由しない）のは、その実体が
     解放済みメモリである可能性があり書き込み自体が use-after-free になり得るためである。
     `fandhe_last_error_message` 側も無効ハンドルを渡された場合は同じスレッドローカルへ
     フォールバックするため、ハンドルなし・無効ハンドルいずれのエラーも詳細が欠落しない。
3. **panic 遮断**: 全エクスポート関数本体を `catch_unwind` で包み、Rust 側の panic が
   Swift 側へ未定義動作として伝播しないようにする。panic を捕捉した場合は
   `FandheErrorCode::PanicCaught` へ変換し、有効ハンドル使用中でも常にスレッドローカルへ
   メッセージを格納する（panic 後の per-handle 実体の整合性を仮定しないため、上記
   「現在値・現在契約」参照）。

### 3.4 ビルド・ターゲット制約

- ワークスペースの `rust-toolchain.toml` は CI ランナーとの一致のため
  1.88.0 に固定されている。4 ターゲット
  （`aarch64-apple-watchos{,-sim}` / `aarch64-apple-visionos{,-sim}`）の rustup 配布は
  Rust 1.96.0 のみであるため、`scripts/apple/build-watchos-visionos-targets.sh` と
  `build-bridge-xcframework.sh` は `cargo +1.96.0` を局所使用する
  （`FANDHE_APPLE_TOOLCHAIN` 環境変数で上書き可）。前者の既定対象パッケージに
  `fandhe-swiftui-bridge` が含まれ、staticlib の生成物を検査する。
- 依存の MSRV フィルタ（`rust-version` が 1.88 以下のもののみ採用）は workspace 全体で
  適用する制約として維持する。
- macOS 上での実ビルド・実リンク確認は TASK-3.5（#231・人間担当）の残っている確認事項
  である。

## 4. 再考条件（設計見直しトリガー）

- **性能計測で JSON 形式が問題化した場合**: 実機検証（TASK-3.5）でフレームタイム・
  レスポンス性への悪影響が確認された場合、3.2 のバイナリ形式（案 B）への移行を検討する。
- **任意アプリ状態への一般化が必要になった場合**: `sample_screen` / `SampleState` を超えた
  任意の `S` をブリッジ経由で扱う要求が生じた場合、マクロ・コード生成による単相化コード
  自動生成を含めて別途設計する（本書のスコープ外）。
- **uniffi-rs / swift-bridge の watchOS 対応状況が改善した場合**: 本書作成時点では
  対応確度が未検証のため不採用としたが、upstream の対応進展があれば cbindgen 併用案を
  含めて再評価する（TASK-3.6 の継続監視ルールと合わせて確認する）。
- **release ビルドでのエラーメッセージ抑制**: デバッグ用の詳細ダンプ API・冗長な
  エラーメッセージを release ビルドで抑制する feature 設計は未着手である。
- **spec 変更に相当する再判断が必要になった場合**: 上記いずれかの結果として
  REQ-3 の受け入れ基準そのものに影響する変更が必要になった場合、それは spec リポジトリ
  （Fandhe-AI/fandhe-multi-platform-spec、非公開）側の検討事項としてユーザーへ報告する
  （`.claude/rules/out-of-scope-tracking.md` 準拠）。

## 5. セキュリティ考慮事項（OWASP Top 10、security.md 準拠）

- **A03 インジェクション / 不正入力**: FFI 境界は信頼境界とみなす。Swift から渡る
  ポインタ・長さ・JSON ペイロードは使用前に検証する。`codec` 側は長さ上限
  （`MAX_EVENT_PAYLOAD_LEN = 64 * 1024` を JSON パース前に検査）→ UTF-8 検証込みの
  JSON パース → `deny_unknown_fields` による未知タグ・未知フィールドの拒否、という
  順で防御する。`ffi` 側は NULL 検査 → レジストリのメンバーシップ検査 → 実体参照、の
  順で防御する。いずれも `CodecError` / `FandheErrorCode` を返す設計であり
  `unwrap()` / `expect()` を使わずパニックしない。
- **A05 セキュリティ設定ミス**: エラーメッセージに内部ポインタ値・アドレス等を含めない。
  `MAX_ERROR_MESSAGE_LEN`（4 KiB）で切り詰めて格納する。デバッグ用の詳細ダンプ API・
  冗長なエラーメッセージを release ビルドで抑制する feature 設計は未着手（4 節参照）。
- **A06 脆弱な依存**: 依存クレートは MSRV フィルタ（`rust-version` 1.88 以下）で選定する。
  `cargo audit` はリポジトリ CI に未導入であり、導入時に本クレートの依存も対象になる。
- **A08 整合性の不備**: `fandhe_abi_version()`（Swift 側 `FandheABI.verify` がゲート）と
  `SNAPSHOT_SCHEMA_VERSION` の 2 系統で Rust ⇔ Swift の世代整合を検査する。
- **メモリ安全性**: `unsafe` は `ffi` モジュールに閉じ込め、全 `unsafe` ブロックに
  `// SAFETY:` を必須とする。`docs/architecture/unsafe-audit.md` が監査台帳であり、
  新規追加は security-auditor の必須レビュー対象である（security.md 準拠）。所有権規約
  （誰が確保し誰が解放するか）は 3.1 の表を参照。ABA 問題（解放済みアドレスの再利用による
  レジストリ誤判定）は現在も残る既知の制約である（3.1「所有権・寿命規約」参照）。
- **秘密情報**: 本書に秘密情報は含まれない。

## 6. REQ-3・M3 との対応

| 基準 | 本書の節 | 実体 |
| --- | --- | --- |
| REQ-3 受け入れ基準（`04-requirements.md` L60、型安全なブリッジ層の設計・文書化） | 2 節・3 節 | `crates/fandhe-swiftui-bridge/src/{codec,error,ffi/mod,ffi/buffer}.rs` |
| REQ-3 受け入れ基準（コンパイル成功・シミュレータ起動） | 3.4 節 | TASK-3.1（クロスコンパイル）完了済み。SwiftUI 最小画面のシミュレータ起動確認は TASK-3.5（#231・人間担当） |
| `06-roadmap.md` M3（3 プラットフォーム統合・基盤クレート新規開発） | 本書全体 | `crates/fandhe-swiftui-bridge` と `platforms/apple/FandheSwiftUIBridge` の実装 |
| TASK-3.2（`extern "C"` 経由で Rust コア関数を SwiftUI 側から呼び出す、型安全なブリッジ層の設計・実装） | 3.1〜3.3 節 | 設計・実装ともに `crates/fandhe-swiftui-bridge` に反映済み |

実機・シミュレータでの起動確認は TASK-3.5（#231・人間担当）に引き継ぐ。

## 7. 出典

- `docs/spec/04-requirements.md` L53〜66（REQ-3）
- `docs/spec/05-tasks.md` L96〜135（TASK-3.1〜TASK-3.6）
- `docs/spec/06-roadmap.md` L60〜（M3）
- `docs/spec/03-poc/watchos-visionos/README.md`、
  `docs/spec/03-poc/watchos-visionos/minimal-lib/src/lib.rs`（PoC-2 実装）
- `crates/fandhe-swiftui-bridge/src/{codec,error,lib}.rs`、
  `crates/fandhe-swiftui-bridge/src/ffi/{mod,buffer}.rs`
- `platforms/apple/FandheSwiftUIBridge/**`、`platforms/apple/project.yml`
- `scripts/apple/build-bridge-xcframework.sh`、`scripts/apple/xcode-prebuild-bridge.sh`、
  `scripts/apple/build-watchos-visionos-targets.sh`
- `crates/fandhe-core/src/ui/screen.rs`（`Screen<S>::iter()`）、
  `crates/fandhe-core/src/ui/node.rs`（`Node<S>` の `fn` ポインタフィールド、
  `Node<S>::expand_children`）、
  `crates/fandhe-core/src/state/store.rs`（`StateStore<S>` / `Event`）、
  `crates/fandhe-core/src/ui/sample.rs`（`SampleState` / `sample_screen`）
- `Cargo.toml`（`[workspace.dependencies]` の MSRV フィルタ）
- `docs/architecture/unsafe-audit.md`（本リポジトリの `unsafe` 監査台帳）
- `docs/architecture/state-access-api-tradeoff.md`、`docs/architecture/responsibility-boundary.md`
  （本書と同一の置き場・体裁）
- `.claude/rules/coding-rust.md`・`.claude/rules/security.md`（unsafe 境界・OWASP 規約）
