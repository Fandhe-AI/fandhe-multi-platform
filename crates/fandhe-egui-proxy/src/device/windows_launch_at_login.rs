//! Windows 向けログイン時の自動起動の骨格（Issue #696 が置き、実装は Issue #697）。
//! 現時点は [`UnsupportedLaunchAtLogin`] を返すだけのスタブで、ダミーの成功は返さない。
//! `device/launch_at_login.rs` の windows 経路から呼ばれる。後続の子（#697）が本ファイルだけを
//! 埋める。
//!
//! # #697 が埋めるときの約束
//!
//! - 登録先は HKCU の Run キー。値データは引用符付きの実行体パス + `--fandhe-launched-at-login`
//!   （260 文字以内）
//! - StartupApproved は読むだけ（先頭バイト 0x02 は `Enabled`、0x03 は `DisabledByUser`、
//!   それ以外は `Unknown`）。書き込まない
//! - 同じ値名に別の場所の実行体があれば `RegisteredElsewhere` を返し、解除では消さない
//! - 未登録の解除は OS を呼ばず `Ok(NotRegistered)`（べき等）
//! - 設定画面は `ms-settings:startupapps`
//! - 実行体のパスをログ・エラー文言に出さない
//! - dispatch（`device/launch_at_login.rs`）・`device.rs` の `pub mod` 列・共通コア画面
//!   （`launch_at_login_show`）には触らない。Windows 分の文言と操作 UI は (a) で定義済み
//!
//! モジュールは、後続の子がホストテストを足すときに cfg を広げずに済むよう test でも
//! コンパイルする（`device.rs`）。

use fandhe_device::launch_at_login::{LaunchAtLogin, UnsupportedLaunchAtLogin};

/// Windows の `LaunchAtLogin` 実装を返す（現状は `Unsupported` 固定のスタブ）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Box<dyn LaunchAtLogin> {
    Box::new(UnsupportedLaunchAtLogin)
}
