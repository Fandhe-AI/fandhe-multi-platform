//! 型サンプル「メール・SMS の作成」の Web 経路（Issue #1387、親 #1142）。
//!
//! `fandhe_core::screens::compose_mail_sms`（端末依存ゼロ）を [`super::mount::mount_with_effect`]
//! の effect として観測し、`window.location.href` を `mailto:` / `sms:` へ遷移させて結果を
//! `finish_compose` へ書き戻す。`fandhe-device` の `ComposeMailSms` トレイトは経由せず、
//! 検証と URL 組み立ては `ComposeRequest`（macOS・Windows と共用）に一本化する。
//! `fandhe-device` module doc の「非対応（Web ほか）→ `Unsupported`」は
//! `fandhe-egui-proxy` の wasm canvas 経路の話で、本モジュールとは別経路。
//!
//! # 結果の写像
//!
//! - 入力不正: URL へ遷移せず `Invalid`（文言は `fandhe-egui-proxy/src/device/compose_mail_sms.rs`
//!   の `to_outcome` と同期する）
//! - `window` が無い: `Unsupported`
//! - `set_href` が成功: `HandedOff`（ブラウザへ渡した、まで。ハンドラの有無・送信は取れない）
//! - `set_href` が例外（sandbox iframe の遷移拒否等）: 固定文言の `Failed`。例外の `message` と
//!   宛先・件名・本文・URL は画面にも `console` にも出さない（security.md A09）
//!
//! `Queued` / `Saved` / `Cancelled` / `NoHandler` は返さない（ハンドラが無くても例外は出ない）。
//! 呼び出しは同期。クリックから effect まで同期に繋がる中で遷移させる。
//!
//! # 守る範囲 / 守らない範囲
//!
//! 守る: 1 マウント内の要求と完了の順序（発送済み世代の記録と `finish_compose` の世代照合）、
//! 入力検証と URL への注入防止、`HandedOff` を送信済みと見せないこと、マウント破棄時の記録解放。
//! 守らない: ハンドラの有無、作成画面を開いた後の利用者の操作、ブラウザ差による
//! ページ遷移扱い、デスクトップでの `sms:` の無反応、複数タブ、wasm canvas 経路。

use fandhe_core::screens::compose_mail_sms::{
    ComposeFailure, ComposeRequestView, ComposeTarget, CoreOutcome,
};
use fandhe_device::compose_mail_sms::{ComposeAttachment, ComposeRequest};
use fandhe_device::DeviceError;

/// 遷移拒否の固定文言（例外の内容は含めない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const NAVIGATE_FAILED: &str = "ブラウザが作成画面への遷移を拒否しました";

/// 画面の要求を検証済みの `ComposeRequest` にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn build_request(view: &ComposeRequestView) -> Result<ComposeRequest, DeviceError> {
    match view.target {
        ComposeTarget::Mail => {
            let req =
                ComposeRequest::mail(&view.to, &view.cc, &view.bcc, &view.subject, &view.body)?;
            match &view.attachment {
                Some(a) => req.with_attachment(ComposeAttachment::new(
                    &a.file_name,
                    &a.mime_type,
                    &a.data,
                )?),
                None => Ok(req),
            }
        }
        ComposeTarget::Sms if view.to.len() > 1 => Err(DeviceError::InvalidInput("compose to")),
        ComposeTarget::Sms => {
            ComposeRequest::sms(view.to.first().map_or("", String::as_str), &view.body)
        }
    }
}

/// 入力不正を画面用の失敗にする。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn invalid_failure(e: DeviceError) -> ComposeFailure {
    ComposeFailure::Invalid(
        match e {
            DeviceError::InvalidInput("compose to") => "宛先",
            DeviceError::InvalidInput("compose cc") => "CC",
            DeviceError::InvalidInput("compose bcc") => "BCC",
            DeviceError::InvalidInput("compose subject") => "件名",
            DeviceError::InvalidInput("compose body") => "本文",
            DeviceError::InvalidInput("compose attachment") => "添付",
            _ => "入力",
        }
        .to_string(),
    )
}

/// `set_href` の結果の写像。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn outcome_from_navigate(r: Result<(), ()>) -> Result<CoreOutcome, ComposeFailure> {
    match r {
        Ok(()) => Ok(CoreOutcome::HandedOff),
        Err(()) => Err(ComposeFailure::Failed(NAVIGATE_FAILED.to_string())),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::compose_mail_sms::{
        finish_compose, mark_composing, take_request, ComposeMailSmsState, CoreOutcome,
    };

    use super::{build_request, invalid_failure, outcome_from_navigate};
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
    pub fn compose_mail_sms_effect(
        state: &ComposeMailSmsState,
        applier: &Applier<ComposeMailSmsState>,
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
        applier.apply(mark_composing);
        let result = match build_request(&view) {
            Err(e) => Err(invalid_failure(e)),
            Ok(req) => match web_sys::window() {
                None => Ok(CoreOutcome::Unsupported),
                Some(w) => match req.to_url() {
                    Ok(url) => outcome_from_navigate(w.location().set_href(&url).map_err(|_| ())),
                    // 添付は mailto: で渡せない。落として開かず非対応にする。
                    Err(_) => Ok(CoreOutcome::Unsupported),
                },
            },
        };
        applier.apply(move |s| finish_compose(s, generation, result));
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::compose_mail_sms_effect;

/// マウント破棄時に発送記録を解放する。`mount::sweep_mounts` から呼ばれる。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::release_mount;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_core::screens::compose_mail_sms::{
        compose_mail, compose_mail_with_attachment, compose_sms, take_request, ComposeMailSmsState,
    };

    fn view(f: fn(&mut ComposeMailSmsState)) -> ComposeRequestView {
        let mut s = ComposeMailSmsState::default();
        f(&mut s);
        take_request(&s).unwrap()
    }

    #[test]
    fn fixed_values_build() {
        let m = build_request(&view(compose_mail)).unwrap();
        assert!(m.to_url().unwrap().starts_with("mailto:"));
        let s = build_request(&view(compose_sms)).unwrap();
        assert!(s.to_url().unwrap().starts_with("sms:"));
    }

    #[test]
    fn attachment_builds_but_has_no_url() {
        let r = build_request(&view(compose_mail_with_attachment)).unwrap();
        assert!(r.attachment().is_some());
        // 遷移せず Unsupported にする根拠（mailto: は添付を持てない）。
        assert!(matches!(
            r.to_url(),
            Err(DeviceError::Unsupported("compose attachment"))
        ));
    }

    #[test]
    fn injection_is_invalid_not_url() {
        let mut v = view(compose_mail);
        v.to = vec!["a@example.invalid?bcc=x@example.invalid".to_string()];
        let e = build_request(&v).unwrap_err();
        assert_eq!(invalid_failure(e), ComposeFailure::Invalid("宛先".into()));
        let mut v = view(compose_mail);
        v.cc = vec!["a@example.invalid&bcc=x@example.invalid".to_string()];
        let e = build_request(&v).unwrap_err();
        assert_eq!(invalid_failure(e), ComposeFailure::Invalid("CC".into()));
        let mut v = view(compose_mail);
        v.subject = "a\r\nBcc: x".to_string();
        let e = build_request(&v).unwrap_err();
        assert_eq!(invalid_failure(e), ComposeFailure::Invalid("件名".into()));
        assert_eq!(
            invalid_failure(DeviceError::InvalidInput("x")),
            ComposeFailure::Invalid("入力".into())
        );
    }

    #[test]
    fn navigate_maps_without_faking_success() {
        assert_eq!(outcome_from_navigate(Ok(())), Ok(CoreOutcome::HandedOff));
        assert_eq!(
            outcome_from_navigate(Err(())),
            Err(ComposeFailure::Failed(NAVIGATE_FAILED.to_string()))
        );
    }
}
