//! システム音量・消音状態の取得と変更（Phase 11 の拡充ループ、親 #166、Issue #785、対応表
//! `docs/device/capabilities/volume-control.json`）。
//!
//! 扱う範囲は「既定の出力（Android はメディア音量）の現在の音量と消音状態を読む」ことと、
//! 「1 段階ずつ上げ下げする」ことだけ。消音の切り替え・着信音などほかの音量・出力機器の選択は
//! 扱わない。`fandhe-egui-proxy` の橋渡し層（`device/volume_control.rs`）が呼び、結果を
//! 画面（`fandhe_core::screens::volume_control_show`）へ返す。対象は macOS・Windows（#785）、
//! iOS（#786）、Android（#787）。Web は `no` のため [`UnsupportedVolumeControl`] を返す。
//!
//! # 契約
//!
//! - **OS を正にする**: 音量・消音・既定の出力機器の写しを持たない。毎回 OS へ問う。変更後は
//!   要求値ではなく、同じ呼び出しの中で読み戻した値を返す（`device-bridge-contracts.md`「6」）
//! - **結果の種類（三値 + 失敗）**: 変更済み（[`StepOutcome::Changed`]、読み戻した値）/
//!   変わらなかった（[`StepOutcome::Unchanged`]、上限・下限。**失敗ではない**）/
//!   変更できない（[`StepOutcome::NotChangeable`]、音量固定・音量を持たない出力機器・OS の UI でだけ
//!   変えられる環境）/ 権限拒否 / 非対応 / 失敗。失敗を値に畳まない（同「10」）
//! - **エラー種別**: 非対応 → `Unsupported`、権限拒否（Android のおやすみモードによる
//!   `SecurityException`）→ `PermissionDenied`、OS 呼び出しの失敗・期限切れ・値域外 → `Internal`。
//!   利用者が取り消す経路は無いので `Cancelled` は返さない
//! - **`step` の手順**: 変更前の読み取り → 変更 → 同じ呼び出しの中で読み戻し。読み戻しに失敗したら
//!   `Err(Internal)`（変更を成功として返さない）。[`VolumeControlMode::OsSlider`] と
//!   [`VolumeControlMode::NotChangeable`] では OS へ渡さず
//!   `Ok(StepOutcome::NotChangeable(reading))` を返す
//! - **値域の検証は丸め・変換の前**: OS から返った音量は [`validate_level`] を通す
//! - **段の幅**: Windows は `VolumeStepUp/Down`、Android は `adjustStreamVolume`（いずれも OS 任せ）。
//!   macOS だけアプリが決めた 1/[`MACOS_STEPS`] 刻み（[`macos_next_level`]）
//! - **外部からの変更**: ハードウェアボタン・OS の UI・他アプリによる変更は、画面が出ている間
//!   [`READ_INTERVAL`] ごとの読み直しで反映する。通知は購読しない。表示が同じでも読み直しは省かず、
//!   値を推測・補間しない。読み取りに失敗したら読み直しを止め、「読み直す」で再開する
//! - **スレッド**: macOS・iOS・Android は UI スレッドから同期で呼ぶ。Windows は COM（MTA）の都合で
//!   ワーカー上で呼び、期限 [`OS_CALL_DEADLINE`] を単調時計で測って、期限後に届いた結果は捨てる
//!
//! # OS ごとの写像（後続の OS 実装はこの契約で埋める）
//!
//! - iOS（#786）: `AVAudioSession.outputVolume` → `level`、`muted` は `None`（消音スイッチを読む公開
//!   API は無い）、`mode` は常に `OsSlider`（`MPVolumeView` は画面に 1 個だけ置く）。音声セッションの
//!   category・有効化は変えない。UI を要する呼び出しは main queue で行い、値の取得後に
//!   `crate::ios::request_repaint` で次フレームを起こす
//! - Android（#787）: `STREAM_MUSIC` の `getStreamVolume / getStreamMaxVolume` → `level`、
//!   `isStreamMute`（API 23+。minSdk 未満なら `None`）→ `muted`、`isVolumeFixed` が真なら
//!   `NotChangeable`、それ以外は `Steps`（`adjustStreamVolume(ADJUST_RAISE | ADJUST_LOWER, 0)`）。
//!   `SecurityException` は `PermissionDenied`。JNI の署名は Java 側・no-op スタブ・実体の 3 か所で一致させる
//!
//! # 守る範囲と守らない範囲
//!
//! 守るのは（1 プロセス内で）、読み戻しで表示を決めること、上限・下限・音量固定を成功や失敗に
//! すり替えないこと、期限後の結果を成功にしないこと、確保した COM 資源を全経路で解放すること。
//! 守らない範囲（`AGENTS.md`「端末機能の並行性は守備範囲で判定する」）は、他から変えてから
//! 表示に反映されるまでの最長 [`READ_INTERVAL`] の遅れ、iOS の `outputVolume` が音声セッション
//! 無効時に古い値を返す OS 側の挙動、読み取りから変更までの間に他から変わった場合（既定の出力機器の
//! 切り替えを含む）の 1 段階の起点のずれ、複数プロセス・複数窓、出力機器ごとの音量の曲線の違い。
//! いずれも表示が一時的に食い違うだけで、データ消失・資源漏れ・嘘の成功には至らない。

use std::time::Duration;

use crate::DeviceError;

/// 画面が OS を読み直す間隔（pull 型）。
pub const READ_INTERVAL: Duration = Duration::from_secs(1);

/// Windows のワーカー呼び出しが結果を待つ上限。
pub const OS_CALL_DEADLINE: Duration = Duration::from_secs(5);

/// macOS の 1 段階の幅は 1/`MACOS_STEPS`（段の幅を返す API が無いためアプリが決める）。
pub const MACOS_STEPS: u32 = 16;

/// 格子判定・比較の許容誤差（OS の Float32 の丸めを吸収する）。
const EPSILON: f32 = 1e-4;

/// 1 段階の向き。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepDirection {
    /// 上げる。
    Up,
    /// 下げる。
    Down,
}

/// 音量の変え方（OS の戻り値で決める）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeControlMode {
    /// アプリから 1 段階ずつ変えられる（macOS・Windows・Android）。
    Steps,
    /// OS の UI（iOS の `MPVolumeView`）でだけ変えられる。
    OsSlider,
    /// 変えられない（音量固定・音量を持たない出力機器・出力機器なし）。
    NotChangeable,
}

/// OS から読んだ音量の状態。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolumeReading {
    /// 現在の音量（0.0〜1.0）。音量を持たない出力機器・出力機器なしは `None`。
    pub level: Option<f32>,
    /// 消音中か。読めない OS・機器は `None`。
    pub muted: Option<bool>,
    /// 変え方。
    pub mode: VolumeControlMode,
}

/// `step` の結果。いずれも読み戻した値を持つ。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepOutcome {
    /// 読み戻しで値が変わった。
    Changed(VolumeReading),
    /// 上限・下限などで値が変わらなかった（失敗ではない）。
    Unchanged(VolumeReading),
    /// 変えられない環境だった。
    NotChangeable(VolumeReading),
}

/// OS から返った音量の検証。非有限・範囲外は `Internal`。丸め・変換の前に通す。
pub fn validate_level(raw: f32) -> Result<f32, DeviceError> {
    if raw.is_finite() && (0.0..=1.0).contains(&raw) {
        Ok(raw)
    } else {
        Err(DeviceError::Internal("volume-control level"))
    }
}

/// macOS の 1/[`MACOS_STEPS`] 刻みの格子上で、`current` から `dir` へ 1 段階動かした値。
/// 格子外の値は向きに応じて次の格子点へ寄せる。上限・下限で動かないときは `None`。
pub fn macos_next_level(current: f32, dir: StepDirection) -> Option<f32> {
    let steps = MACOS_STEPS as f32;
    let x = current * steps;
    let nearest = x.round();
    let k = if (x - nearest).abs() < EPSILON {
        match dir {
            StepDirection::Up => nearest + 1.0,
            StepDirection::Down => nearest - 1.0,
        }
    } else {
        match dir {
            StepDirection::Up => x.ceil(),
            StepDirection::Down => x.floor(),
        }
    };
    (0.0..=steps).contains(&k).then_some(k / steps)
}

/// 変更前と読み戻しから結果の種類を決める。読み戻しが変えられない環境を示すなら `NotChangeable`、
/// 音量または消音が変わっていれば `Changed`、変わっていなければ `Unchanged`。
pub fn classify_step(before: &VolumeReading, after: VolumeReading) -> StepOutcome {
    if after.mode != VolumeControlMode::Steps {
        return StepOutcome::NotChangeable(after);
    }
    let level_same = match (before.level, after.level) {
        (Some(a), Some(b)) => (a - b).abs() < EPSILON,
        (None, None) => true,
        _ => false,
    };
    if level_same && before.muted == after.muted {
        StepOutcome::Unchanged(after)
    } else {
        StepOutcome::Changed(after)
    }
}

/// システム音量の契約（モジュール doc の「契約」節）。
pub trait VolumeControl: Send + Sync {
    /// 現在の音量・消音・変え方を OS から読む。
    fn read(&self) -> Result<VolumeReading, DeviceError>;

    /// 1 段階上げ下げし、同じ呼び出しの中で読み戻した結果を返す。
    fn step(&self, direction: StepDirection) -> Result<StepOutcome, DeviceError>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedVolumeControl;

impl VolumeControl for UnsupportedVolumeControl {
    fn read(&self) -> Result<VolumeReading, DeviceError> {
        Err(DeviceError::Unsupported("volume-control"))
    }

    fn step(&self, _direction: StepDirection) -> Result<StepOutcome, DeviceError> {
        Err(DeviceError::Unsupported("volume-control"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(level: Option<f32>, muted: Option<bool>, mode: VolumeControlMode) -> VolumeReading {
        VolumeReading { level, muted, mode }
    }

    #[test]
    fn unsupported_is_not_a_fake_success() {
        let u = UnsupportedVolumeControl;
        assert!(matches!(u.read(), Err(DeviceError::Unsupported(_))));
        assert!(matches!(
            u.step(StepDirection::Up),
            Err(DeviceError::Unsupported(_))
        ));
    }

    #[test]
    fn validate_level_rejects_invalid_before_rounding() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.01, 1.01] {
            assert!(
                matches!(validate_level(bad), Err(DeviceError::Internal(_))),
                "{bad}"
            );
        }
        for ok in [0.0, 0.5, 1.0] {
            assert_eq!(validate_level(ok).unwrap(), ok);
        }
    }

    #[test]
    fn macos_next_level_on_grid() {
        assert_eq!(macos_next_level(0.5, StepDirection::Up), Some(9.0 / 16.0));
        assert_eq!(macos_next_level(0.5, StepDirection::Down), Some(7.0 / 16.0));
        assert_eq!(macos_next_level(0.0, StepDirection::Up), Some(1.0 / 16.0));
        assert_eq!(
            macos_next_level(1.0, StepDirection::Down),
            Some(15.0 / 16.0)
        );
    }

    #[test]
    fn macos_next_level_off_grid_snaps_by_direction() {
        // 0.30 * 16 = 4.8
        assert_eq!(macos_next_level(0.30, StepDirection::Up), Some(5.0 / 16.0));
        assert_eq!(
            macos_next_level(0.30, StepDirection::Down),
            Some(4.0 / 16.0)
        );
    }

    #[test]
    fn macos_next_level_stops_at_limits() {
        assert_eq!(macos_next_level(1.0, StepDirection::Up), None);
        assert_eq!(macos_next_level(0.0, StepDirection::Down), None);
        // 格子の誤差の範囲は格子上として扱う。
        assert_eq!(macos_next_level(1.0 - 1e-6, StepDirection::Up), None);
    }

    #[test]
    fn classify_changed_unchanged_and_not_changeable() {
        let before = reading(Some(0.5), Some(false), VolumeControlMode::Steps);
        let up = reading(Some(0.6), Some(false), VolumeControlMode::Steps);
        assert_eq!(classify_step(&before, up), StepOutcome::Changed(up));
        let same = reading(Some(0.5), Some(false), VolumeControlMode::Steps);
        assert_eq!(classify_step(&before, same), StepOutcome::Unchanged(same));
        let fixed = reading(Some(0.5), None, VolumeControlMode::NotChangeable);
        assert_eq!(
            classify_step(&before, fixed),
            StepOutcome::NotChangeable(fixed)
        );
    }

    #[test]
    fn unmute_counts_as_changed() {
        let before = reading(Some(0.5), Some(true), VolumeControlMode::Steps);
        let after = reading(Some(0.5), Some(false), VolumeControlMode::Steps);
        assert_eq!(classify_step(&before, after), StepOutcome::Changed(after));
    }
}
