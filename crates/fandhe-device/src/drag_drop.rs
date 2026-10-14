//! ドラッグ & ドロップ受け取り（drop target）の抽象（Phase 11、親 #1013、Issue #1014）。
//!
//! アプリの画面へファイルを落として受け取る側のみを扱う。アプリからドラッグを
//! 始める送り側は親 issue の方針でスコープ外（実装計画 §8）。
//!
//! [`file_picker`](crate::file_picker) の型をそのまま使い回す。ドロップは OS の
//! 「要求 → 応答」の往復を経ないユーザー操作そのもの（ウィンドウへ落とした瞬間に
//! 中身が確定する）であり、`FilePicker` トレイトのような「1 回呼ぶとダイアログが
//! 開く」抽象は不要。世代番号・`RequestSlot`・ワーカースレッドも要らない
//! （`crates/fandhe-egui-proxy/src/device/drag_drop.rs` が winit のイベントを
//! そのまま UI スレッドで同期処理する、モジュール doc 参照）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 成功・1 件ずつの失敗・キャンセル（ドラッグが領域外へ出た/取り消された）・
//!   0 件は別の結果として扱う。0 件のドロップは失敗として報告する
//!   （契約表 6「嘘の成功」。呼び出し元＝`fandhe-core` の画面状態機械側の責務）
//! - 複数ファイルは 1 件ずつ [`Result`] を持つ。一部が失敗しても全体を成功にしない
//! - 1 ファイルあたりの上限は [`MAX_FILE_BYTES`]（`file_picker` と共有する唯一の SSOT）。
//!   超過は [`DropError::TooLarge`]
//! - フォルダは受け取れない（[`DropError::Folder`]）
//! - ファイル名・パスをエラー文言に含めない（security.md A09）
//! - 後続の iOS / Android / Web（兄弟の子 issue）は本モジュールの [`DropError`]・
//!   上限定数（[`MAX_FILE_BYTES`]・[`MAX_DROP_FILES`]）を共有すること

use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

pub use crate::file_picker::{MAX_FILE_BYTES, PickedFile, read_picked_from};

/// 1 回のドロップで読む件数の上限。超えた分は読まずに 1 件ずつ [`DropError::TooMany`]
/// にする。UI スレッドで同期に読む都合上、停止時間と I/O 量を抑える目的
/// （`MAX_DROP_FILES` 件 × [`MAX_FILE_BYTES`] が 1 回のドロップで読む上限になる）。
pub const MAX_DROP_FILES: usize = 32;

/// ドロップされた 1 件の受け取りに失敗した理由。文言にはパスもファイル名も
/// 含めない（security.md A09。呼び出し元＝`fandhe-egui-proxy` の効果ドライバが
/// 表示用の名前を別途 `Path::file_name` から取り出す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DropError {
    /// ドロップされたパスがフォルダだった（symlink は辿った上での判定）。
    #[error("フォルダは受け取れません")]
    Folder,
    /// [`MAX_FILE_BYTES`] を超えるファイルだった。
    #[error("{} MiB を超えるファイルは受け取れません", MAX_FILE_BYTES / (1024 * 1024))]
    TooLarge,
    /// 存在しない・権限がない等、内容を読み取れなかった。
    #[error("読み取れませんでした")]
    Unreadable,
    /// 1 回のドロップで [`MAX_DROP_FILES`] 件を超えた分（超過分は読まない）。
    #[error("一度に受け取れるのは {MAX_DROP_FILES} 件までです")]
    TooMany,
}

/// ドロップされた 1 件のパスの内容を読み切る。
///
/// 種別確認（ディレクトリ・名前付きパイプ等の除外）は、パスを stat してから
/// 別途 `File::open` する 2 段構えにしない。2 段構えだと、確認と open の間に
/// パスの実体が差し替わる TOCTOU（例: 通常ファイル → FIFO）が生まれ、
/// 差し替え後の `File::open` が書き手の現れない FIFO を無期限に待って UI
/// スレッドを停止させ得る（契約表 1「ライフサイクル境界」・codex 指摘 #1018
/// 対応）。代わりに [`open_for_type_check`] で 1 回だけ開き、以後の種別確認・
/// 読み取りは同じハンドルに対して行う（symlink は open 時に辿った後の実体を
/// 見るので、通常ファイルを指す symlink は許可する）。
///
/// 通常ファイルなら [`read_dropped_from`]（サイズ超過・その他失敗の写像を一本化した
/// 共通部品）へ委譲する。
///
/// 呼び出し元: `fandhe-egui-proxy` の `device/drag_drop.rs`（macOS/Windows。egui
/// `RawInput::dropped_files` が渡すパスをそのまま渡す）と `device/ios_drag_drop.rs`
/// （iOS、Issue #1015。`NSItemProvider` の `loadFileRepresentation...` completion が
/// 渡す一時ファイルの絶対パスを渡す。この一時ファイルは completion が return すると
/// OS が削除する契約のため、completion の中で同期に読み切る）。
pub fn read_dropped(path: &Path) -> Result<PickedFile, DropError> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or(DropError::Unreadable)?;

    // Windows はフォルダを `File::open` で開けず失敗するため、開けなかったときは
    // パスの種別を見てフォルダなら `Folder` を返す（Cursor Bugbot 指摘、PR #1018）。
    // ここでの `is_dir` は失敗理由の分類にだけ使い、読み取りには使わない
    // （TOCTOU の対象外）。
    let file = open_for_type_check(path).map_err(|_| {
        if path.is_dir() {
            DropError::Folder
        } else {
            DropError::Unreadable
        }
    })?;
    let metadata = file.metadata().map_err(|_| DropError::Unreadable)?;
    if metadata.is_dir() {
        return Err(DropError::Folder);
    }
    if !metadata.is_file() {
        return Err(DropError::Unreadable);
    }

    read_dropped_from(file, name)
}

/// 既に開いたファイルハンドルから内容を読み切る（種別確認済みの `File` を渡す契約）。
///
/// [`read_picked_from`]（`take(max+1)` の 1 回の読み取りで上限判定）へ委譲し、サイズ超過
/// （`DeviceError::InvalidInput`）は [`DropError::TooLarge`] へ、それ以外の失敗は
/// [`DropError::Unreadable`] へ写す（[`read_dropped`] と Android 経路の唯一の写像箇所、
/// 実装計画 §3-4「fd 読み取り」）。
///
/// 呼び出し元（後続 Android、Issue #1016）: `fandhe-egui-proxy` の
/// `device/android.rs::nativeOnDragDropItem` 実体。SAF が返す fd は pipe のことがあり
/// `is_file()` 検査が使えないため、[`read_dropped`] の種別確認（`open_for_type_check`/
/// `metadata().is_file()`）は経由せず、Java 側で既に「通常ファイル」と判定済みの fd を
/// そのまま渡す（`ContentResolver#getType` が `Document.MIME_TYPE_DIR` かどうかで
/// フォルダを弾いた残り）。
pub fn read_dropped_from(file: File, name: String) -> Result<PickedFile, DropError> {
    read_picked_from(file, name, MAX_FILE_BYTES).map_err(|e| match e {
        crate::DeviceError::InvalidInput(_) => DropError::TooLarge,
        _ => DropError::Unreadable,
    })
}

/// [`read_dropped`] が種別確認と読み取りを同一ハンドルで行うための open。
///
/// Unix（macOS・Linux 等）では `O_NONBLOCK` を付けて開く（codex 指摘、PR #1018:
/// Linux でも FIFO の open は書き手を無期限に待つ）。付けないと、パスが FIFO であるとき
/// （直接ドロップされた場合・open 直前に差し替えられた場合のいずれも）open
/// 自体が書き手を無期限に待ってしまう。`O_NONBLOCK` は通常ファイルの読み取り
/// 動作には影響しない（読み取りは即座に完了する）ため、返したハンドルを
/// そのまま [`read_picked_from`] へ渡してよい。Windows の名前付きパイプは
/// サーバー不在時 `CreateFile` が即座に失敗する（無期限待ちにならない）ため
/// 素の `File::open` でよい。
#[cfg(unix)]
fn open_for_type_check(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_for_type_check(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_file(contents: &[u8]) -> std::path::PathBuf {
        // 並列実行のテスト同士が同じ時刻（macOS の時計は実質マイクロ秒単位）で
        // 同名ファイルを奪い合わないよう、プロセス内の連番も名前に含める。
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fandhe-drag-drop-test-{}-{}-{}",
            std::process::id(),
            seq,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, contents).expect("一時ファイルを書ける");
        path
    }

    #[test]
    fn read_dropped_from_maps_size_over_limit_to_too_large() {
        let path = write_temp_file(&vec![0u8; MAX_FILE_BYTES + 1]);
        let file = File::open(&path).unwrap();
        let err = read_dropped_from(file, "a.bin".to_string()).unwrap_err();
        assert_eq!(err, DropError::TooLarge);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_dropped_from_maps_empty_name_to_unreadable() {
        let path = write_temp_file(b"hello");
        let file = File::open(&path).unwrap();
        // `read_picked_from` は空/NUL 込みの名前を `DeviceError::Internal` にする
        // （`file_picker.rs` doc 参照）。Android 経路は本テストと同じ写像を通る。
        let err = read_dropped_from(file, String::new()).unwrap_err();
        assert_eq!(err, DropError::Unreadable);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_dropped_returns_name_and_bytes_for_regular_file() {
        let path = write_temp_file(b"hello");
        let picked = read_dropped(&path).unwrap();
        assert_eq!(picked.bytes, b"hello");
        assert_eq!(
            picked.name,
            path.file_name().unwrap().to_string_lossy().into_owned()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_dropped_rejects_directory() {
        let err = read_dropped(std::env::temp_dir().as_path()).unwrap_err();
        assert_eq!(err, DropError::Folder);
    }

    #[test]
    fn read_dropped_rejects_file_over_limit() {
        let path = write_temp_file(&vec![0u8; MAX_FILE_BYTES + 1]);
        let err = read_dropped(&path).unwrap_err();
        assert_eq!(err, DropError::TooLarge);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_dropped_accepts_file_exactly_at_limit() {
        let path = write_temp_file(&vec![0u8; MAX_FILE_BYTES]);
        let picked = read_dropped(&path).unwrap();
        assert_eq!(picked.bytes.len(), MAX_FILE_BYTES);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_dropped_reports_unreadable_for_missing_path() {
        let path = std::env::temp_dir().join("fandhe-drag-drop-test-does-not-exist");
        let err = read_dropped(&path).unwrap_err();
        assert_eq!(err, DropError::Unreadable);
    }

    /// 0 バイトのファイルは正当なファイルとして受け取る（「中身が空」であって
    /// 「ドロップが 0 件」とは別物。0 件の扱いは呼び出し元の画面状態機械の責務）。
    #[test]
    fn read_dropped_accepts_empty_file() {
        let path = write_temp_file(b"");
        let picked = read_dropped(&path).unwrap();
        assert!(picked.bytes.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    /// 名前付きパイプ（FIFO）は通常ファイルではないため、`File::open` が書き手
    /// 待ちで無期限停止する前に `Unreadable` で弾く（codex 指摘 #1018 対応）。
    /// `mkfifo` が無い環境（CI 等）ではスキップする。
    #[test]
    fn read_dropped_rejects_named_pipe_without_blocking() {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fandhe-drag-drop-test-fifo-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let status = std::process::Command::new("mkfifo").arg(&path).status();
        let Ok(status) = status else {
            return;
        };
        if !status.success() {
            return;
        }
        let err = read_dropped(&path).unwrap_err();
        assert_eq!(err, DropError::Unreadable);
        let _ = std::fs::remove_file(&path);
    }
}
