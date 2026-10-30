//! BLE の周辺機器役（自分がアドバタイズし、読み取り専用の特性 1 つを持つ GATT サーバーになる）の抽象
//! （Phase 11 の拡充ループ、親 #164、Issue #778）。
//!
//! 範囲は固定のサービス UUID 1 つと読み取り専用の特性 1 つ（固定の短い文字列を返す）の登録、
//! ローカル名とサービス UUID のアドバタイズ、読み取り要求の通知だけ。書き込み・通知（購読）・
//! ペアリング・背景でのアドバタイズは扱わない。UUID・名前・値は各 OS 実装へ定数で渡し、
//! 利用者・ネットワークから受け取らない。`fandhe-egui-proxy` の effect ドライバが本 trait を呼び、
//! 事象は [`OnEvent`] 経由で画面へ反映される。macOS は CoreBluetooth `CBPeripheralManager`
//! （Issue #778）。iOS は同じ `.m` を共用するが、背景遷移の扱いを含め b-ios（#779）で委譲する。
//!
//! # 結果の種類（契約表 10）
//!
//! 画面に出す結果は次の 7 種。中断を失敗に、失敗を公開中に畳まない。
//!
//! | 区分 | 起点 |
//! |---|---|
//! | 公開中 | 開始のコールバックが成功を返した（[`PeripheralEvent::Advertising`]） |
//! | 停止（利用者） | [`BlePeripheral::stop`] が [`StopOutcome::Stopped`] を返した |
//! | 中断（iOS の背景遷移） | [`EndReason::Interrupted`] |
//! | Bluetooth オフ | [`EndReason::BluetoothOff`] |
//! | 権限拒否 | [`EndReason::PermissionDenied`] |
//! | 非対応 | [`EndReason::Unsupported`]、または非対応 OS の `Err(Unsupported("ble_peripheral"))` |
//! | 失敗 | [`EndReason::Failed`]（サービス登録・開始の失敗、開始後のリセット、未知のコード） |
//!
//! # OS を正にする
//!
//! 公開できるか・許可があるかを事前に問い合わせず、state の通知（`poweredOn` 待ち・
//! `unauthorized`・`poweredOff`・`unsupported`）とサービス登録・アドバタイズ開始のコールバックの
//! 結果を写す。アプリ側に「アドバタイズ中」の記憶を持たない。[`BlePeripheral::start`] の
//! [`StartOutcome::Accepted`] は依頼を受けたことだけを意味し、公開中とは別。公開後に state が
//! `poweredOn` を外れたら資源を解放して [`PeripheralEvent::Ended`] で確定し、自動で再開しない。
//! 数値コードの写像は本モジュールの純関数が SSOT。未知のコードは失敗に倒し、成功を作らない。
//!
//! # 期限・ポーリング
//!
//! 持たない。許可ダイアログの回答待ちに期限は無く、停止・アプリ終了で止まる（契約表 3・4）。
//!
//! # スレッド
//!
//! 各 OS 実装は受付を専用の直列キュー 1 本に集約する（Apple は dispatch の serial queue で
//! main queue は使わない。Windows は `mpsc` と専用スレッド）。そのキュー上で OS の完了を待たない。
//! 完了はコールバックからキューへ積む。`start` は非ブロッキング、`stop` はキューへの短い同期呼び出しで、
//! 開始手順が途中でも待たず、保留中の OS 操作を取り消して戻る。呼び出し元は Rust 側のロックを握ったまま
//! `stop` を呼ばない（契約表 11）。
//!
//! # 読み取りの数え方
//!
//! offset 0 の読み取りだけを [`PeripheralEvent::Read`] として 1 件と数える。長い値の続き読み
//! （offset が 0 でない要求）は応答するが数えない。公開が確定する前（[`PeripheralEvent::Advertising`]
//! より前）に応答した読み取りは捨てず、確定したあとにまとめて数える。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序、嘘の成功の禁止（「公開中」は開始のコールバック成功時だけ）、
//! 全終了経路での解放、古い世代（session）の事象を捨てること、中心役の識別子と読み取りの中身を
//! ログ・画面に出さないこと、開始手順の途中の `stop` が保留中の OS 操作を取り消して
//! [`StopOutcome::Stopped`] を返すこと、取り消しと競合して後から届いた生成結果を世代で照合して捨てること、
//! 公開が確定する前に応答した読み取りを確定後にまとめて数えること。
//!
//! 守らない: 他アプリの同時アドバタイズ、OS によるアドバタイズの間隔・中身の変更、
//! 読み取らない接続の検出、複数プロセスでの同時公開、終了処理と重なった読み取りの数え漏れ、
//! `stopAdvertising` 後に OS 側で止まるまでの時間差、取り消しを無視して完了した OS 側の
//! 非同期操作が OS の中で片付くまでの時間差、読み取り要求の到着レート（ATT の往復で上限が決まり、
//! 件数の通知は 1 件ずつキューへ積む）。

use std::sync::Arc;

use crate::DeviceError;

/// 固定のサービス UUID（本機能専用に採番した 128 bit）。
pub const SERVICE_UUID: &str = "DB43AF8A-2474-4E87-B6C8-742420170E01";
/// 固定の特性 UUID（読み取り専用）。
pub const CHARACTERISTIC_UUID: &str = "DB43AF8A-2474-4E87-B6C8-742420170E02";
/// アドバタイズするローカル名（短い ASCII）。
pub const LOCAL_NAME: &str = "FandheDemo";
/// 特性が返す固定の値（短い ASCII）。
pub const READ_VALUE: &str = "hello";

/// `.m` の事象コード: 公開中（開始のコールバック成功）。
pub const CODE_ADVERTISING: i32 = 0;
/// `.m` の事象コード: 読み取り 1 件。
pub const CODE_READ: i32 = 1;
/// `.m` の事象コード: Bluetooth オフ。
pub const CODE_BLUETOOTH_OFF: i32 = 2;
/// `.m` の事象コード: 権限拒否。
pub const CODE_PERMISSION_DENIED: i32 = 3;
/// `.m` の事象コード: 非対応。
pub const CODE_UNSUPPORTED: i32 = 4;
/// `.m` の事象コード: 失敗。
pub const CODE_FAILED: i32 = 5;
/// `.m` の事象コード: 中断（iOS の背景遷移。b-ios が発火）。
pub const CODE_INTERRUPTED: i32 = 6;

/// 公開が終わった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Bluetooth がオフ。
    BluetoothOff,
    /// 権限拒否。
    PermissionDenied,
    /// この端末・環境が周辺機器役に非対応。
    Unsupported,
    /// 失敗（登録・開始の失敗、開始後のリセット、未知のコード）。
    Failed,
    /// 中断（iOS の背景遷移。失敗とは別の区分）。
    Interrupted,
}

/// OS から届く事象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeripheralEvent {
    /// アドバタイズ開始のコールバックが成功した（公開中）。
    Advertising,
    /// 読み取り要求を 1 件応答した。
    Read,
    /// 公開が終わった（資源は解放済み）。
    Ended(EndReason),
}

/// 事象の通知先。任意のスレッドから呼ばれうるため `Send + Sync`。
/// 世代（session）の照合はドライバ側の受け口が行う。
pub type OnEvent = Arc<dyn Fn(PeripheralEvent) + Send + Sync>;

/// `start` の結果（`Err` 以外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    /// 依頼を受けた。公開中かどうかは後続の [`PeripheralEvent`] で決まる。
    Accepted,
}

/// `stop` の結果（`Err` 以外）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// 動いていた公開（開始手順の途中を含む）を止めて資源を解放した。
    Stopped,
    /// 動いていなかった（後始末の経路で失敗にしない）。
    NotRunning,
}

const STOP_FAILED: DeviceError = DeviceError::Internal("ble_peripheral stop");

/// `.m` の事象コード → 事象。未知のコードは `Ended(Failed)` に倒し、成功を作らない。
pub fn event_from_apple_code(code: i32) -> PeripheralEvent {
    match code {
        CODE_ADVERTISING => PeripheralEvent::Advertising,
        CODE_READ => PeripheralEvent::Read,
        CODE_BLUETOOTH_OFF => PeripheralEvent::Ended(EndReason::BluetoothOff),
        CODE_PERMISSION_DENIED => PeripheralEvent::Ended(EndReason::PermissionDenied),
        CODE_UNSUPPORTED => PeripheralEvent::Ended(EndReason::Unsupported),
        CODE_INTERRUPTED => PeripheralEvent::Ended(EndReason::Interrupted),
        // CODE_FAILED と未知のコード。
        _ => PeripheralEvent::Ended(EndReason::Failed),
    }
}

/// `.m` の停止の戻り値（0 が停止、1 が未起動）→ 結果。それ以外は失敗。
pub fn stop_outcome_from_apple(code: i32) -> Result<StopOutcome, DeviceError> {
    match code {
        0 => Ok(StopOutcome::Stopped),
        1 => Ok(StopOutcome::NotRunning),
        _ => Err(STOP_FAILED),
    }
}

/// 周辺機器役の OS 実装が満たす契約。詳細はモジュール doc。
pub trait BlePeripheral: Send + Sync {
    /// 公開を依頼する。`session` は事象を照合する世代で、`on_event` へ事象を流す。
    /// 進行中の公開があれば完了通知なしで置き換える。
    fn start(&self, session: u64, on_event: OnEvent) -> Result<StartOutcome, DeviceError>;

    /// 公開を止めて資源を解放する。完了の事象は流さない（戻り値が結果）。
    fn stop(&self) -> Result<StopOutcome, DeviceError>;
}

/// 非対応 OS 用。`start` は `Unsupported`、`stop` は後始末のため `NotRunning`。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedBlePeripheral;

impl BlePeripheral for UnsupportedBlePeripheral {
    fn start(&self, _session: u64, _on_event: OnEvent) -> Result<StartOutcome, DeviceError> {
        Err(DeviceError::Unsupported("ble_peripheral"))
    }

    fn stop(&self) -> Result<StopOutcome, DeviceError> {
        Ok(StopOutcome::NotRunning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_map() {
        assert_eq!(event_from_apple_code(0), PeripheralEvent::Advertising);
        assert_eq!(event_from_apple_code(1), PeripheralEvent::Read);
        assert_eq!(
            event_from_apple_code(2),
            PeripheralEvent::Ended(EndReason::BluetoothOff)
        );
        assert_eq!(
            event_from_apple_code(3),
            PeripheralEvent::Ended(EndReason::PermissionDenied)
        );
        assert_eq!(
            event_from_apple_code(4),
            PeripheralEvent::Ended(EndReason::Unsupported)
        );
        assert_eq!(
            event_from_apple_code(5),
            PeripheralEvent::Ended(EndReason::Failed)
        );
        assert_eq!(
            event_from_apple_code(6),
            PeripheralEvent::Ended(EndReason::Interrupted)
        );
    }

    #[test]
    fn unknown_codes_fail_and_never_succeed() {
        for code in [-1, 7, 99, i32::MIN, i32::MAX] {
            assert_eq!(
                event_from_apple_code(code),
                PeripheralEvent::Ended(EndReason::Failed)
            );
        }
    }

    #[test]
    fn stop_mapping() {
        assert!(matches!(
            stop_outcome_from_apple(0),
            Ok(StopOutcome::Stopped)
        ));
        assert!(matches!(
            stop_outcome_from_apple(1),
            Ok(StopOutcome::NotRunning)
        ));
        for code in [-1, 2, 99, i32::MIN, i32::MAX] {
            assert!(matches!(
                stop_outcome_from_apple(code),
                Err(DeviceError::Internal("ble_peripheral stop"))
            ));
        }
    }

    #[test]
    fn unsupported_never_succeeds_on_start() {
        let imp = UnsupportedBlePeripheral;
        assert!(matches!(
            imp.start(1, Arc::new(|_| {})),
            Err(DeviceError::Unsupported("ble_peripheral"))
        ));
        assert!(matches!(imp.stop(), Ok(StopOutcome::NotRunning)));
    }

    #[test]
    fn fixed_values_are_short_ascii() {
        for s in [LOCAL_NAME, READ_VALUE] {
            assert!(s.is_ascii() && s.len() <= 20);
        }
        assert_ne!(SERVICE_UUID, CHARACTERISTIC_UUID);
    }
}
