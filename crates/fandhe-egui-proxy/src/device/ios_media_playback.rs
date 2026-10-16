//! iOS 向け音声再生実装（Issue #269。#267 の枠を埋める）。
//! `device/media_playback.rs::platform_playback` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_media_playback`（`apple/fandhe_apple_media_playback.m`、`AVAudioPlayer`）を返す。
//! `ios_network_status.rs` と同様、Linux ホストの `cargo test` では共用実装が存在しないため、
//! 関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - 背景遷移・割り込み開始（通話など）で再生中なら一時停止して `Interrupted` にする。
//!   復帰しても勝手に再開しない。バックグラウンド再生はしない。
//! - `AVAudioSession` の category は変えない（既定のまま）。消音スイッチが入っていると鳴らない。
//! - 権限・usage description は不要。
//!
//! # 再描画（契約表 4）
//!
//! `.m` が OS コールバック（delegate・通知）で状態を変えた直後、`g_lock` を外してから
//! [`fandhe_rust_media_playback_changed`] を呼ぶ。ここで `crate::ios::request_repaint` を呼び
//! 次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、中断を成功の終了や失敗に畳まないこと、復帰で勝手に再開しないこと。
//! 守らない: 他アプリの音声との混ざり、消音スイッチ、位置表示のポーリング間隔ぶんの遅れ。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::media_playback::MediaPlayback;

/// iOS の実装を返す。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn MediaPlayback> {
    super::apple_media_playback::implementation()
}

/// `.m` が状態を変えた直後に呼ぶ受け口。引数なし・ポインタなしで、main queue（通知）または
/// 再生スレッド（delegate）から呼ばれる。`request_repaint` は Mutex を取るだけで `dispatch_sync` は
/// しない（デッドロックしない）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_media_playback_changed() {
    crate::ios::request_repaint();
}
