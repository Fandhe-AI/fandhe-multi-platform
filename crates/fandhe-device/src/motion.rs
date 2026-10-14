//! 端末の傾き（重力ベクトル）検知の抽象（Phase 10、Issue #829）。
//!
//! `Camera`/`Http` が単発の request/response なのに対し、傾きセンサは継続 push で
//! 値が更新され続けるため、本モジュールは「最新値だけを保持する共有状態」を持つ
//! （`DummyMotion` の `Arc<Mutex<(f32, f32)>>`）。実センサ実装（Android/iOS、#831/#832）は
//! push 側から `DummyMotion` 相当の `set` を呼ぶ想定で、`fandhe-egui-proxy` 側への配線・
//! スライダー UI は #830〜#832 のスコープであり本モジュールでは行わない。

use crate::DeviceError;
use std::sync::{Arc, Mutex};

/// 傾き検知の契約。戻り値は重力ベクトル `(x, y)` で各成分 `-1.0..=1.0` に正規化されている
/// 前提（`Storage`/`Camera` と同じくスレッド間で共有されうるため `Send + Sync`、#745 参照）。
/// 対応不可プラットフォームは `DeviceError::Unsupported("motion")` を返す契約とする。
pub trait Motion: Send + Sync {
    /// 最新の傾き `(x, y)` を返す。
    fn tilt(&self) -> Result<(f32, f32), DeviceError>;
}

/// 外部から値を書き換えられるダミー実装（desktop / Web 向け、型サンプルのスライダー代替が
/// `set` を呼ぶ想定）。`Clone` した先で状態を共有する（複数箇所が同じ最新値を参照する想定の
/// 回帰確認はユニットテスト参照）。
#[derive(Clone)]
pub struct DummyMotion(Arc<Mutex<(f32, f32)>>);

impl DummyMotion {
    /// 初期値 `(0.0, 0.0)` で生成する。
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new((0.0, 0.0))))
    }

    /// 共有状態を書き換える。値は `-1.0..=1.0` へ丸めてから保存し、境界超過値が
    /// 下流の物理更新へそのまま伝播しないようにする（security.md A08）。
    /// `f32::clamp` は NaN を比較できずそのまま通すため、保存前に `0.0` へ
    /// 置換して `Motion::tilt` の値域契約（各成分 `-1.0..=1.0`）を守る。
    pub fn set(&self, x: f32, y: f32) {
        let mut state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *state = (clamp_finite(x), clamp_finite(y));
    }
}

/// NaN を `0.0` へ置換してから `-1.0..=1.0` へ丸める（`f32::clamp` は NaN との
/// 比較が常に false になり NaN をそのまま通すため、`DummyMotion::set` 側で
/// 事前に有限値化する）。
fn clamp_finite(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(-1.0, 1.0)
    }
}

impl Default for DummyMotion {
    fn default() -> Self {
        Self::new()
    }
}

impl Motion for DummyMotion {
    fn tilt(&self) -> Result<(f32, f32), DeviceError> {
        // 他所の panic に由来する poison を tilt() の呼び出し側へ伝播させない
        // （coding-rust.md: unwrap/expect を本実装コードで使わない方針。into_inner で復元する）。
        let state = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(*state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Motion` 契約側のエラー経路（`Unsupported`）を確認するための最小モック。
    /// `DummyMotion` 自体は常に成功する契約のため、この最小モックで検証する。
    struct UnsupportedMotion;

    impl Motion for UnsupportedMotion {
        fn tilt(&self) -> Result<(f32, f32), DeviceError> {
            Err(DeviceError::Unsupported("motion"))
        }
    }

    #[test]
    fn set_then_tilt_returns_latest_value() {
        let motion = DummyMotion::new();
        motion.set(0.5, -0.25);
        assert_eq!(motion.tilt().unwrap(), (0.5, -0.25));
    }

    #[test]
    fn set_clamps_out_of_range_values() {
        let motion = DummyMotion::new();
        motion.set(2.0, -2.0);
        assert_eq!(motion.tilt().unwrap(), (1.0, -1.0));
    }

    #[test]
    fn set_replaces_nan_with_finite_value() {
        let motion = DummyMotion::new();
        motion.set(f32::NAN, 0.5);
        let (x, y) = motion.tilt().unwrap();
        assert!(x.is_finite());
        assert_eq!((x, y), (0.0, 0.5));
    }

    #[test]
    fn clone_shares_state() {
        let motion = DummyMotion::new();
        let cloned = motion.clone();
        cloned.set(0.3, 0.4);
        assert_eq!(motion.tilt().unwrap(), (0.3, 0.4));
    }

    #[test]
    fn unsupported_platform_returns_unsupported_error() {
        let motion = UnsupportedMotion;
        assert!(matches!(motion.tilt(), Err(DeviceError::Unsupported(_))));
    }
}
