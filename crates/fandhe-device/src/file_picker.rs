//! ファイル選択（機能 crate `fandhe-cap-file-picker`）・写真選択・ドラッグ & ドロップが共有する、OS に触れない部品（Phase 11、Issue #872）。
//!
//! 機能の契約（`FilePicker` trait と 3 値の結果契約）と OS 実装は `fandhe-cap-file-picker` へ移した（Issue #1354）。ここに残すのは
//! 他の機能（写真選択 `photo_picker`・ドラッグ & ドロップ・ファイル関連付け・フォルダ監視・Web）も使う部品だけで、
//! `fandhe-device` から機能 crate へは依存しない（`docs/operations/capability-crate-migration.md` §7）。
//!
//! - 選択結果 [`PickedFile`] と上限 [`MAX_FILE_BYTES`]（5 OS 共通の唯一の定数）。超過は `Err(DeviceError::InvalidInput)`、読み取り失敗は
//!   `Err(DeviceError::Internal)`
//! - 読み取り: [`read_picked`]（iOS: サンドボックス tmp コピーのパス）・[`read_picked_from`]（Android: SAF の fd を `OwnedFd` で借用した
//!   `File`。`/proc/self/fd/N` の再 open は pipe 型プロバイダで権限再確認に失敗するため使わない）。EOF・読み取り失敗・上限判定の SSOT
//! - OS の受け口が共有する安全な受け取り: [`picked_from_path_bytes`]（iOS の「結果コード + 一時コピーのパス」）・
//!   [`picked_from_document`]（Android の「結果コード + fd の `File` + 表示名」）。ポインタ・fd を包む `unsafe` は各呼び出し元に 1 か所ずつ残す
//! - 表示名の検証 [`sanitize_picked_name`]、Windows の PowerShell ダイアログの起動と結果の写像（[`windows_pick_command`]・
//!   [`run_windows_pick`]・[`PickCommand`]）、フォルダ選択 [`pick_folder`]、二重表示を防ぐ [`BusyGuard`]
//!
//! ファイルパスは返さない・表示しない・ログに出さない（モバイル/Web ではパス自体が無意味であり、返り値に含めると呼び出し元が
//! うっかり表示してしまう）。Web はパスを持たない（DOM の `File` を読む）ため [`MAX_FILE_BYTES`] と 3 値の結果契約のみを共有する。

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
/// Issue #980 codex-review 指摘）。モバイル橋渡し（`fandhe-cap-file-picker` の
/// `driver.rs::bridge`）が毎フレーム `RequestSlot::snapshot` する際の
/// 複製コストは、本型ではなく橋渡し側で `Arc<PickedFile>` として保持することで
/// 軽量化する（`driver.rs` の `bridge::Outcome` 参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedFile {
    /// ファイル名（パスを含まない。パスは呼び出し元へ渡さない契約、モジュール doc 参照）。
    pub name: String,
    /// ファイルの内容（生バイト列）。
    pub bytes: Vec<u8>,
}

/// 選ばれたパスの内容を読み切る（全ターゲットでコンパイル・テスト可能、`cfg` 無し）。
///
/// `max` バイトを超える読み取りを 1 パスで検知する（`take(max as u64 + 1)`）。
/// 事前に `metadata()` でサイズを確認してから読む二段構えにしないのは、確認と
/// 読み取りの間にファイルが差し替わる TOCTOU と、巨大ファイルの `Vec` 確保を
/// 同時に避けるため。
///
/// 呼び出し元: デスクトップ実装（`fandhe-cap-file-picker` の `RfdFilePicker`）と、
/// iOS 橋渡し（[`picked_from_path_bytes`] 経由。`asCopy:YES` のサンドボックス tmp コピーのパス）。
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
/// Android 橋渡し（[`picked_from_document`] 経由）が SAF の fd を `OwnedFd` で借用した `File` へ包んで
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

/// iOS の受け口（ファイル選択の `UIDocumentPickerViewController`・写真選択の `PHPickerViewController`）が共有する、
/// 「結果コード + 一時コピーのパス → 結果」の変換（全ターゲットでコンパイル・テスト可能、`cfg` 無し）。
///
/// `code`: 0 = 選択（`path` が一時コピーの絶対パスの UTF-8 バイト列）・1 = キャンセル（`path` は見ない）・それ以外 = 内部エラー。
/// `what` はエラー識別子（利用者向け文言には出ない）。選択時は [`read_picked`] で同期に読み切って戻る
/// （呼び出し元の `.m` は戻ってから一時コピーを消す）。0 を返したのにパスが無い・UTF-8 でないときは内容を推測せず
/// `Internal(what)`（成功にしない）。
///
/// 呼び出し元: 機能 crate `fandhe-cap-file-picker` の iOS 受け口と、`fandhe-egui-proxy` の写真選択の受け口。ポインタの読み取り
/// （`unsafe`）は各呼び出し元に 1 か所だけ残し、ここは安全な部分（検証と読み取り）だけを持つ。
pub fn picked_from_path_bytes(
    code: i32,
    path: Option<&[u8]>,
    what: &'static str,
) -> Result<Option<PickedFile>, DeviceError> {
    match code {
        0 => {
            let path = path.ok_or(DeviceError::Internal(what))?;
            let path = std::str::from_utf8(path).map_err(|_| DeviceError::Internal(what))?;
            read_picked(Path::new(path), MAX_FILE_BYTES).map(Some)
        }
        1 => Ok(None),
        _ => Err(DeviceError::Internal(what)),
    }
}

/// Android の受け口（SAF の `nativeOnFilePickerResult`・写真選択の `nativeOnPhotoPickerResult`）が共有する、
/// 「結果コード + 受け取った fd の `File` + 表示名 → 結果」の変換（全ターゲットでコンパイル・テスト可能、`cfg` 無し）。
///
/// `code`: 0 = 選択・1 = キャンセル・それ以外 = 失敗（Java 側が名前解決不能・fd 取得失敗・`launch()` 例外などを一律 2 へ畳む）。
/// `file` は `code == 0` かつ fd が有効なときだけ呼び出し元が `OwnedFd::from_raw_fd`（`unsafe`、呼び出し元に 1 か所）で包んで渡す。
/// 名前の検証より **先に** fd の所有権を取る順序を呼び出し元が守れば、ここのどの失敗経路でも `file` の drop で必ず 1 回 close され、
/// detach 済み fd がリークしない（codex P1）。`raw_name` は Java から受け取った DISPLAY_NAME（取れなかったら `None`）。
/// 名前は [`sanitize_picked_name`] で検証し、読み取りと上限判定は [`read_picked_from`] に委譲する。
pub fn picked_from_document(
    code: i32,
    file: Option<File>,
    raw_name: Option<&str>,
) -> Result<Option<PickedFile>, DeviceError> {
    match code {
        1 => return Ok(None),
        0 => {}
        _ => return Err(DeviceError::Internal("file_picker")),
    }
    let file = file.ok_or(DeviceError::Internal("file_picker"))?;
    let raw_name = raw_name.ok_or(DeviceError::Internal("file_picker"))?;
    let name = sanitize_picked_name(raw_name).ok_or(DeviceError::Internal("file_picker"))?;
    read_picked_from(file, name, MAX_FILE_BYTES).map(Some)
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
pub struct PickCommand {
    /// 起動するプログラム（Windows では `powershell.exe` の絶対パス。テストは `/bin/sh` を注入する）。
    pub program: PathBuf,
    /// プログラムへ渡す引数（固定スクリプト。外部入力を連結しない）。
    pub args: Vec<String>,
}

/// Windows 向けダイアログコマンドを組み立てる。`powershell.exe` は絶対パスで起動する
/// （PATH 乗っ取り対策、tts の `windows_speak_command` と同じ理由）。
///
/// `filter` はファイル種別フィルタ（`OpenFileDialog.Filter` 書式）。呼び出し元（写真選択）が渡す
/// ASCII の固定定数だけを受け付け、外部入力は渡さない（A03）。`None` は従来どおり全ファイル。
/// 値はスクリプト先頭の `$filter = '...'` 代入として埋め込むため、`'` を含む値は
/// 組み立て前に拒否できるよう debug_assert で守る。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub fn windows_pick_command(filter: Option<&'static str>) -> PickCommand {
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
pub fn run_windows_pick(
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
/// 写真選択（機能 crate `fandhe-cap-photo-picker` の `RfdPhotoPicker`）も同じ挙動にするため共有する。
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct BusyGuard<'a>(pub &'a AtomicBool);

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

/// DISPLAY_NAME からパスを剥がして末尾要素のみを採用し、NUL・空文字を拒否する
/// （`PickedFile.name` の不変条件「パスを含まない・NUL 無し」）。SAF のプロバイダが `/` を含む DISPLAY_NAME を返す
/// 場合に備え、呼び出し元へパスの断片を渡さない（security.md A03: 検証できない入力を通さない）。
/// Android のファイル選択（`fandhe-cap-file-picker`）とドラッグ & ドロップ（`fandhe-cap-drag-drop`）が共有する
/// 検証規則で、複製して手で合わせないよう本モジュールを正にする。
pub fn sanitize_picked_name(raw: &str) -> Option<String> {
    let name = raw.rsplit('/').next().unwrap_or(raw);
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod sanitize_picked_name_tests {
    use super::sanitize_picked_name;

    #[test]
    fn sanitize_picked_name_strips_leading_path() {
        assert_eq!(
            sanitize_picked_name("a/b/名前.txt").as_deref(),
            Some("名前.txt")
        );
    }

    #[test]
    fn sanitize_picked_name_rejects_nul_and_empty() {
        assert_eq!(sanitize_picked_name(""), None);
        assert_eq!(sanitize_picked_name("a/"), None);
        assert_eq!(sanitize_picked_name("na\0me"), None);
    }

    #[test]
    fn sanitize_picked_name_keeps_plain_name() {
        assert_eq!(sanitize_picked_name("a.txt").as_deref(), Some("a.txt"));
    }
}

#[cfg(test)]
mod picked_from_tests {
    use super::*;

    fn temp_file_with(contents: &[u8]) -> (std::path::PathBuf, File) {
        let path = std::env::temp_dir().join(format!(
            "fandhe-picked-from-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, contents).expect("一時ファイルを書ける");
        let file = File::open(&path).expect("開ける");
        (path, file)
    }

    #[test]
    fn path_bytes_maps_cancel_and_failure_without_reading_path() {
        assert!(matches!(picked_from_path_bytes(1, None, "x"), Ok(None)));
        assert!(matches!(
            picked_from_path_bytes(2, None, "x"),
            Err(DeviceError::Internal("x"))
        ));
        assert!(matches!(
            picked_from_path_bytes(-1, Some(b"/nonexistent"), "x"),
            Err(DeviceError::Internal("x"))
        ));
    }

    #[test]
    fn path_bytes_selected_requires_a_utf8_path() {
        // 契約違反（0 なのにパスが無い）と UTF-8 破損は成功にしない。
        assert!(matches!(
            picked_from_path_bytes(0, None, "what"),
            Err(DeviceError::Internal("what"))
        ));
        assert!(matches!(
            picked_from_path_bytes(0, Some(&[0xff, 0xfe]), "what"),
            Err(DeviceError::Internal("what"))
        ));
    }

    #[test]
    fn path_bytes_selected_reads_the_file_within_limit() {
        let (path, _file) = temp_file_with(b"hello");
        let bytes = path.to_str().expect("UTF-8").as_bytes().to_vec();
        let picked = picked_from_path_bytes(0, Some(&bytes), "what")
            .unwrap()
            .expect("選択");
        assert_eq!(picked.bytes, b"hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn document_maps_cancel_and_failure_codes() {
        assert!(matches!(picked_from_document(1, None, None), Ok(None)));
        assert!(matches!(
            picked_from_document(2, None, Some("a.txt")),
            Err(DeviceError::Internal("file_picker"))
        ));
    }

    #[test]
    fn document_selected_requires_file_and_a_valid_name() {
        let (path, file) = temp_file_with(b"hello");
        // fd が取れていない。
        assert!(matches!(
            picked_from_document(0, None, Some("a.txt")),
            Err(DeviceError::Internal("file_picker"))
        ));
        // 名前が無い・不正（空）は、読み取りに進まず失敗にする。
        assert!(matches!(
            picked_from_document(0, Some(file), None),
            Err(DeviceError::Internal("file_picker"))
        ));
        let file = File::open(&path).expect("開ける");
        assert!(matches!(
            picked_from_document(0, Some(file), Some("a/")),
            Err(DeviceError::Internal("file_picker"))
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn document_selected_strips_path_from_display_name_and_reads() {
        let (path, file) = temp_file_with(b"hello");
        let picked = picked_from_document(0, Some(file), Some("dir/名前.txt"))
            .unwrap()
            .expect("選択");
        assert_eq!(picked.name, "名前.txt");
        assert_eq!(picked.bytes, b"hello");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn document_selected_over_limit_is_invalid_input() {
        let (path, file) = temp_file_with(&vec![0u8; MAX_FILE_BYTES + 1]);
        let err = picked_from_document(0, Some(file), Some("big.bin")).unwrap_err();
        assert!(matches!(err, DeviceError::InvalidInput(_)));
        let _ = std::fs::remove_file(&path);
    }
}
