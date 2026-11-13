//! ゲームコントローラ（ゲームパッド）入力の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行、
//! Android の入力経路は Issue #1303）。
//!
//! 扱う範囲は「接続中の先頭 1 台のスティック・トリガ・ボタンの最新値を OS から読み、
//! `fandhe_core::screens::gamepad_show` へ渡す」ところまで。契約（`contract`）、失敗の種類（`error`）、
//! 画面向け effect ドライバ（[`gamepad_show_effect`]）、Android・Apple・Windows の実装、宣言
//! （`Cargo.toml` の `[package.metadata.fandhe]`）を 1 crate に集める。Web の実装本体は
//! `fandhe-web-dom` に残り、契約だけをこの crate から使う（usb-device と同じ型）。
//! 利用者のアプリはこの crate を依存に足すだけで使える（`docs/architecture/consumer-app-structure.md` §4）。
//!
//! # 依存方向
//!
//! 本 crate → `fandhe-core`（画面の状態関数）・`fandhe-runtime`（`CompletionSender`・再描画）の一方向。
//! egui には依存しない。呼び出し元は `fandhe-gallery`（effect の登録）と、Android の画面ゲートを切り替える
//! `fandhe-gallery` の `android_main`。
//!
//! # Android の入力は `Window.Callback` を包んで観測する（Issue #1303）
//!
//! Android のボタン・軸の入力は `DecorView` から `Window.getCallback()`（通常は Activity）の
//! `dispatchKeyEvent` / `dispatchGenericMotionEvent` へ渡る。従来は利用者の `MainActivity` がこの 2 つを
//! override してから JNI へ中継していたが、それでは「機能を選ぶだけで使える」にならない。
//! `GamepadLifecycle`（`FeatureLifecycleProvider` の派生）が `onHostCreated` で Window の callback を
//! 包み、通知してから元の callback の戻り値をそのまま返す。**イベントは消費しない**ため、TV フォーカス
//! （旧 #280）の十字キーの経路は変わらない。AppCompat も同じ方法で包んでおり、どちらが外側でも委譲は連なる。
//! 接続状態の監視（`InputManager`）は `onHostResumed` / `onHostPaused` に対で結ぶ。
//!
//! # 対象 OS
//!
//! Android・iOS・macOS・Windows・Web。それ以外は [`GamepadError::Unsupported`] 固定で、画面には代替入力が出る。
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の入力の順序、切断後に古い値を返し続けないこと、未接続・非対応・読み取り失敗の区別
//! （嘘の成功の禁止）、`onHostPaused` での登録解除と「無し」の通知、Activity の再生成後に新しい Window を
//! 包み直すこと。
//! 守らない: 起動後に利用者や第三者が `getWindow().setCallback()` で callback を差し替えた場合の
//! ボタン・軸の入力（届かなくなる。接続状態の監視は影響を受けない）、複数台の合成、
//! 複数 deviceId に分かれる機種（split pad）の束ね、OS 側の接続通知との時間差による表示の一時的な食い違い。

// unsafe は Apple の FFI と Android の JNI エクスポートだけに閉じる（`// SAFETY:` を付ける）。
// それ以外の OS では禁止を保つ。
#![cfg_attr(
    not(any(target_os = "ios", target_os = "macos", target_os = "android")),
    forbid(unsafe_code)
)]
#![cfg_attr(
    any(target_os = "ios", target_os = "macos", target_os = "android"),
    deny(unsafe_code)
)]

#[cfg(any(target_os = "android", test))]
mod android;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple;
mod contract;
mod driver;
mod error;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "android")]
pub use android::set_screen_active;
pub use contract::*;
pub use driver::*;
pub use error::GamepadError;
