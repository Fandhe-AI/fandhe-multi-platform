//! ローカル保存の抽象（Phase 8 / Issue #746）。`fandhe-egui-proxy` 側の画面が
//! key-value 形式でファイルを読み書きするための境界。
//!
//! `FsStorage` は `DataDir` 配下に閉じたファイル I/O のみを行い、`key` はパス区切りや
//! `..` を含む文字列を拒否する（A01/A03 パストラバーサル対策、security.md 準拠）。

use crate::{DataDir, DeviceError};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// `write` が一時ファイル名の一意性を確保するための呼び出しごとの連番
/// （プロセス ID と組み合わせ、プロセス内外を問わず衝突しない一時パスを作る）。
static TMP_NONCE: AtomicU64 = AtomicU64::new(0);

/// 一時ファイルを通常 key の名前空間から分離するためのサブディレクトリ名。
/// `validate_key` は `.` を許可するため理論上 key と同名になり得るが、その場合は
/// パスがディレクトリと衝突して `fs::write`/`rename` がエラーを返すだけで済み、
/// 他 key の値が消えることはない（P0 指摘対応、ponytail: 名前衝突時はエラーで
/// 止まる設計。key の禁止語化までは行わない）。
const TMP_SUBDIR: &str = ".fandhe-tmp";

/// 端末保存領域への読み書き契約。実装はスレッド間で共有されうる
/// （呼び出し元は `fandhe-egui-proxy` の描画スレッドと将来の worker スレッド、#745 参照）。
pub trait Storage: Send + Sync {
    /// `key` に対応する値を読む。未存在は `Ok(None)`（エラーにしない契約）。
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError>;
    /// `key` に `bytes` を書く。既存値があれば置き換える。
    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), DeviceError>;
}

/// `key` を許可文字集合（英数字・`.`・`_`・`-`）のみに制限し、パストラバーサルを防ぐ。
///
/// 空文字列・`.`/`..` そのもの・区切り文字（`/`・`\`）を含む値はすべて拒否する。
/// `pub(crate)`: `secure_credential::validate_key` が同じ許可文字集合・トラバーサル
/// 拒否ロジックを再利用する（Issue #922、二重実装を避ける）。
pub(crate) fn validate_key(key: &str) -> Result<(), DeviceError> {
    let is_valid = !key.is_empty()
        && key != "."
        && key != ".."
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-');
    if is_valid {
        Ok(())
    } else {
        Err(DeviceError::InvalidKey(key.to_string()))
    }
}

/// `key` をファイル名へ符号化する（各バイトを小文字 16 進 2 桁にする）。
///
/// `validate_key` は大文字・小文字を両方許可するため、`key` をそのままファイル名に
/// 使うと Windows・大文字小文字を区別しない既定の macOS ボリューム上で `"Note.txt"` と
/// `"note.txt"` が同一ファイルとして衝突し、保存キーの一意性契約が OS 依存で崩れる
/// （P1 指摘対応）。16 進表現は元の大文字・小文字の違いをすべて別の文字列へ写すため、
/// 大文字小文字を区別しないファイルシステムでもキーごとに別ファイルへ分離される。
fn encode_key(key: &str) -> String {
    key.bytes()
        .fold(String::with_capacity(key.len() * 2), |mut acc, b| {
            acc.push_str(&format!("{b:02x}"));
            acc
        })
}

/// `std::fs` による `Storage` の共通実装（desktop 向け。Android/iOS も `DataDir` さえ
/// サンドボックス内パスで渡せば同じ実装で動く想定）。
pub struct FsStorage {
    dir: PathBuf,
}

impl FsStorage {
    /// `dir` を起点ディレクトリとする `FsStorage` を構築する。前提条件なし
    /// （ディレクトリの存在確認・作成は `write` 時に行う）。
    pub fn new(dir: DataDir) -> Self {
        Self { dir: dir.0 }
    }

    /// デスクトップ（macOS/Windows/Linux）向けの既定 `DataDir`（`$HOME/.fandhe`）。
    /// Android/iOS はサンドボックスパスが別途必要なためここでは提供しない
    /// （呼び出し元が `DataDir` を構築する契約、`lib.rs` の `DataDir` doc 参照）。
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn desktop_default() -> Result<DataDir, DeviceError> {
        #[allow(deprecated)]
        std::env::home_dir()
            .map(|home| DataDir(home.join(".fandhe")))
            .ok_or(DeviceError::Unsupported("home directory not found"))
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, DeviceError> {
        validate_key(key)?;
        Ok(self.dir.join(encode_key(key)))
    }
}

impl Storage for FsStorage {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, DeviceError> {
        let path = self.path_for(key)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(DeviceError::Io(e)),
        }
    }

    fn write(&self, key: &str, bytes: &[u8]) -> Result<(), DeviceError> {
        let path = self.path_for(key)?;
        fs::create_dir_all(&self.dir)?;
        // 書きかけファイルが読み戻し破損を起こさないよう、一時ファイル + rename で
        // 原子的に置き換える（security.md A08 整合性の不備対策）。
        // 一時ファイルは通常 key の名前空間（`self.dir` 直下）ではなく専用サブ
        // ディレクトリ `TMP_SUBDIR` に置く。key 直下にプロセス ID + 連番を付けた
        // ファイル名を作る方式（旧実装）だと、`validate_key` を通る任意の key 文字列
        // （例: "note.<pid>.<nonce>.tmp"）が他 key の一時パスと衝突しうる
        // （同一プロセス内で pid は固定、nonce は連番のため字面から逆算可能）。
        // サブディレクトリで名前空間を分離すれば、通常 key のパス（`self.dir` 直下）
        // と一時パス（`self.dir/TMP_SUBDIR` 配下）が構造的に重ならない（P0 指摘対応）。
        let tmp_dir = self.dir.join(TMP_SUBDIR);
        fs::create_dir_all(&tmp_dir)?;
        let nonce = TMP_NONCE.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!("{}.{}.tmp", std::process::id(), nonce);
        let tmp_path = tmp_dir.join(tmp_name);
        // fs::write / fs::rename が失敗した場合、生成済みの一時ファイルを残すと
        // TMP_SUBDIR にゴミが溜め込まれ続ける（P2 指摘対応）。削除自体の失敗は
        // 元のエラーより優先度が低いため無視し、必ず元のエラーを返す。
        if let Err(e) = fs::write(&tmp_path, bytes) {
            let _ = fs::remove_file(&tmp_path);
            return Err(DeviceError::Io(e));
        }
        if let Err(e) = fs::rename(&tmp_path, &path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(DeviceError::Io(e));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_storage() -> FsStorage {
        let dir = std::env::temp_dir().join(format!(
            "fandhe-device-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("システム時刻は UNIX epoch 以降")
                .as_nanos()
        ));
        FsStorage::new(DataDir(dir))
    }

    #[test]
    fn write_then_read_round_trips() {
        let storage = temp_storage();
        storage
            .write("note.txt", b"hello")
            .expect("write は成功する");
        let value = storage.read("note.txt").expect("read は成功する");
        assert_eq!(value, Some(b"hello".to_vec()));
    }

    #[test]
    fn missing_key_returns_none() {
        let storage = temp_storage();
        let value = storage.read("absent").expect("read は成功する");
        assert_eq!(value, None);
    }

    #[test]
    fn traversal_key_is_rejected() {
        let storage = temp_storage();
        let err = storage.write("../x", b"x").unwrap_err();
        assert!(matches!(err, DeviceError::InvalidKey(_)));
        let err = storage.read("../x").unwrap_err();
        assert!(matches!(err, DeviceError::InvalidKey(_)));
    }

    #[test]
    fn stem_sharing_keys_do_not_collide_on_tmp_path() {
        // note.tmp / note.txt は with_extension("tmp") 方式だと同一の一時パスを
        // 奪い合い、後勝ちで先の値が消失していた（P0 指摘の回帰テスト）。
        let storage = temp_storage();
        storage.write("note.tmp", b"A").expect("write は成功する");
        storage.write("note.txt", b"B").expect("write は成功する");
        assert_eq!(
            storage.read("note.tmp").expect("read は成功する"),
            Some(b"A".to_vec())
        );
        assert_eq!(
            storage.read("note.txt").expect("read は成功する"),
            Some(b"B".to_vec())
        );
    }

    #[test]
    fn differently_cased_keys_map_to_distinct_files() {
        // 大文字小文字を区別しないファイルシステム（Windows・macOS 既定）でも
        // "Note.txt" と "note.txt" が別ファイルへ写ることを、符号化後のファイル名
        // レベルで保証する（P1 指摘の回帰テスト）。
        let storage = temp_storage();
        let upper = storage.path_for("Note.txt").expect("valid key");
        let lower = storage.path_for("note.txt").expect("valid key");
        assert_ne!(upper.file_name(), lower.file_name());

        storage.write("Note.txt", b"A").expect("write は成功する");
        storage.write("note.txt", b"B").expect("write は成功する");
        assert_eq!(
            storage.read("Note.txt").expect("read は成功する"),
            Some(b"A".to_vec())
        );
        assert_eq!(
            storage.read("note.txt").expect("read は成功する"),
            Some(b"B".to_vec())
        );
    }

    #[test]
    fn failed_write_does_not_leave_tmp_file_behind() {
        // rename 先を先にディレクトリとして作っておき、fs::rename を確実に失敗させる
        // （P2 指摘の回帰テスト: 失敗時に TMP_SUBDIR 配下のゴミが残らないことを確認する）。
        let storage = temp_storage();
        fs::create_dir_all(&storage.dir).expect("dir 作成は成功する");
        let target = storage.path_for("busy").expect("valid key");
        fs::create_dir_all(&target).expect("target をディレクトリとして作成");

        let err = storage.write("busy", b"x").unwrap_err();
        assert!(matches!(err, DeviceError::Io(_)));

        let tmp_dir = storage.dir.join(TMP_SUBDIR);
        let leftover: Vec<_> = fs::read_dir(&tmp_dir)
            .expect("tmp_dir は create_dir_all 済み")
            .collect();
        assert!(
            leftover.is_empty(),
            "一時ファイルが残留している: {leftover:?}"
        );
    }

    #[test]
    fn empty_key_is_rejected() {
        let storage = temp_storage();
        assert!(matches!(
            storage.write("", b"x").unwrap_err(),
            DeviceError::InvalidKey(_)
        ));
    }
}
