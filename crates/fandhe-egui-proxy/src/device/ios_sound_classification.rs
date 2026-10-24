//! iOS 向け環境音の分類（Issue #725 の (a) ではスタブ。実装は b-ios）。
//!
//! 呼び出し元: `device/sound_classification.rs` の iOS 経路が [`implementation`] を呼ぶ。
//! (a) の時点では `UnsupportedSoundClassifier` を返し、成功を偽らない（開始は `Unsupported`）。
//! b-ios は **このファイルだけ**を差し替え、`apple_sound_classification::implementation` への
//! 委譲にする（dispatch・共通コア画面・`device.rs`・`build.rs` は変えない）。あわせて共用の
//! `apple/fandhe_apple_sound_classification.m` の iOS 区画を埋める（`AVAudioSession` の保存と復元、
//! 背景遷移・音声の割り込みでの中断、値の確定直後の `crate::ios::request_repaint`）。契約の SSOT は
//! `fandhe_device::sound_classification`。

use std::sync::Arc;

use fandhe_device::sound_classification::{SoundClassifier, UnsupportedSoundClassifier};

/// iOS の `SoundClassifier` 実装を返す（b-ios まではスタブ）。
pub(crate) fn implementation() -> Arc<dyn SoundClassifier> {
    Arc::new(UnsupportedSoundClassifier)
}
