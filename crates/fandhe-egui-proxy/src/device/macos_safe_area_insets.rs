//! macOS 向けセーフエリア実装（Issue #444、親 #105）。
//! `macos/fandhe_macos_safe_area_insets.m`（窓の `contentView.safeAreaInsets`、`build.rs` が macOS
//! ターゲットの feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 1 関数で呼ぶ。
//! `unsafe` はこの呼び出しに閉じる。`device/safe_area_insets.rs` の macOS 経路から呼ばれ、
//! 契約は `fandhe_device::safe_area_insets`。値の検査と写像は同モジュールの `insets_from_raw`（SSOT）。
//!
//! # 既知の天井
//!
//! - OS の全画面は中身をセーフエリアの内側へ置くため、窓表示でも全画面でも普段は 0
//! - 複数窓では key window（なければ main window、先頭の窓）の値だけを見る
//! - 実機（切り欠きのある MacBook）の目視確認は #442

use fandhe_device::safe_area_insets::{insets_from_raw, Insets, SafeAreaSource};
use fandhe_device::DeviceError;

// SAFETY: `macos/fandhe_macos_safe_area_insets.m` が定義する C ABI。引数は 4 要素の書き込み可能な
// `double` 配列の先頭ポインタ 1 つで、`.m` は成功時（戻り値 0）にだけ main スレッド上で同期に
// 4 要素を書く。main スレッド以外・NSApp 未確立・窓なしでは何も書かず -1 を返す。
#[allow(unsafe_code)]
extern "C" {
    fn fandhe_macos_safe_area_insets_read(out: *mut f64) -> i32;
}

/// [`SafeAreaSource`] の macOS 実装。状態を持たない。
pub(crate) struct MacosSafeArea;

impl SafeAreaSource for MacosSafeArea {
    fn read(&self) -> Result<Option<Insets>, DeviceError> {
        let mut raw = [0f64; 4];
        // SAFETY: `raw` は呼び出しの間生きている 4 要素の書き込み可能バッファで、上記 extern
        // 宣言の契約（4 要素を書くのみ）に合う。
        #[allow(unsafe_code)]
        let status = unsafe { fandhe_macos_safe_area_insets_read(raw.as_mut_ptr()) };
        insets_from_raw(status, raw).map(Some)
    }
}
