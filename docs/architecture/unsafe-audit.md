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

再現可能な監査は `scripts/audit/unsafe-audit.sh` で行う。

```bash
bash scripts/audit/unsafe-audit.sh
```

スクリプトの処理内容:

1. `cargo metadata --no-deps --format-version 1` と `jq` で workspace メンバーのクレート名・
   クレートルート（`lib.rs`/`main.rs` 等の `src_path`）・クレートディレクトリを構造的に取得する。
2. 各クレートルートに forbid 属性（`unsafe_code` を上書き不可で禁止するもの）が付与されているかを
   検査する（スクリプト冒頭の `FORBID_EXEMPT_CRATES` に列挙したクレートは対象外。coding-rust.md が
   forbid を求めるのは共通コアのみのため、FFI 境界を持つプラットフォーム統合レイヤーのクレートを
   追加する際はこのリストへ明示的に登録する運用とする。登録クレートはスクリプトの
   `FORBID_EXEMPT_CRATES` を参照）。
3. 各クレートの `src/` `tests/` `examples/` `benches/` 配下、および crate root 直下の `build.rs` から
   `unsafe` トークンを単語境界一致（`grep -w`）で検出する。コメント行（trim 後に `//` から始まる行。
   `#![forbid(unsafe_code)]` の行や、日本語コメント中で `unsafe` という語だけに言及している行を
   使用箇所として誤カウントしないための除外）は検出対象から除く。
4. 検出した `unsafe` について、直前の連続するコメント・属性行（`//`/`///`/`#[...]`）を上方向へ
   最大 30 行たどり、`// SAFETY:`（本規約の既定の書式）または rustdoc の `# Safety` セクション
   見出しのいずれかが見つかるかを確認する（`.claude/rules/coding-rust.md` の SAFETY コメント
   必須規約に対応。`pub unsafe extern "C" fn` のように `#[no_mangle]` 等の属性行を挟んで
   `/// # Safety` ドキュメンテーションコメントが続く構成にも対応する）。
   - **複数行の式**: `unsafe` が式の 2 行目以降にあるときは、直上行が式の途中である間
     （空行・コメント・属性でなく、行末が `;` `{` `}` でもない行。最大 15 行）だけ上へたどり、
     その文の先頭行の直前に隣接する SAFETY ブロックも根拠として受け付ける。
     受け付けるのは「SAFETY ブロックの直後から、`unsafe` を含む文が始まる」形のみ。
     空行・別の文・ブロックの区切り（`;` `{` `}`）を挟んだ離れた SAFETY は根拠とみなさない。
     根拠として読むのはコメントだけの行（`//` `/*` `*` で始まる行）と属性だけの行（`#[...]`）に限り、
     コード行の行末コメント・文字列内の `// SAFETY:`・同じ行の末尾コメントは根拠にしない。
     行末に `//` / `/* */` コメントを持つ行は文末かの判別が曖昧なため境界として扱い、
     式の途中にコメントを挟む複数行の式は受け付けない（SAFETY を `unsafe` の直前に付ける）。
     同じ文の中に `unsafe` が複数あれば同じ SAFETY が共有される。別の文の `unsafe` は
     それぞれに SAFETY を付ける。
5. 次の形は根拠を要する使用として数えない（誤検知の除外）: `unsafe extern "C" {` 宣言ブロック
   （個々の呼び出し側が根拠を持つ）、`#[unsafe(no_mangle)]` 等の `#[unsafe(...)]` 属性、
   関数ポインタ型（`unsafe extern "system" fn(` のように `fn` の直後が `(`。関数定義は `fn 名前(`
   なので区別できる）。`unsafe extern "..." fn 名前(` の関数定義は対象のままで、`# Safety` が要る。

判定ロジックだけの確認は `scripts/audit/unsafe-audit.sh --self-test`（固定の入力、cargo 不要）。

違反判定（exit 1）: クレートルートに `#![forbid(unsafe_code)]` が無い場合（`FORBID_EXEMPT_CRATES` 記載
クレートを除く）、または `unsafe` が検出されかつ直前のコメント・属性ブロックに `// SAFETY:` /
`# Safety` のいずれも無い場合。

**macOS ホストでの実行**: BSD `sed` / `grep` でも最後まで走る（#1264）。

手動での再現（クロスチェック用）:

```bash
grep -rn --include="*.rs" -w "unsafe" crates/
```

## 4. 監査結果一覧表

| クレート | `forbid(unsafe_code)` 適用（スクリプト判定） | `unsafe` 検出件数 | SAFETY コメント要否 | 判定 |
|---------|---------------------------|-------------------|---------------------|------|
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
| `fandhe-platform-android` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは `#![deny(unsafe_code)]`、`with_activity` のみ `unsafe` を許容。Android 以外ではクレート全体が `cfg` で空） | 2 件（`with_activity` の `JavaVM::from_raw` と `as_cast_raw`。ホストでは検出されうる） | 2 件とも直前に `// SAFETY:` コメントあり | Issue #1166 追加分。スクリプトの実行結果は CI で確認する（手元 macOS では sed の引数エラーで完走しない） |
| `fandhe-cap-share-receive` | 未適用（`FORBID_EXEMPT_CRATES` により免除。クレートルートは Android・macOS のみ `deny(unsafe_code)` へ緩め、他の OS では `forbid(unsafe_code)` を保つ。`fandhe-egui-proxy` の `device/android_share_receive.rs`・`device/macos_share_receive.rs` から移した。TASK-21.3 / Issue #1173） | 5 件（`src/android.rs` の JNI 実体 `#[unsafe(no_mangle)]` 1、`src/macos.rs` の `unsafe extern "C"` 宣言 1 + `register` 内の呼び出し 1 + `#[unsafe(no_mangle)]` 1 + `std::slice::from_raw_parts` 1） | 5 件とも直前に `// SAFETY:` コメントまたは `# Safety` あり | 手元 macOS ではスクリプトが完走しない場合があるため CI で確認する |
| `fandhe-winit-ime` | 適用済み（`FORBID_EXEMPT_CRATES` 登録済みだが、`grep -qF` によるコメント一致でスクリプトは「適用済み」と判定する。クレートルートの実属性は `#![deny(unsafe_code)]`、`macos_secure_input` モジュールのみ `unsafe` を許容） | 3 件（`macos_secure_input.rs` の `EnableSecureEventInput`/`DisableSecureEventInput`/`IsSecureEventInputEnabled` 呼び出し。macOS 以外のターゲットではモジュール自体が `cfg` で除外されるため検出されない） | 3 件とも直前に `// SAFETY:` コメントあり | 合格 |

**集計（Issue #790 の Android 実装と Issue #791 の iOS 実装を統合した時点の再監査、2026-09-06）**:
クレート数 8、forbid 適用クレート数 7/8（`fandhe-swiftui-bridge` のみ `FORBID_EXEMPT_CRATES`
登録による免除で deny 採用、違反ではない）、`unsafe` 検出件数（合計）39 件（Issue #746 時点の
33 件から、`fandhe-egui-proxy` の `device/android.rs`（Issue #750、`jni::JavaVM::from_raw`・
`Env::as_cast_raw`）2 件と `device/ios.rs`（Issue #751、4 件）が両方とも純増し、合計 6 件増加）。
`bash scripts/audit/unsafe-audit.sh` は macOS ホスト実行時に `--` 非対応の BSD sed で失敗する
既知の制約（本書「3. 監査手順」の「macOS ホストでの実行注意」参照）があり、GNU sed 環境
（Linux ランナー）での再実行は CI（GitHub Actions）に委ねる。

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
|------|------|-------------|
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
