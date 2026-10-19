//! 型サンプル「カスタム URL scheme」の Web 経路（親 #1116、Issue #1219）。
//!
//! `fandhe_core::screens::url_scheme_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。
//! 受信の検査・保持は `fandhe_device::url_scheme`（契約の SSOT）をそのまま使う。
//! Web は `fandhe-device` の trait を通らない（`haptics.rs` と同じ理由）。
//!
//! - 登録: ボタン押下で `navigator.registerProtocolHandler("web+fandhe", <自オリジンの URL>)`
//! - 受信: ハンドラが起動されるとブラウザが自ページへ遷移するため、読み込み時に 1 回だけ
//!   クエリ `url` を読む（ポーリング不要）。検査を通ったものだけ表示し、弾いたものは件数のみ
//! - `Ok` は「登録を要求した」まで。ブラウザの確認結果は検知できない
//!
//! - ショートカット: manifest の `shortcuts`（`web/manifest.webmanifest`）で宣言し、`url` を上記の
//!   受け口（`?screen=url_scheme_show&url=`）へ向ける。新しい受信経路は持たない。項目が出たかは
//!   検知できないため `Installed` にはせず、読み込み時に 1 回 `Declared` / `Unsupported` / `Failed` を注入する。
//!   守らない範囲: 未インストール PWA・Firefox / Safari での非表示、他ページからの同一 URL との区別
//!
//! URL・例外の中身は `console` へ出さない（security.md A09）。

use fandhe_core::screens::url_scheme_show::{ActionFailure, ActionOutcome, ShortcutStatus};

/// ショートカット宣言の状態を画面区分へ写す純関数。
/// 表示は検知できないので成功側は `Declared` 止まり。非 secure は PWA を入れられず、
/// manifest 欠落は構成の不備なので、どちらも成功にしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify_shortcut(secure: bool, has_manifest: bool) -> ShortcutStatus {
    if !secure {
        ShortcutStatus::Unsupported
    } else if !has_manifest {
        ShortcutStatus::Failed
    } else {
        ShortcutStatus::Declared
    }
}

/// 登録時に付ける画面指定とクエリ。`%s` はブラウザが URL エンコードして埋める。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const HANDLER_QUERY: &str = "?screen=url_scheme_show&url=%s";

/// 非 secure context のときの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const INSECURE: &str = "https（または localhost）のページでのみ登録できます";

/// 登録呼び出しが例外になったときの固定文言。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const THREW: &str = "ブラウザが登録を受け付けませんでした";

/// 登録 API の有無・secure context・例外の有無を画面結果へ写す純関数。
/// insecure では関数自体が露出しないため、`Unsupported` と区別するよう secure を先に見る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn classify_register(
    has_fn: bool,
    secure: bool,
    threw: bool,
) -> Result<ActionOutcome, ActionFailure> {
    if !secure {
        return Err(ActionFailure::Failed(INSECURE.to_string()));
    }
    if !has_fn {
        return Err(ActionFailure::Unsupported);
    }
    if threw {
        return Err(ActionFailure::Failed(THREW.to_string()));
    }
    Ok(ActionOutcome::AwaitingConfirmation)
}

/// 自オリジン + 現在のパスからハンドラ URL を組み立てる（外部入力は混ぜない）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn handler_url(origin: &str, pathname: &str) -> String {
    format!("{origin}{pathname}{HANDLER_QUERY}")
}

/// クエリ文字列から `key` の値を、置換なしのパーセントデコードで元のバイト列として取り出す。
/// 厳密な UTF-8 検査は受け口（`receive_bytes`）に任せ、ここでは bytes を壊さない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn query_param_bytes(search: &str, key: &str) -> Option<Vec<u8>> {
    let q = search.strip_prefix('?').unwrap_or(search);
    let raw = q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })?;
    let b = raw.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1])? << 4 | hex(b[i + 2])?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(out)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::Once;

    use fandhe_core::screens::url_scheme_show::{
        apply_shortcut_status, apply_snapshot, finish_action, mark_in_flight, take_request, Action,
        ActionFailure, ActionOutcome, ShortcutStatus, UrlSchemeShowState,
    };
    use fandhe_device::url_scheme::{UrlInbox, WEB_SCHEME};
    use js_sys::{Function, Reflect};
    use wasm_bindgen::JsValue;

    use super::{classify_register, classify_shortcut, handler_url};
    use crate::mount::Applier;

    static INBOX: UrlInbox = UrlInbox::new();
    static INGEST: Once = Once::new();

    /// クエリ `url` を 1 回だけ受け口へ通す。`url` が無ければ受信として数えない。
    fn ingest_query() {
        INGEST.call_once(|| {
            let Some(window) = web_sys::window() else {
                return;
            };
            let Ok(search) = window.location().search() else {
                return;
            };
            // UrlSearchParams は不正 UTF-8 を U+FFFD へ置換して検査をすり抜けるため使わない。
            if let Some(raw) = super::query_param_bytes(&search, "url") {
                // 弾いた場合も件数へ反映済みのため結果は捨てる。
                let _ = INBOX.receive_bytes(Some(&raw), WEB_SCHEME, js_sys::Date::now() as u64);
            }
        });
    }

    fn try_register() -> Result<ActionOutcome, ActionFailure> {
        let Some(window) = web_sys::window() else {
            return classify_register(false, false, false);
        };
        let secure = window.is_secure_context();
        let nav: JsValue = window.navigator().into();
        let f = Reflect::get(&nav, &JsValue::from_str("registerProtocolHandler"))
            .ok()
            .filter(|v| v.is_function());
        let Some(f) = f else {
            return classify_register(false, secure, false);
        };
        let loc = window.location();
        let url = handler_url(
            &loc.origin().unwrap_or_default(),
            &loc.pathname().unwrap_or_default(),
        );
        let threw = Function::from(f)
            .call2(
                &nav,
                &JsValue::from_str(WEB_SCHEME),
                &JsValue::from_str(&url),
            )
            .is_err();
        classify_register(true, secure, threw)
    }

    /// `mount_with_effect` へ渡す effect。
    pub fn url_scheme_show_effect(
        state: &UrlSchemeShowState,
        applier: &Applier<UrlSchemeShowState>,
    ) {
        ingest_query();
        if state.shortcut_status() == ShortcutStatus::Unread {
            let (secure, has_manifest) = web_sys::window()
                .map(|w| {
                    let has = w
                        .document()
                        .and_then(|d| d.query_selector("link[rel=manifest]").ok().flatten())
                        .is_some();
                    (w.is_secure_context(), has)
                })
                .unwrap_or((false, false));
            let st = classify_shortcut(secure, has_manifest);
            applier.apply(move |s| apply_shortcut_status(s, st));
        }
        let snap = INBOX.snapshot();
        // seq 0 は「url 無し」。Unread のままにせず Waiting を一度だけ流す。
        if state.seq() != Some(snap.seq) {
            applier.apply(move |s| {
                apply_snapshot(
                    s,
                    snap.seq,
                    snap.last.map(|r| (r.url, r.received_at_unix_ms)),
                    snap.rejected,
                );
            });
        }
        if let Some(req) = take_request(state) {
            // 反映前に effect が再実行されても同じ要求を二重に処理しない（Windows と同じ）。
            applier.apply(mark_in_flight);
            let result = match req.action {
                Action::Register => try_register(),
                // Web に解除 API は無い。届いても成功に畳まない。
                Action::Unregister => Err(ActionFailure::Unsupported),
            };
            applier.apply(move |s| finish_action(s, req.generation, req.action, result));
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::url_scheme_show_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_covers_four_cases() {
        assert_eq!(
            classify_register(false, true, false),
            Err(ActionFailure::Unsupported)
        );
        assert!(matches!(
            classify_register(false, false, false),
            Err(ActionFailure::Failed(_))
        ));
        assert!(matches!(
            classify_register(true, true, true),
            Err(ActionFailure::Failed(_))
        ));
        assert_eq!(
            classify_register(true, true, false),
            Ok(ActionOutcome::AwaitingConfirmation)
        );
    }

    #[test]
    fn query_param_keeps_invalid_utf8_bytes() {
        let v = query_param_bytes("?screen=x&url=web%2Bfandhe%3A%FF", "url").unwrap();
        assert_eq!(v, b"web+fandhe:\xFF");
        assert!(std::str::from_utf8(&v).is_err());
        assert_eq!(query_param_bytes("?a=1", "url"), None);
        assert_eq!(query_param_bytes("?url=a%2", "url").unwrap(), b"a%2");
        assert_eq!(query_param_bytes("?url=a%41", "url").unwrap(), b"aA");
    }

    #[test]
    fn handler_url_has_placeholder_and_screen() {
        let u = handler_url("https://a.example", "/app/");
        assert_eq!(u, "https://a.example/app/?screen=url_scheme_show&url=%s");
    }

    #[test]
    fn classify_shortcut_never_claims_without_prerequisites() {
        assert_eq!(classify_shortcut(false, true), ShortcutStatus::Unsupported);
        assert_eq!(classify_shortcut(true, false), ShortcutStatus::Failed);
        assert_eq!(classify_shortcut(true, true), ShortcutStatus::Declared);
    }

    #[test]
    fn manifest_shortcut_matches_core_constants() {
        use fandhe_core::screens::url_scheme_show::{SHORTCUT_TITLE, WEB_SHORTCUT_URL};
        let m = include_str!("../web/manifest.webmanifest");
        assert!(m.contains(&format!("\"name\": \"{SHORTCUT_TITLE}\"")));
        let url = m
            .lines()
            .find_map(|l| l.trim().strip_prefix("\"url\": \""))
            .and_then(|l| l.strip_suffix('"'))
            .expect("shortcuts.url");
        let q = &url[url.find('?').unwrap()..];
        let raw = query_param_bytes(q, "url").unwrap();
        assert_eq!(raw, WEB_SHORTCUT_URL.as_bytes());
        fandhe_device::url_scheme::validate_url(
            std::str::from_utf8(&raw).unwrap(),
            fandhe_device::url_scheme::WEB_SCHEME,
        )
        .unwrap();
    }
}
