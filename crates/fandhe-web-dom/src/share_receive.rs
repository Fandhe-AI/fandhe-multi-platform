//! 型サンプル「共有の受け取り」の Web 経路（親 #122、Issue #608）。
//!
//! `fandhe_core::screens::share_receive_show`（端末依存ゼロ）の画面を
//! [`super::mount::mount_with_effect`] の effect として駆動する。呼び出し元は `demo.rs` の
//! `mount_share_receive_show_screen`。受信の検査・保持は `fandhe_device::share_receive`
//! （契約の SSOT）をそのまま使う。Web は `fandhe-device` の trait を通らない
//! （`url_scheme.rs` と同じ理由）。
//!
//! - 受け方: manifest の `share_target`（`method: GET`、Service Worker なし）。インストール済み PWA が
//!   共有先に選ばれると、ブラウザが共有内容をクエリ `share_text` / `share_url` に載せて自ページを開く。
//!   読み込み時に 1 回だけクエリを読み、読んだら `history.replaceState` で共有パラメーターを消す
//! - 範囲: テキスト 1 件（URL もテキストとして扱う）。件名・画像・ファイル・複数件は扱わない
//! - `share_target` には機能検出 API が無く、共有先になれるかは判定できない。「使えます」とは表示しない
//!
//! 守る範囲: 1 プロセス内の検査と最新 1 件の保持、嘘の成功の禁止（secure context 外は未対応）。
//! 守らない範囲: Chromium 以外・未インストール PWA・複数タブ・POST やファイル共有、
//! GET の性質で本文が配信サーバーのアクセスログに残りうること。
//!
//! 本文・例外の中身は `console` へ出さない（security.md A09）。

/// manifest の `share_target.params.text` に対応するクエリ名。manifest とテストで一致を確かめる。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const SHARE_TEXT_PARAM: &str = "share_text";

/// manifest の `share_target.params.url` に対応するクエリ名（`url_scheme` の `url` とは別名）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub const SHARE_URL_PARAM: &str = "share_url";

/// `needle` が `hay` の部分列（連続したバイト列）として含まれるか。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn contains_bytes(hay: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || hay.windows(needle.len()).any(|w| w == needle)
}

/// クエリから共有テキストの生バイト列を選ぶ純関数。
/// どちらのキーも無ければ `None`（通常起動。受信として数えない）。値は置換なしでデコードし、
/// 不正 UTF-8 は受け口（`receive_bytes`）に拒否させる。上限もここでは切り詰めない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn shared_payload(search: &str) -> Option<Vec<u8>> {
    let text = crate::url_scheme::query_param_bytes(search, SHARE_TEXT_PARAM);
    let url = crate::url_scheme::query_param_bytes(search, SHARE_URL_PARAM);
    if text.is_none() && url.is_none() {
        return None;
    }
    let text = text.unwrap_or_default();
    let url = url.unwrap_or_default();
    Some(match (text.is_empty(), url.is_empty()) {
        (false, false) if contains_bytes(&text, &url) => text,
        (false, false) => {
            let mut v = text;
            v.push(b'\n');
            v.extend_from_slice(&url);
            v
        }
        (false, true) => text,
        (true, false) => url,
        (true, true) => Vec::new(),
    })
}

/// 共有パラメーターを消した後の URL（パス + 固定の画面指定）。受信値は混ぜない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn cleaned_url(pathname: &str) -> String {
    format!("{pathname}?screen=share_receive_show")
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::Once;

    use fandhe_core::screens::share_receive_show::{
        apply_snapshot, apply_unsupported, set_guide, ShareGuide, ShareReceiveShowState,
    };
    use fandhe_device::share_receive::ShareInbox;
    use wasm_bindgen::JsValue;

    use super::{cleaned_url, shared_payload};
    use crate::mount::Applier;

    static INBOX: ShareInbox = ShareInbox::new();
    static INGEST: Once = Once::new();

    /// クエリを 1 回だけ受け口へ通し、共有パラメーターを履歴から消す。
    /// キーが無い通常起動は受信として数えず、URL も書き換えない。
    fn ingest_query() {
        INGEST.call_once(|| {
            let Some(window) = web_sys::window() else {
                return;
            };
            let Ok(search) = window.location().search() else {
                return;
            };
            let Some(raw) = shared_payload(&search) else {
                return;
            };
            // 受信時刻は表示用で経過時間には使わないため壁時計でよい。
            // 弾いた場合も件数へ反映済みのため結果は捨てる。
            let _ = INBOX.receive_bytes(Some(&raw), js_sys::Date::now() as u64);
            let pathname = window.location().pathname().unwrap_or_default();
            if let Ok(history) = window.history() {
                // 失敗しても表示には影響させない。
                let _ = history.replace_state_with_url(
                    &JsValue::NULL,
                    "",
                    Some(&cleaned_url(&pathname)),
                );
            }
        });
    }

    /// `mount_with_effect` へ渡す effect。
    pub fn share_receive_show_effect(
        state: &ShareReceiveShowState,
        applier: &Applier<ShareReceiveShowState>,
    ) {
        if state.is_unsupported() {
            return;
        }
        // PWA を入れられない環境は共有先になれない。待たせず未対応として確定する。
        let secure = web_sys::window().is_some_and(|w| w.is_secure_context());
        if !secure {
            applier.apply(apply_unsupported);
            return;
        }
        ingest_query();
        let snap = INBOX.snapshot();
        let need_guide = state.guide() == ShareGuide::None;
        let need_snapshot = state.seq() != Some(snap.seq);
        if need_guide || need_snapshot {
            applier.apply(move |s| {
                if need_guide {
                    set_guide(s, ShareGuide::WebPwa);
                }
                if need_snapshot {
                    apply_snapshot(
                        s,
                        snap.seq,
                        snap.last.map(|t| (t.text, t.received_at_unix_ms)),
                        snap.rejected,
                    );
                }
            });
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::share_receive_show_effect;

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::share_receive::{ShareInbox, MAX_TEXT_BYTES};

    #[test]
    fn no_keys_is_none() {
        assert_eq!(shared_payload(""), None);
        assert_eq!(shared_payload("?screen=share_receive_show"), None);
    }

    #[test]
    fn text_or_url_alone() {
        assert_eq!(shared_payload("?share_text=hi").unwrap(), b"hi");
        assert_eq!(
            shared_payload("?share_url=https%3A%2F%2Fa.b").unwrap(),
            b"https://a.b"
        );
    }

    #[test]
    fn both_contained_or_joined() {
        let q = "?share_text=see%20https%3A%2F%2Fa.b&share_url=https%3A%2F%2Fa.b";
        assert_eq!(shared_payload(q).unwrap(), b"see https://a.b");
        let q = "?share_text=hello&share_url=https%3A%2F%2Fa.b";
        assert_eq!(shared_payload(q).unwrap(), b"hello\nhttps://a.b");
    }

    #[test]
    fn empty_text_falls_back_to_url_and_all_empty_is_some_empty() {
        assert_eq!(shared_payload("?share_text=&share_url=x").unwrap(), b"x");
        assert_eq!(
            shared_payload("?share_text=&share_url=").unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn plus_and_percent20_become_space() {
        assert_eq!(shared_payload("?share_text=a+b%20c").unwrap(), b"a b c");
    }

    #[test]
    fn invalid_utf8_and_nul_are_rejected_by_inbox() {
        let inbox = ShareInbox::new();
        let raw = shared_payload("?share_text=%FF").unwrap();
        assert_eq!(raw, vec![0xFF]);
        assert!(inbox.receive_bytes(Some(&raw), 1).is_err());
        let raw = shared_payload("?share_text=a%00b").unwrap();
        assert!(inbox.receive_bytes(Some(&raw), 2).is_err());
        let snap = inbox.snapshot();
        assert_eq!(snap.rejected, 2);
        assert!(snap.last.is_none());
    }

    #[test]
    fn over_limit_is_rejected_after_join() {
        let text = "a".repeat(MAX_TEXT_BYTES - 1);
        let q = format!("?share_text={text}&share_url=zz");
        let raw = shared_payload(&q).unwrap();
        assert_eq!(raw.len(), MAX_TEXT_BYTES + 2);
        let inbox = ShareInbox::new();
        assert!(inbox.receive_bytes(Some(&raw), 1).is_err());
        assert_eq!(inbox.snapshot().rejected, 1);
    }

    #[test]
    fn cleaned_url_has_no_input() {
        assert_eq!(cleaned_url("/app/"), "/app/?screen=share_receive_show");
    }

    #[test]
    fn manifest_declares_get_share_target() {
        let m = include_str!("../web/manifest.webmanifest");
        assert!(m.contains("\"share_target\""));
        assert!(m.contains("\"method\": \"GET\""));
        assert!(m.contains(&format!("\"text\": \"{SHARE_TEXT_PARAM}\"")));
        assert!(m.contains(&format!("\"url\": \"{SHARE_URL_PARAM}\"")));
        let action = m
            .lines()
            .skip_while(|l| !l.contains("\"share_target\""))
            .find_map(|l| l.trim().strip_prefix("\"action\": \""))
            .expect("share_target.action");
        assert!(!action.contains('?'));
    }
}
