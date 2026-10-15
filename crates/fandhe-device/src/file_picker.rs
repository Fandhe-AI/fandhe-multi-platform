//! ファイル選択ダイアログの抽象（Phase 11、Issue #872）。
//!
//! `Clipboard`/`Tts` と同じ request/response の一発呼び出し型（1 回の `pick()` = 1 回の
//! ダイアログ）。[`FilePicker`] トレイトの同期契約はデスクトップ（macOS/Windows）専用。
//!
//! iOS/Android/Web は本トレイトを通らない。OS がコールバック型（ダイアログ提示 → 結果を
//! 非同期通知）であり、ワーカースレッド + mailbox で同期化すると
//! トレイト実装・mailbox・コア状態機械の 3 層管理が生まれ、再生成・プロセス死・遅延配送で
//! 継ぎ目がずれるため。代わりに `fandhe-web-dom`・`fandhe-egui-proxy`（iOS/Android 橋渡し）は
//! コアの画面状態機械 `fandhe_core::screens::file_pick`（`take_request`/`finish_pick` +
//! `generation` 照合）へ OS コールバックから直接書き戻す。OS 側の責務は
//! 「ダイアログ提示 → 結果を渡された要求 ID（橋渡し側がプロセス内で採番。`generation` との対応は橋渡し側が持つ）付きで 1 回だけ通知 → 一時ファイルの
//! 後始末」のみとし、ネイティブ（iOS/Android）の読み取り・8 MiB 判定は本クレートの
//! [`read_picked`]（iOS: サンドボックス tmp コピーのパス）・[`read_picked_from`]
//! （Android: SAF の fd を `OwnedFd` で借用した `File`。`/proc/self/fd/N` の再 open は
//! pipe 型プロバイダで権限再確認に失敗するため使わない）に一本化する。Web はパスを
//! 持たない（DOM の `File` を読む）ため [`MAX_FILE_BYTES`] と 3 値の結果契約のみを共有する
//! （3 値・上限・読み取り失敗の扱いはトレイト経由・状態機械経由のどちらでも同一）。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - 1 回の [`FilePicker::pick`] 呼び出し = 1 回のダイアログ。ユーザーが閉じるまで
//!   確定しない。**固定タイムアウトを掛けない**
//! - 結果は 3 値: 選択 `Ok(Some(PickedFile))` / キャンセル `Ok(None)`（失敗ではない）/
//!   失敗 `Err`
//! - ファイルパスは返さない・表示しない・ログに出さない（モバイル/Web ではパス自体が
//!   無意味であり、返り値に含めると呼び出し元がうっかり表示してしまう）
//! - ダイアログを閉じた時点で内容を読み切る。サイズ上限は [`MAX_FILE_BYTES`]
//!   （5 OS 共通の唯一の定数）。超過は `Err(DeviceError::InvalidInput)`
//! - 使えない環境（Linux 等）は `Err(DeviceError::Unsupported)`。読み取り失敗は
//!   `Err(DeviceError::Internal)`
//! - 進行中に再度呼ばれたら `Err(DeviceError::Internal("file_picker busy"))`
//!   （二重にダイアログを開かない）
//!
//! macOS の `NSOpenPanel` はメインスレッド必須。呼び出し元（`fandhe-egui-proxy` の
//! effect ドライバ）は UI スレッド（= eframe のメインスレッド）から**同期で**呼ぶ契約とし、
//! 別スレッドへ逃がさない（`camera_capture` のようなワーカースレッド + mailbox 方式を
//! 誤って適用しないこと）。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::DeviceError;

/// 1 ファイルあたりの上限（5 OS 共通の唯一の定数）。超過は `InvalidInput`。
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

/// 選択されたファイルの中身。`name` は UTF-8・NUL 無し、
/// `bytes.len() <= MAX_FILE_BYTES` を実装・呼び出し元双方の不変条件とする。
///
/// `bytes` は公開 API のため `Vec<u8>` のまま維持する（既存呼び出し元の破壊を防ぐ、
/// Issue #980 codex-review 指摘）。モバイル橋渡し（`fandhe-egui-proxy` の
/// `device/file_pick.rs::bridge`）が毎フレーム `RequestSlot::snapshot` する際の
/// 複製コストは、本型ではなく橋渡し側で `Arc<PickedFile>` として保持することで
/// 軽量化する（`file_pick.rs` の `bridge::Outcome` 参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedFile {
    /// ファイル名（パスを含まない。パスは呼び出し元へ渡さない契約、モジュール doc 参照）。
    pub name: String,
    /// ファイルの内容（生バイト列）。
    pub bytes: Vec<u8>,
}

/// ファイル選択ダイアログの契約（モジュール doc の「契約」節を参照。
/// iOS/Android/Web の実装者が読む唯一の仕様）。
pub trait FilePicker: Send + Sync {
    /// OS 標準の「開く」ダイアログを 1 回開く。
    fn pick(&self) -> Result<Option<PickedFile>, DeviceError>;
}

/// 選ばれたパスの内容を読み切る（全ターゲットでコンパイル・テスト可能、`cfg` 無し）。
///
/// `max` バイトを超える読み取りを 1 パスで検知する（`take(max as u64 + 1)`）。
/// 事前に `metadata()` でサイズを確認してから読む二段構えにしないのは、確認と
/// 読み取りの間にファイルが差し替わる TOCTOU と、巨大ファイルの `Vec` 確保を
/// 同時に避けるため。
///
/// 呼び出し元: デスクトップ実装（本ファイルの `RfdFilePicker`）と、
/// `fandhe-egui-proxy` の iOS 橋渡し（`asCopy:YES` のサンドボックス tmp コピーのパス）。
/// パスを開けない橋渡し（Android の SAF fd）は [`read_picked_from`] を直接使う。
/// OS 側で読み取りを再実装しないこと（EOF・読み取り失敗・上限判定の SSOT をここに置く。
/// モジュール doc 参照）。パスは `DeviceError` に含めない（ログ・エラー非表示の契約は維持）。
pub fn read_picked(path: &Path, max: usize) -> Result<PickedFile, DeviceError> {
    let name = path
        .file_name()
        .ok_or(DeviceError::Internal("file_picker name"))?
        .to_string_lossy()
        .into_owned();

    let file = File::open(path).map_err(|_| DeviceError::Internal("file_picker read"))?;
    read_picked_from(file, name, max)
}

/// 開いた読み取り元から内容を読み切る（全ターゲットでコンパイル・テスト可能、`cfg` 無し）。
///
/// `max` バイトを超える読み取りを 1 パスで検知する（`take(max as u64 + 1)`）。
/// 事前に `metadata()` でサイズを確認してから読む二段構えにしないのは、確認と
/// 読み取りの間にファイルが差し替わる TOCTOU と、巨大ファイルの `Vec` 確保を
/// 同時に避けるため。`name` が空または NUL を含む場合は `Err(Internal)`。
///
/// 呼び出し元: [`read_picked`]（デスクトップ実装・iOS 橋渡しの tmp コピーパス）に加え、
/// `fandhe-egui-proxy` の Android 橋渡しが SAF の fd を `OwnedFd` で借用した `File` へ包んで
/// 直接渡す（`/proc/self/fd/N` の再 open は pipe 型プロバイダで権限再確認に失敗するため
/// 使わない）。
pub fn read_picked_from(
    mut reader: impl Read,
    name: String,
    max: usize,
) -> Result<PickedFile, DeviceError> {
    if name.is_empty() || name.contains('\0') {
        return Err(DeviceError::Internal("file_picker name"));
    }

    let mut bytes = Vec::new();
    (&mut reader)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| DeviceError::Internal("file_picker read"))?;
    if bytes.len() > max {
        return Err(DeviceError::InvalidInput("file_picker size"));
    }

    Ok(PickedFile { name, bytes })
}

/// Windows の `OpenFileDialog` を PowerShell 子プロセスで開いたときの、
/// キャンセル専用の終了コード（成功 = 0 と衝突しない値ならなんでもよい）。
/// `Err` にならない終了コードはこれと 0 のみで、それ以外はすべて `Err(Internal)`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_PICK_CANCELLED_EXIT: i32 = 2;

/// PowerShell へ渡す固定スクリプト。テキストや外部入力を連結しない
/// （security.md A03。tts の Windows 経路と同じ方針）。
///
/// 選択時: 標準出力へパスのみを UTF-8 で書いて終了コード 0（既定の OEM コードページだと
/// 日本語等のファイル名が化け、Rust 側の `from_utf8_lossy` で別パスになるため明示する。
/// `[Text.Encoding]::UTF8` は Windows PowerShell 5.1 で BOM 付きになりパス先頭に
/// `U+FEFF` が混入するため、BOM 無しの `UTF8Encoding($false)` を使う）。
/// ダイアログは `TopMost` の owner フォーム付きで開く。owner 無しだと `CREATE_NO_WINDOW`
/// の子プロセスから開いたダイアログが `output()` で止まっている親ウィンドウの背面に隠れ、
/// タスクバー項目も無いため hang に見える。
/// キャンセル時: 何も書かず [`WINDOWS_PICK_CANCELLED_EXIT`] で終了。
/// 例外時（COM 初期化失敗等、codex-review P1 指摘の本題）: 標準エラーへ理由を書いて
/// それ以外の非ゼロで終了する。rfd 0.17 の同期 API は Windows でこの種の失敗を
/// `.ok()` で握り潰しキャンセルへ化けさせるため、rfd を使わずここで自前に区別する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_PICK_SCRIPT: &str = r#"
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    Add-Type -AssemblyName System.Windows.Forms
    $owner = New-Object System.Windows.Forms.Form
    $owner.TopMost = $true
    $dialog = New-Object System.Windows.Forms.OpenFileDialog
    if ($filter) { $dialog.Filter = $filter }
    if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {
        [Console]::Out.Write($dialog.FileName)
        exit 0
    } else {
        exit 2
    }
} catch {
    [Console]::Error.Write($_.Exception.Message)
    exit 1
}
"#;

/// 子プロセス起動情報（プログラム・引数）。`/bin/sh` 等を注入したテストからも
/// [`run_windows_pick`] を通せるよう、実際の Windows 向け組み立て
/// （[`windows_pick_command`]）と実行処理を分離する（tts の `SpeechCommand`/
/// `run_command` と同型）。
pub(crate) struct PickCommand {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
}

/// Windows 向けダイアログコマンドを組み立てる。`powershell.exe` は絶対パスで起動する
/// （PATH 乗っ取り対策、tts の `windows_speak_command` と同じ理由）。
///
/// `filter` はファイル種別フィルタ（`OpenFileDialog.Filter` 書式）。呼び出し元（写真選択）が渡す
/// ASCII の固定定数だけを受け付け、外部入力は渡さない（A03）。`None` は従来どおり全ファイル。
/// 値はスクリプト先頭の `$filter = '...'` 代入として埋め込むため、`'` を含む値は
/// 組み立て前に拒否できるよう debug_assert で守る。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn windows_pick_command(filter: Option<&'static str>) -> PickCommand {
    debug_assert!(filter.is_none_or(|f| f.is_ascii() && !f.contains('\'')));
    let script = match filter {
        Some(f) => format!("$filter = '{f}'\n{WINDOWS_PICK_SCRIPT}"),
        None => WINDOWS_PICK_SCRIPT.to_string(),
    };
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let program = PathBuf::from(system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    PickCommand {
        program,
        args: vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            script,
        ],
    }
}

/// [`PickCommand`] を起動し、終了コードで選択/キャンセル/失敗の 3 値へ写す
/// （モジュール doc の契約）。OS 判定を経ないため、テストから `/bin/sh -c` を
/// 注入して 3 値すべてを実プロセスで再現できる（tts の `run_command` と同じ手法）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn run_windows_pick(
    command: PickCommand,
    max: usize,
) -> Result<Option<PickedFile>, DeviceError> {
    match run_windows_dialog(command)? {
        Some(path) => read_picked(Path::new(&path), max).map(Some),
        None => Ok(None),
    }
}

/// ダイアログ用 PowerShell を起動し、標準出力のパス文字列を返す共通部
/// （ファイル選択 [`run_windows_pick`] とフォルダ選択 [`pick_folder`] が共有する）。
/// 終了コード 0 = 選択（`Some`）、[`WINDOWS_PICK_CANCELLED_EXIT`] = キャンセル（`None`）、
/// それ以外・空の標準出力 = `Err`。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn run_windows_dialog(command: PickCommand) -> Result<Option<String>, DeviceError> {
    let mut cmd = std::process::Command::new(&command.program);
    cmd.args(&command.args);
    #[cfg(windows)]
    {
        // ダイアログ表示中にコンソールウィンドウが一瞬出るのを防ぐ（tts と同じ対策）。
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd
        .output()
        .map_err(|_| DeviceError::Internal("file_picker spawn"))?;

    match output.status.code() {
        Some(0) => {
            if output.stdout.is_empty() {
                return Err(DeviceError::Internal("file_picker empty path"));
            }
            // BOM は上記のとおり出さない設定だが、環境差で混入しても別パスにならないよう外す。
            let path = String::from_utf8_lossy(&output.stdout)
                .trim_start_matches('\u{feff}')
                .to_owned();
            if path.is_empty() {
                return Err(DeviceError::Internal("file_picker empty path"));
            }
            Ok(Some(path))
        }
        Some(WINDOWS_PICK_CANCELLED_EXIT) => Ok(None),
        // 異常終了・シグナル終了（`code()` が `None`）はすべて失敗として扱う。
        // stderr は固定スクリプトが例外メッセージのみを書く経路のため、パス等の
        // 秘匿情報を含まない（security.md A09 と同じ判断、tts と同型）。
        _ => Err(DeviceError::Internal("file_picker dialog failed")),
    }
}

/// フォルダ選択用の固定スクリプト（外部入力を連結しない。A03）。
/// 形は [`WINDOWS_PICK_SCRIPT`] と同じ。`SelectedPath` が空なら何も書かず 0 で終わり、
/// Rust 側の空出力判定が失敗として拾う。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const WINDOWS_FOLDER_SCRIPT: &str = r#"
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    Add-Type -AssemblyName System.Windows.Forms
    $owner = New-Object System.Windows.Forms.Form
    $owner.TopMost = $true
    $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
    if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {
        [Console]::Out.Write($dialog.SelectedPath)
        exit 0
    } else {
        exit 2
    }
} catch {
    [Console]::Error.Write($_.Exception.Message)
    exit 1
}
"#;

/// Windows のフォルダ選択コマンド。`powershell.exe` は絶対パスで起動する。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn windows_folder_command() -> PickCommand {
    let mut command = windows_pick_command(None);
    command.args[3] = WINDOWS_FOLDER_SCRIPT.to_string();
    command
}

/// フォルダ選択ダイアログを 1 回開く（file-watch の「選んだフォルダ」用、Issue #37）。
///
/// 結果は 3 値: 選択 `Ok(Some(path))` / キャンセル `Ok(None)` / 失敗 `Err`。固定タイムアウトは
/// 掛けない。macOS の `NSOpenPanel` はメインスレッド必須のため、UI スレッドから同期で呼ぶ。
/// macOS・Windows 以外は `Err(Unsupported)`。返したパスは呼び出し元（egui-proxy のドライバ）が
/// 保持し、コアの画面状態へは渡さない（表示はフォルダ名のみ）。
pub fn pick_folder() -> Result<Option<PathBuf>, DeviceError> {
    #[cfg(target_os = "macos")]
    {
        Ok(rfd::FileDialog::new().pick_folder())
    }
    #[cfg(target_os = "windows")]
    {
        run_windows_dialog(windows_folder_command()).map(|o| o.map(PathBuf::from))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(DeviceError::Unsupported("folder_picker"))
    }
}

/// busy を必ず戻す小さなガード（early return・panic のどちらの経路でも Drop で戻る）。
/// 写真選択（`photo_picker::RfdPhotoPicker`）も同じ挙動にするため共有する。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) struct BusyGuard<'a>(pub(crate) &'a AtomicBool);

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// macOS/Windows 向け実装。macOS は `rfd`（採用理由は Cargo.toml のコメント参照）へ
/// 委譲し、Windows は PowerShell 子プロセス（[`run_windows_pick`]）を使う。
///
/// Windows で rfd を使わない理由: rfd 0.17 の同期 API は Windows 側で COM 初期化・
/// ダイアログ生成の失敗を `run(self).ok()` で握り潰し `None`（キャンセル）に化けさせ、
/// 本モジュールの契約（キャンセルと失敗は別の結果）に違反する（#872 codex レビュー
/// P1 指摘）。PowerShell 子プロセスなら終了コードで両者を区別できるため、macOS だけ
/// rfd に残し、Windows は自前の子プロセス経路へ切り替えた。
///
/// `busy` は「進行中に再度呼ばれたら `Internal`」という契約（モジュール doc 参照）を
/// 守るためのガード。`NSOpenPanel`/`IFileOpenDialog` はモーダルでブロッキングするため、
/// 呼び出し元が誤って別スレッドから同時に `pick()` を呼んだ場合の多重表示を防ぐ。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct RfdFilePicker {
    busy: AtomicBool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl RfdFilePicker {
    /// 構築自体は失敗しない（ダイアログの生成失敗は `pick()` 呼び出し時に
    /// `Ok(None)`/`Err` として現れる）。
    pub fn new() -> Self {
        Self {
            busy: AtomicBool::new(false),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Default for RfdFilePicker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl FilePicker for RfdFilePicker {
    fn pick(&self) -> Result<Option<PickedFile>, DeviceError> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DeviceError::Internal("file_picker busy"));
        }
        let _guard = BusyGuard(&self.busy);

        // ponytail: 同期モーダル。非モーダル化が要るなら `rfd::AsyncFileDialog` を
        // メインスレッドで spawn する方式へ上げる。
        #[cfg(target_os = "macos")]
        {
            match rfd::FileDialog::new().pick_file() {
                None => Ok(None),
                Some(path) => read_picked(&path, MAX_FILE_BYTES).map(Some),
            }
        }
        #[cfg(target_os = "windows")]
        {
            run_windows_pick(windows_pick_command(None), MAX_FILE_BYTES)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Unsupported` を返す環境（Linux 等）を模したモック（契約側の経路を固定する）。
    struct UnsupportedFilePicker;

    impl FilePicker for UnsupportedFilePicker {
        fn pick(&self) -> Result<Option<PickedFile>, DeviceError> {
            Err(DeviceError::Unsupported("file_picker"))
        }
    }

    fn write_temp_file(contents: &[u8]) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fandhe-file-picker-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, contents).expect("一時ファイルを書ける");
        path
    }

    #[test]
    fn read_picked_returns_name_and_bytes_within_limit() {
        let path = write_temp_file(b"hello");
        let picked = read_picked(&path, 10).unwrap();
        assert_eq!(picked.bytes, b"hello");
        assert_eq!(
            picked.name,
            path.file_name().unwrap().to_string_lossy().into_owned()
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_picked_rejects_file_over_limit() {
        let path = write_temp_file(b"0123456789");
        let err = read_picked(&path, 5).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_picked_fails_for_missing_path() {
        let path = std::env::temp_dir().join("fandhe-file-picker-test-does-not-exist");
        let err = read_picked(&path, MAX_FILE_BYTES).unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)));
    }

    #[test]
    fn read_picked_from_returns_name_and_bytes_within_limit() {
        let picked = read_picked_from(
            std::io::Cursor::new(b"hello".to_vec()),
            "a.txt".to_string(),
            10,
        )
        .unwrap();
        assert_eq!(picked.bytes, b"hello");
        assert_eq!(picked.name, "a.txt");
    }

    #[test]
    fn read_picked_from_rejects_bytes_over_limit() {
        let err = read_picked_from(
            std::io::Cursor::new(b"0123456789".to_vec()),
            "a.txt".to_string(),
            5,
        )
        .unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput(_)));
    }

    #[test]
    fn read_picked_from_rejects_empty_name() {
        let err = read_picked_from(std::io::Cursor::new(b"hello".to_vec()), String::new(), 10)
            .unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)));
    }

    #[test]
    fn unsupported_picker_returns_unsupported_error() {
        let err = UnsupportedFilePicker.pick().unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn rfd_picker_busy_guard_blocks_reentrant_call_and_releases_after() {
        let picker = RfdFilePicker::new();
        picker.busy.store(true, Ordering::Release);
        let err = picker.pick().unwrap_err();
        assert!(matches!(err, DeviceError::Internal(_)));
        // busy を手動で戻す（実ダイアログは開かずガードの片方だけ検証する）。
        picker.busy.store(false, Ordering::Release);
        assert!(!picker.busy.load(Ordering::Acquire));
    }

    #[test]
    fn windows_pick_command_uses_absolute_powershell_and_fixed_script() {
        let command = windows_pick_command(None);
        // Windows 表記のパスは非 Windows ホストでは `is_absolute()` が偽になるため
        // （Unix の絶対パス判定と衝突する）、末尾のファイル名で組み立てを確認する
        // （tts の `windows_speak_command` テストと同じ判断）。
        assert_eq!(
            command.program.file_name().and_then(|n| n.to_str()),
            Some("powershell.exe")
        );
        assert!(
            command
                .args
                .iter()
                .any(|a| a.contains("OpenFileDialog") && a.contains("exit 2")),
            "固定スクリプトに選択/キャンセル分岐が無い: {:?}",
            command.args
        );
    }

    #[test]
    fn windows_pick_command_embeds_filter_only_when_given() {
        let with = windows_pick_command(Some("Images (*.png)|*.png"));
        assert!(
            with.args
                .iter()
                .any(|a| a.starts_with("$filter = 'Images (*.png)|*.png'\n"))
        );
        let without = windows_pick_command(None);
        assert!(without.args.iter().all(|a| !a.contains("$filter = '")));
    }

    /// `run_windows_pick` は OS 判定を経ないため、`/bin/sh -c` を注入して
    /// 選択/キャンセル/失敗の 3 値を実プロセスで再現できる（tts の `run_command`
    /// テストと同じ手法。Windows には `/bin/sh` が無いため `cfg(unix)` に限定）。
    #[cfg(unix)]
    fn shell_pick_command(script: &str) -> PickCommand {
        PickCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), script.to_string()],
        }
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_pick_returns_picked_file_on_success_exit() {
        let path = write_temp_file(b"hello");
        let script = format!("printf '%s' {}", shell_quote(&path));
        let picked = run_windows_pick(shell_pick_command(&script), MAX_FILE_BYTES)
            .unwrap()
            .expect("選択されたはず");
        assert_eq!(picked.bytes, b"hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_pick_strips_leading_bom_from_path() {
        let path = write_temp_file(b"bom");
        let script = format!("printf '\\357\\273\\277%s' {}", shell_quote(&path));
        let picked = run_windows_pick(shell_pick_command(&script), MAX_FILE_BYTES)
            .unwrap()
            .expect("選択されたはず");
        assert_eq!(picked.bytes, b"bom");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_pick_returns_none_on_cancel_exit_code() {
        let result = run_windows_pick(
            shell_pick_command(&format!("exit {WINDOWS_PICK_CANCELLED_EXIT}")),
            MAX_FILE_BYTES,
        );
        assert!(matches!(result, Ok(None)), "{result:?}");
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_pick_returns_internal_error_on_other_nonzero_exit() {
        // codex-review P1 指摘の再発防止: COM 初期化失敗等（キャンセル専用コード
        // 以外の非ゼロ）はキャンセルへ化けさせず `Err` にする。
        let result = run_windows_pick(shell_pick_command("exit 1"), MAX_FILE_BYTES);
        assert!(
            matches!(result, Err(DeviceError::Internal(_))),
            "{result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_pick_returns_internal_error_on_empty_stdout_success() {
        let result = run_windows_pick(shell_pick_command("exit 0"), MAX_FILE_BYTES);
        assert!(
            matches!(result, Err(DeviceError::Internal(_))),
            "{result:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn run_windows_dialog_maps_exit_codes() {
        let path = write_temp_file(b"x");
        let ok = run_windows_dialog(shell_pick_command(&format!(
            "printf '%s' {}",
            shell_quote(&path)
        )))
        .unwrap();
        assert_eq!(ok.as_deref(), path.to_str());
        let _ = std::fs::remove_file(&path);
        assert!(matches!(
            run_windows_dialog(shell_pick_command("exit 2")),
            Ok(None)
        ));
        assert!(matches!(
            run_windows_dialog(shell_pick_command("exit 1")),
            Err(DeviceError::Internal(_))
        ));
        assert!(matches!(
            run_windows_dialog(shell_pick_command("exit 0")),
            Err(DeviceError::Internal(_))
        ));
        // BOM のみの出力はパスが空なので失敗。
        assert!(matches!(
            run_windows_dialog(shell_pick_command("printf '\\357\\273\\277'")),
            Err(DeviceError::Internal(_))
        ));
    }

    #[test]
    fn windows_folder_command_uses_absolute_powershell_and_folder_script() {
        let command = windows_folder_command();
        assert_eq!(
            command.program.file_name().and_then(|n| n.to_str()),
            Some("powershell.exe")
        );
        assert!(
            command
                .args
                .iter()
                .any(|a| a.contains("FolderBrowserDialog") && a.contains("exit 2")),
            "{:?}",
            command.args
        );
    }

    #[test]
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn pick_folder_is_unsupported_off_desktop() {
        assert!(matches!(pick_folder(), Err(DeviceError::Unsupported(_))));
    }

    /// テスト用の最小限のシェル引用（一時ファイルパスは英数字・`-`・`_`・`/` のみで
    /// 構成されるため、単純なシングルクォート囲みで十分）。
    #[cfg(unix)]
    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display())
    }
}
