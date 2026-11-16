//! Android 向けネットワーク接続状態実装（Issue #1282 で枠、#1284 で実装）。
//! `driver.rs::platform_network_status` の Android 経路から呼ばれ、
//! `ConnectivityManager` / `NetworkCapabilities` を読む Java 側
//! `NetworkStatusHelper.read` を JNI の static 呼び出しで橋渡しする。
//!
//! # 同期 pull（変化通知は購読しない）
//!
//! 画面は 1 秒間隔で `read()` を呼ぶため、Windows 実装と同じく毎回 1 回だけ読む。
//! コールバック登録・要求 id・保留状態を持たないので、Activity 再生成・期限・世代の問題は
//! 構造上起きない（再生成後も `android_app()` が最新の Activity を指す）。
//!
//! # 結果コード表（SSOT は Java の `NetworkStatusHelper` 定数。JNI 署名 `()I`）
//!
//! | 値 | 意味 | 写像 |
//! |---|---|---|
//! | 負値 | 取得失敗 | `Internal` |
//! | 0 | アクティブなネットワーク無し | `Offline` |
//! | bit0 | ネットワークあり（必須） | `Online` |
//! | bit1 | `NOT_METERED` あり | `metered: Some(false)`、無ければ `Some(true)` |
//! | bit2/3/4 | Wi-Fi / 携帯 / 有線 | この優先順、無ければ `Other` |
//!
//! bit0 が無い非 0 値・未定義ビットを含む値は `Internal`（成功にしない）。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 失敗・Activity 未確立・未定義コードを「接続あり」にしない。接続詳細をログへ出さない。
//! 守らない: インターネット到達性（`VALIDATED` は見ない）、変化の反映遅れ（最大 1 秒）、
//! `TEMPORARILY_NOT_METERED`（API 30）。

use crate::contract::{ConnectionKind, NetworkSnapshot};
use crate::error::NetworkStatusError;

const FAILED_MSG: &str = "network_status";
const PRESENT: i32 = 1;
const NOT_METERED: i32 = 1 << 1;
const WIFI: i32 = 1 << 2;
const CELLULAR: i32 = 1 << 3;
const ETHERNET: i32 = 1 << 4;
const KNOWN_BITS: i32 = PRESENT | NOT_METERED | WIFI | CELLULAR | ETHERNET;

/// Java が返した結果コードを接続状態へ写す純関数（ホストの `cargo test` で固定する）。
fn snapshot_from_code(code: i32) -> Result<NetworkSnapshot, NetworkStatusError> {
    if code < 0 {
        return Err(NetworkStatusError::Internal(FAILED_MSG));
    }
    if code == 0 {
        return Ok(NetworkSnapshot::Offline);
    }
    if code & PRESENT == 0 || code & !KNOWN_BITS != 0 {
        return Err(NetworkStatusError::Internal(FAILED_MSG));
    }
    let kind = if code & WIFI != 0 {
        ConnectionKind::Wifi
    } else if code & CELLULAR != 0 {
        ConnectionKind::Cellular
    } else if code & ETHERNET != 0 {
        ConnectionKind::Ethernet
    } else {
        ConnectionKind::Other
    };
    Ok(NetworkSnapshot::Online {
        kind,
        metered: Some(code & NOT_METERED == 0),
    })
}

#[cfg(target_os = "android")]
use std::sync::Arc;

#[cfg(target_os = "android")]
use crate::contract::NetworkStatus;

/// Android 実装（`MainActivity.networkStatusRead` を同期で呼ぶ）。
#[cfg(target_os = "android")]
struct AndroidNetworkStatus;

#[cfg(target_os = "android")]
impl AndroidNetworkStatus {
    /// `NetworkStatusHelper.read(Context)` を static 呼び出しする。Activity 未確立・JNI 失敗は `None`。
    /// JavaVM と Activity の取得は `with_activity_class` が持つため、ここに `unsafe` は無い。
    fn call_read(&self) -> Option<i32> {
        use fandhe_platform_android::{JniCallFailed, android_app, with_activity_class};
        use jni::objects::JValue;
        let app = android_app()?;
        with_activity_class(
            &app,
            jni::jni_str!("com.fandhe.platform.NetworkStatusHelper"),
            |env, activity, class| -> Result<i32, JniCallFailed> {
                let value = env.call_static_method(
                    class,
                    jni::jni_str!("read"),
                    jni::jni_sig!((android.content.Context) -> int),
                    &[JValue::Object(activity)],
                )?;
                Ok(value.i()?)
            },
        )
        .ok()
    }
}

#[cfg(target_os = "android")]
impl NetworkStatus for AndroidNetworkStatus {
    fn read(&self) -> Result<NetworkSnapshot, NetworkStatusError> {
        // Activity 未確立は「非対応」ではなく一時的な失敗（Android は対応 OS）。
        let code = self
            .call_read()
            .ok_or(NetworkStatusError::Internal(FAILED_MSG))?;
        snapshot_from_code(code)
    }
}

/// Android の実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> Arc<dyn NetworkStatus> {
    Arc::new(AndroidNetworkStatus)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn online(kind: ConnectionKind, metered: bool) -> NetworkSnapshot {
        NetworkSnapshot::Online {
            kind,
            metered: Some(metered),
        }
    }

    fn is_internal(r: Result<NetworkSnapshot, NetworkStatusError>) -> bool {
        matches!(r, Err(NetworkStatusError::Internal(_)))
    }

    #[test]
    fn failures_and_unknown_codes_never_become_online() {
        for code in [
            -1,
            -2,
            i32::MIN,
            2,
            WIFI,
            1 << 5,
            PRESENT | (1 << 9),
            i32::MAX,
        ] {
            assert!(is_internal(snapshot_from_code(code)), "code={code}");
        }
    }

    #[test]
    fn zero_is_offline() {
        assert!(matches!(
            snapshot_from_code(0),
            Ok(NetworkSnapshot::Offline)
        ));
    }

    #[test]
    fn kind_priority_and_metered() {
        let cases = [
            (
                PRESENT | WIFI | CELLULAR,
                online(ConnectionKind::Wifi, true),
            ),
            (
                PRESENT | CELLULAR | ETHERNET | NOT_METERED,
                online(ConnectionKind::Cellular, false),
            ),
            (PRESENT | ETHERNET, online(ConnectionKind::Ethernet, true)),
            (PRESENT, online(ConnectionKind::Other, true)),
        ];
        for (code, want) in cases {
            assert_eq!(snapshot_from_code(code).unwrap(), want, "code={code}");
        }
    }
}
