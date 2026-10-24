//! iOS 向け環境音の分類（Issue #726、親 #142 の b-ios）。
//!
//! 呼び出し元: `device/sound_classification.rs` の iOS 経路が [`implementation`] を呼ぶ。
//! 実体は macOS と共用の `apple_sound_classification::implementation`（SoundAnalysis の内蔵分類器を
//! `apple/fandhe_apple_sound_classification.m` の C ABI 経由で呼ぶ）への委譲で、dispatch・共通コア画面・
//! ドライバ・trait 契約（`fandhe_device::sound_classification`）は変えない。
//!
//! iOS 固有の処理は `.m` の `TARGET_OS_IPHONE` 区画にある。
//!
//! - `AVAudioSession` の category / options を保存して Record にし、全終了経路で戻す
//! - 背景遷移（`DidEnterBackground`）・音声の割り込み（Began）・メディアサービスのリセットを中断として
//!   確定する（自動では再開しない。許可ダイアログ中の Inactive は中断にしない）
//! - 非同期で値が確定した直後に、`.m` が [`fandhe_rust_sound_classification_changed`] を呼んで
//!   次フレームを起こす（入力がないと egui が描画せず、画面が処理中のまま止まるため）
//!
//! # 守る範囲・守らない範囲
//!
//! 守る範囲は `apple_sound_classification` と同じ（嘘の成功の禁止、全終了経路での資源の解放と
//! セッションの復元、旧世代の通知を返さない）。守らない範囲は背景での分類、中断後の自動再開、
//! マイクを使う他の機能との同時使用（1 プロセス 1 画面の前提）、複数プロセス。
//!
//! # 既知の天井
//!
//! 実機での分類と、割り込み・背景遷移の挙動は実機確認（#716）で確かめる。

use std::sync::Arc;

use fandhe_device::sound_classification::SoundClassifier;

/// iOS の `SoundClassifier` 実装を返す（macOS と共用の Apple 実装へ委譲する）。
pub(crate) fn implementation() -> Arc<dyn SoundClassifier> {
    super::apple_sound_classification::implementation()
}

/// 値の確定（結果・失敗・中断）の通知（`.m` の非同期経路から、制御キューの外で呼ばれる）。
/// 次フレームを起こすだけで、引数もポインタも取らず、panic せず、`dispatch_sync` もしない
/// （契約表 4・11）。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_sound_classification_changed() {
    crate::ios::request_repaint();
}
