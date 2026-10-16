//! Android 向け印刷の骨格（Issue #325 で置いたスタブ。実装は #328）。
//! `device/print.rs` の Android 経路（dispatch は #325 で最終形）から呼ばれる。#328 が
//! `PrintHelper`（`PrintManager` + `PrintDocumentAdapter`）を JNI で呼ぶ形で `begin_print` の中身を
//! 置き換える。dispatch・画面・文言は触らない。結果は `device/print.rs` の
//! `outcome_from_android_code` の表に合わせて `bridge::deliver` へ届ける。
//!
//! 現状は `Unsupported` を返す（嘘の成功にならない）。

use fandhe_device::print::PrintRequest;
use fandhe_device::DeviceError;

/// 印刷ダイアログの表示を依頼する。#328 まで非対応。
pub(crate) fn begin_print(_request_id: u64, _req: &PrintRequest) -> Result<(), DeviceError> {
    Err(DeviceError::Unsupported("print"))
}
