//! Phase 11 の拡充ループ（親 #1136、Issue #1330）: 接続中の音声入力・出力デバイスの名前を
//! 一覧で 1 回読む契約。`fandhe-egui-proxy` の `device/audio_devices.rs` が起動時と「再取得」
//! ボタン押下時に [`AudioDevices::list`] を 1 回だけ呼び、各 OS 実装（macOS: Core Audio、
//! Windows: MMDevice、iOS: #1331、Android: #1332）が本トレイトを実装する。Web は
//! `fandhe-web-dom` 側（#1333）で `enumerateDevices` を使い、本トレイトは経由しない。
//!
//! 範囲は「一覧を出すだけ」。出力先の選択・切り替え・音量操作は扱わない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`AudioDeviceList`]。方向ごとに [`AudioDirection`] で「列挙できた（0 件を含む）」
//!   「列挙する手段が無い」「入力を許すセッションのときだけ取れる」を区別する。0 件は正常
//! - 非対応: `Err(DeviceError::Unsupported("audio_devices"))`
//! - 失敗: `Err(DeviceError::Internal("audio_devices list"))`（OS 呼び出しの失敗、または両方向とも
//!   列挙できなかった場合。[`require_listed`]）
//!
//! `PermissionDenied` は返さない（権限ダイアログの経路が無い。I/O を開かず名前だけ読む）。
//! キャンセルと「正常終了 / 失敗」の三値は無い（同期の 1 回読みのみ）。
//!
//! # 取らないもの
//!
//! デバイス ID・UID・エンドポイント ID・MAC は読まない・保持しない・ログに出さない
//! （プライバシー）。種別（内蔵 / Bluetooth 等）・既定・使用中の印も持たない。名前の加工は
//! [`crate::device_info::clean_text`]（制御文字除去・128 文字）のみ。
//!
//! # 件数上限
//!
//! 各方向 [`MAX_DEVICES`] 件。超過分は捨て、「ほか N 件」も出さない。同名は畳まない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 0 件と「取得できない」の区別、失敗を成功値へ畳まないこと、ID を取らないこと、名前と
//! 件数の上限、1 プロセス内の取得。
//! 守らない範囲: OS 間の名前の表記や粒度の一致、読み取り後の接続・取り外しの反映（再取得まで）、
//! 複数プロセスやタブ間の一致、OS 呼び出しが戻らない場合の期限（Windows は呼び出し側が
//! ワーカーと 5 秒の期限で守る。中断できない COM 呼び出しはプロセス終了で OS が片付ける）。
//!
//! 並行性: 読み取りは 1 要求 1 回で、購読・コールバックを持たない。集約すべき並行経路は無い。
//!
//! # 既知の天井
//!
//! Android の内蔵機器は機種名で並ぶ。iOS の入力一覧はマイクを使うセッション中のみ取れる
//! （[`AudioDirection::OnlyWhileMicActive`]）。

use crate::DeviceError;
use crate::device_info::clean_text;

/// 1 方向あたりの最大件数。
pub const MAX_DEVICES: usize = 64;

/// 音声デバイス 1 件。`name` が `None` は名前を取れなかった項目（件数には数える）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioDevice {
    /// 表示名（[`clean_text`] 済み）。
    pub name: Option<String>,
}

/// 1 方向（入力 / 出力）の列挙結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioDirection {
    /// 列挙できた。0 件も含む。
    Listed(Vec<AudioDevice>),
    /// この方向を列挙する手段が無い、または信頼できる値が取れない。
    Unavailable,
    /// 入力を許すセッションのときだけ取れる（iOS の入力。#1331 が使う）。
    OnlyWhileMicActive,
}

/// 入出力の列挙結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceList {
    /// 入力デバイス。
    pub inputs: AudioDirection,
    /// 出力デバイス。
    pub outputs: AudioDirection,
}

/// 接続中の音声デバイスを 1 回読む。
pub trait AudioDevices: Send + Sync {
    /// 契約はモジュール doc 参照。
    fn list(&self) -> Result<AudioDeviceList, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedAudioDevices;

impl AudioDevices for UnsupportedAudioDevices {
    fn list(&self) -> Result<AudioDeviceList, DeviceError> {
        Err(DeviceError::Unsupported("audio_devices"))
    }
}

/// OS が返した名前の列を [`AudioDirection::Listed`] にする。各名前を [`clean_text`] に通し、
/// 先頭 [`MAX_DEVICES`] 件だけ残す。同名は畳まず、名前の無い項目も件数に数える。全 OS 実装が共用する。
pub fn listed(names: impl IntoIterator<Item = Option<String>>) -> AudioDirection {
    AudioDirection::Listed(
        names
            .into_iter()
            .take(MAX_DEVICES)
            .map(|n| AudioDevice {
                name: n.as_deref().and_then(clean_text),
            })
            .collect(),
    )
}

/// 両方向とも `Listed` でなければ `Internal`（全不明を成功に畳まない）。
pub fn require_listed(list: AudioDeviceList) -> Result<AudioDeviceList, DeviceError> {
    if matches!(list.inputs, AudioDirection::Listed(_))
        || matches!(list.outputs, AudioDirection::Listed(_))
    {
        Ok(list)
    } else {
        Err(DeviceError::Internal("audio_devices list"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedAudioDevices.list(),
            Err(DeviceError::Unsupported("audio_devices"))
        ));
    }

    #[test]
    fn listed_cleans_names_and_keeps_nameless_and_duplicates() {
        let AudioDirection::Listed(v) = listed([
            Some(" Mic\u{0}\n1 ".into()),
            Some("".into()),
            None,
            Some("Mic1".into()),
        ]) else {
            panic!()
        };
        let names: Vec<_> = v.iter().map(|d| d.name.as_deref()).collect();
        assert_eq!(names, [Some("Mic1"), None, None, Some("Mic1")]);
    }

    #[test]
    fn listed_caps_at_max() {
        let AudioDirection::Listed(v) = listed((0..65).map(|i| Some(format!("d{i}")))) else {
            panic!()
        };
        assert_eq!(v.len(), MAX_DEVICES);
        assert_eq!(v[63].name.as_deref(), Some("d63"));
    }

    #[test]
    fn require_listed_rejects_only_all_non_listed() {
        let bad = AudioDeviceList {
            inputs: AudioDirection::Unavailable,
            outputs: AudioDirection::OnlyWhileMicActive,
        };
        assert!(matches!(
            require_listed(bad),
            Err(DeviceError::Internal("audio_devices list"))
        ));
        let one = AudioDeviceList {
            inputs: AudioDirection::Unavailable,
            outputs: AudioDirection::Listed(vec![]),
        };
        assert!(require_listed(one).is_ok());
    }
}
