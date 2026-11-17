//! iOS 向けファイル選択（Issue #873、案 B）。`crate::driver` の iOS 経路から [`present_file_picker`] が呼ばれ、`ios/fandhe_ios_file_picker.m`
//! （`UIDocumentPickerViewController`）が提示する。他の端末機能の実装と異なり `FilePicker` トレイトを **通らない**
//! （`crate::contract` の doc「案 B」参照）。提示関数は即時 return し、結果は非同期に [`fandhe_rust_file_picker_outcome`] 経由で
//! `crate::driver::bridge` へ直接届く。ブロッキング待ちが無いため、ワーカースレッド・mailbox は持たない。
//!
//! `unsafe` は FFI の呼び出しと、受け口のポインタを読む 1 行、`#[unsafe(no_mangle)]` に限る。読み取り・8 MiB 判定・EOF の扱いは
//! `fandhe_device::file_picker::picked_from_path_bytes`（`read_picked` に一本化）が担い、`.m` 側で読み取りループを持たない。

use fandhe_device::file_picker::picked_from_path_bytes;

use crate::error::FilePickerError;

// SAFETY: `ios/fandhe_ios_file_picker.m` が定義する C ABI（`build.rs` が iOS ターゲットのみコンパイル・リンクする）。契約は
// 下記 [`present_file_picker`]・[`fandhe_rust_file_picker_outcome`] のコメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_file_picker_pick(request_id: u64) -> i32;
}

/// `ios/fandhe_ios_file_picker.m` の delegate から呼ばれる `extern "C"` エクスポート関数（Issue #873、案 B）。契約（`.m` ファイル冒頭コメント）:
///   `code`: 0 = 選択（`path_ptr`/`path_len` が tmp コピーの絶対パスを指す UTF-8 バイト列）・1 = キャンセル・それ以外 = 内部エラー。
///   0 以外では `path_ptr` を参照しない。
///
/// 選択時は本関数内で `picked_from_path_bytes`（`read_picked`）を同期に呼び切ってから戻る契約（`.m` 側はこの呼び出しが戻ってから
/// tmp コピーを削除する）。iOS の `asCopy:YES` コピーは元のファイル名を保つため、`read_picked` が `path` から導く名前をそのまま使う
/// （Android のように表示名を別途上書きする必要が無い）。
///
/// 呼び出しスレッドは `.m` 側の global queue（main ではない、選択時の同期読み取りで main を塞がないため）。
/// `bridge::deliver` はスレッド安全なため、どのスレッドから呼ばれても安全（`unsafe` はポインタを読む 1 箇所のみに閉じる）。
///
/// `#[allow(unsafe_code)]` は `#[unsafe(no_mangle)]` が `lib.rs` の iOS `deny(unsafe_code)` 対象になるための宣言上の要請。
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
extern "C" fn fandhe_rust_file_picker_outcome(
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
    // code 3 = 進行中の選択画面がある間の新しい要求（busy。`.m` 側が返す）。
    let result = if code == 3 {
        Err(FilePickerError::Internal("file_picker busy"))
    } else {
        picked_from_path_bytes(code, path, "file_picker").map_err(FilePickerError::from)
    };
    crate::driver::bridge::deliver(request_id, result);
    // 改訂 4: pull 型 bridge は sender も画面の `generation` も持たないため、結果を置いただけでは次の描画が起きる保証が無い
    // （`drive_mobile_with` は毎フレームの effect 呼び出しでしか拾えない）。無条件に呼んで確実に起こす。
    fandhe_runtime::request_repaint();
}

/// iOS 提示関数（Issue #873、案 B）。`crate::driver` のモバイル版 `drive_mobile_with` から呼ばれる。`request_id` は `bridge::begin` が
/// 払い出すプロセス内一意な ID（画面の `generation` ではない）。即時 return（`.m` 側が `dispatch_async(main)` へ提示処理を積んで戻る）
/// ため、呼び出し元をブロックしない。iOS は常に `true`（`.m` 側の `fandhe_ios_file_picker_pick` は常に 0）を返し、提示失敗
/// （top view controller 不在等）も code 2 で非同期に通知する（`.m` ファイル冒頭コメント参照）。戻り値 `false` は Android の JNI 失敗専用
/// （同期的に提示依頼自体が失敗しコールバックが二度と来ないケース）で、iOS では到達しない。
pub(crate) fn present_file_picker(request_id: u64) -> bool {
    #[allow(unsafe_code)]
    // SAFETY: `fandhe_ios_file_picker_pick` の契約（`.m` ファイル冒頭コメント）: 引数はプリミティブ整数のみでポインタを経由しない。
    // 戻り値 0 は提示を依頼できたことのみを意味し、実際の選択結果は非同期に `fandhe_rust_file_picker_outcome` 経由で `bridge` へ届く。
    unsafe {
        fandhe_ios_file_picker_pick(request_id) == 0
    }
}
