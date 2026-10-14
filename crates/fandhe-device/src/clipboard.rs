//! クリップボード読み書きの抽象（Phase 11、Issue #859）。
//!
//! `Storage`/`Camera` と同じ request/response の一発呼び出し型で、`Motion` のような
//! 継続 push の共有状態は持たない。OS のクリップボードは呼び出しごとに最新値を
//! 返すため、読み出しは毎回 OS へ問い合わせる（キャッシュしない）。
//!
//! デスクトップ（macOS/Windows/Linux）実装 [`ArboardClipboard`] は `arboard` へ委譲する。
//! `NSPasteboard`/WinRT `Clipboard`/X11 の各 API を自前で叩くと本クレートの
//! `#![forbid(unsafe_code)]` を破ることになるのに対し、`arboard` は 3 OS を safe API で
//! 覆うため（選定根拠は `Cargo.toml` の依存コメント参照）。iOS（`UIPasteboard`）と
//! Android（`ClipboardManager`）は safe な Rust バインディングが無いため
//! `fandhe-egui-proxy` 側（FFI 境界を持てるレイヤ）で実装する。
//!
//! [`ArboardClipboard`] はその 3 OS でのみ存在する（arboard が Android/iOS の
//! バックエンドを持たずコンパイルが通らないため、`Cargo.toml` の target 分岐と対で
//! `cfg` を付けている）。どのプラットフォームでも使える [`DummyClipboard`] は
//! プロセス内に閉じたフォールバックとして常に提供する。

use crate::DeviceError;

/// クリップボード読み書きの契約（`Storage`/`Camera` と同じくスレッド間で共有され
/// うるため `Send + Sync`）。対応不可プラットフォームは
/// `DeviceError::Unsupported("clipboard")` を返す契約とする。
pub trait Clipboard: Send + Sync {
    /// クリップボードのテキストを読む。空・非テキスト内容は `Ok(None)`。
    ///
    /// iOS 16 以降は他アプリがコピーした内容の読み取りで OS の許可アラートが出る
    /// （拒否されると `Ok(None)` 相当になる）。自アプリがコピーした内容の読み取りは
    /// アラート無しで通る（`docs/device/capabilities/clipboard.json` の iOS 欄）。
    fn get_text(&self) -> Result<Option<String>, DeviceError>;

    /// クリップボードへテキストを書く。既存内容は置き換わる。
    fn set_text(&self, text: &str) -> Result<(), DeviceError>;
}

/// デスクトップ（macOS/Windows/Linux）実装。`arboard` のハンドルを 1 個だけ保持する。
///
/// ハンドルを呼び出しごとに作り直さないのは X11 のクリップボード所有権モデルの都合で、
/// `arboard` は X11 で内容を提供し続けるためのバックグラウンドスレッドをハンドルに
/// 紐づけて持つ。ハンドルを drop すると書き込んだ内容が他アプリから読めなくなるため、
/// プロセスの生存期間中は同じハンドルを使い回す（`arboard` の `SetExtLinux` 等の
/// ドキュメント参照）。`arboard::Clipboard` は `Sync` ではないため `Mutex` で包んで
/// トレイトの `Send + Sync` 契約を満たす。
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub struct ArboardClipboard(std::sync::Mutex<arboard::Clipboard>);

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl ArboardClipboard {
    /// OS のクリップボードへ接続する。接続自体が失敗する環境（ディスプレイサーバ
    /// 不在のヘッドレス Linux 等）では `DeviceError::Unsupported` を返す。
    pub fn new() -> Result<Self, DeviceError> {
        arboard::Clipboard::new()
            .map(|c| Self(std::sync::Mutex::new(c)))
            .map_err(|_| DeviceError::Unsupported("clipboard"))
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Clipboard for ArboardClipboard {
    fn get_text(&self) -> Result<Option<String>, DeviceError> {
        // 他所の panic 由来の poison を呼び出し側へ伝播させない
        // （coding-rust.md: unwrap/expect を本実装コードで使わない。`motion` と同じ判断）。
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match guard.get_text() {
            Ok(text) if text.is_empty() => Ok(None),
            Ok(text) => Ok(Some(text)),
            // クリップボードが空・非テキスト（画像のみ等）は異常ではないため
            // `Ok(None)` へ畳む。`arboard` はこの区別を `ContentNotAvailable` で
            // 表すが、エラー型の内部構造を呼び出し側へ漏らさない（security.md A09）。
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(_) => Err(DeviceError::Internal("clipboard read")),
        }
    }

    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .set_text(text.to_string())
            .map_err(|_| DeviceError::Internal("clipboard write"))
    }
}

/// プロセス内のメモリ上だけで読み書きするダミー実装（ヘッドレステスト・
/// OS クリップボードへ接続できない環境向けのフォールバック）。
#[derive(Clone, Default)]
pub struct DummyClipboard(std::sync::Arc<std::sync::Mutex<Option<String>>>);

impl DummyClipboard {
    /// 空の状態で生成する。
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clipboard for DummyClipboard {
    fn get_text(&self) -> Result<Option<String>, DeviceError> {
        let guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 空文字列は「空」として `None` へ畳む（`ArboardClipboard`・iOS・Android の
        // 各実装と同じ規則に揃える。実装ごとに空状態の表示が変わらないようにする、
        // codex-review 指摘 P2、PR #860）。
        Ok(guard.clone().filter(|text| !text.is_empty()))
    }

    fn set_text(&self, text: &str) -> Result<(), DeviceError> {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(text.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Clipboard` 契約側のエラー経路（`Unsupported`）を確認する最小モック。
    struct UnsupportedClipboard;

    impl Clipboard for UnsupportedClipboard {
        fn get_text(&self) -> Result<Option<String>, DeviceError> {
            Err(DeviceError::Unsupported("clipboard"))
        }
        fn set_text(&self, _text: &str) -> Result<(), DeviceError> {
            Err(DeviceError::Unsupported("clipboard"))
        }
    }

    #[test]
    fn dummy_round_trips_text() {
        let clipboard = DummyClipboard::new();
        clipboard.set_text("コピーした文字列").unwrap();
        assert_eq!(
            clipboard.get_text().unwrap(),
            Some("コピーした文字列".to_string())
        );
    }

    #[test]
    fn dummy_normalizes_empty_string_to_none() {
        // 実 OS 実装（空なら `Ok(None)`）と挙動を揃える（codex-review P2、PR #860）。
        let clipboard = DummyClipboard::new();
        clipboard.set_text("").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), None);
    }

    #[test]
    fn dummy_is_empty_before_first_write() {
        assert_eq!(DummyClipboard::new().get_text().unwrap(), None);
    }

    #[test]
    fn dummy_set_text_replaces_previous_value() {
        let clipboard = DummyClipboard::new();
        clipboard.set_text("1 回目").unwrap();
        clipboard.set_text("2 回目").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), Some("2 回目".to_string()));
    }

    #[test]
    fn dummy_clone_shares_state() {
        let clipboard = DummyClipboard::new();
        clipboard.clone().set_text("共有").unwrap();
        assert_eq!(clipboard.get_text().unwrap(), Some("共有".to_string()));
    }

    /// OS のクリップボードと実際に相互運用できることを確認する手動テスト（macOS 限定）。
    ///
    /// `#[ignore]` を付けて既定の `cargo test` からは外す: 実 OS のクリップボードを
    /// 書き換える副作用があり、ヘッドレス CI（ディスプレイサーバ不在）では
    /// `ArboardClipboard::new()` 自体が失敗するため。実行は
    /// `cargo test -p fandhe-device --lib -- --ignored --nocapture` で行う
    /// （Issue #859 の Mac 目視確認の機械的な裏付けとして使う）。
    #[test]
    #[ignore = "実 OS のクリップボードを書き換えるため手動実行（--ignored）"]
    #[cfg(target_os = "macos")]
    fn arboard_interoperates_with_os_clipboard() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let clipboard = ArboardClipboard::new().expect("macOS では接続できる");

        // (1) 外部アプリ（pbcopy）が置いた内容を読めること。
        // プロセス内のダミーでは通らない経路であることがこれで示せる。
        let external = "外部から置いた文字列";
        let mut pbcopy = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .expect("pbcopy を起動できる");
        pbcopy
            .stdin
            .as_mut()
            .expect("stdin がある")
            .write_all(external.as_bytes())
            .expect("pbcopy へ書ける");
        assert!(pbcopy.wait().expect("pbcopy の終了を待てる").success());
        assert_eq!(clipboard.get_text().unwrap(), Some(external.to_string()));

        // (2) 書いた内容を外部アプリ（pbpaste）から読めること。
        let written = "fandhe が書いた文字列";
        clipboard.set_text(written).expect("書き込める");
        let pbpaste = Command::new("pbpaste")
            .output()
            .expect("pbpaste を起動できる");
        assert!(pbpaste.status.success());
        assert_eq!(String::from_utf8_lossy(&pbpaste.stdout), written);
    }

    #[test]
    fn unsupported_platform_returns_unsupported_error() {
        let clipboard = UnsupportedClipboard;
        assert!(matches!(
            clipboard.get_text(),
            Err(DeviceError::Unsupported(_))
        ));
        assert!(matches!(
            clipboard.set_text("x"),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
