//! macOS / iOS 向けファイルの関連付け受信（Issue #416。親 #102 の a・b-ios）。
//! `device/file_association.rs::platform` の macOS 分岐と `main.rs`（登録）から使われる。
//! macOS は `apple/fandhe_apple_file_association.m` が登録する Apple Event `kAEOpenDocuments`
//! ハンドラから [`fandhe_rust_file_association_received`] が呼ばれる。iOS 側の `.m` は
//! b-ios が同じファイルの `TARGET_OS_IOS` 分岐に書く（同じ C ABI 関数を呼ぶ）。
//!
//! 受け口 [`INBOX`] はプロセス全体の static で、UI ができる前（コールドスタート）に届いた
//! ファイルもここに溜まる。検査・読み取り・保持は共通側の
//! `fandhe_device::file_association::FileInbox` に任せ、本モジュールは FFI の橋渡し
//! （`unsafe` 3 箇所）だけを持つ。パス・名前・中身はログへ出さない。

use fandhe_device::file_association::FileInbox;
#[cfg(target_os = "macos")]
use fandhe_device::file_association::{FileAssociation, FileSnapshot};
#[cfg(target_os = "macos")]
use fandhe_device::DeviceError;

#[cfg(target_os = "macos")]
// SAFETY: `apple/fandhe_apple_file_association.m` が定義する C ABI（`build.rs` がコンパイル・
// リンクする）。契約は `.m` ファイル冒頭コメントに集約する。
#[allow(unsafe_code)]
unsafe extern "C" {
    /// `kAEOpenDocuments` ハンドラを登録する。main thread から呼ぶ。冪等。
    fn fandhe_apple_file_association_register();
}

/// 受信したファイルの受け口（プロセス全体で 1 つ）。
static INBOX: FileInbox = FileInbox::new();

/// 受信ハンドラを登録する。イベントループ開始前（`run_native` より前）に main thread から
/// 呼ぶ。起動時のファイルに間に合わせるため `.m` 側が起動通知も使う。
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn register() {
    // SAFETY: 引数なしの C 関数。main thread から呼ぶ契約は呼び出し元（`main`）が守る。
    unsafe { fandhe_apple_file_association_register() }
}

/// 受信時刻（UNIX ms）。表示用の時刻で、経過時間の計測ではない。エポック前なら 0。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `.m` 側のハンドラから呼ばれる。`ptr` はファイルパスのバイト列（`len` バイト、
/// `fileSystemRepresentation`）で、呼び出し中だけ有効。null または `len == 0` は
/// 「パスが取れなかった」として拒否に数える。読み取りは呼び出しの中で完結する。
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_file_association_received(ptr: *const u8, len: usize) {
    use std::os::unix::ffi::OsStrExt;
    let path = if ptr.is_null() || len == 0 {
        None
    } else {
        // SAFETY: `.m` 側が `fileSystemRepresentation` の先頭と `strlen` を渡す契約で、
        // ポインタは本呼び出しの間だけ有効。null と len 0 は上で除外済みで、読み取りを
        // 含め呼び出し中に完結させる（保持しない）。
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        Some(std::path::Path::new(std::ffi::OsStr::from_bytes(bytes)))
    };
    INBOX.receive_path(path, now_ms());
}

/// macOS 向け `FileAssociation` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
#[cfg(target_os = "macos")]
pub(crate) struct AppleFileAssociation;

#[cfg(target_os = "macos")]
impl FileAssociation for AppleFileAssociation {
    fn snapshot(&self) -> Result<FileSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}
