//! macOS / iOS 向け地図スナップショット実装（Issue #273 macOS・#274 iOS）。
//! `device/map_view.rs` の macOS / iOS 経路から呼ばれ、MapKit 実装
//! （`apple/fandhe_apple_map_view.m`、`build.rs` が feature `device` 有効時のみコンパイル・
//! リンクする）を `unsafe extern "C"` 2 関数で呼ぶ。`unsafe` はこのモジュールの extern 宣言と
//! 受け口に閉じ `// SAFETY:` で契約を明記する（`apple_photo_library.rs` と同型、coding-rust.md の
//! FFI 例外規定準拠）。
//!
//! `.m` 側の完了から [`fandhe_rust_map_view_outcome`] が呼ばれ、`device/map_view::bridge` へ
//! 結果を置く（pull 型）。結果コード → 結果の写像は `device/map_view.rs::outcome_from_apple_code`
//! に一本化する。座標・画素はログへ出力しない（security.md A09）。
//!
//! iOS ではコールバックが egui の `update` 外から来るので、結果を置いた直後に
//! `ios::request_repaint` で次フレームを起こす（契約表 4）。

use fandhe_device::map_view::{MapRegion, MapSnapshotter};
use fandhe_device::DeviceError;

// SAFETY: `apple/fandhe_apple_map_view.m` が定義する C ABI。契約は `.m` 冒頭コメントと
// 下記 `start` のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_apple_map_view_start(
        request_id: u64,
        lat: f64,
        lon: f64,
        span_m: f64,
        width: u32,
        height: u32,
    ) -> i32;
    fn fandhe_apple_map_view_cancel(request_id: u64);
}

/// MapKit による [`MapSnapshotter`]。
pub struct AppleMapSnapshotter;

static INSTANCE: AppleMapSnapshotter = AppleMapSnapshotter;

/// プロセスで共有する実装を返す（`device/map_view.rs` の最終形の入口）。
pub(crate) fn snapshotter() -> &'static AppleMapSnapshotter {
    &INSTANCE
}

impl MapSnapshotter for AppleMapSnapshotter {
    #[allow(unsafe_code)]
    fn start(
        &self,
        request_id: u64,
        region: MapRegion,
        size_px: (u32, u32),
    ) -> Result<(), DeviceError> {
        // SAFETY: 引数はすべて値渡しのプリミティブで、ポインタを渡さない。座標は
        // `MapRegion::new` が検証済み。`.m` 側は main スレッド以外なら副作用なく 5 を返す。
        let code = unsafe {
            fandhe_apple_map_view_start(
                request_id,
                region.center_lat(),
                region.center_lon(),
                region.span_m(),
                size_px.0,
                size_px.1,
            )
        };
        super::map_view::outcome_from_apple_code(code)
    }

    #[allow(unsafe_code)]
    fn cancel(&self, request_id: u64) {
        // SAFETY: 値渡しのみ。`.m` 側は id が進行中のものと一致するときだけ取り消す。
        unsafe { fandhe_apple_map_view_cancel(request_id) }
    }
}

/// `.m` 側の完了から呼ばれる `extern "C"` 関数。コードの解釈は
/// `device/map_view.rs::outcome_from_apple_code`。`ptr` は呼び出しの間だけ有効で、所有権は
/// 移らない（解放関数は公開しない）。画素は検査を通ったときだけ呼び出しの中でコピーする。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_map_view_outcome(
    request_id: u64,
    code: i32,
    ptr: *const u8,
    len: usize,
    width: u32,
    height: u32,
) {
    let side = fandhe_core::RGBA_MAX_SIDE as usize;
    let valid = code == 0
        && !ptr.is_null()
        && (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            == Some(len)
        && len <= side * side * 4;
    let rgba = if valid {
        // SAFETY: `.m` 側が `len` バイトの有効なバッファを渡す契約。上で NULL でないこと・
        // `len == width*height*4`・上限以下を確かめた。呼び出しが返るまでは生存し、
        // ここで `Vec` へコピーするため返却後は参照しない。
        Some(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
    } else {
        None
    };
    super::map_view::bridge::deliver_rgba(request_id, code, rgba, width, height);
    #[cfg(target_os = "ios")]
    crate::ios::request_repaint();
}
