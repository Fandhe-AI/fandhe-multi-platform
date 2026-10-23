//! iOS 向け気圧・近接センサー実装（Issue #610、親 #123）。`ios/fandhe_ios_environment_sensors.m`
//! （CoreMotion `CMAltimeter` の気圧購読と `UIDevice` の近接監視、`build.rs` が iOS ターゲットの
//! feature `device` 有効時にコンパイルする）を `unsafe extern "C"` 6 関数で呼ぶ。#609 のスタブを
//! 置き換えたもので、dispatch（`device/environment_sensors.rs`）・共通コア画面・trait 契約
//! （`fandhe_device::environment_sensors`）は変えない。
//!
//! 照度は iOS に公開 API が無い（SensorKit は研究用 entitlement が要る）ので常に `UnsupportedOnOs`。
//!
//! `.m` は OS の生値と状態コードを返すだけで、範囲検査と契約値への写像は [`start_result`] /
//! [`read_pressure_result`] / [`read_proximity_result`] が行う（ホストでテストできるよう
//! `cfg(any(ios, test))`）。気圧は kPa の範囲検査を hPa への換算前に行う（`pressure_hpa_from_kpa`）。
//!
//! # 再描画・ライフサイクル
//!
//! `.m` が値・失敗を確定した直後に `fandhe_rust_environment_sensors_changed` を呼び、
//! `crate::ios::request_repaint` で次フレームを起こす。背景遷移・前景復帰は
//! `note_backgrounded` / `note_resumed`（ドライバの待機起点の引き直し）へ中継する。
//!
//! # 守る範囲・守らない範囲
//!
//! - 守る: 非搭載（シミュレータ含む）と OS 非対応を区別する、拒否・失敗・範囲外・古い値を成功値へ
//!   畳まない、二重購読しない、停止と背景遷移で保持値を捨てる
//! - 守らない: 気圧の絶対精度、近接で画面が消えること、複数プロセス、表示の遅れ
//!
//! # 既知の天井
//!
//! - 初回の許可ダイアログ（気圧）に 5 秒以内に答えないと、ドライバの期限で `NoReading` になる
//! - 近接の初期値と気圧の配信周期は実機で確認する（#602）
//! - シミュレータでは気圧・近接とも `NotPresent` になる

#[cfg(any(target_os = "ios", test))]
use fandhe_device::environment_sensors::{
    is_fresh, pressure_hpa_from_kpa, EnvReading, EnvSensorFailure, Proximity, PRESSURE_STALE_MS,
};

/// `.m` の start の戻り値を契約へ写す（0 成功 / 1 非搭載 / 2 権限拒否 / 他 失敗）。
#[cfg(any(target_os = "ios", test))]
fn start_result(code: i32) -> Result<(), EnvSensorFailure> {
    match code {
        0 => Ok(()),
        1 => Err(EnvSensorFailure::NotPresent),
        2 => Err(EnvSensorFailure::PermissionDenied),
        _ => Err(EnvSensorFailure::Failed),
    }
}

/// 気圧の read の戻り値を契約へ写す（0 値なし / 1 値あり / 2 権限拒否 / 他 失敗）。
/// 古い値・範囲外・NaN は値なし（`.m` 側の守りと二重にする）。
#[cfg(any(target_os = "ios", test))]
fn read_pressure_result(
    code: i32,
    kpa: f64,
    age_ms: f64,
) -> Result<Option<EnvReading>, EnvSensorFailure> {
    match code {
        0 => Ok(None),
        1 => {
            if !is_fresh(0.0, age_ms, PRESSURE_STALE_MS) {
                return Ok(None);
            }
            Ok(pressure_hpa_from_kpa(kpa).map(EnvReading::PressureHpa))
        }
        2 => Err(EnvSensorFailure::PermissionDenied),
        _ => Err(EnvSensorFailure::Failed),
    }
}

/// 近接の read の戻り値を契約へ写す（0 値なし / 1 値あり。`near` は 1 = 近い、0 = 遠い / 他 失敗）。
#[cfg(any(target_os = "ios", test))]
fn read_proximity_result(code: i32, near: i32) -> Result<Option<EnvReading>, EnvSensorFailure> {
    match code {
        0 => Ok(None),
        1 => Ok(match near {
            1 => Some(EnvReading::Proximity(Proximity::Near)),
            0 => Some(EnvReading::Proximity(Proximity::Far)),
            _ => None,
        }),
        _ => Err(EnvSensorFailure::Failed),
    }
}

#[cfg(target_os = "ios")]
mod imp {
    use std::sync::Arc;

    use fandhe_device::environment_sensors::{
        EnvReading, EnvSensorFailure, EnvironmentSensors, SensorKind,
    };

    use super::{read_pressure_result, read_proximity_result, start_result};

    // SAFETY: `ios/fandhe_ios_environment_sensors.m`（`build.rs` が iOS ターゲットの feature `device` 有効時に
    // コンパイルする）が定義する C ABI。契約は `.m` 冒頭コメント。
    #[allow(unsafe_code)]
    extern "C" {
        fn fandhe_ios_env_pressure_start() -> i32;
        fn fandhe_ios_env_pressure_read(kpa: *mut f64, age_ms: *mut f64) -> i32;
        fn fandhe_ios_env_pressure_stop();
        fn fandhe_ios_env_proximity_start() -> i32;
        fn fandhe_ios_env_proximity_read(near: *mut i32) -> i32;
        fn fandhe_ios_env_proximity_stop();
    }

    struct IosEnvironmentSensors;

    impl EnvironmentSensors for IosEnvironmentSensors {
        #[allow(unsafe_code)]
        fn start(&self, kind: SensorKind) -> Result<(), EnvSensorFailure> {
            match kind {
                // SAFETY: 引数なし。気圧の購読開始は任意のスレッドから呼べる。
                SensorKind::Pressure => start_result(unsafe { fandhe_ios_env_pressure_start() }),
                // SAFETY: 引数なし。`.m` は main 以外なら何もせず失敗コードを返す。
                SensorKind::Proximity => start_result(unsafe { fandhe_ios_env_proximity_start() }),
                SensorKind::Light => Err(EnvSensorFailure::UnsupportedOnOs),
            }
        }

        #[allow(unsafe_code)]
        fn read(&self, kind: SensorKind) -> Result<Option<EnvReading>, EnvSensorFailure> {
            match kind {
                SensorKind::Pressure => {
                    let (mut kpa, mut age_ms) = (0.0_f64, 0.0_f64);
                    // SAFETY: 2 本のポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
                    let code = unsafe { fandhe_ios_env_pressure_read(&mut kpa, &mut age_ms) };
                    read_pressure_result(code, kpa, age_ms)
                }
                SensorKind::Proximity => {
                    let mut near = 0_i32;
                    // SAFETY: ポインタは呼び出しの間有効な排他参照で、`.m` は保持しない。
                    let code = unsafe { fandhe_ios_env_proximity_read(&mut near) };
                    read_proximity_result(code, near)
                }
                SensorKind::Light => Err(EnvSensorFailure::UnsupportedOnOs),
            }
        }

        #[allow(unsafe_code)]
        fn stop(&self, kind: SensorKind) {
            match kind {
                // SAFETY: 引数なし。冪等。
                SensorKind::Pressure => unsafe { fandhe_ios_env_pressure_stop() },
                // SAFETY: 引数なし。冪等。`.m` は main 以外なら main queue へ非同期に送る。
                SensorKind::Proximity => unsafe { fandhe_ios_env_proximity_stop() },
                SensorKind::Light => {}
            }
        }
    }

    /// iOS の `EnvironmentSensors` 実装を返す（`device/environment_sensors.rs` の iOS 経路から呼ばれる）。
    pub(crate) fn implementation() -> Arc<dyn EnvironmentSensors> {
        Arc::new(IosEnvironmentSensors)
    }
}

#[cfg(target_os = "ios")]
pub(crate) use imp::implementation;

/// `.m` が値・失敗を確定した直後に呼ぶ受け口。引数なし・ポインタなしで、`dispatch_sync` は
/// しない（契約表 11）。panic しない。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_environment_sensors_changed() {
    crate::ios::request_repaint();
}

/// 背景遷移の通知（`.m` の `DidEnterBackground` から）。背景中は `NoReading` の待機を止める。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_environment_sensors_backgrounded() {
    super::environment_sensors::note_backgrounded();
}

/// 前景復帰の通知（`.m` の `WillEnterForeground` から）。ドライバの待機起点を引き直させる。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_environment_sensors_resumed() {
    super::environment_sensors::note_resumed();
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_codes_never_fold_to_ok() {
        assert_eq!(start_result(0), Ok(()));
        assert_eq!(start_result(1), Err(EnvSensorFailure::NotPresent));
        assert_eq!(start_result(2), Err(EnvSensorFailure::PermissionDenied));
        assert_eq!(start_result(3), Err(EnvSensorFailure::Failed));
        assert_eq!(start_result(-1), Err(EnvSensorFailure::Failed));
    }

    #[test]
    fn pressure_read_codes() {
        assert_eq!(read_pressure_result(0, 101.3, 0.0), Ok(None));
        assert_eq!(
            read_pressure_result(2, 0.0, 0.0),
            Err(EnvSensorFailure::PermissionDenied)
        );
        assert_eq!(
            read_pressure_result(3, 0.0, 0.0),
            Err(EnvSensorFailure::Failed)
        );
        assert_eq!(
            read_pressure_result(-1, 0.0, 0.0),
            Err(EnvSensorFailure::Failed)
        );
    }

    #[test]
    fn pressure_converts_kpa_to_hpa() {
        match read_pressure_result(1, 101.325, 10.0) {
            Ok(Some(EnvReading::PressureHpa(h))) => assert!((h - 1013.25).abs() < 1e-6),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn pressure_out_of_range_is_none() {
        for kpa in [29.9, 110.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(read_pressure_result(1, kpa, 0.0), Ok(None), "{kpa}");
        }
    }

    #[test]
    fn stale_or_invalid_age_is_none() {
        for age in [2000.1, -1.0, f64::NAN] {
            assert_eq!(read_pressure_result(1, 101.3, age), Ok(None), "{age}");
        }
    }

    #[test]
    fn proximity_read_codes() {
        assert_eq!(
            read_proximity_result(1, 1),
            Ok(Some(EnvReading::Proximity(Proximity::Near)))
        );
        assert_eq!(
            read_proximity_result(1, 0),
            Ok(Some(EnvReading::Proximity(Proximity::Far)))
        );
        assert_eq!(read_proximity_result(1, 2), Ok(None));
        assert_eq!(read_proximity_result(0, 1), Ok(None));
        assert_eq!(read_proximity_result(3, 0), Err(EnvSensorFailure::Failed));
        assert_eq!(read_proximity_result(-1, 0), Err(EnvSensorFailure::Failed));
    }
}
