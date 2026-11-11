# `unsafe` ブロック監査（本実装版）

## 1. 目的・位置づけ

本書は TASK-7.1（`docs/spec/05-tasks.md` L237〜242）の成果物であり、REQ-7（`docs/spec/04-requirements.md`
L104〜113）の受け入れ基準 1「共通コア・最適化レイヤーのすべてのコードが Rust で実装され、`unsafe` ブロック
の使用箇所が一覧化・監査される」（L109）を満たすために、workspace 全クレートを対象に `unsafe` の使用箇所を
一覧化・監査したものです。

- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています。spec 自体の変更はこのリポジトリでは
  行いません（`docs/architecture/crate-feature-matrix.md`・`docs/architecture/responsibility-boundary.md`
  と同じ置き場・体裁）。
- **ロードマップ上の位置づけ**: `docs/spec/06-roadmap.md`（M2、L30〜58）は TASK-7.1 を M2 の完了基準の 1 つ
  として掲げており（L53「`unsafe` ブロックの使用箇所が全コードから一覧化・監査されている（TASK-7.1）」）、
  本書の作成をもってこの完了基準を満たします。
- **前提タスク**: TASK-1.1（完了済み。`crates/fandhe-core` の宣言的 UI DSL・状態管理の最小実装）。

## 2. 監査対象

- **監査日**: 2026-09-06（再監査。初版は 2026-07-29、2026-07-30 に TASK-3.2-S3（#60）が
  `fandhe-swiftui-bridge` 追加分を再監査、2026-09-05 に Issue #746 前の全クレート再監査）
- **対象コミット**: 本 PR のブランチ HEAD（Issue #746、`fandhe-device` 新設時点）
- **workspace メンバー**: `Cargo.toml` の `members` に列挙された全 8 クレート（`fandhe-core` /
  `fandhe-device` / `fandhe-egui-proxy` / `fandhe-fonts` / `fandhe-prototype` /
  `fandhe-swiftui-bridge` / `fandhe-web-dom` / `fandhe-winit-ime`）。`fandhe-device` は
  Phase 8（端末機能の型検証、親 #752）の土台で、feature `device` ごと切り離し可能な構成
  （device-feature-removal.md「端末機能（Phase 8）の切り離し手順」参照）。§4 の一覧表は全クレートを対象とする。
  網羅的な現況は `bash scripts/audit/unsafe-audit.sh` の実行結果が正となる。
- **依存クレート**: `[workspace.dependencies]` はランタイム 18 件（`serde` / `serde_json` /
  `thiserror` / `taffy` / `egui` / `eframe` / `accesskit` / `wgpu` / `ordered-float` / `image` /
  `wasm-bindgen` / `wasm-bindgen-futures` / `web-sys` / `console_error_panic_hook` /
  `unicode-segmentation` / `winit` / `accesskit_winit` / `ureq`）+ dev 専用 3 件（`trybuild` /
  `egui_kittest` / `ab_glyph`）。`ureq`（Issue #746、`fandhe-device::http` の共通実装が使用）の
  選定理由はルート `Cargo.toml` のコメント参照。`cargo metadata` の対象は `--no-deps` により
  workspace メンバー自身のコードに限定しており、依存クレート内部の `unsafe` は本監査のスコープ外。

## 3. 監査手順

監査は 2 つに分かれる。`forbid(unsafe_code)` の適用と `unsafe` の一覧は `scripts/audit/unsafe-audit.sh`、
SAFETY コメントの有無は clippy の lint が検査する（Issue #1269）。

### 3.1 一覧とクレートルートの検査（スクリプト）

```bash
bash scripts/audit/unsafe-audit.sh
```

1. `cargo metadata --no-deps --format-version 1` と `jq` で workspace メンバーのクレート名・
   クレートルート・クレートディレクトリを取得する。
2. 各クレートルートに `#![forbid(unsafe_code)]` が付与されているかを検査する
   （スクリプト冒頭の `FORBID_EXEMPT_CRATES` に列挙したクレートは対象外。FFI 境界を持つクレートを
   追加する際はこのリストへ明示的に登録する）。
3. 各クレートの `src/` `tests/` `examples/` `benches/` 配下、および crate root 直下の `build.rs` から
   `unsafe` トークンを検出して一覧にする（コメント行は除く）。件数は参考値で、判定には使わない。

違反（exit 1）は forbid の未適用のみ。macOS 標準の BSD ツールでも最後まで走る（#1264）。

### 3.2 SAFETY の検査（clippy）

ルート `Cargo.toml` の `[workspace.lints]` に次を `deny` で定義し、全クレートが `[lints] workspace = true`
で継承する。ルートの `clippy.toml` の `check-private-items = true` で、非公開の `unsafe fn` も対象にする。

| lint | 見るもの |
| --- | --- |
| `clippy::undocumented_unsafe_blocks` | `unsafe {}` ブロックと `unsafe impl` に `SAFETY:` コメントがあるか |
| `clippy::missing_safety_doc` | `unsafe fn` に `/// # Safety` があるか |
| `unsafe_op_in_unsafe_fn`（rustc） | `unsafe fn` の本体でも `unsafe {}` で囲んでいるか |

行単位の文字列照合は、別の文の SAFETY を借りる穴を塞げなかったため廃止した。
`clippy::unnecessary_safety_comment`（`unsafe extern {}` のコメントを誤検知する）と
`multiple_unsafe_ops_per_block` は有効にしない。

#### 受け付ける形

- `unsafe {}` ブロックと `unsafe impl`: `SAFETY:` を含むコメントを、ブロックの直前、またはブロックを含む
  文（式文・`let`・`match` のアーム）の直前に置く。属性と空行を間に挟んでよい。
- `match` のアームと `if` / `else` の枝は、それぞれに自分のコメントが要る。
- 1 つの文に含まれる複数のブロックは、その文の直前の 1 つのコメントで足りる。
- `unsafe fn` は、非公開・FFI コールバックも含めて `/// # Safety` が要る。
- 要らないもの: `unsafe extern "C" {}` ブロック（Rust 2024）、`#[unsafe(no_mangle)]` 等の属性、
  関数ポインタ型（`unsafe extern "system" fn(...)`）。

#### clippy でも見られない形（レビューで見る）

- 文字列リテラルの中の `SAFETY:`
- 中身のない `SAFETY:`
- `macro_rules!` の中の `unsafe`

#### 実行するターゲット

`cfg` で除外されたコードは、そのターゲットで clippy を走らせないと検査されない。次の構成を CI
（`.github/workflows/ci.yml`）で必須にしている（Issue #1273 の内容を #1274 で入れた）。新しい `cfg` の
target を足すときは、この一覧と CI のジョブにも足す。

| 構成 | CI のジョブ |
|------|-------------|
| ホスト（Linux）`--all-targets --all-features` | `cargo clippy` |
| `x86_64-pc-windows-gnu` `--workspace --all-targets` | `cargo clippy` |
| `wasm32-unknown-unknown`（`fandhe-web-dom`） | `cargo clippy` |
| `aarch64-linux-android` `--all-features`（`cargo ndk`） | `android クロスビルド` |
| macOS ホスト `--all-targets --all-features`、`aarch64-apple-ios`（既定 feature と `--all-features`） | `cargo clippy (apple)` |

手元で再現するコマンド:

```bash
# ホスト（CI の ubuntu でも同じ）
cargo clippy --workspace --all-targets --all-features -- -D warnings
# Windows
cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu
# Android（NDK の環境変数は scripts/android/android-env.sh を source）
cargo ndk -t arm64-v8a clippy --workspace --all-features
# Apple（macOS のみ。ホストは上のホストと同じコマンド）
cargo clippy --workspace --target aarch64-apple-ios
cargo clippy --workspace --target aarch64-apple-ios --all-features
# Web
cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown -- -D warnings
```

## 4. 監査結果一覧表

| クレート | `forbid(unsafe_code)` 適用（スクリプト判定） | `unsafe` 検出件数 | SAFETY コメント要否 | 判定 |
| --- | --- | --- | --- | --- |
| `fandhe-core` | 適用済み（`crates/fandhe-core/src/lib.rs` の crate root、`#![forbid(unsafe_code)]`） | 0 件 | 該当なし | 合格 |
| `fandhe-device` | 適用済み（`crates/fandhe-device/src/lib.rs`、`#![forbid(unsafe_code)]`） | 0 件 | 該当なし | 合格 |
| `fandhe-fonts` | 適用済み（`crates/fandhe-fonts/src/lib.rs`、`#![forbid(unsafe_code)]`） | 0 件 | 該当なし | 合格 |
| `fandhe-prototype` | 適用済み（`crates/fandhe-prototype/src/lib.rs`、`#![forbid(unsafe_code)]`） | 0 件 | 該当なし | 合格 |
| `fandhe-web-dom` | 適用済み（`crates/fandhe-web-dom/src/lib.rs`、`#![forbid(unsafe_code)]`） | 0 件 | 該当なし | 合格 |
| `fandhe-gallery` | 免除（`FORBID_EXEMPT_CRATES`。クレートルートは Android のみ `deny(unsafe_code)` で、`android_main` の `#[no_mangle]` 1 関数にだけ `#[allow(unsafe_code)]` を付ける。android-activity の固定シンボルは最終成果物の cdylib が持つ必要があるため `fandhe-egui-proxy` から移した。TASK-21.2 / Issue #1165） | 1 件（`#[no_mangle]`） | `crates/fandhe-gallery/src/lib.rs` の `# Safety` | 合格 |
| `fandhe-egui-proxy` | 適用済み（`FORBID_EXEMPT_CRATES` 登録済みだが、`grep -qF` によるコメント一致でスクリプトは「適用済み」と判定する。クレートルートの実属性は `#![cfg_attr(not(any(target_os = "android", target_os = "ios")), forbid(unsafe_code))]`／Android・iOS では `deny`） | 1 件（`src/android.rs:279`、`libc::__system_property_get`（feature `debug-proxy-app` 限定）。カメラ撮影の Android 2 件・iOS 4 件は `fandhe-cap-camera-capture` へ移した、Issue #1170） | 直前に `// SAFETY:` コメントあり | 合格 |
| `fandhe-render-egui` | 適用済み（`#![forbid(unsafe_code)]` を全ターゲットで適用。描画とアプリ起動を `fandhe-egui-proxy` から移したクレートで、`unsafe` を含まない。TASK-21.2 / Issue #1164） | 0 件 | — | 合格 |
| `fandhe-platform-apple` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは `#![deny(unsafe_code)]` を採用し、`main_thread` モジュールのみ `unsafe` を許容する。`crates/fandhe-egui-proxy/src/device/ios.rs` から移した `pthread_main_np` の宣言と呼び出し、TASK-21.2 / Issue #1168） | 2 件（`src/lib.rs` の `unsafe extern "C"` 宣言 1 + `pthread_main_np` 呼び出し 1。iOS・macOS ターゲットのみ） | 2 件とも直前に `// SAFETY:` コメントあり | 合格 |
| `fandhe-cap-camera-capture` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは Android・iOS のみ `deny(unsafe_code)` へ緩める `cfg_attr`。Issue #1170） | 5 件（iOS: `src/ios.rs` の `unsafe extern "C"` 宣言 1 + `fandhe_ios_camera_capture` 呼び出し 1 + `free_captured_buffer` 内の解放呼び出し 1 + `std::slice::from_raw_parts` 1／Android: `src/android.rs` の `#[unsafe(no_mangle)]` JNI エクスポート 1。JavaVM と Activity の `unsafe` は `fandhe-platform-android` の `with_activity` に集約済み） | 4 件は直前に `// SAFETY:` コメントあり、`no_mangle` 1 件は `# Safety` 節あり | 合格 |
| `fandhe-swiftui-bridge` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは `#![deny(unsafe_code)]` を採用し、設計文書 §3.1「unsafe 境界の規約」のとおり `ffi` モジュールのみ `unsafe` を許容し他モジュールは禁止する） | 29 件（`src/ffi/mod.rs` 25 件 = 公開 `unsafe extern "C" fn` 4 + 本体内の `unsafe` ブロック 6（196・227・275・278・309・347 行）+ `#[cfg(test)]` 内の呼び出し 15 件、`src/ffi/buffer.rs` 4 件。スクリプトは `src/` 配下を一律に走査する仕様のためテストコードも件数に含む） | 29 件とも直前のコメント・属性ブロックに `// SAFETY:` または `# Safety` あり | 合格 |
| `fandhe-platform-android` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは `#![deny(unsafe_code)]`、`with_activity` のみ `unsafe` を許容。Android 以外ではクレート全体が `cfg` で空） | 2 件（`with_activity` の `JavaVM::from_raw` と `as_cast_raw`。ホストでは検出されうる） | 2 件とも直前に `// SAFETY:` コメントあり | Issue #1166 追加分 |
| `fandhe-platform-web` | 適用済み（`crates/fandhe-platform-web/src/lib.rs`、`#![forbid(unsafe_code)]`、TASK-21.4 / Issue #1185） | 0 件 | 該当なし | 合格 |
| `fandhe-cap-share-receive` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは Android・macOS のみ `deny(unsafe_code)` へ緩め、他の OS では `forbid(unsafe_code)` を保つ。`fandhe-egui-proxy` の `device/android_share_receive.rs`・`device/macos_share_receive.rs` から移した。TASK-21.3 / Issue #1173） | 5 件（`src/android.rs` の JNI 実体 `#[unsafe(no_mangle)]` 1、`src/macos.rs` の `unsafe extern "C"` 宣言 1 + `register` 内の呼び出し 1 + `#[unsafe(no_mangle)]` 1 + `std::slice::from_raw_parts` 1） | 5 件とも直前に `// SAFETY:` コメントまたは `# Safety` あり | スクリプトで確認する |
| `fandhe-winit-ime` | 適用済み（`FORBID_EXEMPT_CRATES` 登録済みだが、`grep -qF` によるコメント一致でスクリプトは「適用済み」と判定する。クレートルートの実属性は `#![deny(unsafe_code)]`、`macos_secure_input` モジュールのみ `unsafe` を許容） | 3 件（`macos_secure_input.rs` の `EnableSecureEventInput`/`DisableSecureEventInput`/`IsSecureEventInputEnabled` 呼び出し。macOS 以外のターゲットではモジュール自体が `cfg` で除外されるため検出されない） | 3 件とも直前に `// SAFETY:` コメントあり | 合格 |

**集計（Issue #790 の Android 実装と Issue #791 の iOS 実装を統合した時点の再監査、2026-09-06）**:
クレート数 8、forbid 適用クレート数 7/8（`fandhe-swiftui-bridge` のみ `FORBID_EXEMPT_CRATES`
登録による免除で deny 採用、違反ではない）、`unsafe` 検出件数（合計）39 件（Issue #746 時点の
33 件から、`fandhe-egui-proxy` の `device/android.rs`（Issue #750、`jni::JavaVM::from_raw`・
`Env::as_cast_raw`）2 件と `device/ios.rs`（Issue #751、4 件）が両方とも純増し、合計 6 件増加）。
`bash scripts/audit/unsafe-audit.sh` は macOS（BSD ツール）でも完走する（#1264）。SAFETY の検査は §3.2 の clippy が担う。

**スクリプト判定の補足**: `fandhe-egui-proxy` と `fandhe-winit-ime` の「適用済み」は、クレートルートの
`lib.rs` 冒頭のドキュメンテーションコメント（または `examples/*.rs` の `#![forbid(unsafe_code)]`）への
`grep -qF` 一致によるものであり、lib クレートルート自体の属性は前者が `cfg_attr` 条件付き forbid、
後者が `deny` である。両クレートとも `FORBID_EXEMPT_CRATES` に登録済みのため、この表記ゆれは判定結果
（違反なし）には影響しない。

## 5. 依存クレートの扱い

依存クレート内部の `unsafe` は本監査のスコープ外とする（`cargo metadata --no-deps` により
workspace メンバー自身のコードに限定）。既知脆弱性確認（`cargo audit`）は REQ-6・
`.claude/rules/security.md` の管轄。依存クレート追加時は本監査と `cargo audit` の双方を実行する。

## 6. 今後の運用

- **FFI 境界での `unsafe` 追加時**: `.claude/rules/coding-rust.md` の規約（最小範囲の関数・モジュールに
  閉じる／`// SAFETY:` コメントで安全性根拠を明記する／安全な公開 API でラップし `unsafe` を呼び出し側へ
  漏らさない）に従う。現行の FFI 境界クレート（`fandhe-swiftui-bridge` の `ffi` モジュール、
  `fandhe-winit-ime` の `macos_secure_input` モジュール、`fandhe-egui-proxy` の Android 分岐
  `android.rs` と iOS 分岐 `device/ios.rs`）はいずれも FFI 境界で `unsafe` が不可避なため forbid を付けられず、
  `scripts/audit/unsafe-audit.sh` 冒頭の `FORBID_EXEMPT_CRATES`（`scripts/android/versions.env` と同様の
  SSOT パターン）へ明示的に登録済みである。新規クレートを追加する際も同様に登録する運用とする。
  登録しない限り forbid 未適用は違反（exit 1）として検出されるため、登録漏れによる監査基準の
  暗黙的な緩和を防ぐ。
- **クレート追加時の再監査**: workspace にクレートを追加するたびに `bash scripts/audit/unsafe-audit.sh`
  を再実行し、本書の「4. 監査結果一覧表」を更新する。
- **security-auditor によるレビュー**: `.claude/rules/security.md` の運用に従い、`unsafe` の新規追加は
  security-auditor の読み取り専用レビュー対象とする。

## 7. REQ-7 受け入れ基準・M2 完了基準との整合確認

| 基準 | 内容 | 本書での対応 |
| --- | --- | --- |
| REQ-7 受け入れ基準 1（`04-requirements.md` L109） | 共通コア・最適化レイヤーのすべてのコードが Rust で実装され、`unsafe` ブロックの使用箇所が一覧化・監査される | 本書の「2. 監査対象」「4. 監査結果一覧表」で対応（**本書のスコープ**） |
| REQ-7 受け入れ基準 2（`04-requirements.md` L110） | 全対象プラットフォームの Rust ターゲットへ `rustup target add` のみでコンパイルできることを確認する | TASK-7.2（`05-tasks.md` L244〜249）のスコープ（本書のスコープ外。別イシューで対応） |
| `06-roadmap.md` M2 完了基準（L53） | `unsafe` ブロックの使用箇所が全コードから一覧化・監査されている（TASK-7.1） | 本書と `scripts/audit/unsafe-audit.sh` の作成をもって TASK-7.1 の成果物とする |

## 8. 出典

- `docs/spec/04-requirements.md` L104〜113（REQ-7）
- `docs/spec/05-tasks.md` L235〜249（REQ-7 に関するタスク、TASK-7.1・TASK-7.2）
- `docs/spec/06-roadmap.md` L30〜58（M2）
- `crates/fandhe-core/src/lib.rs`・`crates/fandhe-device/src/lib.rs`（`#![forbid(unsafe_code)]` の適用箇所）
- `scripts/audit/unsafe-audit.sh`（本書の監査結果を生成する再現可能スクリプト）
- `crates/fandhe-egui-proxy/src/lib.rs`・`crates/fandhe-egui-proxy/src/android.rs`（Android 分岐の
  `unsafe` 境界）・`crates/fandhe-egui-proxy/src/device/ios.rs`（iOS 分岐の `unsafe` 境界、Issue #751）
- `crates/fandhe-swiftui-bridge/src/ffi/`（`mod.rs`・`buffer.rs`、FFI 境界の `unsafe` 一式）
- `crates/fandhe-winit-ime/src/macos_secure_input.rs`（Carbon SecureEventInput FFI 境界）
- `docs/architecture/crate-feature-matrix.md`・`docs/architecture/responsibility-boundary.md`（同一の
  置き場・体裁の先行成果物）
