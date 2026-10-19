//! Windows 向けファイルの関連付け受信のスタブ（Issue #416。windows の子が中身だけを埋める）。
//! `device/file_association.rs::platform` の Windows 経路と `main.rs` から使われる。
//! 今は `Unsupported` を返し、登録ボタンも出さない（`registrar()` が `None`）。
//!
//! # windows の子への申し送り
//!
//! - シグネチャ（`implementation` / `registrar` / `is_file_launch`）は変えない。dispatch・
//!   `main.rs` の配線は (a) で済んでいる
//! - 登録は `HKCU\Software\Classes\Fandhe.EguiProxy.Txt.1` の ProgID と、
//!   `.txt\OpenWithProgids` に値を 1 つ足すだけ（既定アプリは奪わない）。完了後に
//!   `SHChangeNotify(SHCNE_ASSOCCHANGED)`。解除は自分が足した値と ProgID だけ消す
//! - 起動済みなら新プロセスの起動引数のパスを既存インスタンスへ pipe で転送する。
//!   `windows_url_scheme` の pipe に 1 バイトの種別（`U`=URL、`F`=ファイル）を足して多重化する
//! - 引数のパスは `fandhe_device::file_association::validate_windows_path` で検査し、
//!   `launch_unaltered` を使うかは windows の子が判断する
//! - レジストリのエラーは `map_registry_error` で契約上のエラー種別へ写す

use std::ffi::OsString;

use fandhe_device::file_association::{FileAssociation, UnsupportedFileAssociation};

use super::windows_url_scheme::Registrar;

/// Windows の実装を返す。windows の子が実体に差し替える。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &UnsupportedFileAssociation
}

/// 登録・解除の差し込み口。スタブの間は `None`（ボタンを出さない）。
pub fn registrar() -> Option<&'static Registrar> {
    None
}

/// 起動引数がファイルを開く起動か。スタブの間は常に `false`。
pub fn is_file_launch(_args: &[OsString]) -> bool {
    false
}
