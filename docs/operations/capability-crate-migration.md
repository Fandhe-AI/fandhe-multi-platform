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
| Windows・Web の実装がある機能 | `fandhe-platform-windows`（`open_url`・`SE_ERR_NOASSOC` を含む）/ `-web`（#1184・#1185）は済み。Windows は `src/windows.rs`（scaffold が移す）、Web は `fandhe-web-dom/src/<stem>.rs` を `src/web.rs` へ `git mv` して手で移す（下の「Web の移し方」）。`inventory` の注意は読み流してよい |
| driver が egui 固有の読み書き口（`fandhe_render_egui::EguiSenderExt`）を使う機能 | 機能 crate は egui 非依存のため、そのままは移せない。拡大率・フォーカス・ファイルのドラッグ・ポインタ押下は `sender.host_view()`（`fandhe_runtime::HostView`、§13）で読み書きできる。`HostView` に無い口（`viewport_size`・`request_screenshot` / `tagged_screenshot`・`egui_context`・`canvas_*`）を使う機能は、その口を設計してから移す |
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
| `driver.rs` | import を直し、入口を `pub fn platform_<stem>()`（stem は crate 名から作る。`--module` で違っても揃える）にして役割を doc に書く。`sync --check` が crate ルートからの公開を検査する。テストの `egui_sender` は `CompletionSender::new(tx, Arc::new(NoopWaker))` に直す（`fandhe-cap-camera-capture` のテストと同じ形） |
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
| ツール | `node --test scripts/device/migrate-capability.test.mjs`、`node scripts/device/migrate-capability.mjs sync --check`（全機能 crate の公開入口と一覧のずれ） |
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

`sync --check` は書き込まず、目印ブロックのずれと、公開入口 `pub fn platform_<stem>()` の欠けを検査します（ずれ・欠けがあれば終了コード 1）。
`sync` は目印が無いと失敗します。`tests/gen.rs`（cargo-fandhe）が `build.gradle.kts` の行を文字列で照合するため、glob にはしていません。

## 6. 並列で作業するときの注意

- 一覧の衝突は片側を取って `sync` をやり直す。`Cargo.lock` は取り直す（`cargo metadata` で再生成）
- 共有ファイル（`device.rs`・`fandhe-egui-proxy/build.rs`・`MainActivity.java`・`registry.rs`）は同じ箇所を触りやすい。
  該当行の削除だけに留め、整形や並べ替えを混ぜない
- 連番つきのファイルは作らない

## 7. 共有コードの過渡期の規則と Web の移し方（第 1 束 #1186）

- 未移行のモジュールが使い続ける記号（例: `fandhe_device::http::{check_url, MAX_BODY_BYTES}`）は `fandhe-device` に残し、
  機能 crate が `fandhe-device` に依存して参照する。`fandhe-device` から機能 crate へは依存させない
- 機能 crate の終了時の片付け・Android の画面ゲート・再生成時の破棄は、最終成果物（`fandhe-gallery`）が呼ぶ。
  `fandhe-egui-proxy` から機能 crate へは依存しない（`fandhe-egui-proxy::device::shutdown` は未移行の機能の停止だけを持つ）
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
| edition 2024 への移行で壊れる書き方 | `scaffold` が `gen` を `r#gen`（読みやすい名前へ変えてよい）、`extern "C" {}` を `unsafe extern "C" {}`、`#[no_mangle]` を `#[unsafe(no_mangle)]` へ直す。入れ子の `if` / `if let` は let チェーンへ（`cargo clippy --fix` が直す）。`scaffold` の後に `cargo clippy -p fandhe-cap-<id> --all-targets` を通すと拾える |
| `DeviceError` の写し | `scaffold` は機能名か `_` だけを渡すバリアントを引数なしにし、`Internal("calendar save")`・`Internal(MODEL_NOT_READY)`・`Internal(stage)` のように機能名以外を渡すバリアントは `&'static str` つきで出して引数を残す。`Disabled` などの未写像バリアントは手で足す。バリアントが 0 件の機能（pedometer・ar・call-ui）は `error.rs` を作らない |
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
| `build.rs` の framework・弱リンク | `scaffold` が出す `compile_objc` の framework は仮の値。元の `fandhe-egui-proxy/build.rs` の呼び出しと照合して直す（audio-devices は CoreAudio・AVFoundation）。弱リンク（`weak_link_framework`）や OS ごとの分岐を元の関数が持っていたら移す。弱リンクと Swift の薄層は `links` + `export_link_args()` で最終成果物へ届け、`fandhe-gallery` の `build.rs` が `import_link_args` を足す。ホストの `cargo test` で同じ機能 crate を直接の依存にする別 crate（`fandhe-web-dom`）にも `build.rs` で `import_link_args` が要る |
| 共有の MP4・base64url など OS に触れない部品 | 他機能も使う純関数は `fandhe-device` の同名モジュールに最小限だけ残し、機能 crate が参照する（media-transcode の box 走査と `DURATION_TOLERANCE`、passkey-signin の base64url）。失敗は理由を持たない型にして、呼び出し側が自機能のエラーへ写す |
| 前面の復帰・離脱に参加する機能 | `FeatureLifecycleProvider` の `onHostResumed` / `onHostPaused`（screen-capture が使う）。どちらも `Activity.onResume()` / `onPause()` の中、つまり `super.onResume()` / `super.onPause()` の中で呼ばれる。従来の `MainActivity` は `super` の後で helper を呼んでいたため、`super` に対する前後だけが違う |
| Activity に紐づく helper（app-badge・midi・passkey-signin・screen-capture） | Helper を `HELPERS`（`WeakHashMap`）で Activity ごとに引く。コンストラクタは private にし、Rust から呼ぶ入口は第 1 引数が `Activity` の static メソッドにする。helper が無いときは結果が来ないまま待たせず、失敗で確定する。`onDestroy` を持つ helper は provider の `onHostDestroyed` から `detach` 経由で呼ぶ |
| 1 つの Helper が Context を取る同期呼び出し | `read(Context)` を `read(Context activity)` にして、先頭で `activity.getApplicationContext()` を取る。`MainActivity` の 1 行ラッパーは消す |
| 前面サービス（screen-capture） | サービス本体を機能 crate の `android/java` へ移し、宣言の `components`（`kind = "service"`、`attributes = { foregroundServiceType = ... }`）と `permissions` に書く。Manifest の要素は宣言と完全に一致させる |
| Credential Manager など Gradle の依存（passkey-signin） | 宣言の `gradle` に書く。版は `scripts/android/versions.env` と一致させる（cargo-fandhe の突き合わせテストが確かめる） |
| 文字列資源（app-badge） | 機能 crate の `android/res/values/strings.xml` へ移し、宣言の `res` に書く。アプリの `strings.xml` からは消す |
| 共通層へ上げる小さな部品 | 複数の未移行機能が使う JNI 文字列の読み取り（`read_ble_string`）は `fandhe-platform-android::read_bounded_string` へ上げ、`fandhe-egui-proxy` の同名関数は委譲にする。フレームごとの経過時間の実測（`measure_dt_ms`）は `fandhe-runtime::tick` へ上げた |
| 経路が終了処理を持つ機能（midi） | 終了時の停止は `fandhe-gallery` の終了フックが呼ぶ（§7） |

### 後回しにした機能と理由

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| network-status | 未移行の `fandhe_apple_lan_discovery.m` が `fandhe_apple_network_status_read` を呼ぶ | lan-discovery と一緒に移す |
| device-info | 未移行の `apple_quick_settings_tile.rs` が `macos_device_info::implementation()` を呼ぶ | quick-settings-tile の後 |
| now-playing-controls | `media_playback`（egui-proxy・web-dom の両方）が `now_playing::sync*` を呼ぶ | media-playback と同じ PR で移す |
| pen-stylus | Android が `MainActivity.dispatchTouchEvent` の上書きを要する。provider では置き換えられない | §12 の `Window.Callback` を包む型で移せる（`dispatchTouchEvent` の観測。共通層への引き上げはそのとき行う） |
| picture-in-picture | Android が `onPictureInPictureModeChanged` の上書きと `<activity supportsPictureInPicture>` 属性を要する | 宣言の拡張が先 |
| fullscreen-immersive | `FullscreenHelper.onInsets` が、`MainActivity` の唯一の decorView insets リスナー（safe-area と共用）から呼ばれる | insets を配る口を `fandhe-platform-android` に置く設計が先 |
| compose-mail-sms | 未移行の `windows_web_auth_session.rs` が `windows_compose_mail_sms::{open_url, SE_ERR_NOASSOC}` を呼ぶ | `open_url` は #1192 で `fandhe-platform-windows` へ上げた（`windows_compose_mail_sms` は再公開だけを持つ）。web-auth-session と同時に移すか、その再公開を共通層への参照に付け替えて移す （第 8 束 #1193 で移した） |
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
| `Internal` が識別子を持つ機能（pdf-viewer） | `scaffold` が `error.rs` を `Internal(&'static str)` で出し、呼び出しの引数も残す。表示文言には出さない |
| `Internal` の文字列を捨てる機能（media-cast の Windows） | 画面もログも固定文言に畳むなら単位バリアントのまま、段の識別子の引数は `_` 始まりにして残す |
| 同じ機能の Web 実装が参照する記号 | `fandhe-web-dom` が `fandhe_device::<stem>` を使っていたら、機能 crate への依存（wasm32 でも通る）に付け替える（usb-device・media-cast・system-accent-color）。コンパイルしないと気づけないため、移行のたびに `cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown --all-features` も通す |
| `fandhe-device` に残す共有の純関数・型 | 他機能が使う `device_info::clean_text`・`media_playback::demo_wav`・`Image` は `fandhe-device` に残し、機能 crate が依存する（§7） |
| 終了時の片付け（media-cast の `shutdown`） | §7 の規則。`fandhe-gallery` の終了フックが `fandhe_cap_media_cast::shutdown()` を呼ぶ |
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
| wifi-info | 未移行の wifi-connect の Windows 実装（`windows_wifi_connect.rs`）が `windows_wifi_info::radio_off` を使う | wifi-connect と同時に移す（第 9 束 #1329 で移した） |
| local-peer-connect | `NEARBY_WIFI_DEVICES` の `usesPermissionFlags="neverForLocation"` を宣言の `Permission`（name・max-sdk のみ）で表せず、`cargo-fandhe` の `tests/gen.rs` が通らない | 宣言に権限フラグを足してから |
| web-auth-session | Android は `MainActivity.onNewIntent` が `WebAuthSessionHelper.onNewIntent` を先に呼び、url-scheme の受信箱へ流さない分岐を持つ（provider に onNewIntent の口が無い）。Windows は `windows_compose_mail_sms::{open_url, SE_ERR_NOASSOC}` を使う | Activity の Intent を配る口の設計が先。`open_url` は #1192 で `fandhe-platform-windows` へ上げた |
| payment-request | Google Pay の `<application>` 直下の `meta-data`（`com.google.android.gms.wallet.api.enabled`）を宣言で表せない（`meta-data` はコンポーネント単位だけ） | 宣言に application の meta-data を足してから |
| music-library | Android が許可ランチャーと `onDestroy` を持つ（provider と JNI エクスポートが要る）。Windows の `windows_photo_library::known_folder` を `fandhe-platform-windows` へ上げる共通層の変更も要る | 共通層の先行変更とあわせて別の周で |
| translation | Android がランチャーと `onDestroy`・`reset_pending_tracking` を持ち、Apple は Swift の薄層（`links` と `import_link_args`）が要る | 別の周で |
| flashlight | Android の Helper が `onStart` / `onStop` / `onDestroy` に参加する。`FeatureLifecycleProvider` の Started / Stopped の口は #1192 で足した | 口が `main` に入った後に移せる（app-lifecycle と同じ形）（第 9 束 #1329 で移した） |
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
| `Internal` / `Disabled` が識別子を持つ機能（spell-check・app-attest・geocoding-search） | `scaffold` が `error.rs` を `&'static str` つきで出す。テストが `Internal(_)` や識別子で照合するため、全 OS の呼び出しを元のソースと突き合わせる。`Disabled`（`scaffold` が写さない）は手で足す |
| `<application>` の属性（per-app-language の `android:localeConfig`） | 宣言の `components` は子要素だけを表し、`<application>` の属性は表せない。アプリが対応する言語の一覧でもあるため、`AndroidManifest.xml` と `res/xml/locale_config.xml` に残す |
| iOS の Info.plist のキー（uwb-ranging） | `usage-descriptions` にキーを、`NSBonjourServices` は `info-plist` の配列で宣言する。説明文は gallery の `[package.metadata.fandhe-app.ios.usage-descriptions]` に置く。複数機能が共有するキー（`NSLocalNetworkUsageDescription`）は、最初に移した機能が宣言する |
| `fandhe-device` に残す純関数の公開（spell-check の `translation::valid_lang_tag`） | `pub(crate)` だった関数は `pub` にして機能 crate が依存する（§7）。複製して手で合わせない |
| 終了時の片付け（serial-port の `shutdown`） | §7 の規則。`fandhe-gallery` の終了フックが `fandhe_cap_serial_port::shutdown()` を呼ぶ |
| Windows の `windows` クレートの feature | 移した実装が使う feature を機能 crate の target 依存へ書く（serial-port の `CreateFileW` は `Win32_Security` も要る）。`fandhe-egui-proxy` 側の feature は他機能が使うため消さない。`cargo clippy -p <crate> --target x86_64-pc-windows-gnu --all-targets` で拾える |
| Web 実装が参照する記号 | `fandhe-web-dom` の serial-port が `fandhe_device::serial_port` を使っていたため、機能 crate への依存に付け替えた（user-idle は参照が無く不要）。`cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown` で拾える |
| Android 全体の型検査 | `ANDROID_NDK_HOME` を設定して `cargo ndk -t arm64-v8a clippy -p <crate> --all-features -- -D warnings` が通る。Android 限定の unused import（app-attest の `JObject`）はホストでは拾えない |

### 後回しにした機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| in-app-browser | Android の Helper が `onResume` / `onPause` / `onDestroy` に参加する。`FeatureLifecycleProvider` に Resumed / Paused の口が無い（#1301 で足す） | #1301 のマージ後（第 9 束 #1329 で移した） |
| environment-sensors | Android が `onResume` / `onPause` / `onDestroy` に参加する（同上）。egui-proxy の `android.rs` にある feature 無効時の no-op スタブも消す必要がある | #1301 のマージ後 |
| capture-protection | Android の Helper が `onStart` / `onStop` に参加する。`FeatureLifecycleProvider` の Started / Stopped の口は #1192 で足した | 口が `main` に入った後に移せる（第 9 束 #1329 で移した） |
| activity-recognition | Android が `onStart` / `onStop` に参加し、`ActivityRecognitionReceiver` も持つ（口は #1192 で足した） | 口が `main` に入った後に移せる（第 9 束 #1329 で移した） |
| bluetooth-classic | Android が `onResume` / `onPause` と許可ランチャーを持つ。共有ファイル `device/android.rs` の `read_ble_string` を使う | `read_ble_string` を共通層へ上げてから（bluetooth-le と同時が自然） |
| window-management | driver と OS 実装が `egui::Context`・`ViewportCommand` を直接使う（§1 の egui 固有の口） | `fandhe-runtime` にウィンドウ操作の egui 非依存の口を設計してから |
| screen-orientation | driver が `EguiSenderExt::viewport_size` を使う | 同上 |
| launch-at-login | Windows 実装が未移行の `windows_compose_mail_sms::open_url` を使う（`open_url` は #1192 で `fandhe-platform-windows` へ上げた） | 再公開を共通層への参照に付け替えて移せる （第 8 束 #1193 で障害が解消。次の周で移せる）（第 9 束 #1329 で移した） |
| autofill-otp | iOS の受け口 `fandhe_ios_text_input_set_one_time_code_hint` が、描画層の IME 橋渡し（`fandhe-egui-proxy/ios/fandhe_ios_text_input.m`）の入力欄 static に埋まっていて切り離せない | 入力欄のヒントを渡す口を描画層に設計してから |
| game-center | 全 OS が `possible` で実装も `impl` も無く、移すものが無い | 実装時に最初から crate で作る |
| app-intents | macOS の実装が url-scheme の受け口（`device/url_scheme.rs`・`FandheWidgetHost`）そのもので、独自のモジュールが無い | url-scheme と同時に扱う |

## 12. ゲームパッド（#1303）で出た型

Android のゲームパッド入力は利用者の `MainActivity` の `dispatchKeyEvent` / `dispatchGenericMotionEvent` の上書きで受けていた。
`FeatureLifecycleProvider` では受けられないため、扱いを決めたうえで移した。

| 型 | 扱い |
|----|------|
| Activity の入力の観測 | provider（`GamepadLifecycle`）が `onHostCreated` で `Window.Callback` を包み、通知してから元の戻り値をそのまま返す（消費しない）。`Window.Callback` の全メソッドを明示的に委譲し、API 24 以降の default メソッドも飛ばさない。`getDecorView()` は呼ばない。`gen` で `MainActivity` へ挿入する案と手順書で案内する案は採らない（`consumer-app-structure.md` §4） |
| 守らない範囲 | 起動後に `getWindow().setCallback()` で callback を差し替えると入力が届かなくなる。接続状態の監視（`InputManager`）は影響を受けない。`crate` doc と対応表の `note` に書く |
| `MainActivity` 由来の JNI エクスポート名 | `Java_com_fandhe_platform_MainActivity_native…` を `Java_com_fandhe_platform_GamepadHelper_native…` に付け替え、Java の native 宣言（`GamepadHelper`）と署名を 2 か所で照合する。Rust から Java は呼ばないため `with_activity_class` は要らない |
| feature 無効時の no-op スタブ | 旧 JNI 名のスタブを egui-proxy から消し、`fandhe-gallery` に新しい名前で置く（share-receive と同じ。provider は feature を知らず native を呼ぶため、実体が無いと `UnsatisfiedLinkError` になる） |
| 画面ゲート | `ACTIVE_SCREEN_USES_GAMEPAD` を機能 crate の static へ移し、`set_screen_active(bool)` で公開する。`fandhe-gallery` の `android_main` が呼ぶ |
| Web | 実装本体は `fandhe-web-dom` に残し、契約と `GamepadError`（識別子つき）だけ機能 crate から使う。永続的な非対応と非標準配置の区別は識別子で保つ |

## 13. 描画層の口 `HostView`（#1302）で出た型

dynamic-type・share-sheet・drag-drop の 3 機能を移した。3 機能とも driver が egui 固有の読み書き口を使っていたため、
先に `fandhe-runtime` へ描画層の口を足した。

### 口の設計

| 項目 | 内容 |
|------|------|
| 型 | `fandhe_runtime::HostView`。メソッドはすべて既定の実装を持つ（`focused` / `file_drag` / `pointer_primary_down` / `set_zoom_factor` / `applied_zoom_factor`） |
| 既定値 | 描画層が観測できない場合は「観測していない」側へ倒す（`focused` は `true`、`file_drag` は空、`set_zoom_factor` は何もしない）。既定値だけで成功の表示や権限の行使を起こさない |
| 届け方 | `RepaintWaker::host_view`（既定実装つき）を `CompletionSender::host_view` で effect へ渡す。`EffectFn` の型も、既存の `NoopWaker` も変えない |
| `Renderer` との関係 | `Renderer` には載せない。`Renderer::render` は UI スレッドで画面を描く契約で、effect からは見えないため。`RepaintWaker` と同じく、レンダラ実装が提供する別の契約として扱う |
| 呼んでよい時点 | `EffectFn` と同じ、描画ループのフレーム末尾（UI スレッド）だけ |
| egui 実装 | `fandhe-render-egui` の `EguiWaker`。適用済みの拡大率は `egui::Context` 単位に持つ（プロセス全体の static にしない） |

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| driver のテスト | `egui_sender` の代わりに、`HostView` を実装した起床口（`Mutex<Option<f32>>` で拡大率を記録する等）で `CompletionSender::new` する。描画コンテキスト単位の分離（PR #920）は `fandhe-render-egui` のテストで固定する |
| 契約を `fandhe-device` に残す機能（drag-drop） | `file-association` が `fandhe_device::drag_drop` を使うため、契約は動かさず機能 crate が `fandhe-device` に依存する（§7）。`scaffold` は契約も `git mv` するので使わず、手で移す |
| 複数 stem の機能（drag-drop） | `drag_drop`・`drop_session`・`ios_drag_drop` を手で移す。集約器 `drop_session` は Android の JNI 受け口と iOS が共有する |
| Android の画面の表示状態と再生成時の破棄（drag-drop） | `fandhe-gallery` の `android_main` が `fandhe-egui-proxy` の `android::requested_app_name` で画面名を決める。機能 crate が `set_screen_active` / `reset_sessions` を `pub` で公開し、`fandhe-gallery` が呼ぶ（§7）。JNI の `nativeDragDropIsActive` は機能 crate の Helper 側へ移した |
| DecorView への取り付け（drag-drop） | `onHostCreated` で `getDecorView()` を呼ぶと、後続の `requestWindowFeature` が使えなくなるおそれがある。`onHostResumed` で取り付ける（同じリスナーの再設定は無害） |
| Activity を跨いで生きる受信器（share-sheet） | Application Context へ登録する static の受信器と要求 id は、`MainActivity` のときと同じく static に置く。ランチャーだけ Activity ごとの Helper に持つ |
| 複製していた検証規則（drag-drop） | SAF の表示名の検証 `sanitize_picked_name` はファイル選択と drag-drop が使うため、`fandhe-device::file_picker` へ上げて共有する |
| `scaffold` が書く `build.rs` | 元の framework と照合する（dynamic-type は iOS だけ `UIKit`、share-sheet は `ui_framework()` だけ、drag-drop は iOS だけ `UIKit` + `UniformTypeIdentifiers`） |
| 旧受け口の取りこぼし | `device/android.rs` のような共有ファイルに残った JNI 受け口は、`cargo ndk clippy -p fandhe-egui-proxy` で拾える（ホストの検査では拾えない） |

## 14. 第 7 束（#1192）で出た型と後回しの機能

第 7 束（20 機能）のうち、app-progress-attention・human-presence・system-sound・managed-app-config・display-refresh-hdr・
removable-storage・memory-pressure・audio-session・external-app-launch・open-system-settings・color-picker・digital-credentials・
wallpaper-set・file-preview・app-lifecycle の 15 機能と、WinRT 完了の積み込みを共通層へ上げた後の video-capture を移した。
device-identifier は第 1 束で移行済みで、本束では確かめただけ（`crates/fandhe-cap-device-identifier`、対応表の `impl` も新しいパス）。
残りの 3 機能は下の表の理由で止まっている。

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| 共通層への引き上げ（`open_url`） | 複数の未移行機能が使う Windows の `ShellExecuteW`（`open_url`・`SE_ERR_NOASSOC`）を `fandhe-platform-windows` へ上げた。渡すのは各機能が組み立てた固定 scheme の URL だけで、任意の scheme・引数を受ける口ではない。`windows_compose_mail_sms` は再公開だけを持ち、未移行の web-auth-session・launch-at-login の参照は変えない。`SE_ERR_NOASSOC` はホストのテストでも写像を検証するため、Windows 以外では同じ値（公式の表）を置く |
| `FeatureLifecycleProvider` の `onHostStarted` / `onHostStopped` | app-lifecycle が使う。どちらも `Activity.onStart()` / `onStop()` の中、つまり `super.onStart()` / `super.onStop()` の中で呼ばれる。従来の `MainActivity.onStart` は `super` の後で、`onStop` は `super` の前に helper を呼んでいた。`super` に対する前後だけが違い、`isChangingConfigurations()` の判定は同じコールバックの内側で行うので変わらない。到着順の扱い（契約表 1）は購読の id 照合と、通知時の実際の状態の読み直しで保つ |
| WinRT 完了の積み込み（video-capture） | `IAsyncOperation` / `IAsyncAction` の完了を命令へ包んでワーカーのキューへ積む `spawn` / `spawn_action` を、camera-preview-stream の Windows 実装から `fandhe_platform_windows::winrt_async` へ上げた（先行コミット）。待たずに `SetCompleted` から積む形（契約表 11）は変えない。camera-preview-stream の参照も共通層へ付け替えた |
| 機能専用の FileProvider（file-preview・video-capture） | 共有の `${applicationId}.compose`（メール添付・動画の撮影と共用）から外し、`FilePreviewFileProvider`（`FileProvider` の空の派生）と `res/xml/file_preview_file_paths.xml`（`cache/file_preview/` だけ）を機能 crate が持つ。宣言の `components`（`exported = false`・`grantUriPermissions = "true"`・`meta-data`）と `AndroidManifest.xml` に同じ内容を書く。公開するパスが狭まる（最小公開）。新しい権限は足さない。共有 provider の公開パスの一覧からは `file_preview` を消す。video-capture も同じ形（`VideoCaptureFileProvider`・`cache/video_capture/`・authority `${applicationId}.fandhe.video-capture.files`）で、共有 compose provider の公開パスから `video_capture` を消した（メール添付だけが残る） |
| Context を取る static の入口（managed-app-config・wallpaper-set） | `read(Context)` / `set(Context, …)` を、第 1 引数が `Activity` の static にして先頭で `getApplicationContext()` を取る（§9 の「1 つの Helper が Context を取る同期呼び出し」と同じ）。`MainActivity` の 1 行ラッパーは消す |
| 起動を依頼する Helper（external-app-launch・open-system-settings） | Activity ごとの helper を `HELPERS` に持ち、static の入口 `launch(Activity, …)` / `open(Activity, …)` が `boolean` を返す。表に無ければ `false` を返し、Rust が失敗として確定する。起動直前に RESUMED・`isFinishing` を見直す（契約表 2）処理は変えない |
| `onDestroy` を持つ Helper（system-sound・removable-storage・memory-pressure・audio-session） | §8 の型（private コンストラクタ・`HELPERS`・`<Name>Lifecycle`）。インスタンスメソッドは `private` にして、static の入口が表から引いて委譲する。helper が無い要求は失敗で確定する（stop は購読が無いので `0`）。provider は宣言の `components` と `AndroidManifest.xml` に同じ name・authorities（`${applicationId}.fandhe.<id>`）で書く |
| 見本画像の扱い（wallpaper-set） | 未移行の photo-save の `pub(crate)` の `sample_png()` は機能 crate から見えないため、driver に同じ生成を持ち（`image` の `png` だけ）、`// ponytail:` で「photo-save を移すときに共有を見直す」と書く。`PngImage`・`FsStorage` は他機能も使うため `fandhe-device` に残して参照する（§7） |
| Web の参照の付け替え（display-refresh-hdr・memory-pressure・external-app-launch・color-picker・digital-credentials・app-lifecycle） | 本体は `fandhe-web-dom` に残し、`fandhe_device::<stem>` の参照を機能 crate への依存に付け替える。契約の関数が機能固有のエラー型を返すようになるため、Web 側のエラー型の写像も付け替える（color-picker・digital-credentials）。`cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown --all-features` で拾える |
| 受け口を持つ driver（removable-storage・memory-pressure・app-lifecycle） | `.m` が呼ぶ `#[unsafe(no_mangle)] extern "C"` の受け口は driver に残る。`scaffold` は driver の `unsafe` を OS 実装のファイルへ写せず止まるため、受け口の属性を一時的に外して実行し、後で戻す。`lib.rs` の cfg（`any(target_os, test)`）は元の `device.rs` の宣言から写す（共有ファイルを先に直すと読めなくなる） |
| 公開入口のない機能（digital-credentials） | ネイティブ OS は全て `Unsupported` で OS 実装が無い。`platform_digital_credentials()` が同期で `Unsupported` を返し、effect はそれを呼ぶ |
| 終了時の片付け（memory-pressure・audio-session） | §7 の規則。`fandhe-gallery` の終了フック（と `ProxyApp` の破棄）が `shutdown()` を呼ぶ |
| `reset_pending_tracking` | audio-session・external-app-launch・open-system-settings・wallpaper-set・file-preview は機能 crate が `pub` で公開し、`fandhe-gallery` の `android_main` が呼ぶ。`fandhe-egui-proxy` の `android.rs` からは消す |
| `windows` の feature | 移した実装が使う feature を機能 crate の target 依存へ書く（file-preview は `std`、memory-pressure は `Win32_Security`、external-app-launch は `Win32_System_Registry` も要る）。`cargo clippy -p <crate> --target x86_64-pc-windows-gnu --all-targets` で拾える |

### 後回しにした機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| age-range | 全 OS が `possible` で実装も `impl` も無く、移すものが無い（game-center と同じ扱い） | 実装するときに最初から crate で作る |
| background-upload | 未移行の iOS AppDelegate（`ios/fandhe_ios_url_scheme.m` の `handleEventsForBackgroundURLSession`）が転送先として `fandhe_apple_background_upload.m` に結びついている（background-download と同じ止まり方） | AppDelegate のイベントを配る口を設計した後、または url-scheme・background-download と同時に移す |
| motion-raw-sensors | (1) Windows 実装が未移行の `windows_environment_sensors::ensure_minimize_watcher` を使い、ウォッチャー側も `windows_motion_raw_sensors::note_minimized` / `note_restored` を呼び返していて、双方向に結びついている。(2) iOS の `ios/fandhe_ios_motion_raw_sensors.m` が、未移行の tilt-motion の `ios/fandhe_ios_motion.m` が定義する共有の `CMMotionManager`（`fandhe_ios_shared_motion_manager` など）を extern で使う | (1) environment-sensors と同じ PR で移す。または最小化の通知を配る口を `fandhe-platform-windows` に置いてから。(2) tilt-motion・environment-sensors と同じ PR で移す。manager を `fandhe-platform-apple` へ上げると CoreMotion が全アプリにリンクされ、未選択機能の混入検査（#1178）の趣旨に反するため上げない |
## 14. 第 6 束（#1191）で出た型と後回しの機能

第 6 束（20 機能）のうち、sound-classification・companion-device・smart-home・eye-dropper・global-hotkey・
wifi-connect・volume-control・sleep-lock-events の 8 機能を移した。残りは下の表の理由で止まっている。
理由が解消してから移す。

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| 他機能と共有する純関数の引き上げ（wifi-connect） | Windows の電源判定 `radio_off` と定数は wifi-info の Windows 実装も使う。複製せず `fandhe-device::wifi_info` へ `pub` で上げ、テストも一緒に移した（先行コミット）。wifi-info の移行を妨げない |
| 契約を `fandhe-device` に残す機能（eye-dropper） | color-picker（未移行）と Web 実装（`fandhe-web-dom`）が `PickedColor` などを使うため、契約は動かさない。`scaffold` は契約も `git mv` するので使わず、driver・macOS 実装・`.m` だけ手で移す（§13 の drag-drop と同じ）。macOS の `NSColorSampler` は非同期で trait を通らないため、`platform_eye_dropper()` は常に `UnsupportedEyeDropper` を返し、doc に書いた |
| driver が持つ `extern "C"` の受け口（sleep-lock-events） | `scaffold` は driver に `unsafe` があると止まる。受け口を OS 実装（macOS）のファイルへ移し、driver は `pub(crate)` の `fire` を呼ぶだけにしてから移す |
| 状態を持たない Helper は Context を取る static に（volume-control） | 状態もライフサイクルも持たない同期呼び出しは provider を作らず、`read(Context)` / `adjust(Context, int)` にして `MainActivity` の 1 行ラッパーを消す（§9）。Android の `unsafe` が `with_activity_class` へ移って残らないため、`lib.rs` の Android を `forbid` へ戻した |
| 購読・保留要求を持つ Helper（sleep-lock-events・wifi-connect） | `HELPERS`（`WeakHashMap`）と `<Name>Lifecycle` の組にする。Helper が無いとき、`start` は失敗コード、`connect` は `nativeOn…Result` へ FAILED を返して確定する。状態を読むだけの `read` は Helper を要さないので `Context` を取る static にする |
| 結果ランチャーを持つ Helper（companion-device） | ランチャーは `onHostCreated`（STARTED より前）で登録する。`pick` は `boolean` を返し、Helper が無ければ false を返して Rust が `Internal` で確定する（結果の native を呼ばない）。`reset_pending_tracking` は機能 crate が `pub` で公開し、`fandhe-gallery` の `android_main` が呼ぶ |
| 共有ファイルの JNI 補助（companion-device） | 機器名の読み取り `read_ble_string` は `fandhe_platform_android::read_bounded_string` に置き換える（§9） |
| Windows の `windows` クレートの feature | 移した実装が使う feature を機能 crate の target 依存へ書く。ホストの検査では拾えないため、`cargo clippy -p <crate> --all-targets --target x86_64-pc-windows-gnu` で確かめる（global-hotkey は `Win32_System_Threading`、sleep-lock-events は `Win32_Graphics_Gdi`、wifi-connect は `Win32_NetworkManagement_Ndis` が要った） |
| `Disabled` が識別子を持つ機能（wifi-connect） | `error.rs` に `Disabled(&'static str)` を手で足した。`Io` は driver のテストでしか使っていなかったため、`Internal("other")` のテストに置き換えた |
| iOS・macOS の usage description | 説明文は `fandhe-gallery` の `[package.metadata.fandhe-app.*.usage-descriptions]` に置く。iOS は `cargo-fandhe` の `tests/gen.rs` が現在の `Info.plist` と一致を確かめるため、既存の文言と同じにする（マイクは「マイク録音」「音声認識」「動画の撮影」） |
| Manifest の既存の権限行 | 機能 crate の `permissions`・`features` に宣言を足しても、`AndroidManifest.xml` の既存の `uses-permission` は消さない（他機能と共有する行があるため。app-badge の `POST_NOTIFICATIONS` と同じ） |
| SAFETY コメントの指し先 | 移した関数（`android_sleep_lock_events.rs::call_activity`）を根拠にしていた他モジュールのコメントは、元の根拠 `fandhe_platform_android::with_activity` を指す形へ直した |

### 後回しにした機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| wallet-pass・cloud-kv-sync・cloud-db-sync | 全 OS が `possible` で実装も `impl` も無く、移すものが無い | 実装時に最初から crate で作る |
| quick-settings-tile | macOS 実装が未移行の `macos_device_info::implementation()` で OS の版を読む。device-info は §9 で「quick-settings-tile の後」とされており、互いに待っている | device-info と同じ PR で 2 機能を移す（または OS の版の読み取りを `fandhe-platform-apple` へ上げる） |
| alarm | Windows 実装が未移行の `windows_notification::{ensure_app_aumid, setting_result_for, APP_AUMID}` を使う | notification と同時に移すか、AUMID の登録を `fandhe-platform-windows` へ上げてから |
| foldable-posture | Android の Helper が `onStart` / `onStop` に参加する。`FeatureLifecycleProvider` に Started / Stopped の口が無い | flashlight・capture-protection と同じ共通層の変更の後（第 9 束 #1329 で移した） |
| ble-peripheral | Android の Helper が `onStop` に参加する（同上）。`reset_pending_tracking` と `device::shutdown` もある | Started / Stopped の口の後 |
| health-data | `HealthPermissionsRationaleActivity` を指す `<activity-alias>` を、宣言の `ComponentKind`（activity / service / receiver / provider）で表せない | 宣言に activity-alias を足してから |
| car-display | `<application>` 直下の `meta-data` 2 件を宣言で表せない（payment-request と同じ） | 宣言に application の meta-data を足してから |
| deep-link | Android の振り分けが `MainActivity.deliverUrlIntent`（url-scheme の受け口）に埋まっている。`url_scheme::UrlInbox` も使う | url-scheme と同時に移す（Intent を配る口の設計が先） |
| camera-preview-stream | 未移行の video-capture の Windows 実装が `windows_camera_preview_stream::{spawn, spawn_action}` を使う | 第 7 束（#1192）で `spawn` が共通層（`fandhe_platform_windows::winrt_async`）へ上がり、障害が解消した。次の周で移せる（第 9 束 #1329 で移した） |
| open-in-shell | 未移行の recent-documents が driver の `open_in_shell::prepare_sample_file_in` と、Windows の `windows_open_in_shell::to_win32_path` を使う | recent-documents と同じ PR で移す （第 8 束 #1193 で recent-documents と同時に移した） |

wifi-info（§10 の後回し）は、wifi-connect の移行で `radio_off` が `fandhe-device::wifi_info` へ上がったため、
参照の障害が解消した。次の周で移せる。

## 15. 第 8 束（#1193）で出た型と後回しの機能

第 8 束（19 機能）のうち、radio-control・image-generation・file-provider・video-transcode・nfc・scanner-devices・
open-in-shell・recent-documents・compose-mail-sms（compose-mail-attachment を含む）・clipboard の 10 crate（11 機能）を移した。
ar-camera-view は iOS の実装が `fandhe-cap-ar` にあるため新しい crate を作らず、対応表の `impl` を `fandhe-cap-ar` のパスへ直した。
残りは下の表の理由で移していない。

### 手作業で気づいた型

| 型 | 扱い |
|----|------|
| 2 つの id を 1 つの crate にまとめる（compose-mail-sms・compose-mail-attachment） | 2 つの id が同じモジュール（`compose_mail_sms` と、macOS・Windows の `compose_attachment_file`）を共有するため、crate は 1 つにして宣言の `id` は compose-mail-sms にする。`scaffold` は片方の stem しか移さないので、もう一方（`compose_attachment_file.rs`・`ComposeHelper.java`）は `git mv` で手で移す。2 つの対応表の `impl` は同じ crate のパスを指す。`scaffold` が機械的に `super::` で書く OS 実装から driver への参照は、`crate::driver::` へ手で直す |
| 再公開が無くなる（compose-mail-sms の `open_url`） | `windows_compose_mail_sms` が持っていた `open_url`・`SE_ERR_NOASSOC` の再公開は、web-auth-session・launch-at-login の参照を `fandhe_platform_windows` へ付け替える先行コミットで先に消した（Windows 以外のテストが `SE_ERR_NOASSOC` を要るときは、同じ値を cfg で置く）。§9 の compose-mail-sms の障害はこれで解消した |
| 機能専用の FileProvider（compose-mail-sms） | file-preview と同じ形で `ComposeMailFileProvider`（`FileProvider` の空の派生）と `res/xml/compose_mail_file_paths.xml`（`cache/compose/` だけ）を機能 crate に持つ。lifecycle provider の authority（`${applicationId}.fandhe.compose-mail-sms`）と区別するため、FileProvider は `.files` を付ける。共有の `${applicationId}.compose` の provider と `compose_file_paths.xml` は、動画の撮影も機能専用へ移ったため撤去した |
| 契約を他の crate が使う機能の同時移行（open-in-shell・recent-documents） | recent-documents が open-in-shell の `ShellPath`・`prepare_sample_file_in`・Windows の `to_win32_path` を使い、契約の `fandhe-device` 側も互いに参照していたため、1 コミットで移した。recent-documents の crate が open-in-shell の crate に依存する。`prepare_sample_file_in`・`to_win32_path` は `pub` にして crate ルートから出す（検査は変えない）。`to_win32_path` は OS に触れない純関数のため、`windows` モジュールは全 OS で載せる。エラーは `From<OpenInShellError> for RecentDocumentsError` で識別子ごと 1:1 に写す。§14 の open-in-shell の障害はこれで解消した |
| Windows の他機能が使う記号 | external-app-launch の Windows 実装のコメントが指していた `windows_open_in_shell.rs` の `ComGuard` は、新しいパス（`fandhe-cap-open-in-shell`）へ直した |
| 他機能と共有する画像の復号の複製（scanner-devices） | 表示用の復号（`MAX_SIDE`・`MAX_ALLOC`・`DISPLAY_SIDE`・`image::Limits`）は未移行の photo-picker の `decode_for_display` が持っていた。scanner-devices は document-scan の前例に合わせて driver に同じ処理を持ち、`// ponytail:` で「photo-picker を移すときに共有の置き場を決める」と書いた。契約側の `PickedPhoto`・`validate_photo`・`MAX_PHOTO_BYTES` は `fandhe-device` に残して参照する（§7） |
| Swift の薄層（image-generation） | `build.rs` を `fandhe-cap-on-device-llm` と同じ形（`compile_swift` + `export_link_args()`）で書き、`Cargo.toml` に `links` を置く。最終成果物（gallery）の `build.rs` が `import_link_args("fandhe_cap_image_generation")` を足す。`scaffold` は Swift の `build.rs` を作らない |
| Android の static の入口と JNI の呼び先の変更（radio-control・nfc・scanner-devices・compose-mail-sms・video-transcode・file-provider） | §8・§9 の型（`HELPERS` + `<Name>Lifecycle`・Context を取る static）に合わせた。helper が無いとき、保留中の要求を持つ機能（radio-control の `set`・nfc の `start`・scanner-devices の `start`・compose の `compose`）は結果を失敗で確定する。nfc は `onHostPaused`（`Activity.onPause()` の `super.onPause()` の中で届く）で reader mode を解除する。`MainActivity` では `super` の後に呼んでいたため前後だけが違い、起動時の RESUMED の見直しと要求 id の照合で順序の入れ替わりは吸収される |
| DocumentsProvider（file-provider） | `FileProviderDocumentsProvider` を機能 crate の `android/java` へ移し、宣言の `components`（`exported = true`・`permission = MANAGE_DOCUMENTS`・`DOCUMENTS_PROVIDER` の intent-filter）を `AndroidManifest.xml` と完全に一致させる。変更の通知は `MainActivity` を経由せず、static の `notifyChanged(Context)` を `with_activity_class` で呼ぶ。`unsafe` が残らないため全 OS で `forbid(unsafe_code)` にした |
| 共有ファイルに埋まった古い型（clipboard） | `device/ios.rs` の `IosClipboard`・`device/android.rs` の `AndroidClipboard`・`MainActivity` の `clipboardRequest` / `nativeOnClipboardResult` を機能 crate へ切り出した。Android は `ClipboardHelper` の static の `request(Context, long, int, String)` にし、JNI のエクスポート名を `Java_com_fandhe_platform_MainActivity_nativeOnClipboardResult` から `Java_com_fandhe_platform_ClipboardHelper_nativeOnClipboardResult` へ変えた（Java の native 宣言・Rust の受け口・呼び出しの署名の 3 か所を照合）。待ちは `Mailbox` から `RequestSlot` へ置き換え、`egui-proxy` の `Mailbox` から使われなくなった `begin`・`next_id` を消した（TTS は `begin_at*` を使う）。`arboard` は `fandhe-device` から機能 crate のデスクトップ向け依存へ移した。他機能の SAFETY コメントが指していた `AndroidClipboard::request` は `fandhe-cap-camera-capture` の `AndroidCamera::capture` へ直した |
| Web の参照の付け替え（video-transcode・nfc・compose-mail-sms） | 本体は `fandhe-web-dom` に残し、`fandhe_device::<stem>` の参照を機能 crate への依存に付け替えた。機能 crate のエラー型（`VideoTranscodeError`・`ComposeMailSmsError`）で写像するため、Web 側のエラー型の参照も付け替える。`cargo clippy -p fandhe-web-dom --target wasm32-unknown-unknown --all-features` で拾える |
| `uses-feature` の宣言と突き合わせテスト（nfc） | `android.hardware.nfc` を nfc の宣言へ書いたため、`cargo-fandhe` の `tests/gen.rs` が「nfc の feature は宣言に無い」と仮定していた比較を、宣言側と現行 Manifest の完全一致へ直した |
| `reset_pending_tracking` | nfc・scanner-devices・video-transcode・compose-mail-sms・clipboard・radio-control は機能 crate が `pub` で公開し、`fandhe-gallery` の `android_main` が呼ぶ。nfc と scanner-devices は driver 側と Android の橋渡し側の 2 つの追跡を 1 つの入口から捨てる。`fandhe-egui-proxy` の `android.rs` からは消した |
| `windows` の feature | 移した実装が使う feature を機能 crate の target 依存へ書く（video-transcode は `Media_Core` ほか、scanner-devices は `Devices_Scanners`・`Devices_Enumeration`、open-in-shell は `Win32_System_Registry` も要る）。`cargo clippy -p <crate> --all-targets --target x86_64-pc-windows-gnu` で拾える |

### 移さなかった機能

| 機能 | 理由 | 解消の見込み |
|------|------|--------------|
| ar-camera-view | iOS の実装は `fandhe-cap-ar`（同じ `ARSession`）にあり、Web は `fandhe-web-dom/src/ar.rs` が `fandhe-cap-ar` の契約を使う。新しい crate は作らない | 対応表 iOS 欄の `impl` を `fandhe-cap-ar` のパスへ書いた（コードは変えていない） |
| print-screen | driver が `EguiSenderExt` の `request_screenshot` / `tagged_screenshot`（egui 固有の口）を使い、`HostView` には画面の読み戻しの口が無い（§1・§9 の print と同じ止まり方） | print と一緒に、`HostView` へ画面の読み戻しの口を設計してから移す |
| dark-mode・gpu-backend・keyboard-mouse・soft-keyboard-ime・tv-focus・ui-core-nodes | 端末機能ではない（group が「既存機能の棚卸し」）。§1 の表のとおり crate を作らない | 作らない |
| music-recognition・watch-connectivity | 全 OS が `possible` / `n-a` / `no` で実装も `impl` も無く、移すものが無い（game-center・age-range と同じ扱い） | 実装するときに最初から crate で作る |

compose-mail-sms（§9）・launch-at-login（§11）・open-in-shell（§14）は、この束で移した、または障害が解消した。
launch-at-login は `windows_compose_mail_sms::open_url` の参照が `fandhe_platform_windows` へ付け替わったため、第 9 束（#1329、§16）で移した。

## 16. 第 9 束（#1329）で出た型

第 9 束（flashlight・capture-protection・activity-recognition・foldable-posture・in-app-browser・wifi-info・
launch-at-login・camera-preview-stream）で、後回しにしていた障害が解消した 8 機能を移した。はじめて出た型と、その扱いは次のとおり。

| 型 | 扱い |
|----|------|
| Started / Stopped / Resumed / Paused に参加する Android の Helper | Helper を private コンストラクタ + `HELPERS`（`WeakHashMap`）+ static の入口（第 1 引数は `Activity`）にし、`<Name>Lifecycle`（`FeatureLifecycleProvider` 派生）が `onHostCreated` で `attach`、`onHostStarted` などで Helper の static を呼び、`onHostDestroyed` で `detach` する。`super.onXxx()` の中で届くため、従来の `MainActivity` との前後の違いを Lifecycle クラスのコメントに書く。Helper が無い（生成前・破棄後）要求は、成功にも待ちにもせず失敗・「読めない」で確定する |
| `onCreate` の直後に状態を掛け直す Helper（capture-protection） | `attach` の中で Helper を作った直後に掛け直す（`onHostCreated` は `Activity.onCreate` の中で届き、最初のフレームより前）。要求値は static のまま持ち越す |
| 保留要求・許可ランチャーを持つ Helper（wifi-info・activity-recognition・camera-preview-stream） | ランチャーの登録は `attach`（STARTED より前）で、キーは明示する（従来の文字列のまま）。JNI は `with_activity_class` + `call_static_method` にして、Rust 側の `unsafe` を無くす（JNI のエクスポート名は変わらない） |
| 機能 crate が GMS を使う Helper（activity-recognition） | GMS の判定を Helper の private メソッドに持つ（`DocumentScanHelper` と同じ）。決済が使う `GmsAvailability` は残す。`ActivityRecognitionReceiver` は `scaffold` が動かさないため `git mv` し、宣言の `components` に `kind = "receiver"`（`exported = false`・intent-filter なし）で書く |
| driver の `unsafe`（flashlight の `IsIconic`） | `scaffold` が止まる前に、Windows 実装のファイルへ安全な `pub(crate)` 関数（`main_window_minimized`）として移し、driver はそれを呼ぶ |
| 契約の一部だけを 2 機能が使う（wifi-info の `radio_off`、camera-preview-stream の縮小） | OS に触れない部品だけを `fandhe-device` の同名モジュールに最小限残し（wifi-info は `RADIO_ON`・`RADIO_STATE_OFF`・`radio_off`、camera-preview-stream は `preview_size`・`ScaledFrame`・`PackedLayout`・`downscale_packed`・`downscale_yuv420`）、機能 crate の `contract.rs` から `pub use` で再公開する。`scaffold` は契約を丸ごと `git mv` するため、実行後に手で分ける。他機能（`fandhe-cap-wifi-connect`・`fandhe-cap-ar`）の参照は変えない |
| 終了・再生成で後片付けが要る機能（in-app-browser・camera-preview-stream） | `reset_pending_tracking`（と `shutdown`）を `pub` にし、`fandhe-gallery` の `android_main` と `shutdown()` が呼ぶ。`fandhe-egui-proxy` の `android.rs`・`device.rs` からは消す |
| iOS と macOS が同じ `.m` を共有する機能（camera-preview-stream） | `scaffold` の出力（`src/ios.rs`・`ios/*.m`）の名前のまま、mod の cfg は元の `any(target_os = "ios", target_os = "macos", test)`。`build.rs` は iOS と macOS で framework の組を分け、crate ルートの `deny(unsafe_code)` に macOS を足す。宣言は `ios` と `macos` の両方に書く |
| `DeviceError` を使わない機能（in-app-browser・activity-recognition・camera-preview-stream） | `scaffold` が出した `error.rs` を消し、`lib.rs` の `mod error` と `pub use` も消す。使うバリアントが `Disabled` などだけ未写像になる機能（wifi-info）は、`error.rs` へ手で足して文言を固定するテストにも足す |
| `scaffold` が出す edition 2024 の let チェーン警告 | `cargo clippy --fix --allow-dirty -p fandhe-cap-<id> --all-targets` が直す（Windows 実装は `--target x86_64-pc-windows-gnu` でも 1 度通す） |
