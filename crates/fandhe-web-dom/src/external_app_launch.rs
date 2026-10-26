//! 型サンプル「他アプリの起動（電話・地図）」の Web 経路（Issue #864、親 #176）。
//!
//! `fandhe_core::screens::external_app_launch_show`（端末依存ゼロ）を [`super::mount::mount_with_effect`]
//! の effect として観測し、`window.location.href` へ固定の `tel:` / `geo:` を代入して結果を
//! `finish_launch` へ書き戻す。開く URI は `fandhe_device::external_app_launch` の固定定数
//! （`PHONE_URI` / `GEO_URI`）だけで、外部入力から URL を組み立てる経路は無い。
//! `ExternalAppLaunch` トレイトは経由しない（`compose_mail_sms` の Web 経路と同じ構成）。
//! `fandhe-device` の「Web: `HandedOff` しか返せない」は本モジュールの話で、
//! `fandhe-egui-proxy` の wasm canvas 経路（`Unsupported` のまま）とは別経路。
//!
//! # 結果の写像
//!
//! - `window` が無い: `Unsupported`
//! - `set_href` が成功: `HandedOff`（ブラウザへ渡した、まで。ハンドラの有無は取れない）
//! - `set_href` が例外（sandbox iframe の遷移拒否等）: 固定文言の `Failed`。例外の `message` と
//!   URL は画面にも `console` にも出さない（security.md A09）
//!
//! `NoHandler` / `Cancelled` / `TargetUnavailable` は返さない（ハンドラが無くても例外は出ない）。
//! 呼び出しは同期。クリックから effect まで同期に繋がる中で遷移させる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（発送済み世代の記録と `finish_launch` の世代照合）、
//! 例外を成功に畳まないこと、固定値以外を開かないこと、マウント破棄時の記録解放。
//! 守らない: ハンドラの有無、ブラウザ差によるページ遷移扱い・無反応、PC ブラウザでの無反応、
//! 複数タブ、wasm canvas 経路。

use fandhe_core::screens::external_app_launch_show::{
    CoreOutcome, ExternalLaunchFailure, LaunchKind,
};
use fandhe_device::external_app_launch::{GEO_URI, PHONE_URI};

/// 遷移拒否の固定文言（例外の内容は含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NAVIGATE_FAILED: &str = "ブラウザが他のアプリへの遷移を拒否しました";

/// 操作に対応する固定 URI。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn uri_for(kind: LaunchKind) -> &'static str {
    match kind {
        LaunchKind::Phone => PHONE_URI,
        LaunchKind::Map => GEO_URI,
    }
}

/// `set_href` の結果の写像。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn outcome_from_navigate(r: Result<(), ()>) -> Result<CoreOutcome, ExternalLaunchFailure> {
    match r {
        Ok(()) => Ok(CoreOutcome::HandedOff),
        Err(()) => Err(ExternalLaunchFailure::Failed(NAVIGATE_FAILED.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::external_app_launch_show::{
        finish_launch, mark_launching, take_request, CoreOutcome, ExternalAppLaunchState,
    };

    use super::{outcome_from_navigate, uri_for};
    use crate::mount::Applier;

    thread_local! {
        /// マウントごとの発送済み世代（同じ世代を二重に遷移させない）。
        static DISPATCHED: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// マウント破棄時（`mount::sweep_mounts` から）に発送記録を解放する。
    pub(crate) fn release_mount(mount_id: u64) {
        DISPATCHED.with(|d| d.borrow_mut().remove(&mount_id));
    }

    /// `mount_with_effect` へ渡す effect。要求 1 件ごとに 1 回だけ遷移する。
    pub fn external_app_launch_show_effect(
        state: &ExternalAppLaunchState,
        applier: &Applier<ExternalAppLaunchState>,
    ) {
        let Some(view) = take_request(state) else {
            return;
        };
        let generation = view.generation;
        let mount_id = applier.mount_id();
        let fresh =
            DISPATCHED.with(|d| d.borrow_mut().insert(mount_id, generation) != Some(generation));
        if !fresh {
            return;
        }
        applier.apply(mark_launching);
        let result = match web_sys::window() {
            None => Ok(CoreOutcome::Unsupported),
            Some(w) => {
                outcome_from_navigate(w.location().set_href(uri_for(view.kind)).map_err(|_| ()))
            }
        };
        applier.apply(move |s| finish_launch(s, generation, result));
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::external_app_launch_show_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::external_app_launch_show::{
        finish_launch, mark_launching, open_map, open_phone, take_request, ExternalAppLaunchState,
        FAILED_PREFIX, HANDED_OFF_STATUS,
    };

    #[test]
    fn uri_is_fixed_per_kind() {
        assert_eq!(uri_for(LaunchKind::Phone), PHONE_URI);
        assert!(uri_for(LaunchKind::Phone).starts_with("tel:"));
        assert_eq!(uri_for(LaunchKind::Map), GEO_URI);
        assert!(uri_for(LaunchKind::Map).starts_with("geo:"));
    }

    #[test]
    fn navigate_maps_without_faking_success() {
        assert_eq!(outcome_from_navigate(Ok(())), Ok(CoreOutcome::HandedOff));
        assert_eq!(
            outcome_from_navigate(Err(())),
            Err(ExternalLaunchFailure::Failed(NAVIGATE_FAILED.to_string()))
        );
    }

    #[test]
    fn flow_reaches_status_or_error() {
        for (ok, check) in [(true, HANDED_OFF_STATUS), (false, FAILED_PREFIX)] {
            let mut s = ExternalAppLaunchState::default();
            open_phone(&mut s);
            let g = take_request(&s).unwrap().generation;
            mark_launching(&mut s);
            finish_launch(
                &mut s,
                g,
                outcome_from_navigate(if ok { Ok(()) } else { Err(()) }),
            );
            let text = if ok { &s.status } else { &s.error };
            assert!(text.starts_with(check));
        }
        let mut s = ExternalAppLaunchState::default();
        open_map(&mut s);
        assert_eq!(take_request(&s).unwrap().kind, LaunchKind::Map);
    }
}
