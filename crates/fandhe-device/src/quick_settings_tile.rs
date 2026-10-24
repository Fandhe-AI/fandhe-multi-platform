//! クイック設定タイル・コントロール（Phase 11 の拡充ループ、親 #139、Issue #719）。
//!
//! 扱う範囲は「OS のシステム UI（macOS のコントロールセンター、Android のクイック設定）に置いた
//! 1 操作のボタンが押された回数を、OS 側の保存先から読んで画面に出す」ところまで。押下の処理と
//! 保存は OS 側の拡張（macOS は WidgetKit 拡張の `AppIntent`、Android は `TileService`）が担い、
//! 本 trait は回数の読み出しだけを持つ。アプリが起動していないときに押した分も、保存先に残って
//! いるため次の読み出しで数に入る。呼び出し元は `fandhe-egui-proxy` の
//! `device/quick_settings_tile.rs`（画面向け effect ドライバ）と各 OS 実装。
//!
//! # 契約（各 OS 実装・呼び出し元が守る不変条件）
//!
//! - **結果は 3 種**: `Ok(TileSnapshot)`（回数。未押下は 0）、`Err(Internal)`（読み出し失敗）、
//!   `Err(Unsupported)`（非対応）。0 と失敗を区別し、失敗・非対応を 0 として見せない
//! - **キャンセル・正常終了・失敗の三値**（`device-bridge-contracts.md` 10）: 利用者のキャンセル操作も
//!   権限ダイアログも無い。ただし macOS 15 以降は App Group コンテナへの初回アクセスで OS の確認が
//!   出うる。拒否は読み出し失敗（`Internal`）として出し、0 にしない
//! - **押下と保存**: 押下 1 回で 1 だけ増やす。同じプロセス内で読み・増加・保存を排他にする。
//!   保存に失敗した押下は数えない。既存の保存内容が壊れていたら 0 に戻さず失敗にする。
//!   `u64::MAX` で溢れる場合も失敗にする（巻き戻さない）
//! - **保存形式（macOS）**: App Group コンテナ直下の 1 ファイル（key [`COUNT_KEY`]）。中身は ASCII
//!   10 進数で改行なし。読み側の厳格な解釈は [`parse_count`] に集約する。ファイルが無いのは
//!   拡張が一度も走っていない場合で `Ok(0)`
//! - **非対応 OS は全メソッドが `Err(Unsupported)`**: 初期化できない実装も
//!   [`UnsupportedQuickSettingsTile`] へ倒す。ダミーの成功へは倒さない
//! - **回数・ファイル内容・パスをログ・エラー文に出さない**（`device-bridge-contracts.md` 8）
//!
//! # 守る範囲と守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）
//!
//! 守る: 1 プロセス内の読み・増加・保存の排他、保存失敗を数えないこと、0 と失敗の区別、
//! 嘘の成功の禁止。
//! 守らない: アプリからタイル・コントロールを追加させる操作、タイル・コントロール側への回数の
//! 表示、ポーリング間隔ぶんの表示の遅れ、複数ユーザー・複数端末、アンインストールでの回数の消失、
//! 複数プロセス（拡張プロセスが複数同時に走る場合）の排他、プロセス終了で Android の失敗件数が
//! 消えること。

use crate::DeviceError;

/// 回数の保存 key。`FsStorage` が 16 進符号化したファイル名は
/// `717569636b5f73657474696e67735f636f756e74`（Swift 側の定数と一致させる）。
pub const COUNT_KEY: &str = "quick_settings_count";

/// 回数の読み出し結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileSnapshot {
    /// 押された回数。未押下は 0。
    pub count: u64,
    /// このプロセス内で保存に失敗した押下の件数。取れる実装（Android）だけが `Some`。
    /// macOS は拡張が別プロセスで失敗をシステム UI へ throw するため `None`。
    pub failed_presses: Option<u64>,
}

/// クイック設定タイル・コントロールの回数の読み出し。
pub trait QuickSettingsTile: Send + Sync {
    /// 現在の回数を読む。読み出しはブロックしうるため、呼び出し側はワーカースレッドで呼ぶ。
    fn snapshot(&self) -> Result<TileSnapshot, DeviceError>;
}

/// 非対応 OS 用。常に `Unsupported("quick_settings_tile")` を返す。
pub struct UnsupportedQuickSettingsTile;

impl QuickSettingsTile for UnsupportedQuickSettingsTile {
    fn snapshot(&self) -> Result<TileSnapshot, DeviceError> {
        Err(DeviceError::Unsupported("quick_settings_tile"))
    }
}

/// 保存内容を回数へ厳格に解釈する。受理するのは ASCII 数字だけで、空・符号・空白・改行・
/// 非 UTF-8・`u64` の溢れは `Err(Internal)`（0 に丸めない）。
pub fn parse_count(bytes: &[u8]) -> Result<u64, DeviceError> {
    let err = || DeviceError::Internal("quick_settings_tile");
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(err());
    }
    let s = std::str::from_utf8(bytes).map_err(|_| err())?;
    s.parse::<u64>().map_err(|_| err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_digits_only() {
        assert!(matches!(parse_count(b"0"), Ok(0)));
        assert!(matches!(parse_count(b"42"), Ok(42)));
        assert!(matches!(parse_count(b"18446744073709551615"), Ok(u64::MAX)));
    }

    #[test]
    fn parse_rejects_everything_else() {
        let bad: [&[u8]; 8] = [
            b"",
            b"+1",
            b"-1",
            b" 1",
            b"1\n",
            b"1a",
            &[0xff, 0xfe],
            b"18446744073709551616",
        ];
        for b in bad {
            assert!(
                matches!(
                    parse_count(b),
                    Err(DeviceError::Internal("quick_settings_tile"))
                ),
                "{b:?}"
            );
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedQuickSettingsTile.snapshot(),
            Err(DeviceError::Unsupported("quick_settings_tile"))
        ));
    }
}
