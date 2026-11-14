//! foldable-posture（折りたたみの姿勢・ヒンジ）の機能 crate（REQ-21 / TASK-21.4、Issue #1183 の手順で移行）。
//! Android は Jetpack WindowManager の `FoldingFeature`、Web は Window Segments / `screen.isExtended` 系の値を読む
//! （Web の本体は `fandhe-web-dom` に残り、本 crate の契約と写像 `reading_from_raw` を使う）。他の OS は `Unsupported`。
//! `fandhe-gallery` の画面登録から effect `foldable_posture_show_effect` が呼ばれ、`READ_INTERVAL` ごとに読み直す。
//!
//! 守る範囲: 未受信と失敗を「平ら」にしないこと、`onStop` 後に前回値を返さないこと、購読の登録と解除の対応。
//! 守らない範囲: 反映の遅れ（1 秒 + 1 フレーム）、再生成直後の一時的な未受信、閉じた端末と広げた端末の区別、ヒンジの角度。
//!
//! 契約（`contract`）・失敗の種類（`error`）・effect ドライバ（`driver`）・OS 実装を 1 つに集める。
//! 画面の状態型を参照してよいのは `driver` だけ。入口 `pub fn platform_foldable_posture()` は状態型に依存しないため、
//! フレームワークの外のアプリも使える。

// OS 実装は `fandhe_platform_android::with_activity_class` 経由の JNI だけで、本 crate は `unsafe` を持たない。
#![forbid(unsafe_code)]

#[cfg(any(target_os = "android", test))]
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
mod android;
mod contract;
mod driver;
mod error;

pub use contract::*;
pub use driver::*;
pub use error::FoldablePostureError;
