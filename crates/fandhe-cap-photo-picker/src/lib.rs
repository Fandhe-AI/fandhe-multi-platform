//! 写真選択（photo-picker）の機能 crate（REQ-21 / TASK-21.4、Issue #1355、手順は `docs/operations/capability-crate-migration.md`）。
//! OS 標準の写真ピッカー（iOS の `PHPickerViewController`、Android の `PickVisualMedia`、macOS・Windows の「開く」ダイアログ）で
//! 画像を 1 枚選ばせ、**復号できることを確かめてから**画面へ返す。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装（macOS・Windows は `desktop`、iOS は `ios`、
//! Android は `android`）を 1 つに集める。画面の状態型を参照してよいのは `driver` だけ。状態型に依存しない入口は 3 つある。
//! 同期の [`platform_photo_picker`]（文字認識・バーコード読み取りの機能 crate がデスクトップ経路で使う。macOS・Windows は
//! ダイアログを開いて結果を返し、iOS・Android・Linux は `Unsupported` を返す）。iOS・Android の非同期の [`begin_photo_pick`] /
//! [`poll_photo_pick`] / [`discard_photo_pick`]（画面 effect と同じ slot を持ち主で分けて共有し、進行中は busy で断る）。iOS 専用の
//! [`present_ios_photo_picker`]（PHPicker の受け口を共有する文字認識・バーコード読み取りが、要求ごとに受け取りを登録して使う）。
//!
//! 呼び出し元は最終成果物（`fandhe-gallery`）の画面レジストリ（`photo_pick_effect`）と、Android の `android_main`（`reset_pending_tracking`）。
//! Android の JNI 受け口（`nativeOnPhotoPickerResult`）と iOS の FFI 受け口は、この crate 内の `bridge::deliver` / 登録済みの受け取りへ
//! 結果を届ける。Web の実体は `fandhe-web-dom` に残る。
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序・嘘の成功の禁止（キャンセルと失敗を区別し、復号できない画像・0 バイト・形式不明・
//! 上限超過を成功にしない。古い世代の結果は捨てる）・選んだ画像のパス・名前・中身をログに出さないこと・サイズと寸法の上限。
//! 守らない範囲: OS がコールバックしない（シーン破棄等）ときの OS 側の固着（固定タイムアウトは掛けない）、Activity 再生成をまたいだ
//! 保留中の要求の復元（契約は「復元せず破棄」。`reset_pending_tracking` が捨てる）、複数プロセス・複数タブ。
//! 契約の詳細（2 段検査・3 値・既知の制約）の SSOT は `fandhe_device::photo_picker` のモジュール doc。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(target_os = "android")]
mod android;
mod contract;
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod desktop;
mod driver;
mod error;
#[cfg(target_os = "ios")]
mod ios;
// iOS の受け取りの登録表。OS に触れない純粋なロジックのため、ホストのテストでも通す。
#[cfg(any(test, target_os = "ios"))]
mod pending;

pub use contract::*;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use desktop::RfdPhotoPicker;
pub use driver::*;
pub use error::PhotoPickerError;
#[cfg(target_os = "ios")]
pub use ios::{IosPickDeliver, present_ios_photo_picker};
