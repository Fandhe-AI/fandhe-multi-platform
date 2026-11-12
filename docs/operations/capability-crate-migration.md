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
| Windows・Web の実装がある機能 | `fandhe-platform-windows` / `-web`（#1184・#1185）は済み。Windows は `src/windows.rs`（scaffold が移す）、Web は `fandhe-web-dom/src/<stem>.rs` を `src/web.rs` へ `git mv` して手で移す（下の「Web の移し方」）。`inventory` の注意は読み流してよい |
| driver が egui 固有の読み書き口（`fandhe_render_egui::EguiSenderExt`: `set_zoom_factor`・`focused`・`file_drag` など）を使う機能（dynamic-type・share-sheet の Windows 経路・drag-drop） | 機能 crate は egui 非依存のため、そのままは移せない。`fandhe-runtime` に egui 非依存の口を足す設計が先に要る。先行して移さない |
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

## 7. 共有コードの過渡期の規則と Web の移し方（第 1 束 #1186）

- 未移行のモジュールが使い続ける記号（例: `fandhe_device::http::{check_url, MAX_BODY_BYTES}`）は `fandhe-device` に残し、
  機能 crate が `fandhe-device` に依存して参照する。`fandhe-device` から機能 crate へは依存させない
- 機能の終了時の片付け（`fandhe-egui-proxy::device::shutdown` が呼ぶ `stop_shared_*`）が残る間は、
  `fandhe-egui-proxy` が該当の機能 crate へ optional 依存する（feature `device`。#1194 で消える過渡期の依存）
- Web: `fandhe-web-dom/src/<stem>.rs` を `crates/fandhe-cap-<id>/src/web.rs` へ `git mv` し、
  `crate::mount::Applier` を `fandhe_platform_web::Applier` に直す。ブラウザ API の依存は wasm32 の target 依存に書く。
  内側の `mod web` は親と同名で clippy に怒られるため `mod browser` などにする。ブラウザ側の解放が要る機能は
  effect の入口で `fandhe_platform_web::on_mount_release` を呼び、`fandhe-web-dom` の `release_features` から行を消す。
  `fandhe-web-dom` は機能 crate に依存して `demo.rs` から effect を呼ぶ
- wasm32 で解決できない依存（`ureq` など）は native の target 依存にし、driver も `cfg(not(target_arch = "wasm32"))` で外す
- 純関数のテストが private 関数に触れる場合は、元のモジュールに残る関数のテストを元の場所へ戻す

## 8. 第 2 束（#1187）で出た型

第 2 束（pedometer・ml-inference・power-state・screen-wake-lock・ar・call-ui・live-activity・haptics・nlp・calendar）で
はじめて出た型と、その扱いです。

| 型 | 扱い |
|----|------|
| edition 2024 への移行で壊れる書き方 | `gen` は予約語（識別子を変える）、`extern "C" {}` は `unsafe extern "C" {}`、`#[no_mangle]` は `#[unsafe(no_mangle)]`、入れ子の `if` / `if let` は let チェーンへ（`cargo clippy --fix` が直す）。`scaffold` の後に `cargo clippy -p fandhe-cap-<id> --all-targets` を通すと拾える |
| `DeviceError` の写し | `scaffold` は定数の文字列を持つバリアントを引数なしにする。元が `Internal("calendar save")` のように識別子で分岐しているなら、`error.rs` のバリアントを `&'static str` つきにして元の識別子を戻す。バリアントが 0 件の機能（pedometer・ar・call-ui）は `error.rs` を作らない |
| MainActivity に埋まった Android 実装（haptics・nlp・calendar） | 機能 crate に `android.rs` と `<Pascal>Helper.java` を新設する。状態を持たない同期呼び出しは Helper の static メソッドだけにし、provider は作らない。保留中の要求・ランチャーを持つ機能は `FeatureLifecycleProvider` の派生と `HELPERS`（`WeakHashMap`）で Activity ごとに引く。Helper が無いときは失敗で確定する（結果が来ないまま待たせない）。`MainActivity` のクラスを Java から直接参照しない（起動 Intent は `getLaunchIntentForPackage`） |
| 文字列資源（call-ui・live-activity） | 機能 crate の `android/res/values/` に移し、宣言の `res` に書く。アプリの `strings.xml` からは消す |
| Swift の薄層（live-activity） | `Cargo.toml` に `links = "fandhe_cap_<id>"` を置き、`build.rs` の最後で `export_link_args()` を呼ぶ。最終成果物（gallery）の `build.rs` が `import_link_args("fandhe_cap_<id>")` を feature `device` が有効なときだけ呼ぶ。ウィジェット拡張が共有する Swift ファイルは機能 crate へ移し、`platforms/apple/project.yml` のパスを直す |
| Activity 再生成時の破棄・終了処理 | `reset_pending_tracking` は機能 crate が `pub` で公開し、gallery の `android_main` が呼ぶ。`fandhe-egui-proxy` の `android.rs` からは消す |
| macOS の usage description（calendar） | 機能の宣言（`[package.metadata.fandhe.macos]`）にキーを書き、文言は gallery の `[package.metadata.fandhe-app.macos.usage-descriptions]` に置く |
| 同じ機能の Web 実装が使う純関数・定数 | 機能 crate の `contract.rs` へ置き、`fandhe-web-dom` が機能 crate に依存する（`share-receive` と同じ形。power-state・ar・nlp）。機能 crate は wasm32 でも通るよう、OS 別の依存は `target` で外す。Web 実装の本体は `fandhe-web-dom` に残す |
| 他機能も使う純粋なコード（ar） | §7 の規則どおり `fandhe-device` に残して機能 crate が依存する（`camera_preview_stream`・`compass`）。OS に触れない縮小や待ちの追跡は §8 の「型・検証」の扱い |
| 他機能の OS 実装を参照している機能の後回し | 未移行の機能がこの機能の OS 実装を使っている間は移さない。`fandhe-egui-proxy` から機能 crate への依存は作らない。参照が解消してから移す（第 2 束では photo-picker・image-recognition・url-scheme） |
| 他の束の機能 crate に依存する機能の後回し | 機能 crate から機能 crate への依存は、依存先が `main` に入ってから足す（第 2 束では geofence→location、background-task→local-storage-kv、map-view→network-status） |
| 共有の SAFETY コメント | 移した関数を根拠に指しているコメントは、unsafe の根拠が共通層（`fandhe_platform_android::with_activity`）へ移った場合に、そこを指す形へ直す |
| 複数の stem・id と stem が違う機能 | `scaffold` が使えない。`git mv` で手で移し、対応表の `impl` を新しいパスへ書き換える |
