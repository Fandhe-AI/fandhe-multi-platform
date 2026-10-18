//! iOS 向けロック画面・OS のメディア操作実装（Issue #382。親 #98 の b-ios）。
//! `device/now_playing.rs::platform_now_playing` の iOS 経路から呼ばれ、macOS と共用の
//! `apple_now_playing`（`apple/fandhe_apple_now_playing.m`、MediaPlayer）を返す。
//! Linux ホストの `cargo test` では共用実装が存在しないため、関数と import は iOS 限定にしている。
//!
//! # iOS 固有の挙動
//!
//! - 前景だけで扱う。`UIBackgroundModes` と `AVAudioSession` の category は変えない（既定の `soloAmbient` のまま
//!   コントロールセンターに出るかは実機で確認する。#378）。
//! - 背景（`DidEnterBackground`〜`WillEnterForeground`）の間に届いた OS の操作は、Rust へ渡さず拒否する（背景で受けた操作に成功を返さない）。
//!   Inactive（コントロールセンター・通知センターを開いた間）は前景として扱い、そこからの操作は受け付ける。
//! - main thread からの `clear` / `shutdown` は登録の解放を済ませてから戻る。main 以外から呼ばれた場合は main へ非同期に回して
//!   即座に戻り、解放の完了は待たない（守らない範囲）。後から publish が成功していたら古い取り下げは捨てる。
//!
//! # 再描画（契約表 4）
//!
//! OS の操作を受けた直後に `apple_now_playing` の受け口が `crate::ios::request_repaint` を呼び、次フレームを起こす。
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の操作順、嘘の成功の禁止、登録の解放。
//! 守らない: 背景に回った間にコントロールセンターへ残る表示、PiP との `AVAudioSession` の取り合い、複数プロセス。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::now_playing::NowPlaying;

/// iOS の実装を返す（macOS と共用）。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn NowPlaying> {
    super::apple_now_playing::implementation()
}
