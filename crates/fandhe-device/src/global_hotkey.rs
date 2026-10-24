//! グローバルホットキー（アプリが前面でなくても押下を受け取る固定のキー組み合わせ 1 つ）の抽象
//! （Phase 11 の拡充ループ、親 #150、Issue #766）。
//!
//! 範囲は固定の組み合わせ 1 つ（macOS は Control + Shift + Option + K、Windows は
//! Ctrl + Shift + Alt + K）の登録・解除・押下通知だけ。組み合わせは利用者・ネットワークから
//! 受け取らず、各 OS 実装が定数で持つ。`fandhe-egui-proxy` の effect ドライバが本 trait を呼び、
//! 押下は [`OnPress`] 経由で画面の回数へ反映される。macOS は Carbon の `RegisterEventHotKey`
//! （許可不要）、Windows は `RegisterHotKey`（#767）で実装する。
//!
//! # 結果の 4 種（契約表 10）
//!
//! | 区分 | 返す値 |
//! |---|---|
//! | 登録できた | `Ok(RegisterOutcome::Registered)` |
//! | 使用中（OS が重複で拒否した） | `Ok(RegisterOutcome::InUse)` |
//! | 非対応 OS | `Err(Unsupported("global_hotkey"))` |
//! | 失敗（OS の失敗・スレッド違反など） | `Err(Internal("global_hotkey register"))` |
//!
//! 使用中を失敗や成功に畳まない。`PermissionDenied` / `Cancelled` は返さない（許可を求めない方式で、
//! 利用者のキャンセル UI も無いため）。
//!
//! # OS を正にする
//!
//! 組み合わせが空いているか・許可があるかを事前に判定せず、登録 API の戻り値を写す。macOS は
//! `kEventHotKeyExclusive` 付きで登録し `eventHotKeyExistsErr`（-9878）を使用中へ、Windows は
//! `ERROR_HOTKEY_ALREADY_REGISTERED`（1409）を使用中へ写す（写像は本モジュールの純関数が SSOT）。
//! OS のエラーコードは戻り値・ログ・画面に出さない。
//!
//! # 登録の記録
//!
//! 登録の記録は OS の登録を持つハンドル 1 つだけ（macOS は `.m` の `EventHotKeyRef`、Windows は
//! 専用スレッド）。「登録中」のフラグは別に持たない。画面の表示は登録・解除の戻り値からだけ作る。
//! 押下の受け口は配送経路であり、登録中かどうかの判定には使わない。
//!
//! - 登録中に [`GlobalHotkey::register`] を呼んでも OS を二重に呼ばず `Ok(Registered)` を返し、
//!   [`OnPress`] だけを新しいものへ差し替える（冪等）
//! - 解除は OS が成功を返したときだけハンドルを手放す。失敗なら
//!   `Err(Internal("global_hotkey unregister"))` を返し、登録中のまま残して再試行できる。
//!   未登録での解除は `Ok(NotRegistered)`
//! - アプリ終了（`device::shutdown`）でも解除する
//!
//! # スレッド
//!
//! macOS の登録・解除・押下は main thread。Windows は専用スレッド 1 本に登録・受信・解除を集約し
//! （`MOD_NOREPEAT`、解除は登録したスレッドから）、UI スレッドからの呼び出しは専用スレッドの返事を
//! 同期に待つ。期限・ポーリングは持たない（押下は OS の通知だけで数える。契約表 3・4）。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の登録・解除・押下の順序、使用中・非対応・失敗を成功に畳まないこと、
//! 登録失敗や解除後に古い [`OnPress`] を呼ばないこと、終了時に登録を外すこと、押されたキーの内容と
//! OS のエラーコードを出さないこと。
//!
//! 守らない: macOS で他アプリが非排他で登録した同じ組み合わせとの重なり、OS 予約の組み合わせと
//! 重なって押下が届かない場合の検出、キー配列による文字の違い、複数プロセスでの同時登録、
//! 終了処理と重なった押下の数え漏れ、OS 側の非同期確定との時間差で表示が一時的に食い違うこと。

use std::sync::Arc;

use crate::DeviceError;

/// 押下の通知先。任意のスレッドから呼ばれうるため `Send + Sync`。
pub type OnPress = Arc<dyn Fn() + Send + Sync>;

/// 登録の結果（`Err` 以外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterOutcome {
    /// 登録できた（登録中の再呼び出しも含む）。
    Registered,
    /// OS が重複で拒否した（ほかのアプリが使っている）。
    InUse,
}

/// 解除の結果（`Err` 以外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnregisterOutcome {
    /// OS が解除に成功した。
    Unregistered,
    /// 登録していなかった（後始末の経路で失敗にしない）。
    NotRegistered,
}

/// `.m` が main thread 以外で呼ばれたときに返す番兵値（OSStatus の既存値と衝突しない正の値）。
pub const MACOS_NOT_MAIN_THREAD: i32 = 1;
/// `.m` の解除が「未登録」のときに返す番兵値。
pub const MACOS_NOT_REGISTERED: i32 = 2;
/// macOS の `eventHotKeyExistsErr`。
pub const MACOS_EVENT_HOTKEY_EXISTS: i32 = -9878;
/// Windows の `ERROR_HOTKEY_ALREADY_REGISTERED`。
pub const WINDOWS_HOTKEY_ALREADY_REGISTERED: u32 = 1409;

const REGISTER_FAILED: DeviceError = DeviceError::Internal("global_hotkey register");
const UNREGISTER_FAILED: DeviceError = DeviceError::Internal("global_hotkey unregister");

/// macOS の `RegisterEventHotKey` 相当の戻り値 → 結果（未知のコードは失敗に倒し成功を作らない）。
pub fn register_outcome_from_macos(status: i32) -> Result<RegisterOutcome, DeviceError> {
    match status {
        0 => Ok(RegisterOutcome::Registered),
        MACOS_EVENT_HOTKEY_EXISTS => Ok(RegisterOutcome::InUse),
        _ => Err(REGISTER_FAILED),
    }
}

/// macOS の解除の戻り値 → 結果。
pub fn unregister_outcome_from_macos(status: i32) -> Result<UnregisterOutcome, DeviceError> {
    match status {
        0 => Ok(UnregisterOutcome::Unregistered),
        MACOS_NOT_REGISTERED => Ok(UnregisterOutcome::NotRegistered),
        _ => Err(UNREGISTER_FAILED),
    }
}

/// Windows の `RegisterHotKey` の結果 → 結果（#767 の実装が使う）。
pub fn register_outcome_from_windows(
    succeeded: bool,
    last_error: u32,
) -> Result<RegisterOutcome, DeviceError> {
    if succeeded {
        Ok(RegisterOutcome::Registered)
    } else if last_error == WINDOWS_HOTKEY_ALREADY_REGISTERED {
        Ok(RegisterOutcome::InUse)
    } else {
        Err(REGISTER_FAILED)
    }
}

/// グローバルホットキーの OS 実装が満たす契約。詳細はモジュール doc。
pub trait GlobalHotkey: Send + Sync {
    /// 固定の組み合わせを登録し、押されるたびに `on_press` を呼ぶ。登録中の再呼び出しは OS を
    /// 呼ばず `Ok(Registered)` を返し、`on_press` だけを差し替える。
    fn register(&self, on_press: OnPress) -> Result<RegisterOutcome, DeviceError>;

    /// 登録を外す。OS が成功を返したときだけ記録を手放し、失敗なら登録中のまま残す。
    fn unregister(&self) -> Result<UnregisterOutcome, DeviceError>;
}

/// 非対応 OS 用。登録は `Unsupported`、解除は後始末のため `NotRegistered`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedGlobalHotkey;

impl GlobalHotkey for UnsupportedGlobalHotkey {
    fn register(&self, _on_press: OnPress) -> Result<RegisterOutcome, DeviceError> {
        Err(DeviceError::Unsupported("global_hotkey"))
    }

    fn unregister(&self) -> Result<UnregisterOutcome, DeviceError> {
        Ok(UnregisterOutcome::NotRegistered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_register_failed(r: &Result<RegisterOutcome, DeviceError>) -> bool {
        matches!(r, Err(DeviceError::Internal("global_hotkey register")))
    }

    #[test]
    fn macos_register_mapping() {
        assert!(matches!(
            register_outcome_from_macos(0),
            Ok(RegisterOutcome::Registered)
        ));
        assert!(matches!(
            register_outcome_from_macos(-9878),
            Ok(RegisterOutcome::InUse)
        ));
        for code in [
            -9879,
            MACOS_NOT_MAIN_THREAD,
            MACOS_NOT_REGISTERED,
            -1,
            i32::MAX,
        ] {
            assert!(is_register_failed(&register_outcome_from_macos(code)));
        }
    }

    #[test]
    fn macos_unregister_mapping() {
        assert!(matches!(
            unregister_outcome_from_macos(0),
            Ok(UnregisterOutcome::Unregistered)
        ));
        assert!(matches!(
            unregister_outcome_from_macos(MACOS_NOT_REGISTERED),
            Ok(UnregisterOutcome::NotRegistered)
        ));
        for code in [MACOS_NOT_MAIN_THREAD, -9879, -50, i32::MIN] {
            assert!(matches!(
                unregister_outcome_from_macos(code),
                Err(DeviceError::Internal("global_hotkey unregister"))
            ));
        }
    }

    #[test]
    fn windows_register_mapping() {
        assert!(matches!(
            register_outcome_from_windows(true, 0),
            Ok(RegisterOutcome::Registered)
        ));
        assert!(matches!(
            register_outcome_from_windows(false, 1409),
            Ok(RegisterOutcome::InUse)
        ));
        // 成功フラグが優先され、失敗時の未知コードは成功にならない。
        assert!(matches!(
            register_outcome_from_windows(true, 1409),
            Ok(RegisterOutcome::Registered)
        ));
        assert!(is_register_failed(&register_outcome_from_windows(false, 5)));
    }

    #[test]
    fn unsupported_never_succeeds_on_register() {
        let imp = UnsupportedGlobalHotkey;
        assert!(matches!(
            imp.register(Arc::new(|| {})),
            Err(DeviceError::Unsupported("global_hotkey"))
        ));
        assert!(matches!(
            imp.unregister(),
            Ok(UnregisterOutcome::NotRegistered)
        ));
    }
}
