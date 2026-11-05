//! iOS 向けカメラ実装（Issue #751）。`crate::platform_camera`（`cfg(target_os = "ios")` 分岐）から
//! 構築される。
//!
//! 実装は `unsafe extern "C"` で `ios/fandhe_ios_camera.m`（本 crate の `build.rs` が iOS
//! ターゲットのみコンパイルする ObjC。UIKit の `UIImagePickerController` + AVFoundation の
//! 権限確認）を呼ぶ。`unsafe` はこの FFI 呼び出し（[`IosCamera::capture`] と解放用の
//! [`free_captured_buffer`]）に閉じ、`// SAFETY:` で契約を明記する
//! （`lib.rs` の iOS `deny(unsafe_code)` 緩和、coding-rust.md の FFI 例外規定準拠）。

use crate::{Camera, CameraError, Image};

/// 縮小後の画像の長辺上限（ピクセル）。`fandhe_ios_camera.m` 側の
/// `FandheEncodeImageRGBA` へそのまま渡す。上限を固定することで最大確保量を
/// 512*512*4 = 1 MiB 程度に抑える（security.md A05、メモリ枯渇対策）。
const MAX_CAPTURE_EDGE: u32 = 512;

// SAFETY: `ios/fandhe_ios_camera.m` が定義する C ABI（`build.rs` が iOS ターゲットの
// みコンパイル・リンクする）。契約は下記 `capture` 実装のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_camera_capture(
        max_edge: u32,
        out_rgba: *mut *mut u8,
        out_w: *mut u32,
        out_h: *mut u32,
    ) -> i32;
    fn fandhe_ios_camera_free(ptr: *mut u8);
}

/// `fandhe_ios_camera_capture` が確保したバッファを解放する（`NULL` 許容）。
/// 契約違反（戻り値と out 引数の不整合）を検出したエラー経路からまとめて呼ぶための
/// 小関数（`unsafe` を 1 箇所に集約し、`if`/`match` の枝ごとに SAFETY コメントの
/// 直前行が途切れる問題を避ける、`unsafe-audit.sh` の探索仕様に合わせた形）。
#[allow(unsafe_code)]
fn free_captured_buffer(ptr: *mut u8) {
    // SAFETY: `fandhe_ios_camera_free` は NULL でも安全（`.m` 側は `free(NULL)` に
    // 委譲するのみ）。非 NULL の場合は `fandhe_ios_camera_capture` が返した
    // malloc 領域を指す前提で、呼び出し元（`capture`）はこの呼び出し後に
    // `ptr` を再利用しない契約にする。
    unsafe { fandhe_ios_camera_free(ptr) };
}

/// iOS 実カメラ実装。`fandhe_ios_camera.m` の `UIImagePickerController` を
/// ワーカースレッド（`spawn_capture` が呼ぶ）から同期的に起動する
/// （ObjC 側が `dispatch_async(main)` + セマフォで橋渡しする、`.m` ファイルのコメント参照）。
pub struct IosCamera;

impl Camera for IosCamera {
    #[allow(unsafe_code)]
    fn capture(&self) -> Result<Image, CameraError> {
        let mut out_rgba: *mut u8 = std::ptr::null_mut();
        let mut out_w: u32 = 0;
        let mut out_h: u32 = 0;

        // SAFETY: `fandhe_ios_camera_capture` の契約（`.m` ファイル冒頭コメント）:
        // 戻り値 0 のときのみ `out_rgba` が `out_w * out_h * 4` バイトの有効な
        // malloc 領域を指し、所有権は呼び出し側（本関数）に移る。0 以外では
        // 3 個の out ポインタはいずれも書き換えられない（このスコープ内で
        // 初期値のまま扱う）。3 個の out 引数は上記でスタック上に確保済みの
        // 有効な `u32`/`*mut u8` へのポインタであり、`fandhe_ios_camera` 以外の
        // スレッドが同時に書き換えることはない（呼び出しはこの関数呼び出し中のみ）。
        let code = unsafe {
            fandhe_ios_camera_capture(MAX_CAPTURE_EDGE, &mut out_rgba, &mut out_w, &mut out_h)
        };

        match code {
            0 => {}
            1 => return Err(CameraError::Unsupported("camera")),
            2 => return Err(CameraError::PermissionDenied("camera")),
            3 => return Err(CameraError::Cancelled("camera")),
            // 4 = 内部エラー（presenter 不在・エンコード失敗・二重呼び出し等、
            // `fandhe_ios_camera.m` 冒頭コメントの契約）。機種としてカメラが
            // 使えないわけではないため `Unsupported` にせず区別する
            // （PR #791 レビュー指摘: Internal errors reported as unsupported）。
            _ => return Err(CameraError::Internal("camera bridge")),
        }

        if out_w == 0 || out_h == 0 || out_rgba.is_null() {
            // 契約違反（0 を返したのに寸法/ポインタが不正）を検出した経路。
            // `out_rgba` が確保済みなら解放してからエラーへ落とす
            // （`fandhe_ios_camera_free` は NULL を渡しても安全、`.m` 側は
            // `free(NULL)` に委譲するのみ。SAFETY コメントは下記関数側に集約する）。
            free_captured_buffer(out_rgba);
            return Err(CameraError::InvalidImageSize(out_w, out_h));
        }

        let byte_len = match (out_w as usize)
            .checked_mul(out_h as usize)
            .and_then(|px| px.checked_mul(4))
        {
            Some(len) => len,
            None => {
                // 契約違反（`out_w`/`out_h` の積が u32 に収まらない）を検出した経路。
                free_captured_buffer(out_rgba);
                return Err(CameraError::InvalidImageSize(out_w, out_h));
            }
        };

        // SAFETY: 戻り値 0 の契約により `out_rgba` は `byte_len` バイトの有効な
        // 読み取り可能領域を指す（`byte_len` は `out_w * out_h * 4` と一致するよう
        // 上で計算済み）。`to_vec` でコピーした後、直後に [`free_captured_buffer`]
        // で ObjC 側の所有権を解放する（このコピー後は `out_rgba` を再利用しない）。
        let rgba = unsafe { std::slice::from_raw_parts(out_rgba, byte_len) }.to_vec();
        free_captured_buffer(out_rgba);

        Ok(Image {
            width: out_w,
            height: out_h,
            rgba,
        })
    }
}
