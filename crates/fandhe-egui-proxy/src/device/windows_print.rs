//! Windows 向け印刷の骨格（Issue #325 で置いたスタブ。実装は #326）。
//! `device/print.rs` の Windows 経路（dispatch は #325 で最終形）から呼ばれる。#326 が
//! `PrintDlgExW` + GDI で `implementation` の中身を置き換える。`windows` crate を使う ffi 部分は
//! モジュール内側で `cfg(target_os = "windows")` に閉じる。dispatch・画面・文言は触らない。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use fandhe_device::print::{Print, UnsupportedPrint};

/// Windows の `Print` 実装を返す。#326 まで非対応。
pub(crate) fn implementation() -> Box<dyn Print> {
    Box::new(UnsupportedPrint)
}
