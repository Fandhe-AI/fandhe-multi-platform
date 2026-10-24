//! Windows 向け「既定アプリで開く・Explorer で表示」の骨格（Issue #749 が置き、実装は Issue #750）。
//! 現時点は [`UnsupportedOpenInShell`] を返すだけのスタブで、ダミーの成功は返さない。
//! `device/open_in_shell.rs` の windows 経路から呼ばれる。後続の子（#750）が本ファイルだけを埋める。
//!
//! # #750 が埋めるときの約束
//!
//! - 開く: `ShellExecuteW`（verb `open`、`lpParameters` は NULL）。戻り値が 32 を超えれば受理
//!   （`HandedOff`）、`SE_ERR_NOASSOC` は `NoHandler`
//! - 選択表示: `SHOpenFolderAndSelectItems` と `ILCreateFromPathW`。PIDL は全経路で `ILFree` する
//! - COM（STA）は `windows_launch_at_login.rs` の `ComGuard` で初期化する
//! - 危険な型は `AssocIsDangerous` で、OS を呼ぶ前に拒否する（`InvalidInput("shell type")`）
//! - UNC、デバイスパス（`\\.\`）、代替データストリーム、MAX_PATH 超を拒否する。
//!   `canonicalize` が返す `\\?\` 接頭辞の扱いもここで決める
//! - パスと HRESULT はログに出さない
//! - dispatch（`device/open_in_shell.rs`）・`device.rs` の `pub mod` 列・共通コア画面
//!   （`open_in_shell`）には触らない。Windows 分の文言と操作 UI は (a) で定義済み
//!
//! モジュールは、後続の子がホストテストを足すときに cfg を広げずに済むよう test でも
//! コンパイルする（`device.rs`）。

use fandhe_device::open_in_shell::{OpenInShell, UnsupportedOpenInShell};

/// Windows の `OpenInShell` 実装を返す（現状は `Unsupported` 固定のスタブ）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn implementation() -> Box<dyn OpenInShell> {
    Box::new(UnsupportedOpenInShell)
}
