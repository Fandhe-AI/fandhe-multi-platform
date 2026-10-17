//! iOS 向けピクチャ・イン・ピクチャ実装（Issue #365。親 #97 の b-ios）。
//! `device/picture_in_picture.rs::platform_pip` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_picture_in_picture`（`apple/fandhe_apple_picture_in_picture.m`、AVKit の sample buffer 経路）を返す。
//! Linux ホストの `cargo test` では共用実装が存在しないため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - `Info.plist` の `UIBackgroundModes` に `audio` が必要（AVKit が PiP の条件とする）。背景での音声再生には使わない。
//! - `prepare` で `AVAudioSession` を playback + mixWithOthers にし、解放の全経路で元へ戻す。
//! - 小窓中の背景遷移は中断にしない（AVKit が小窓を維持する）。
//! - `isPictureInPictureSupported` が NO の環境（シミュレータで報告あり）は `Unsupported`（画面は「非対応」）。
//!
//! # 再描画（契約表 4）
//!
//! `.m` が delegate で状態を変えた直後、`g_lock` を外してから [`fandhe_rust_picture_in_picture_changed`] を呼ぶ。
//! ここで `crate::ios::request_repaint` を呼び次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗・非対応を成功に畳まないこと、資源と音声セッションの解放。
//! 守らない: 画面内プレビューと OS 側映像の色替えの時刻ずれ、小窓の位置・大きさ、画面回転への追従。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::picture_in_picture::PictureInPicture;

/// iOS の実装を返す（macOS と共用）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn PictureInPicture> {
    super::apple_picture_in_picture::implementation()
}

/// `.m` が状態を変えた直後に呼ぶ受け口。引数なし・ポインタなしで、main thread（delegate）から呼ばれる。
/// `request_repaint` は Mutex を取るだけで `dispatch_sync` はしない（デッドロックしない）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_picture_in_picture_changed() {
    crate::ios::request_repaint();
}
