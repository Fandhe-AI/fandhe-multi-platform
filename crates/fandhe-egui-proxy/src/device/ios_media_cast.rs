//! iOS 向け外部機器への再生実装（Issue #387。親 #99 の b-ios）。
//! `device/media_cast.rs::platform_cast` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_media_cast`（`apple/fandhe_apple_media_cast.m`、`AVRoutePickerView` + `AVAudioPlayer` +
//! `AVAudioSession.currentRoute`）を返す。Linux ホストの `cargo test` では共用実装が無いため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - ピッカーは右下に固定した本物の `AVRoutePickerView`（[`PickerKind::Native`]）。`show_picker` は `Unsupported` のまま。
//! - `AVAudioSession` を playback + longFormAudio にするのはこの画面の間だけ。変更前の値を保存し、失敗を含む全経路で戻す。
//! - 背景遷移・割り込み開始で止めて `CastPlayback::Interrupted` にし、勝手に再開しない。
//!   `UIBackgroundModes` の `audio` は PiP 用で、背景で鳴らし続けることには使わない。
//! - 「外部で再生中」は `currentRoute.outputs` に AirPlay の出力があるときだけ。名前は画面にだけ出し、ログ・保存はしない。
//!
//! # 再描画（契約表 4）
//!
//! `.m` が通知・delegate で状態を変えた直後、`g_lock` を外してから [`fandhe_rust_media_cast_changed`] を呼ぶ。
//! ここで `crate::ios::request_repaint` を呼び次フレームを起こす（入力が無いと egui が描画せず表示が止まるため）。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、失敗を成功値へ畳まないこと、資源と音声セッションの解放。
//! 守らない: PiP 等の他画面との `AVAudioSession` の同時利用、OS の UI で切り替えた反映のポーリング間隔ぶんの遅れ、
//! 複数プロセス、受信機側の品質。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::media_cast::MediaCast;

/// iOS の実装を返す（macOS と共用）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn MediaCast> {
    super::apple_media_cast::implementation()
}

/// `.m` が状態を変えた直後に呼ぶ受け口。引数なし・ポインタなしで、main thread（通知・delegate）から呼ばれる。
/// `request_repaint` は Mutex を取るだけで `dispatch_sync` はしない（デッドロックしない）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_media_cast_changed() {
    crate::ios::request_repaint();
}
