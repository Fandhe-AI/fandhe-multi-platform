//! 付属機器のペアリングの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/companion-device.json`、親 #143、Issue #740）。
//!
//! 扱う範囲は「OS の機器選択画面で BLE 機器を 1 台選び、その名前を知る」こと。機器との通信・
//! ボンディング・関連付けを残すことは扱わない（作った関連付けは名前を取得したら外す）。
//! 対象は Android 13（API 33）以上の `CompanionDeviceManager` のみ。iOS は
//! `AccessorySetupKit` を宣言すると `bluetooth-le` と両立しないため保留（OS 別モジュールを置かず
//! `Unsupported`。保留を解くときは dispatch の変更を別 issue で扱う）、macOS・Windows は
//! `no`、Web は対象外。`fandhe-egui-proxy` の effect ドライバ（`device/companion_device.rs`）が
//! 本モジュールの trait を呼び、OS 層（#741 Android）が実装する。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「選んだ」（[`CompanionDeviceOutcome::Selected`]。名前つき・
//!   名前なしの両方を含む）/「キャンセル」（[`CompanionDeviceOutcome::Cancelled`]。利用者が
//!   選択画面を閉じた。失敗にしない）/ 失敗（[`CompanionDeviceFailure`] の理由別、非対応を含む）
//! - **失敗の理由**（写像表）:
//!
//! | 状況 | Android | 返す値 |
//! | --- | --- | --- |
//! | 非対応 | API 32 以下・機能なし | `Unsupported` |
//! | 位置情報サービスがオフ | — | `LocationOff` |
//! | Bluetooth がオフ | — | `BluetoothOff` |
//! | 見つからない | 探索の期限切れ | `NotFound` |
//! | 前面でない | 前面の Activity なし | `NotForeground` |
//! | 表示までの期限切れ | Rust の期限 | `TimedOut` |
//! | 表示前の中断 | 再生成・終了 | `Interrupted` |
//! | 関連付けを外せない・未知のコード | — | `Internal` |
//! | 利用者が閉じた | `RESULT_CANCELED` | `Ok(Cancelled)` |
//! | 選んだ | — | `Ok(Selected(name))` |
//!
//! - **期限**（契約表 3）: 選択画面が出るまでの [`PRESENT_DEADLINE`]。起点は要求を観測した時の
//!   `Instant`（単調時計）。選択中は期限を持たない。期限切れと表示開始は同じロックで排他にし、
//!   期限切れ・中断を確定した後は OS に選択画面を出させない。値は仮で、実機確認（#734）で見直す
//! - **同時 1 件と世代**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と画面の
//!   世代の両方で捨てる
//! - **成功値の条件**（契約表 6）: `Ok(Selected)` と `Ok(Cancelled)` は表示開始
//!   （bridge の `try_begin_present`）を記録した要求にだけ認める。記録のない要求に届いた成功値は
//!   `Internal` にする
//! - **関連付けの解除**（契約表 9）: 作った関連付けは名前を取得したら外す。OS 層は
//!   `disassociate` を `complete_from_os` より前に行い、再生成で要求を失った結果が届いた場合
//!   （古い要求）も外してから捨てる。外せなければ `Internal`。成功は「選んで、かつ関連付けを
//!   外し終えた」ことを意味する
//! - **機器名の正規化**: SSOT は [`DeviceName::from_os`]。OS 層は名前を必ず通す
//! - **秘密値**（契約表 8）: 機器名と OS のエラー文をログ・画面のエラー文・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、期限切れ・中断後に選択画面を
//! 出さないこと、関連付けを残さないこと、機器名と OS のエラー文を出さないこと。
//! 守らない範囲: Android 12L 以下、関連付けを残すこと、背景での起動、機器との通信・ボンディング、
//! 選択画面の表示中のプロセス死、複数プロセス。

use std::fmt;
use std::time::Duration;

/// 要求を観測してから機器選択画面を出すまでの期限（Rust のドライバだけが持つ）。
/// 仮の値で、実機確認（#734）で見直す。
pub const PRESENT_DEADLINE: Duration = Duration::from_secs(10);

/// 機器名の最大バイト数（BLE の機器名の上限）。
pub const MAX_NAME_BYTES: usize = 248;

/// 正規化済みの機器名。[`DeviceName::from_os`] を通ったものだけが作れる。`Debug` は中身を伏せる。
#[derive(Clone, PartialEq, Eq)]
pub struct DeviceName(String);

impl DeviceName {
    /// OS から届いた名前を正規化する（外部の BLE 機器が自由に決める値のため、表示の偽装と
    /// 改行の注入を防ぐ SSOT）。
    ///
    /// - 空文字列と空白だけの文字列は「名前なし」（`None`）
    /// - 制御文字と双方向の書式文字は U+FFFD に置き換える
    /// - 文字の境界を保ったまま [`MAX_NAME_BYTES`] バイトで切る
    /// - それ以外は入力どおり残す（前後の空白も trim しない）
    pub fn from_os(raw: &str) -> Option<DeviceName> {
        if raw.chars().all(char::is_whitespace) {
            return None;
        }
        let mut out = String::new();
        for c in raw.chars() {
            let c = if c.is_control() || is_bidi_format(c) {
                '\u{FFFD}'
            } else {
                c
            };
            if out.len() + c.len_utf8() > MAX_NAME_BYTES {
                break;
            }
            out.push(c);
        }
        Some(DeviceName(out))
    }

    /// 画面へ渡す名前。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 双方向の書式文字か（表示順を入れ替えて名前を偽装できるため置換する）。
fn is_bidi_format(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

impl fmt::Debug for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceName(<redacted>)")
    }
}

/// 成功側の結果（失敗ではない 2 値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompanionDeviceOutcome {
    /// 機器を選んだ。名前が取れなかった（空）場合は `None`。
    Selected(Option<DeviceName>),
    /// 利用者が選択画面を閉じた（失敗にしない）。
    Cancelled,
}

/// 選べなかった理由（機器名・OS のエラー文は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionDeviceFailure {
    /// 非対応 OS・API レベル。
    Unsupported,
    /// 位置情報サービスがオフ。
    LocationOff,
    /// Bluetooth がオフ。
    BluetoothOff,
    /// 探索しても機器が見つからない。
    NotFound,
    /// 前面の Activity がない。
    NotForeground,
    /// 選択画面を出すまでの期限切れ。
    TimedOut,
    /// 表示前の中断（再生成・終了）。
    Interrupted,
    /// 関連付けを外せない・未知のコードなど。
    Internal,
}

/// 付属機器選択の OS 実装。`pick` は「受理」だけを同期で返し、結果は OS 層が
/// `fandhe-egui-proxy` の bridge（`complete_from_os`）へ 1 回だけ非同期に届ける。
pub trait CompanionDevicePicker: Send + Sync {
    /// `request_id` の要求として機器選択画面の表示を OS へ依頼する。
    /// 表示の直前に bridge の `try_begin_present` を呼び、`false` なら表示しない。
    fn pick(&self, request_id: u64) -> Result<(), CompanionDeviceFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedCompanionDevicePicker;

impl CompanionDevicePicker for UnsupportedCompanionDevicePicker {
    fn pick(&self, _request_id: u64) -> Result<(), CompanionDeviceFailure> {
        Err(CompanionDeviceFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> String {
        DeviceName::from_os(s).unwrap().as_str().to_string()
    }

    #[test]
    fn empty_and_blank_are_nameless() {
        assert_eq!(DeviceName::from_os(""), None);
        assert_eq!(DeviceName::from_os("   \t "), None);
    }

    #[test]
    fn plain_name_is_kept_without_trim() {
        assert_eq!(name("Band 3"), "Band 3");
        assert_eq!(name("  Band  "), "  Band  ");
        assert_eq!(name("心拍計"), "心拍計");
    }

    #[test]
    fn control_and_bidi_chars_are_replaced() {
        assert_eq!(name("a\nb\u{0}c"), "a\u{FFFD}b\u{FFFD}c");
        assert_eq!(
            name("a\u{202E}b\u{2066}c\u{200F}"),
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}"
        );
    }

    #[test]
    fn long_name_is_cut_on_char_boundary() {
        let ascii = "a".repeat(MAX_NAME_BYTES + 10);
        assert_eq!(name(&ascii).len(), MAX_NAME_BYTES);
        // 3 バイト文字 83 個 = 249 バイト。境界を保って 82 個（246 バイト）で切る。
        let jp = "あ".repeat(83);
        let n = name(&jp);
        assert_eq!(n.len(), 246);
        assert!(n.chars().all(|c| c == 'あ'));
        let exact = "a".repeat(MAX_NAME_BYTES);
        assert_eq!(name(&exact), exact);
    }

    #[test]
    fn debug_hides_name() {
        let n = DeviceName::from_os("SecretBand").unwrap();
        let d = format!(
            "{n:?} {:?}",
            CompanionDeviceOutcome::Selected(Some(n.clone()))
        );
        assert!(!d.contains("Secret"), "{d}");
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedCompanionDevicePicker.pick(1),
            Err(CompanionDeviceFailure::Unsupported)
        );
    }
}
