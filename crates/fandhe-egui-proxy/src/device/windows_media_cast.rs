//! Windows 向け外部機器への再生（Issue #386 の骨格。実装は親 #99 の windows 子）。
//! `device/media_cast.rs::platform_cast` の Windows 経路から呼ばれる。現状は `UnsupportedMediaCast` を返す
//! スタブで、対応表の Windows セルは `possible` のまま（`impl` に書かない）。
//!
//! # windows 子が埋めるときの注意
//!
//! - ピッカーは `Windows.Media.Casting.CastingDevicePicker`。デスクトップアプリは `IInitializeWithWindow` で
//!   owner window を関連付ける（`PickerKind::App`）
//! - 接続は UI スレッドで行い、`RouteState::Connecting` は `CONNECT_DEADLINE` で打ち切る
//! - `windows` クレートの feature 追加は windows 子で `Cargo.toml` に足す（本 PR では触らない）
//! - 素の exe で接続できなければ対応表を保留へ移す

#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use fandhe_device::media_cast::{MediaCast, UnsupportedMediaCast};

/// Windows の実装を返す（windows 子まではスタブ）。
#[cfg(target_os = "windows")]
pub(crate) fn implementation() -> Arc<dyn MediaCast> {
    Arc::new(UnsupportedMediaCast)
}
