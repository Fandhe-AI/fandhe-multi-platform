//! iOS 向けセーフエリア実装（Issue #445、親 #105。#444 のスタブを置き換え）。
//! `ios/fandhe_ios_safe_area_insets.m`（key window の `safeAreaInsets`、`build.rs` が iOS
//! ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じる。`device/safe_area_insets.rs` の iOS 経路から呼ばれ、
//! 契約は `fandhe_device::safe_area_insets`。値の検査と写像は同モジュールの `insets_from_raw`（SSOT）。
//!
//! # 再描画
//!
//! UI スレッドで同期に読むだけで OS コールバックを持たないため、`crate::ios::request_repaint` は
//! 呼ばない。再描画は driver の `request_repaint_after` と完了通知（`CompletionSender::send`）が起こす。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 取れないとき（window なし・main 以外）の 0 を成功にしないこと、失敗後に前回の値を返さないこと
//! - 守らない: 反映は最大 1 秒 + 1 フレーム、key window の値だけ、起動直後に window が
//!   key になる前は 1 回「失敗」になりうる（次の読み取りで戻る）、egui の zoom が 1 でないときの
//!   point とのずれ
//!
//! 実機の目視確認は #442。

#[cfg(target_os = "ios")]
use std::sync::Arc;

#[cfg(target_os = "ios")]
use fandhe_device::safe_area_insets::{insets_from_raw, Insets, SafeAreaSource};
#[cfg(target_os = "ios")]
use fandhe_device::DeviceError;

// SAFETY: `ios/fandhe_ios_safe_area_insets.m` が定義する C ABI。引数は 4 要素の書き込み可能な
// `double` 配列の先頭ポインタ 1 つで、`.m` は成功時（戻り値 0）にだけ main スレッド上で同期に
// 4 要素を書き、ポインタを保持しない。main 以外・window なしでは何も書かず -1 を返す。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_ios_safe_area_insets_read(out: *mut f64) -> i32;
}

/// [`SafeAreaSource`] の iOS 実装。状態を持たない。
#[cfg(target_os = "ios")]
struct IosSafeArea;

#[cfg(target_os = "ios")]
impl SafeAreaSource for IosSafeArea {
    fn read(&self) -> Result<Option<Insets>, DeviceError> {
        let mut raw = [0f64; 4];
        // SAFETY: `raw` は呼び出しの間生きている 4 要素の書き込み可能バッファで、上記 extern
        // 宣言の契約（4 要素を書くのみ）に合う。
        #[allow(unsafe_code)]
        let status = unsafe { fandhe_ios_safe_area_insets_read(raw.as_mut_ptr()) };
        insets_from_raw(status, raw).map(Some)
    }
}

/// iOS の実装を返す。`device/safe_area_insets.rs` の iOS 経路から呼ばれる。
#[cfg(target_os = "ios")]
pub(crate) fn implementation() -> Arc<dyn SafeAreaSource> {
    Arc::new(IosSafeArea)
}
