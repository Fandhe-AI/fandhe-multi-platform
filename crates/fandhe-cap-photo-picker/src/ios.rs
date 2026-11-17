//! iOS 向け写真選択（Issue #1104）の受け口。`ios/fandhe_ios_photo_picker.m`（`PHPickerViewController`）を C ABI で呼び、結果は
//! 非同期に [`fandhe_rust_photo_picker_outcome`] 経由で、[`present_ios_photo_picker`] に渡された受け取り（`IosPickDeliver`）へ届く。
//! 写真選択の `driver`・文字認識（`fandhe-cap-image-recognition`）・バーコード読み取り（`fandhe-cap-barcode-scan`）が同じ PHPicker の受け口を
//! 共有する。要求ごとに受け取りを `PendingDelivers` へ登録し、受け口は id で 1 件を取り出して呼ぶ（機能を知らない）。
//!
//! `PhotoPicker` トレイトを通らない。提示関数は即時 return し、ブロッキング待ちが無いためワーカースレッド・mailbox は持たない。
//! `unsafe` は FFI の呼び出しと、受け口のポインタを読む 1 行、`#[unsafe(no_mangle)]` に限る。読み取り・8 MiB 判定・EOF の扱いは
//! `fandhe_device::file_picker::picked_from_path_bytes`（`read_picked` に一本化）が担う。

use fandhe_device::file_picker::picked_from_path_bytes;

use crate::error::PhotoPickerError;
pub use crate::pending::IosPickDeliver;
use crate::pending::PendingDelivers;

static PENDING: PendingDelivers = PendingDelivers::new();

// SAFETY: `ios/fandhe_ios_photo_picker.m` が定義する C ABI（`build.rs` が iOS ターゲットのみコンパイル・リンクする）。契約は
// 下記 [`present_ios_photo_picker`]・[`fandhe_rust_photo_picker_outcome`] のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_photo_picker_pick(request_id: u64) -> i32;
}

/// `ios/fandhe_ios_photo_picker.m` の完了ハンドラから呼ばれる `extern "C"` エクスポート（Issue #1104）。`code` の意味・パスの寿命は
/// `fandhe-cap-file-picker` の `fandhe_rust_file_picker_outcome` と同じ（0 = 選択でパスは `NSItemProvider` が渡した一時ファイル、
/// 1 = キャンセル、それ以外 = 変換失敗等の内部エラー）。選択時は `picked_from_path_bytes` が同期に読み切ってから戻り、続けて
/// 登録された受け取りが 2 段検査（`validate_photo` → 復号確認）などを **この呼び出しスレッド**（`NSItemProvider` のバックグラウンド
/// キュー）で行う。UI スレッドでは復号しない。知らない id は捨てる。
///
/// 受け取りの後に `request_repaint` を呼ぶ。pull 型の bridge は sender を持たないため、結果を置いただけでは入力のない egui が
/// 次フレームを描かず、画面が処理中のまま止まる。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn fandhe_rust_photo_picker_outcome(
    request_id: u64,
    code: i32,
    path_ptr: *const u8,
    path_len: usize,
) {
    // 0 以外では `path_ptr` を参照しない。0 なのに NULL は契約違反で、`picked_from_path_bytes` が失敗にする（成功にしない）。
    let path = if code == 0 && !path_ptr.is_null() {
        // SAFETY: `code == 0` の契約により `path_ptr` は `path_len` バイトの有効な読み取り可能領域を指す UTF-8 バイト列。
        // `picked_from_path_bytes` はこの呼び出しの中で読み切って戻るため、以降このポインタを再利用しない。
        Some(unsafe { std::slice::from_raw_parts(path_ptr, path_len) })
    } else {
        None
    };
    let result =
        picked_from_path_bytes(code, path, "photo_picker pick").map_err(PhotoPickerError::from);
    PENDING.dispatch(request_id, result);
    fandhe_runtime::request_repaint();
}

/// iOS 提示関数。`deliver` を要求ごとに登録して PHPicker を出す。即時 return する（`.m` 側が `dispatch_async(main)` で提示する）。
/// 提示失敗も code 2 で非同期に通知するため、iOS では通常 `true`。`false`（提示依頼自体の同期的な失敗）のときは登録を消して返す
/// （受け取りは呼ばれない。呼び出し側が失敗として確定する）。
pub fn present_ios_photo_picker(deliver: IosPickDeliver) -> bool {
    let request_id = PENDING.register(deliver);
    #[allow(unsafe_code)]
    // SAFETY: `fandhe_ios_photo_picker_pick` の契約（`.m` ファイル冒頭コメント）: 引数はプリミティブ整数のみでポインタを経由しない。
    // 戻り値 0 は提示を依頼できたことのみを意味し、結果は非同期に `fandhe_rust_photo_picker_outcome` へ届く。
    let presented = unsafe { fandhe_ios_photo_picker_pick(request_id) == 0 };
    if !presented {
        PENDING.cancel(request_id);
    }
    presented
}
