//! iOS 向け `fandhe-device` 実装（Issue #751）。写真選択（Issue #1104）の `PHPickerViewController` の受け口を持つ。
//! 実カメラ撮影は機能 crate `fandhe-cap-camera-capture`、読み上げ・ファイル選択・マイク録音・音声認識は機能 crate
//! `fandhe-cap-tts`・`fandhe-cap-file-picker`・`fandhe-cap-mic-record`・`fandhe-cap-speech-recognition` へ移した
//! （Issue #1170・#1354）。写真選択の `.m`（`ios/fandhe_ios_photo_picker.m`）は `device/photo_pick.rs` の iOS 経路から呼ばれ、
//! 結果は非同期に [`fandhe_rust_photo_picker_outcome`] 経由で `device/photo_pick.rs::bridge` 等へ届く。受け取りの検証と読み取りは
//! `fandhe_device::file_picker::picked_from_path_bytes` に一本化している（ポインタを読む `unsafe` はここに 1 行だけ残る）。

use fandhe_device::file_picker::picked_from_path_bytes;

// SAFETY: `ios/fandhe_ios_photo_picker.m` が定義する C ABI（`build.rs` が iOS
// ターゲットのみ・feature `device` 有効時のみコンパイル・リンクする）。契約は
// 下記 [`present_photo_picker`]・[`fandhe_rust_photo_picker_outcome`] のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_photo_picker_pick(request_id: u64) -> i32;
}

/// `ios/fandhe_ios_photo_picker.m` の完了ハンドラから呼ばれる `extern "C"` エクスポート
/// （Issue #1104）。`code` の意味・パスの寿命は `fandhe-cap-file-picker` の `fandhe_rust_file_picker_outcome` と同じ
/// （0 = 選択でパスは `NSItemProvider` が渡した一時ファイル、1 = キャンセル、
/// それ以外 = 変換失敗等の内部エラー）。選択時は `fandhe_device::file_picker::picked_from_path_bytes` が同期に読み切って
/// から戻り、続けて 2 段検査（`validate_photo` → 復号確認）を **この呼び出しスレッド**
/// （`NSItemProvider` のバックグラウンドキュー）で行う。UI スレッドでは復号しない。
#[allow(unsafe_code)]
#[no_mangle]
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
    let result = picked_from_path_bytes(code, path, "photo_picker pick");
    // 同じ PHPicker の受け口を写真選択と文字認識（#1252）で共有し、id のタグで振り分ける。
    if crate::device::ios_image_recognition::owns(request_id) {
        crate::device::ios_image_recognition::deliver(request_id, result);
    } else if crate::device::ios_barcode_scan::owns(request_id) {
        crate::device::ios_barcode_scan::deliver(request_id, result);
    } else {
        crate::device::photo_pick::bridge::deliver(request_id, result);
    }
    // pull 型 bridge は sender を持たないため、結果を置いたら描画を起こす
    // （`fandhe-cap-file-picker` の `fandhe_rust_file_picker_outcome` と同じ）。
    crate::ios::request_repaint();
}

/// iOS 提示関数（Issue #1104）。`device/photo_pick.rs::drive_mobile` から呼ばれ、
/// 即時 return する（`.m` 側が `dispatch_async(main)` で提示する）。提示失敗も code 2 で
/// 非同期に通知するため、iOS では常に `true`。
pub(crate) fn present_photo_picker(request_id: u64) -> bool {
    #[allow(unsafe_code)]
    // SAFETY: `fandhe_ios_photo_picker_pick` の契約（`.m` ファイル冒頭コメント）:
    // 引数はプリミティブ整数のみでポインタを経由しない。戻り値 0 は提示を依頼できた
    // ことのみを意味し、結果は非同期に `fandhe_rust_photo_picker_outcome` へ届く。
    unsafe {
        fandhe_ios_photo_picker_pick(request_id) == 0
    }
}
