//! Bluetooth LE 周辺機器スキャンの抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/bluetooth-le.json`）。
//!
//! 扱う範囲は「周辺の BLE 機器を一定時間（[`SCAN_DURATION_MS`]）スキャンし、識別子・名前・
//! 電波強度（RSSI, dBm）を一覧で返す」ことだけ。接続・読み書き・ペアリング・
//! バックグラウンドスキャンは扱わない。対象は iOS / Android / macOS / Windows / Web。
//! Web は Web Bluetooth の `requestDevice` で利用者が 1 台選ぶ形（Chrome 系のみ。
//! `navigator.bluetooth` が無いブラウザは `Unsupported`）で、RSSI は取れないため
//! 未取得（`None`）で表す。
//!
//! request/response 型（要求 1 件を出して 1 件の結果を待つ）で、橋渡し層
//! （`fandhe-egui-proxy` の `device/`・`fandhe-web-dom`）は
//! `crates/fandhe-egui-proxy/src/device/request_slot.rs` の `RequestSlot` を使って
//! 要求追跡を行う前提。各 OS 実装が集めた生の結果は [`normalize_scan`] で契約の形
//! （重複まとめ・RSSI 降順・件数上限）に整える。
//!
//! # 契約
//!
//! - **結果の 1 件**: 識別子（OS が付ける不透明な文字列）・名前（無ければ `None`）・
//!   RSSI（無ければ `None`。`0` などの番兵値にしない、`device-bridge-contracts.md`
//!   「8. 秘密値」）
//! - **同じ機器は 1 件**: 識別子が同じものは 1 件にまとめ、最後に観測した RSSI を残す
//!   （名前は後から得られたものを優先し、無くなった値で消さない）
//! - **並び**: RSSI 降順、未取得は末尾。同順位は識別子の昇順
//! - **件数上限**: 並べた後の先頭 [`MAX_DEVICES`] 件だけ返す
//! - **0 台は成功**: 空リストを `Ok` で返す。失敗にしない
//! - **失敗の区別**: Bluetooth がオフ → `Err(Disabled)`、非搭載・Web で API なし →
//!   `Err(Unsupported)`、権限拒否 → `Err(PermissionDenied)`、それ以外 → `Err(Internal)`。
//!   いずれも空リストの成功へ畳まない（`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **キャンセル**: Web で利用者が選択ダイアログを閉じたときは `Err(Cancelled)`
//!   （失敗として見せない、同「10. 三値」）
//! - **再要求は置き換え**: スキャン中の新しい要求は古い要求を置き換え、古い世代の
//!   結果・完了通知は照合して捨てる（同「2. 並行性・世代」。`RequestSlot` の世代照合）
//! - **画面を離れる・終了時**: 実装は進行中のスキャンを止め、確保したハンドルを解放する
//!   （同「1. ライフサイクル境界」「9. 失敗時のリソース解放」）。スキャンは固定時間
//!   で自然に終わるため、期限は実装側の [`SCAN_DURATION_MS`] だけとし、呼び出し側で
//!   別の期限を持たない
//!
//! # 守る範囲と既知の制約
//!
//! 守るのは 1 画面・1 プロセス内の要求と完了の順序、失敗を成功と見せないこと、
//! スキャンの停止（資源の解放）。既知の制約（`AGENTS.md`「端末機能の並行性は守備範囲で
//! 判定する」）として、同一機器の RSSI は 1 回の観測値で揺れる（平滑化しない）。
//! 複数プロセスが同時にスキャンした場合の結果の違い、OS が広告を間引く影響は扱わない。
//!
//! **アプリの背景遷移はスキャンの失敗として扱わない**（オーナー判断 10/5）。前景用の
//! スキャン中にアプリが背景へ移っても、実装はスキャンを止めず期限（[`SCAN_DURATION_MS`]）
//! で通常どおり終了し、それまでに届いた結果を成功として返してよい。背景中は OS が広告の
//! 受信を間引く・止める（iOS 等）ため件数が減りうるのは既知の制約。背景遷移と期限到達の
//! 前後関係を判定に使わない（一般的な前景スキャンの扱いと同じ）。

use std::collections::HashMap;

use crate::DeviceError;

/// 1 回のスキャン時間（ミリ秒）。
pub const SCAN_DURATION_MS: u64 = 10_000;
/// 返す機器数の上限。
pub const MAX_DEVICES: usize = 50;

/// スキャンで見つかった BLE 機器 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BleDevice {
    /// OS が付ける識別子（不透明な文字列。iOS / macOS は CoreBluetooth の UUID、Android / Windows は Bluetooth アドレス相当）。
    pub id: String,
    /// 広告された名前。無ければ `None`。
    pub name: Option<String>,
    /// 電波強度（dBm）。取れなければ `None`（Web は常に `None`）。
    pub rssi_dbm: Option<i16>,
}

/// BLE スキャンの契約（モジュール doc の「契約」節）。
pub trait BluetoothLe: Send + Sync {
    /// 約 [`SCAN_DURATION_MS`] スキャンして [`normalize_scan`] 済みの一覧を返す。
    fn scan(&self) -> Result<Vec<BleDevice>, DeviceError>;
}

/// 対象外 OS（Linux 等）・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedBluetoothLe;

impl BluetoothLe for UnsupportedBluetoothLe {
    fn scan(&self) -> Result<Vec<BleDevice>, DeviceError> {
        Err(DeviceError::Unsupported("bluetooth_le"))
    }
}

/// 観測順の生の結果を契約の形（同一 id を 1 件にまとめ、RSSI 降順・未取得は末尾、
/// 上限 [`MAX_DEVICES`]）へ整える。各 OS 実装が `scan` の戻り値を作る直前に使う。
pub fn normalize_scan(observed: impl IntoIterator<Item = BleDevice>) -> Vec<BleDevice> {
    let mut merged: HashMap<String, BleDevice> = HashMap::new();
    for d in observed {
        match merged.get_mut(&d.id) {
            Some(prev) => {
                prev.rssi_dbm = d.rssi_dbm;
                if d.name.is_some() {
                    prev.name = d.name;
                }
            }
            None => {
                merged.insert(d.id.clone(), d);
            }
        }
    }
    let mut out: Vec<BleDevice> = merged.into_values().collect();
    // `Option<i16>` は None < Some なので、降順にすると未取得が末尾になる。
    out.sort_by(|a, b| b.rssi_dbm.cmp(&a.rssi_dbm).then_with(|| a.id.cmp(&b.id)));
    out.truncate(MAX_DEVICES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(id: &str, name: Option<&str>, rssi: Option<i16>) -> BleDevice {
        BleDevice {
            id: id.into(),
            name: name.map(Into::into),
            rssi_dbm: rssi,
        }
    }

    #[test]
    fn merges_same_id_keeping_latest_rssi_and_known_name() {
        let out = normalize_scan([dev("a", Some("A"), Some(-80)), dev("a", None, Some(-60))]);
        assert_eq!(out, vec![dev("a", Some("A"), Some(-60))]);
    }

    #[test]
    fn sorts_rssi_desc_with_unknown_last() {
        let out = normalize_scan([
            dev("c", None, None),
            dev("b", None, Some(-90)),
            dev("a", None, Some(-40)),
        ]);
        let ids: Vec<_> = out.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn empty_is_empty_and_cap_applies() {
        assert!(normalize_scan([]).is_empty());
        let many = (0..MAX_DEVICES + 5).map(|i| dev(&format!("{i:03}"), None, Some(-50)));
        assert_eq!(normalize_scan(many).len(), MAX_DEVICES);
    }

    #[test]
    fn unsupported_scan_is_error_not_empty() {
        assert!(matches!(
            UnsupportedBluetoothLe.scan(),
            Err(DeviceError::Unsupported("bluetooth_le"))
        ));
    }
}
