//! Bluetooth Classic（BR/EDR）の機器一覧と検索の抽象（Phase 11 の拡充ループ、親 #130、
//! Issue #659、対応表 `docs/device/capabilities/bluetooth-classic.json`）。
//!
//! 扱う範囲は「ペアリング済み機器の一覧（画面を開いたときに読む）」と「周辺の機器を固定窓
//! （[`DISCOVERY_WINDOW_MS`]）で 1 回検索する」ことだけ。結果の 1 件は名前・種別・ペアリング済みか・
//! 電波強度（取れる OS のみ）。接続（RFCOMM / SPP）・ペアリング操作・Bluetooth の ON 操作は扱わない。
//! 対象は macOS・Windows・Android。iOS は公開 API が無く、Web は BLE のみのため対象外。
//! BLE の周辺機器は `bluetooth_le` の範囲で、本機能の結果に含めない。
//!
//! 一覧は同期の 1 回読み（[`BluetoothClassic::paired`]、`usb_device` と同型）、検索は
//! `fandhe-egui-proxy` の `device/bluetooth_classic.rs` が pull 型（`lan_discovery` と同型）で
//! 駆動する。各 OS が集めた生の結果は [`normalize_devices`] で契約の形に整える。
//!
//! # 契約
//!
//! - **0 件は成功**: 空リストを `Ok` で返す。失敗にしない
//! - **失敗の区別**: Bluetooth がオフ → `Disabled`、権限の拒否 → `PermissionDenied`、非対応 OS・
//!   アダプタなし → `Unsupported`。中断・期限超過・起動失敗・Android 30 以下で位置情報がオフの
//!   場合は `Internal`。いずれも空リストの成功へ畳まない（`device-bridge-contracts.md` 6）。
//!   失敗したときは部分結果を捨てる
//! - **三値の扱い**: 利用者が取り消す操作は無いため、結果は成功（0 件を含む）と失敗の 2 値で、
//!   `Cancelled` は返さない（同 10）
//! - **再要求は置き換え**: 前の検索を置き換え、古い世代の完了は照合して捨てる（同 2）
//! - **期限**: 窓は呼び出しの開始を起点に単調時計で測る（同 3）。Windows の同期 inquiry は途中で
//!   止められないため、窓に [`WINDOWS_DEADLINE_GRACE_MS`] を足した時点で `Internal` を確定し、
//!   遅れて届いた結果は世代照合で捨てる
//! - **アドレスを持たない**: 機器のアドレスは [`ClassicDevice`] に含めない。同一機器の判定に必要な
//!   OS のキーは [`DeviceKey`] に閉じ込め、画面側（`fandhe-core`）へは渡さない
//! - **取れない値は `None`**: 名前が無い場合は `None`（空文字の番兵にしない）、電波強度が取れない場合も
//!   `None`（127 や 0 の番兵にしない、同 8）
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 受付は OS ごとに 1 本の直列経路へ集める。
//!
//! | OS | 直列経路 |
//! |---|---|
//! | macOS | main queue（IOBluetooth は run loop が要る） |
//! | Windows | 専用ワーカー |
//! | Android | main Handler |
//!
//! 守る: 1 プロセス内の要求と完了の順序、嘘の成功を出さないこと、検索の停止と資源の解放。
//! 守らない範囲:
//!
//! - 検索結果のペアリング済み機器が、窓の時点で圏内にあること
//! - 窓内で名前が解決しきれないこと
//! - 電波強度が取れない OS・機器があること
//! - OS による inquiry の間引き
//! - 他アプリが同時に検索している場合の結果の違い
//! - 発見可能モードでない機器が出ないこと
//!
//! 機器名・アドレスはログに出さない（security.md A09）。

use std::collections::HashMap;
use std::fmt;

use crate::DeviceError;

/// 1 回の検索窓（ミリ秒）。
pub const DISCOVERY_WINDOW_MS: u64 = 12_000;
/// Windows が窓に足す猶予（ミリ秒）。この時点で `Internal` を確定する（同期 inquiry を止められないため）。
pub const WINDOWS_DEADLINE_GRACE_MS: u64 = 3_000;
/// 返す機器数の上限。
pub const MAX_DEVICES: usize = 50;
/// 機器名の最大バイト数（Bluetooth の機器名の上限）。
pub const MAX_NAME_BYTES: usize = 248;

/// Class of Device の major class に対応する機器の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    /// パソコン。
    Computer,
    /// 電話。
    Phone,
    /// ネットワーク機器。
    Network,
    /// オーディオ・ビデオ。
    AudioVideo,
    /// 入力機器など。
    Peripheral,
    /// 画像機器（プリンタなど）。
    Imaging,
    /// ウェアラブル。
    Wearable,
    /// 玩具。
    Toy,
    /// 健康機器。
    Health,
    /// その他（Miscellaneous）。
    Misc,
    /// 未分類（major class 0x1F）。
    Uncategorized,
    /// 未定義の値、または種別を取れなかった。
    Unknown,
}

/// 検索・一覧で見つかった機器 1 件。アドレスは持たない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassicDevice {
    /// 機器名。取れなければ `None`。
    pub name: Option<String>,
    /// 種別。
    pub kind: DeviceKind,
    /// ペアリング済みか。
    pub paired: bool,
    /// 電波強度（dBm）。取れなければ `None`。
    pub rssi: Option<i8>,
}

/// 同一機器をまとめるための不透明キー（OS のアドレス文字列など）。
/// `Debug` は伏せ字で、`Display` は実装しない。画面側（`fandhe-core`）へは渡さない。
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct DeviceKey(String);

impl DeviceKey {
    /// OS が返した識別子からキーを作る。空文字は `None`。
    pub fn new(raw: impl Into<String>) -> Option<Self> {
        let raw = raw.into();
        (!raw.is_empty()).then_some(Self(raw))
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeviceKey(<redacted>)")
    }
}

/// Bluetooth Classic の契約（モジュール doc の「契約」節）。
pub trait BluetoothClassic: Send + Sync {
    /// ペアリング済み機器を 1 回読む。[`normalize_devices`] 済み。
    fn paired(&self) -> Result<Vec<ClassicDevice>, DeviceError>;
    /// 約 [`DISCOVERY_WINDOW_MS`] 検索して [`normalize_devices`] 済みの一覧を返す。
    fn discover(&self) -> Result<Vec<ClassicDevice>, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedBluetoothClassic;

impl BluetoothClassic for UnsupportedBluetoothClassic {
    fn paired(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
        Err(DeviceError::Unsupported("bluetooth_classic"))
    }

    fn discover(&self) -> Result<Vec<ClassicDevice>, DeviceError> {
        Err(DeviceError::Unsupported("bluetooth_classic"))
    }
}

/// Class of Device（24 bit）の major class（bit 8〜12）を種別へ写す。
/// 0x1F は `Uncategorized`、未定義の値は `Unknown`。
pub fn major_from_cod(cod: u32) -> DeviceKind {
    match (cod >> 8) & 0x1F {
        0x01 => DeviceKind::Computer,
        0x02 => DeviceKind::Phone,
        0x03 => DeviceKind::Network,
        0x04 => DeviceKind::AudioVideo,
        0x05 => DeviceKind::Peripheral,
        0x06 => DeviceKind::Imaging,
        0x07 => DeviceKind::Wearable,
        0x08 => DeviceKind::Toy,
        0x09 => DeviceKind::Health,
        0x00 => DeviceKind::Misc,
        0x1F => DeviceKind::Uncategorized,
        _ => DeviceKind::Unknown,
    }
}

/// OS の生の電波強度を dBm の `i8` へ写す。値域（-127..=20）の検証を変換より先に行い、
/// 範囲外と未取得の番兵（127）は `None`。
pub fn rssi_from_raw(raw: i32) -> Option<i8> {
    if !(-127..=20).contains(&raw) {
        return None;
    }
    i8::try_from(raw).ok()
}

/// 機器名を整える。制御文字を除き、[`MAX_NAME_BYTES`] 以内に文字境界で切る。
/// 空、または制御文字を除くと空になる名前は `None`。
pub fn clamp_name(name: String) -> Option<String> {
    let mut name: String = name.chars().filter(|c| !c.is_control()).collect();
    if name.trim().is_empty() {
        return None;
    }
    if name.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name.truncate(end);
    }
    Some(name)
}

/// 観測順の生の結果を契約の形へ整える。
///
/// - 同じキーは 1 件にまとめる。名前と電波強度は後から届いた `Some` を採り、`None` では消さない。
///   種別は `Unknown` でないものを採る。`paired` は OR を取る
/// - 並びはペアリング済みが先、次に名前の昇順（名前なしは末尾）。上限 [`MAX_DEVICES`] 件
pub fn normalize_devices(
    observed: impl IntoIterator<Item = (DeviceKey, ClassicDevice)>,
) -> Vec<ClassicDevice> {
    let mut merged: HashMap<DeviceKey, ClassicDevice> = HashMap::new();
    for (key, dev) in observed {
        match merged.get_mut(&key) {
            Some(prev) => {
                if dev.name.is_some() {
                    prev.name = dev.name;
                }
                if dev.rssi.is_some() {
                    prev.rssi = dev.rssi;
                }
                if dev.kind != DeviceKind::Unknown {
                    prev.kind = dev.kind;
                }
                prev.paired |= dev.paired;
            }
            None => {
                merged.insert(key, dev);
            }
        }
    }
    let mut out: Vec<ClassicDevice> = merged.into_values().collect();
    out.sort_by(|a, b| {
        b.paired
            .cmp(&a.paired)
            .then_with(|| match (&a.name, &b.name) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
    });
    out.truncate(MAX_DEVICES);
    out
}

/// macOS・Android 実装が共通で使う完了コード → 契約のエラー（SSOT。OS 側で二重に写像しない）。
///
/// | code | 写像 |
/// |---|---|
/// | 0 | 成功 |
/// | 1 | `PermissionDenied` |
/// | 2 | `Disabled` |
/// | 3 | `Unsupported` |
/// | その他 | `Internal` |
pub fn decode_code(code: i32) -> Result<(), DeviceError> {
    const F: &str = "bluetooth_classic";
    match code {
        0 => Ok(()),
        1 => Err(DeviceError::PermissionDenied(F)),
        2 => Err(DeviceError::Disabled(F)),
        3 => Err(DeviceError::Unsupported(F)),
        _ => Err(DeviceError::Internal("bluetooth_classic outcome")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> DeviceKey {
        DeviceKey::new(s).unwrap()
    }

    fn dev(name: Option<&str>, paired: bool, rssi: Option<i8>) -> ClassicDevice {
        ClassicDevice {
            name: name.map(Into::into),
            kind: DeviceKind::Unknown,
            paired,
            rssi,
        }
    }

    #[test]
    fn major_from_cod_table() {
        let cases = [
            (0x0000_0100, DeviceKind::Computer),
            (0x0000_0204, DeviceKind::Phone),
            (0x0000_0300, DeviceKind::Network),
            (0x0000_0418, DeviceKind::AudioVideo),
            (0x0000_0540, DeviceKind::Peripheral),
            (0x0000_0600, DeviceKind::Imaging),
            (0x0000_0700, DeviceKind::Wearable),
            (0x0000_0800, DeviceKind::Toy),
            (0x0000_0900, DeviceKind::Health),
            (0x0000_0000, DeviceKind::Misc),
            (0x0000_1F00, DeviceKind::Uncategorized),
            (0x0000_0A00, DeviceKind::Unknown),
            (0x0000_1E00, DeviceKind::Unknown),
        ];
        for (cod, kind) in cases {
            assert_eq!(major_from_cod(cod), kind, "cod={cod:#x}");
        }
        // service class など上位ビットは major に影響しない。
        assert_eq!(major_from_cod(0x0020_0204), DeviceKind::Phone);
    }

    #[test]
    fn rssi_from_raw_rejects_sentinel_and_out_of_range() {
        assert_eq!(rssi_from_raw(-60), Some(-60));
        assert_eq!(rssi_from_raw(-127), Some(-127));
        assert_eq!(rssi_from_raw(20), Some(20));
        assert_eq!(rssi_from_raw(127), None);
        assert_eq!(rssi_from_raw(21), None);
        assert_eq!(rssi_from_raw(-128), None);
        assert_eq!(rssi_from_raw(i32::MIN), None);
        assert_eq!(rssi_from_raw(i32::MAX), None);
    }

    #[test]
    fn clamp_name_handles_boundary_empty_and_control() {
        assert_eq!(clamp_name(String::new()), None);
        assert_eq!(clamp_name("\u{1}\n\t".into()), None);
        assert_eq!(clamp_name("   ".into()), None);
        assert_eq!(clamp_name("ab\u{1}c".into()).as_deref(), Some("abc"));
        // 3 バイト文字で上限をまたぐ: 248 = 82 * 3 + 2 なので 82 文字で切れる。
        let long = "あ".repeat(100);
        let out = clamp_name(long).unwrap();
        assert!(out.len() <= MAX_NAME_BYTES);
        assert_eq!(out.chars().count(), 82);
    }

    #[test]
    fn normalize_merges_without_erasing_with_none_and_ors_paired() {
        let out = normalize_devices([
            (key("a"), dev(Some("Alpha"), false, Some(-50))),
            (key("a"), dev(None, true, None)),
        ]);
        assert_eq!(out, vec![dev(Some("Alpha"), true, Some(-50))]);
        let out = normalize_devices([
            (key("a"), dev(None, false, None)),
            (key("a"), dev(Some("Late"), false, Some(-70))),
        ]);
        assert_eq!(out, vec![dev(Some("Late"), false, Some(-70))]);
    }

    #[test]
    fn normalize_keeps_known_kind_over_unknown() {
        let mut phone = dev(None, false, None);
        phone.kind = DeviceKind::Phone;
        let out = normalize_devices([(key("a"), phone), (key("a"), dev(None, false, None))]);
        assert_eq!(out[0].kind, DeviceKind::Phone);
    }

    #[test]
    fn normalize_sorts_paired_first_then_name_with_nameless_last() {
        let out = normalize_devices([
            (key("1"), dev(None, false, None)),
            (key("2"), dev(Some("b"), false, None)),
            (key("3"), dev(Some("z"), true, None)),
            (key("4"), dev(Some("a"), false, None)),
            (key("5"), dev(None, true, None)),
        ]);
        let names: Vec<_> = out.iter().map(|d| (d.paired, d.name.as_deref())).collect();
        assert_eq!(
            names,
            vec![
                (true, Some("z")),
                (true, None),
                (false, Some("a")),
                (false, Some("b")),
                (false, None)
            ]
        );
    }

    #[test]
    fn normalize_caps_and_accepts_empty() {
        assert!(normalize_devices([]).is_empty());
        let many = (0..MAX_DEVICES + 5).map(|i| (key(&i.to_string()), dev(Some("x"), false, None)));
        assert_eq!(normalize_devices(many).len(), MAX_DEVICES);
    }

    #[test]
    fn decode_code_table() {
        assert!(decode_code(0).is_ok());
        assert!(matches!(
            decode_code(1),
            Err(DeviceError::PermissionDenied(_))
        ));
        assert!(matches!(decode_code(2), Err(DeviceError::Disabled(_))));
        assert!(matches!(decode_code(3), Err(DeviceError::Unsupported(_))));
        assert!(matches!(decode_code(4), Err(DeviceError::Internal(_))));
        assert!(matches!(decode_code(-1), Err(DeviceError::Internal(_))));
    }

    #[test]
    fn unsupported_is_error_not_empty_success() {
        assert!(matches!(
            UnsupportedBluetoothClassic.paired(),
            Err(DeviceError::Unsupported("bluetooth_classic"))
        ));
        assert!(matches!(
            UnsupportedBluetoothClassic.discover(),
            Err(DeviceError::Unsupported("bluetooth_classic"))
        ));
    }

    #[test]
    fn device_key_debug_hides_content_and_rejects_empty() {
        let k = key("AA-BB-CC-DD-EE-FF");
        let s = format!("{k:?}");
        assert!(!s.contains("AA-BB"));
        assert!(DeviceKey::new("").is_none());
    }
}
