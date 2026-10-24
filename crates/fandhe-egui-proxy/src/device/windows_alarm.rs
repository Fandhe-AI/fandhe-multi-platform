//! Windows 向けアラーム実装（Issue #721、親 #140）。`device/alarm.rs` の Windows
//! 経路から呼ばれ、`fandhe_device::alarm::Alarm` を WinRT
//! `ScheduledToastNotification` + `ToastNotifier::AddToSchedule` で実装する。
//! `scenario="alarm"` のトーストは閉じるまで画面に残り、アラーム音を繰り返す
//! （公式）。予約したトーストはアプリが動いていなくても OS が時刻に出す。
//!
//! # 送り手（AUMID）
//!
//! 通知機能（`windows_notification.rs`）が登録するスタートメニューのショートカット
//! の AUMID を使い回す（[`super::windows_notification::ensure_app_aumid`]）。他アプリの
//! AUMID は借りず、新しい権限・manifest・package identity も足さない。
//!
//! # 嘘の成功の防止
//!
//! `AddToSchedule` の直後に予約一覧を読み直し、自分の Id が無ければ
//! `Unsupported("alarm")` を返す。package identity の無い実行形態で OS が予約を
//! 黙って捨てる場合に、成功を装わないため（`device-bridge-contracts.md`
//! 「6. 嘘の成功」）。通知の設定が無効なら予約前に失敗させる。
//!
//! # 守る範囲・守らない範囲
//!
//! trait doc（`fandhe_device::alarm`）の契約に従う。実装固有の点は次のとおり。
//! 古い予約を消してから新しい予約を足すため、途中で失敗すると古い予約は戻らず
//! 「予約なし」になる。schedule・cancel・state はプロセス内で `Mutex` により直列化し、
//! 判定と更新を同じロックの内側で行う。
//!
//! 時刻変換・検証・設定値の写像・XML 定数などの純関数は、ホストの `cargo test` で
//! 検証できるよう `cfg(target_os = "windows")` を付けない。WinRT 呼び出しだけを
//! [`imp`] に閉じる。`unsafe` は使わない（safe な WinRT API と既存の `ComInit` のみ）。

use fandhe_device::DeviceError;

/// 本アプリの予約を識別する Id（`ScheduledToastNotification::SetId` は 16 文字以下）。
const ALARM_ID: &str = "fandhe-alarm";

/// トーストの本体（固定リテラル）。可変の文言は XML へ連結せず `CreateTextNode` で
/// 差し込む（XML 注入を防ぐ、security.md A03）。`scenario="alarm"` はボタンが 1 つ
/// 以上無いと普通のトーストとして扱われる（公式）ため、`dismiss` ボタンを持つ。
const TOAST_XML: &str = concat!(
    r#"<toast scenario="alarm">"#,
    r#"<visual><binding template="ToastGeneric"><text></text><text></text></binding></visual>"#,
    r#"<audio src="ms-winsoundevent:Notification.Looping.Alarm" loop="true"/>"#,
    r#"<actions><action activationType="system" arguments="dismiss" content=""/></actions>"#,
    r#"</toast>"#
);

/// トーストの表題。
const TOAST_TITLE: &str = "アラーム";
/// トーストの本文。
const TOAST_BODY: &str = "予約した時刻になりました";

/// 1601-01-01（Windows の FILETIME 基準）から 1970-01-01 までの 100ns 刻みの数。
const UNIX_EPOCH_AS_FILETIME_TICKS: i64 = 116_444_736_000_000_000;
/// 1 ミリ秒あたりの 100ns 刻みの数。
const TICKS_PER_MS: i64 = 10_000;

/// UNIX ms を WinRT `DateTime.UniversalTime`（1601 年基準の 100ns 刻み）へ変換する。
/// オーバーフローは `InvalidInput`（範囲の検証は変換前に済ませず、変換自体で行う）。
pub(crate) fn unix_ms_to_filetime_ticks(ms: u64) -> Result<i64, DeviceError> {
    i64::try_from(ms)
        .ok()
        .and_then(|v| v.checked_mul(TICKS_PER_MS))
        .and_then(|v| v.checked_add(UNIX_EPOCH_AS_FILETIME_TICKS))
        .ok_or(DeviceError::InvalidInput("alarm time"))
}

/// [`unix_ms_to_filetime_ticks`] の逆変換。UNIX エポックより前は `None`。
pub(crate) fn filetime_ticks_to_unix_ms(ticks: i64) -> Option<u64> {
    let since_epoch = ticks.checked_sub(UNIX_EPOCH_AS_FILETIME_TICKS)?;
    // 0 方向への丸めでエポック直前の負値が 0 に畳まれないよう、先に符号で弾く。
    if since_epoch < 0 {
        return None;
    }
    u64::try_from(since_epoch / TICKS_PER_MS).ok()
}

/// 予約時刻が現在より未来であることを確かめる。過去・現在は `InvalidInput`
/// （`AddToSchedule` も過去の時刻では失敗する、公式）。
pub(crate) fn validate_fire_at(fire_at_unix_ms: u64, now_unix_ms: u64) -> Result<(), DeviceError> {
    if fire_at_unix_ms <= now_unix_ms {
        Err(DeviceError::InvalidInput("alarm time"))
    } else {
        Ok(())
    }
}

/// 予約一覧の Id が本アプリの予約か。
fn is_alarm_id(id: &str) -> bool {
    id == ALARM_ID
}

#[cfg(target_os = "windows")]
pub(super) use imp::WindowsAlarm;

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    use fandhe_device::alarm::{Alarm, AlarmState};
    use fandhe_device::DeviceError;
    use windows::core::HSTRING;
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::Foundation::DateTime;
    use windows::UI::Notifications::{
        ScheduledToastNotification, ToastNotificationManager, ToastNotifier,
    };

    use super::super::windows_notification::{
        ensure_app_aumid, setting_result_for, ComInit, APP_AUMID,
    };
    use super::{
        filetime_ticks_to_unix_ms, is_alarm_id, unix_ms_to_filetime_ticks, validate_fire_at,
        ALARM_ID, TOAST_BODY, TOAST_TITLE, TOAST_XML,
    };

    /// WinRT による `Alarm` 実装。`lock` は schedule・cancel・state を直列化する。
    pub(crate) struct WindowsAlarm {
        lock: Mutex<()>,
    }

    impl WindowsAlarm {
        pub(crate) fn new() -> Self {
            Self {
                lock: Mutex::new(()),
            }
        }
    }

    fn now_unix_ms() -> Result<u64, DeviceError> {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| DeviceError::Internal("alarm clock"))?;
        u64::try_from(d.as_millis()).map_err(|_| DeviceError::Internal("alarm clock"))
    }

    /// 登録済み AUMID の送り手を作り、通知設定が有効であることを確かめる。
    /// 呼び出し元が `ComInit` 済みのスレッド上で呼ぶ契約。
    fn open_notifier() -> Result<ToastNotifier, DeviceError> {
        ensure_app_aumid("alarm")?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_AUMID))
                .map_err(|_| DeviceError::Internal("alarm notifier"))?;
        let setting = notifier
            .Setting()
            .map_err(|_| DeviceError::Internal("alarm setting"))?;
        setting_result_for(setting.0, "alarm")?;
        Ok(notifier)
    }

    /// 予約一覧のうち本アプリの予約を集める。
    fn own_entries(
        notifier: &ToastNotifier,
    ) -> Result<Vec<ScheduledToastNotification>, DeviceError> {
        const ERR: DeviceError = DeviceError::Internal("alarm list");
        let list = notifier.GetScheduledToastNotifications().map_err(|_| ERR)?;
        let size = list.Size().map_err(|_| ERR)?;
        let mut out = Vec::new();
        for i in 0..size {
            let item = list.GetAt(i).map_err(|_| ERR)?;
            let id = item.Id().map_err(|_| ERR)?;
            if is_alarm_id(&id.to_string()) {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// 固定 XML に表題・本文を `CreateTextNode` で差し込む。
    fn build_xml() -> Result<XmlDocument, DeviceError> {
        const ERR: DeviceError = DeviceError::Internal("alarm xml");
        let xml = XmlDocument::new().map_err(|_| ERR)?;
        xml.LoadXml(&HSTRING::from(TOAST_XML)).map_err(|_| ERR)?;
        let texts = xml
            .GetElementsByTagName(&HSTRING::from("text"))
            .map_err(|_| ERR)?;
        for (index, value) in [(0u32, TOAST_TITLE), (1u32, TOAST_BODY)] {
            let node = texts.Item(index).map_err(|_| ERR)?;
            let text = xml.CreateTextNode(&HSTRING::from(value)).map_err(|_| ERR)?;
            node.AppendChild(&text).map_err(|_| ERR)?;
        }
        Ok(xml)
    }

    impl Alarm for WindowsAlarm {
        fn schedule(&self, fire_at_unix_ms: u64) -> Result<(), DeviceError> {
            let _guard = self
                .lock
                .lock()
                .map_err(|_| DeviceError::Internal("alarm lock"))?;
            validate_fire_at(fire_at_unix_ms, now_unix_ms()?)?;
            let ticks = unix_ms_to_filetime_ticks(fire_at_unix_ms)?;
            let _com = ComInit::new("alarm com init")?;
            let notifier = open_notifier()?;

            // 置き換え: 既存の予約を先に全部消す。以降で失敗しても古い予約は
            // 戻さない（trait doc の契約）。
            for old in own_entries(&notifier)? {
                notifier
                    .RemoveFromSchedule(&old)
                    .map_err(|_| DeviceError::Internal("alarm remove"))?;
            }

            let xml = build_xml()?;
            let toast = ScheduledToastNotification::CreateScheduledToastNotification(
                &xml,
                DateTime {
                    UniversalTime: ticks,
                },
            )
            .map_err(|_| DeviceError::Internal("alarm create"))?;
            toast
                .SetId(&HSTRING::from(ALARM_ID))
                .map_err(|_| DeviceError::Internal("alarm create"))?;
            notifier
                .AddToSchedule(&toast)
                .map_err(|_| DeviceError::Internal("alarm add"))?;

            // OS が予約を黙って捨てていないかを読み直して確かめる（嘘の成功の禁止）。
            if own_entries(&notifier)?.is_empty() {
                return Err(DeviceError::Unsupported("alarm"));
            }
            Ok(())
        }

        fn cancel(&self) -> Result<(), DeviceError> {
            let _guard = self
                .lock
                .lock()
                .map_err(|_| DeviceError::Internal("alarm lock"))?;
            let _com = ComInit::new("alarm com init")?;
            let notifier = open_notifier()?;
            for old in own_entries(&notifier)? {
                notifier
                    .RemoveFromSchedule(&old)
                    .map_err(|_| DeviceError::Internal("alarm remove"))?;
            }
            Ok(())
        }

        fn state(&self) -> Result<AlarmState, DeviceError> {
            let _guard = self
                .lock
                .lock()
                .map_err(|_| DeviceError::Internal("alarm lock"))?;
            let _com = ComInit::new("alarm com init")?;
            let notifier = open_notifier()?;
            let entries = own_entries(&notifier)?;
            let Some(first) = entries.first() else {
                return Ok(AlarmState::NotScheduled);
            };
            let delivery = first
                .DeliveryTime()
                .map_err(|_| DeviceError::Internal("alarm list"))?;
            let fire_at_unix_ms = filetime_ticks_to_unix_ms(delivery.UniversalTime)
                .ok_or(DeviceError::Internal("alarm list"))?;
            Ok(AlarmState::Scheduled { fire_at_unix_ms })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_converts_to_filetime_epoch() {
        assert_eq!(
            unix_ms_to_filetime_ticks(0).unwrap(),
            UNIX_EPOCH_AS_FILETIME_TICKS
        );
    }

    #[test]
    fn time_conversion_round_trips() {
        for ms in [0u64, 1, 1_700_000_060_000, 4_102_444_800_000] {
            let ticks = unix_ms_to_filetime_ticks(ms).unwrap();
            assert_eq!(filetime_ticks_to_unix_ms(ticks), Some(ms));
        }
    }

    #[test]
    fn before_unix_epoch_is_none() {
        assert_eq!(
            filetime_ticks_to_unix_ms(UNIX_EPOCH_AS_FILETIME_TICKS - 1),
            None
        );
        assert_eq!(filetime_ticks_to_unix_ms(0), None);
        assert_eq!(filetime_ticks_to_unix_ms(i64::MIN), None);
    }

    #[test]
    fn overflow_is_invalid_input() {
        assert!(matches!(
            unix_ms_to_filetime_ticks(u64::MAX),
            Err(DeviceError::InvalidInput("alarm time"))
        ));
        assert!(matches!(
            unix_ms_to_filetime_ticks(i64::MAX as u64),
            Err(DeviceError::InvalidInput("alarm time"))
        ));
    }

    #[test]
    fn past_and_present_times_are_rejected() {
        assert!(matches!(
            validate_fire_at(100, 200),
            Err(DeviceError::InvalidInput("alarm time"))
        ));
        assert!(matches!(
            validate_fire_at(200, 200),
            Err(DeviceError::InvalidInput("alarm time"))
        ));
        assert!(validate_fire_at(201, 200).is_ok());
    }

    #[test]
    fn toast_xml_is_alarm_scenario_with_dismiss_button() {
        assert!(TOAST_XML.contains(r#"scenario="alarm""#));
        assert!(TOAST_XML.contains("<action "));
        assert!(TOAST_XML.contains(r#"loop="true""#));
        assert!(TOAST_XML.contains("Notification.Looping.Alarm"));
    }

    #[test]
    fn alarm_id_fits_winrt_limit_and_matches_exactly() {
        assert!(ALARM_ID.chars().count() <= 16);
        assert!(is_alarm_id(ALARM_ID));
        assert!(!is_alarm_id("other"));
        assert!(!is_alarm_id(""));
    }
}
