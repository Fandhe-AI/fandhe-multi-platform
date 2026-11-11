# 端末機能を機能 crate へ移す手順

端末機能を `fandhe-egui-proxy` / `fandhe-device` から `crates/fandhe-cap-<id>` へ移す作業の手順書です（Issue #1183、
TASK-21.4）。構成の根拠は `docs/architecture/consumer-app-structure.md`（§8 が共有コードの扱い）、
宣言の書き方は `docs/architecture/capability-declaration.md` です。

先行 5 機能（camera-capture・document-scan・nfc-card-emulation・share-receive・home-widget）と、
手順の試行に使った `device-identifier` が実例です。迷ったら `fandhe-cap-nfc-card-emulation`（Android の JNI）と
`fandhe-cap-camera-capture`（iOS の FFI・ObjC）を見ます。

## 1. 移す対象の仕分け

| 対象 | 扱い |
|------|------|
| 端末機能でない行（`ui-widget-catalog`・`checkbox-list`・`dark-mode`・`gpu-backend` など） | crate を作らない |
| Windows・Web の実装がある機能 | `fandhe-platform-windows` / `-web`（#1184・#1185）の後で移す。`inventory` が注意を表示する |
| 実装が共有ファイル（`device/android.rs`・`device/ios.rs`・`MainActivity.java`）に埋まった古い型（tts・clipboard・http・storage など） | `scaffold` では動かせない。`inventory` の参照行を見て手で切り出す |
| 対応表の id と stem が違う機能（`http-get` → `http`、`dynamic-type` → `text_scale` など） | `--module <stem>` で渡す |
| stem が複数ある機能 | stem ごとに手で移す。`scaffold` は 1 stem だけ扱う |

## 2. 定型レイアウト

```text
crates/fandhe-cap-<id>/
├── Cargo.toml          # 依存と [package.metadata.fandhe]
├── build.rs            # ObjC があるときだけ
├── src/lib.rs          # crate doc・unsafe の属性・mod・再公開
├── src/contract.rs     # ← fandhe-device/src/<stem>.rs
├── src/error.rs        # <Pascal>Error
├── src/driver.rs       # ← fandhe-egui-proxy/src/device/<stem>.rs
├── src/<os>.rs         # ← device/<os>_<stem>.rs（android・apple・ios・macos・windows）
├── ios/ macos/ apple/  # ← fandhe-egui-proxy の同名ディレクトリの .m・.h・.swift
└── android/java/com/fandhe/platform/   # ← <Pascal>Helper.java
```

- `unsafe` を持たなければ `#![forbid(unsafe_code)]`。持つなら、該当 OS だけ `deny` へ緩める `cfg_attr` の組にする
  （`fandhe-cap-camera-capture` と同じ形）。`scripts/audit/unsafe-audit.sh` は `fandhe-cap-*` のクレートルートに
  `forbid(unsafe_code)` が無ければ違反にする
- 宣言の `frameworks` には基盤（Foundation・UIKit・CoreGraphics）を書かない
- 画面の状態型（`fandhe_core::screens`）を参照してよいのは `driver.rs` だけ。契約と OS 実装は依存させない

## 3. 手順

```bash
node scripts/device/migrate-capability.mjs inventory <id> [--module <stem>]
node scripts/device/migrate-capability.mjs scaffold <id> [--module <stem>] [--dry-run]
```

1. `inventory`: 対応表の `impl` の実在、移動先、`DeviceError` のバリアント、機能間の共有、共有ファイル内の参照行を読む
2. 共有コードを `consumer-app-structure.md` §8 の表で仕分ける。他機能の OS 機能を実際に使うなら機能 crate 同士の依存、
   型・検証だけなら `fandhe-device` に残す
3. `scaffold`: `git mv`、`Cargo.toml`・`error.rs`・`lib.rs`・`build.rs` の生成、参照の機械的な置換を行う。
   最後に `sync` を呼び、残件（手作業）を表示する
4. 手作業（次の表）
5. 検証（4 章）

### 手作業の項目

| 項目 | 内容 |
|------|------|
| 共通部品への置換の確認 | `crate::app::CompletionSender` → `fandhe_runtime::CompletionSender`、`super::request_slot` → `fandhe_runtime::request_slot`、`crate::{android,ios}::request_repaint()` → `fandhe_runtime::request_repaint()`、`crate::android::android_app()` → `fandhe_platform_android::android_app()`、`JniCallFailed` → `fandhe_platform_android::JniCallFailed`（`scaffold` が置換する） |
| エラー型 | `<Pascal>Error` の文言を固定するテストを足す。写せないバリアント（`Io` など）は `error.rs` に手で足す |
| `driver.rs` | import を直し、`platform_<stem>()` を `pub` にして役割を doc に書く。テストの `egui_sender` は `CompletionSender::new(tx, Arc::new(NoopWaker))` に直す（`fandhe-cap-camera-capture` のテストと同じ形） |
| JNI | `MainActivity` のインスタンスメソッド呼び出しをやめ、Helper の static メソッドを `fandhe_platform_android::with_activity_class` で呼ぶ。Helper は渡された Context から `getApplicationContext()` を取る。JNI の署名は Java と Rust の 2 か所で照合する。手本は `fandhe-cap-nfc-card-emulation/src/android.rs` |
| ObjC | `build.rs` の `compile_objc` に framework を渡す。`.m` 冒頭コメントのパスを直す |
| ライフサイクル | Activity のライフサイクルに参加する機能は `FeatureLifecycleProvider`（`fandhe-platform-android`）を使い、宣言の `components` に provider を書く |
| 宣言 | 権限・usage description・framework・`<queries>`・コンポーネントを `[package.metadata.fandhe]` に書き、`cargo run -p cargo-fandhe -- check` で現行の設定と突き合わせる |
| 共有ファイル | `fandhe-egui-proxy/src/device.rs` の `mod` 宣言、`fandhe-egui-proxy/build.rs` の `compile_ios_*`、`fandhe-device/src/lib.rs` の `pub mod`、`MainActivity.java` の受け口、`fandhe-gallery/src/screens/registry.rs` の effect の参照先を直す。他機能の SAFETY コメントが移した関数を根拠に指している場合は元の根拠へ直す |
| 対応表 | `docs/device/capabilities/<id>.json` の `impl` を新しいパスにする |
| crate doc | 役割・呼び出し元・守る範囲と守らない範囲を `lib.rs` に書く（`code-comment-style.md`）。`.claude/rules/device-bridge-contracts.md` の自己点検を移行 PR でも行う |

## 4. 検証

| 確認 | コマンド |
|------|---------|
| 整形・lint | `cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets --all-features -- -D warnings` |
| テスト | `cargo test --workspace --all-features` |
| テストの移動漏れ | 移行前に `cargo test -p fandhe-device -p fandhe-egui-proxy -- --list` の `<stem>` を含む件数を控え、移行後の `cargo test -p fandhe-cap-<id> -- --list` の件数と照合する（減っていないこと） |
| 宣言 | `cargo run -p cargo-fandhe -- check`、`cargo test -p cargo-fandhe` |
| 対応表 | `node scripts/device/build-capabilities.mjs --check` |
| unsafe | `bash scripts/audit/unsafe-audit.sh` |
| iOS | `scripts/ios/check-lib.sh`（iOS の cfg を触るため必須。CI では検査されない） |
| Android | `bash scripts/android/build-android.sh`。NDK が無ければ CI の android-build で確認し、PR 本文に未検証と書く |
| wasm・Windows | `cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown --all-features -- -D warnings`、`cargo clippy --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings` |
| ツール | `node --test scripts/device/migrate-capability.test.mjs` |
| 実挙動 | デスクトップで `FANDHE_PROXY_APP=<screen> cargo run -p fandhe-gallery` を起動し、従来どおりの表示になること |

iOS・Android の実機での確認は人間担当です。移行 PR では行わず、PR 本文に明記します。

## 5. 一覧の同期

機能 crate を足しても、次の一覧は手で編集しません。

| 一覧 | 仕組み |
|------|--------|
| `Cargo.toml` の `members` | glob（`crates/fandhe-cap-*`） |
| `scripts/android/build-android.sh` の `-p` | `crates/fandhe-cap-*/` から導出 |
| `scripts/ios/check-lib.sh` の `-p` | 同上 |
| `scripts/audit/unsafe-audit.sh` | `fandhe-cap-*` の接頭辞の規則 |
| `fandhe-gallery/Cargo.toml` の feature `device` と依存、`platforms/android/app/build.gradle.kts` の `srcDirs` | `BEGIN fandhe-cap` / `END fandhe-cap` の間を `node scripts/device/migrate-capability.mjs sync` が書き直す（冪等） |

`sync` は目印が無いと失敗します。`tests/gen.rs`（cargo-fandhe）が `build.gradle.kts` の行を文字列で照合するため、glob にはしていません。

## 6. 並列で作業するときの注意

- 一覧の衝突は片側を取って `sync` をやり直す。`Cargo.lock` は取り直す（`cargo metadata` で再生成）
- 共有ファイル（`device.rs`・`fandhe-egui-proxy/build.rs`・`MainActivity.java`・`registry.rs`）は同じ箇所を触りやすい。
  該当行の削除だけに留め、整形や並べ替えを混ぜない
- 連番つきのファイルは作らない
