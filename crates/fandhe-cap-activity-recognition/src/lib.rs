//! activity-recognition（歩行・乗車などの行動認識）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! iOS は `CMMotionActivityManager`、Android は Play services の `ActivityRecognitionClient`、Windows は `ActivitySensor` で読む。
//! `fandhe-gallery` の画面登録から effect `activity_recognition_show_effect` が呼ばれ、継続配信の値を pull で読む。
//!
//! 守る範囲: 嘘の値を出さないこと、二重に購読しないこと、停止・背景・破棄・失敗で購読を解除すること（解除が未確認の間は新しい購読をしない）。
//! 守らない範囲: GMS の配信間隔と遅延、背景中の受信、過去の履歴、作り直し後の失敗表示。行動種別・確度はログに出さない。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_activity_recognition()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// unsafe は OS 実装の FFI・JNI エクスポートに限る（`// SAFETY:` を付ける）。該当 OS だけ deny へ緩める。
#![cfg_attr(
    not(any(target_os = "android", target_os = "ios")),
    forbid(unsafe_code)
)]
#![cfg_attr(any(target_os = "android", target_os = "ios"), deny(unsafe_code))]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
#[cfg(any(target_os = "ios", test))]
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
mod ios;
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows;

pub use contract::*;
pub use driver::*;
