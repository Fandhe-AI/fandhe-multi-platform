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

## 9. 第 3 束（#1188）で出た型

第 3 束（accessibility-prefs・multi-display・audio-devices・local-fonts・file-watch・on-device-llm・media-transcode・app-badge・midi・passkey-signin・screen-capture）で
はじめて出た型と、その扱いです。

| 型 | 扱い |
|----|------|
| `scaffold` の誤写像 | `scaffold` は元のコードが識別子（`MODEL_NOT_READY` など）で分岐していても `Internal` を引数なしにする箇所がある。移した後に `error.rs` の対象バリアントを `&'static str` つきへ戻し、引数なしの `Internal` には元の文字列を書く。`stage` のように呼び出し側が渡すラベルがあるバリアント（screen-capture）も同じ |
| `build.rs` の framework・弱リンク | `scaffold` が出す `compile_objc` の framework は仮の値。元の `fandhe-egui-proxy/build.rs` の呼び出しと照合して直す（audio-devices は CoreAudio・AVFoundation）。弱リンク（`weak_link_framework`）や OS ごとの分岐を元の関数が持っていたら移す。弱リンクと Swift の薄層は `links` + `export_link_args()` で最終成果物へ届け、`fandhe-gallery` の `build.rs` が `import_link_args` を足す。ホストの `cargo test` で同じ機能 crate を直接の依存にする別 crate（`fandhe-web-dom`）にも `build.rs` で `import_link_args` が要る |
| 共有の MP4・base64url など OS に触れない部品 | 他機能も使う純関数は `fandhe-device` の同名モジュールに最小限だけ残し、機能 crate が参照する（media-transcode の box 走査と `DURATION_TOLERANCE`、passkey-signin の base64url）。失敗は理由を持たない型にして、呼び出し側が自機能のエラーへ写す |
| 前面の復帰・離脱に参加する機能 | `FeatureLifecycleProvider` の `onHostResumed` / `onHostPaused`（screen-capture が使う）。どちらも `Activity.onResume()` / `onPause()` の中、つまり `super.onResume()` / `super.onPause()` の中で呼ばれる。従来の `MainActivity` は `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う |
| Activity に紐づく helper（app-badge・midi・passkey-signin・screen-capture） | Helper を `HELPERS`（`WeakHashMap`）で Activity ごとに引く。コンストラクタは private にし、Rust から呼ぶ入口は第 1 引数が `Activity` の static メソッドにする。helper が無いときは結果が来ないまま待たせず、失敗で確定する。`onDestroy` を持つ helper は provider の `onHostDestroyed` から `detach` 経由で呼ぶ |
| 1 つの Helper が Context を取る同期呼び出し | `read(Context)` を `read(Context activity)` にして、先頭で `activity.getApplicationContext()` を取る。`MainActivity` の 1 行ラッパーは消す |
| 前面サービス（screen-capture） | サービス本体を機能 crate の `android/java` へ移し、宣言の `components`（`kind = "service"`、`attributes = { foregroundServiceType = ... }`）と `permissions` に書く。Manifest の要素は宣言と完全に一致させる |
| Credential Manager など Gradle の依存（passkey-signin） | 宣言の `gradle` に書く。版は `scripts/android/versions.env` と一致させる（cargo-fandhe の突き合わせテストが確かめる） |
| 文字列資源（app-badge） | 機能 crate の `android/res/values/strings.xml` へ移し、宣言の `res` に書く。アプリの `strings.xml` からは消す |
| 共通層へ上げる小さな部品 | 複数の未移行機能が使う JNI 文字列の読み取り（`read_ble_string`）は `fandhe-platform-android::read_bounded_string` へ上げ、`fandhe-egui-proxy` の同名関数は委譲にする。フレームごとの経過時間の実測（`measure_dt_ms`）は `fandhe-runtime::tick` へ上げた |
| 経路が終了処理を持つ機能（midi） | `device::shutdown` が呼ぶ停止は `fandhe-egui-proxy` が機能 crate へ任意依存して呼ぶ（§7、#1194 で消える） |

### 後回しにした機能と理由

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| network-status | 未移行の `fandhe_apple_lan_discovery.m` が `fandhe_apple_network_status_read` を呼ぶ | lan-discovery と一緒に移す |
| device-info | 未移行の `apple_quick_settings_tile.rs` が `macos_device_info::implementation()` を呼ぶ | quick-settings-tile の後 |
| now-playing-controls | `media_playback`（egui-proxy・web-dom の両方）が `now_playing::sync*` を呼ぶ | media-playback と同じ PR で移す |
| pen-stylus | Android が `MainActivity.dispatchTouchEvent` の上書きを要する。provider では置き換えられない | Activity の入力を横取りする口の設計が先 |
| picture-in-picture | Android が `onPictureInPictureModeChanged` の上書きと `<activity supportsPictureInPicture>` 属性を要する | 宣言の拡張が先 |
| fullscreen-immersive | `FullscreenHelper.onInsets` が、`MainActivity` の唯一の decorView insets リスナー（safe-area と共用）から呼ばれる | insets を配る口を `fandhe-platform-android` に置く設計が先 |
| compose-mail-sms | 未移行の `windows_web_auth_session.rs` が `windows_compose_mail_sms::{open_url, SE_ERR_NOASSOC}` を呼ぶ | web-auth-session と一緒に移す、または `open_url` を `fandhe-platform-windows` へ上げる |
| print・print-screen | `print_screen_page_effect` が egui 固有の読み書き口（`EguiSenderExt` の `request_screenshot`・`tagged_screenshot`）を使う | `fandhe-runtime` に egui 非依存の口を足す設計が先 |
| in-app-purchase | 全 OS が未実装（`possible`）で `impl` が無い | 実装後 |

## 10. 第 4 束（#1189）で出た型と後回しの機能

第 4 束（20 機能）のうち、system-accent-color・focus-mode-status・usb-device・pdf-viewer・media-cast の 5 機能を移した。
残りは下の表の理由で止まっている。理由が解消してから移す。

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| `scaffold` が拾わない Android の Helper | Helper の名前が `<Pascal>Helper` でない機能（pdf-viewer の `PdfPageHelper`）は `git mv` されない。手で `android/java/` へ移し、対応表の `impl` も直す |
| `scaffold` が書く `build.rs` の framework | 元の `compile_*` の framework と必ず照合する（scaffold の既定は誤ることがある。usb-device は `Foundation` + `IOKit`、pdf-viewer は `CoreGraphics`、media-cast は iOS と macOS で別） |
| `Internal` が識別子を持つ機能（pdf-viewer） | §8 のとおり `error.rs` を `Internal(&'static str)` にし、`scaffold` が単位バリアントへ潰した呼び出し（`Internal("…")`）を元の文字列へ戻す。表示文言には出さない |
| `Internal` の文字列を捨てる機能（media-cast の Windows） | 画面もログも固定文言に畳むなら単位バリアントのまま、段の識別子の引数は `_` 始まりにして残す |
| edition 2024 の予約語 | `gen` を識別子に使う箇所（media-cast の世代）は `epoch` などへ変える。`#[no_mangle]` は `#[unsafe(no_mangle)]` |
| 同じ機能の Web 実装が参照する記号 | `fandhe-web-dom` が `fandhe_device::<stem>` を使っていたら、機能 crate への依存（wasm32 でも通る）に付け替える（usb-device・media-cast・system-accent-color）。コンパイルしないと気づけないため、移行のたびに `cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown --all-features` も通す |
| `fandhe-device` に残す共有の純関数・型 | 他機能が使う `device_info::clean_text`・`media_playback::demo_wav`・`Image` は `fandhe-device` に残し、機能 crate が依存する（§7） |
| 終了時の片付け（media-cast の `shutdown`） | §7 の過渡期の依存。`fandhe-egui-proxy` が feature `device` で optional 依存し、`device::shutdown` から `fandhe_cap_media_cast::shutdown()` を呼ぶ |
| Android 全体の型検査 | NDK が手元にあれば `ANDROID_NDK_HOME` と `CC_aarch64_linux_android`（NDK の clang）を設定して `cargo clippy -p <crate> --target aarch64-linux-android --all-features` で通る（`--all-targets` は既存のテストが通らない） |

### 後回しにした機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| background-download | 未移行の iOS AppDelegate（`ios/fandhe_ios_url_scheme.m`）が `fandhe_apple_bgdl_handle_events` を呼ぶ | iOS の AppDelegate イベントを配る口の設計が先、または url-scheme・background-upload と同時に移す |
| app-shortcuts | macOS の `.m` が egui-proxy の `fandhe_rust_url_scheme_received` を呼ぶ。Windows は `windows_url_scheme` と `windows_notification::lpwstr_variant` を使う。iOS は `fandhe_ios_scene.m` に埋まっている | url-scheme と同時に移す |
| file-association | `windows_url_scheme` の内部（`win::`・`map_registry_error`）を使う。iOS・macOS は url_scheme の `.m` が受け口 | url-scheme と同時に移す |
| spotlight-index | driver が `url_scheme::platform_url_scheme()` を使い、apple の `.m` が `fandhe_rust_url_scheme_received` を呼ぶ | url-scheme と同時に移す |
| barcode-scan | `photo_pick::implementation`・`ios::present_photo_picker`・`image_recognition::precheck` を使う。photo-picker 側も barcode_scan の参照で止まっていて循環している | photo-picker・image-recognition と同じ PR で移す |
| lan-discovery | apple の `.m` が `fandhe_apple_network_status_read` を、Windows が `windows_network_status` を使う。network-status も `map_view.rs` が OS 実装を使うため移せない | network-status・map-view と同時に移す |
| wifi-info | 未移行の wifi-connect の Windows 実装（`windows_wifi_connect.rs`）が `windows_wifi_info::radio_off` を使う | wifi-connect と同時に移す |
| local-peer-connect | `NEARBY_WIFI_DEVICES` の `usesPermissionFlags="neverForLocation"` を宣言の `Permission`（name・max-sdk のみ）で表せず、`cargo-fandhe` の `tests/gen.rs` が通らない | 宣言に権限フラグを足してから |
| web-auth-session | Android は `MainActivity.onNewIntent` が `WebAuthSessionHelper.onNewIntent` を先に呼び、url-scheme の受信箱へ流さない分岐を持つ（provider に onNewIntent の口が無い）。Windows は `windows_compose_mail_sms::{open_url, SE_ERR_NOASSOC}` を使う | Activity の Intent を配る口の設計が先。open_url は compose-mail-sms と同時に platform-windows へ上げる |
| payment-request | Google Pay の `<application>` 直下の `meta-data`（`com.google.android.gms.wallet.api.enabled`）を宣言で表せない（`meta-data` はコンポーネント単位だけ） | 宣言に application の meta-data を足してから |
| music-library | Android が許可ランチャーと `onDestroy` を持つ（provider と JNI エクスポートが要る）。Windows の `windows_photo_library::known_folder` を `fandhe-platform-windows` へ上げる共通層の変更も要る | 共通層の先行変更とあわせて別の周で |
| translation | Android がランチャーと `onDestroy`・`reset_pending_tracking` を持ち、Apple は Swift の薄層（`links` と `import_link_args`）が要る | 別の周で |
| flashlight | Android の Helper が `onStart` / `onStop` / `onDestroy` に参加する。`FeatureLifecycleProvider` に Started / Stopped の口を足す共通層の変更が先 | 共通層の先行変更とあわせて別の周で |
| safe-area-insets | egui-proxy の `android.rs` にある insets の static と JNI エクスポートを `fandhe-platform-android` へ上げる共通層の変更が先 | 共通層の先行変更とあわせて別の周で |
| compass-heading | 第 3 束 #1301 の `FeatureLifecycleProvider.onHostResumed` / `onHostPaused` に依存する。`WaitTracker`・`NO_READING_TIMEOUT_MS` を ar と未移行の `environment_sensors` が使うため、`fandhe-device::compass` へ最小限を残す設計も要る | #1301 のマージ後 |

## 11. 第 5 束（#1190）で出た型と後回しの機能

第 5 束（20 機能）のうち、in-app-review・uwb-ranging・user-idle・serial-port・app-attest・
per-app-language・screen-brightness・spell-check・geocoding-search の 9 機能を移した。
残りは下の表の理由で止まっている。理由が解消してから移す。

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| `scaffold` が書く `build.rs` の framework | 元の `compile_*` の framework と必ず照合する（scaffold の既定は誤る。in-app-review は `StoreKit`（iOS は `UIKit` も）、user-idle は `CoreGraphics`、serial-port は `IOKit`、geocoding-search は `CoreLocation` + `MapKit`、per-app-language は `Foundation` のみ）。宣言の `frameworks` には基盤を除いたものだけを書く |
| `scaffold` が拾わない Android の Helper | Helper の名前が `<Pascal>Helper` でない機能（geocoding-search の `GeocodingHelper`）は `git mv` されない。手で `android/java/` へ移し、対応表の `impl` も直す |
| Activity のインスタンスを持つ Helper（screen-brightness・spell-check・geocoding-search） | Helper を private コンストラクタ + static の入口（`set(Activity, …)` など）にし、Activity ごとの helper は `HELPERS`（`Collections.synchronizedMap(new WeakHashMap<>())`）に持つ。`FeatureLifecycleProvider` の派生（`<Name>Lifecycle`）が `onHostCreated` で `attach`、`onHostDestroyed` で `detach` する。Helper が無い（生成前・破棄後）要求は FAILED で確定し、結果が来ないまま待たせない。provider は宣言の `components` と `AndroidManifest.xml` に同じ名前・authorities で書く。Helper が Activity を持たない設計（geocoding-search）は、static の入口が `getApplicationContext()` を取り出して渡す |
| Helper の native コールバック（spell-check・geocoding-search） | Helper クラスの `native` と Rust の JNI エクスポート名（`Java_com_fandhe_platform_<Helper>_native…`）は移動で変わらない。Rust 側の呼び出しだけ `with_activity_class` + `call_static_method` に変え、JNI の署名は Java と Rust の 2 か所で照合する |
| Android の `unsafe` が `with_activity_class` へ移って残らない機能（app-attest・screen-brightness・per-app-language） | `lib.rs` の `cfg_attr` から Android を外す（app-attest は全 OS で `forbid(unsafe_code)`）。JNI エクスポート（`#[unsafe(no_mangle)]`）を持つ spell-check・geocoding-search は `deny` のまま |
| `Internal` / `Disabled` が識別子を持つ機能（spell-check・app-attest・geocoding-search） | §8 のとおり `error.rs` を `&'static str` つきにする。`scaffold` が単位バリアントへ潰した呼び出しは元の文字列へ戻す（テストが `Internal(_)` や識別子で照合するため、全 OS の呼び出しを元のソースと突き合わせる）。`Disabled`（`scaffold` が写さない）は手で足す |
| `<application>` の属性（per-app-language の `android:localeConfig`） | 宣言の `components` は子要素だけを表し、`<application>` の属性は表せない。アプリが対応する言語の一覧でもあるため、`AndroidManifest.xml` と `res/xml/locale_config.xml` に残す |
| iOS の Info.plist のキー（uwb-ranging） | `usage-descriptions` にキーを、`NSBonjourServices` は `info-plist` の配列で宣言する。説明文は gallery の `[package.metadata.fandhe-app.ios.usage-descriptions]` に置く。複数機能が共有するキー（`NSLocalNetworkUsageDescription`）は、最初に移した機能が宣言する |
| `fandhe-device` に残す純関数の公開（spell-check の `translation::valid_lang_tag`） | `pub(crate)` だった関数は `pub` にして機能 crate が依存する（§7）。複製して手で合わせない |
| 終了時の片付け（serial-port の `shutdown`） | §7 の過渡期の依存。`fandhe-egui-proxy` が feature `device` で optional 依存し、`device::shutdown` から `fandhe_cap_serial_port::shutdown()` を呼ぶ |
| Windows の `windows` クレートの feature | 移した実装が使う feature を機能 crate の target 依存へ書く（serial-port の `CreateFileW` は `Win32_Security` も要る）。`fandhe-egui-proxy` 側の feature は他機能が使うため消さない。`cargo clippy -p <crate> --target x86_64-pc-windows-gnu --all-targets` で拾える |
| Web 実装が参照する記号 | `fandhe-web-dom` の serial-port が `fandhe_device::serial_port` を使っていたため、機能 crate への依存に付け替えた（user-idle は参照が無く不要）。`cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown` で拾える |
| Android 全体の型検査 | `ANDROID_NDK_HOME` を設定して `cargo ndk -t arm64-v8a clippy -p <crate> --all-features -- -D warnings` が通る。Android 限定の unused import（app-attest の `JObject`）はホストでは拾えない |

### 後回しにした機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| in-app-browser | Android の Helper が `onResume` / `onPause` / `onDestroy` に参加する。`FeatureLifecycleProvider` に Resumed / Paused の口が無い（#1301 で足す） | #1301 のマージ後 |
| environment-sensors | Android が `onResume` / `onPause` / `onDestroy` に参加する（同上）。egui-proxy の `android.rs` にある feature 無効時の no-op スタブも消す必要がある | #1301 のマージ後 |
| capture-protection | Android の Helper が `onStart` / `onStop` に参加する。`FeatureLifecycleProvider` に Started / Stopped の口が無い | flashlight と同じ共通層の変更の後 |
| activity-recognition | Android が `onStart` / `onStop` に参加し、`ActivityRecognitionReceiver` も持つ | Started / Stopped の口の後 |
| bluetooth-classic | Android が `onResume` / `onPause` と許可ランチャーを持つ。共有ファイル `device/android.rs` の `read_ble_string` を使う | `read_ble_string` を共通層へ上げてから（bluetooth-le と同時が自然） |
| window-management | driver と OS 実装が `egui::Context`・`ViewportCommand` を直接使う（§1 の egui 固有の口） | `fandhe-runtime` にウィンドウ操作の egui 非依存の口を設計してから |
| screen-orientation | driver が `EguiSenderExt::viewport_size` を使う | 同上 |
| launch-at-login | Windows 実装が未移行の `windows_compose_mail_sms::open_url` を使う | compose-mail-sms と同時に `open_url` を `fandhe-platform-windows` へ上げる |
| autofill-otp | iOS の受け口 `fandhe_ios_text_input_set_one_time_code_hint` が、描画層の IME 橋渡し（`fandhe-egui-proxy/ios/fandhe_ios_text_input.m`）の入力欄 static に埋まっていて切り離せない | 入力欄のヒントを渡す口を描画層に設計してから |
| game-center | 全 OS が `possible` で実装も `impl` も無く、移すものが無い | 実装時に最初から crate で作る |
| app-intents | macOS の実装が url-scheme の受け口（`device/url_scheme.rs`・`FandheWidgetHost`）そのもので、独自のモジュールが無い | url-scheme と同時に扱う |
