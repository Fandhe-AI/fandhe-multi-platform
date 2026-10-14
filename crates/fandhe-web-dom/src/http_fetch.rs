//! 型サンプル「HTTP 取得（GET 限定）」の Web 経路（Issue #985）。
//!
//! `fandhe_core::screens::http_fetch`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、ブラウザの `fetch`
//! （Fetch API）で GET だけを行って結果を書き戻す。
//!
//! Web は `fandhe-device` の `Http` トレイト（`UreqHttp`）を通らない。`UreqHttp` は
//! wasm32 では `ureq` の推移依存（`getrandom`）が解決できず存在しない
//! （`fandhe_device::http` モジュール doc 参照）ため、本モジュールが DOM レンダラ側の
//! 対応実装にあたる。ただし本文上限・タイムアウト・件数上限・JSON 解析
//! （`MAX_BODY_BYTES`/`TIMEOUT`/`MAX_ITEMS`/`parse_items`）は desktop 実装
//! （`fandhe_device::http`）と共有する（二重管理を避ける SSOT 方針、Issue #985）。
//!
//! # 取得先（`FANDHE_ITEMS_URL`、`fandhe-egui-proxy::device::http_fetch::ITEMS_URL` と
//! 同じ env 名・同じ `option_env!` 方式）
//!
//! ビルド時に指定し、未指定なら相対 URL `"items.json"`（同一オリジン、配布物は
//! `scripts/web/build-wasm-dom.sh` が `web/items.json` を同梱する）を使う。実行時の
//! 利用者入力（`?url=` 等）で取得先を変える経路は作らない（任意の URL への取得を許す
//! 経路を避ける、security.md A01）。
//!
//! ビルド時に指定された値も無検証では使わない（`fetch` 自体は `UreqHttp::get` の
//! スキーム/ホスト検証（`fandhe_device::http` 参照）を通らないため）。
//! [`is_allowed_items_url`] で、相対 URL・`https://`・ループバック/私設 IPv4
//! 宛ての `http://` のみへ絞り、それ以外（公開ホスト宛て平文 http 等）はネットワークへ
//! 出る前に拒否する（P1 指摘対応、security.md A02）。
//!
//! # ブラウザ側の失敗の畳み込み（対応表 `docs/device/capabilities/http-get.json` の
//! web 欄と対）
//!
//! 通信失敗・CORS 拒否・2xx 以外・リダイレクト・本文上限超過・解析失敗・期限切れは、
//! いずれも `finish_fetch(Err(...))` へ畳む（嘘の成功を作らない、
//! device-bridge-contracts.md 6 節）。ブラウザは CORS 拒否と通信失敗を区別できない
//! （どちらも `fetch` の reject・`TypeError`）ため、同じ文言で示す。空配列 `[]` の
//! 応答は正しい結果（「0 件取得」）であり、失敗の畳み込みではない。
//!
//! # 期限（`TIMEOUT`・`AbortController`）
//!
//! `fandhe_device::http::TIMEOUT`（15 秒）と同じ値を `AbortController` の期限に使う
//! （デスクトップと Web で期限を揃える、device-bridge-contracts.md 3 節）。`signal` は
//! `fetch` と本文の読み取り（`array_buffer()`）の両方に効く。フェッチが先に終わる
//! 通常経路では `clear_timeout_with_handle` でタイマーを解除して `Closure` を drop する
//! （`mic_record.rs::Session` と同じ「`forget` しない」方針。`Closure::once_into_js` は
//! 発火しない場合に `Closure` を解放しない既知の制約があるため使わない、
//! `tts.rs::SpeakHandlers` の doc 参照）。
//!
//! # 本文上限（`MAX_BODY_BYTES`）
//!
//! `Content-Length` ヘッダがあれば本文を読む前に確認する。ヘッダが無い・信頼できない
//! （chunked 応答等）場合に備え、本文は `array_buffer()` で一括確保せず
//! `ReadableStream`（`response.body()` の reader）でチャンクごとに読み、蓄積量が
//! `MAX_BODY_BYTES` を超えた時点で reader を `cancel` し `AbortController` も
//! `abort` して打ち切る（P1 指摘対応。一括確保後の事後確認だと上限がメモリ枯渇防止
//! として機能しないため）。
//!
//! # 二重起動防止
//!
//! [`super::mount::Applier::apply`] は effect 実行中（`state` が借用中）の同期呼び出しを
//! マイクロタスクへ退避する（`Applier::apply` の doc 参照）。本モジュールは
//! `Requested` を見たらまず `spawn_local` へ丸ごと逃がし、その中で `applier.apply` に
//! より「`Requested` のままか」を確認しつつ `mark_loading` を適用する。`spawn_local` の
//! タスクが実行される時点では元の `render_once` 呼び出しは終わっており `state` の
//! 借用は外れているため、`apply` はその場で同期的に実行され、`Rc<Cell<bool>>` へ
//! 成否を記録できる。これが `true` のときだけ実際の `fetch` を始めるため、同じ
//! `Requested` を複数回の再描画で観測しても二重取得しない（`fandhe-core::screens::http_fetch`
//! の `Requested` → `Loading` 二段階契約、device-bridge-contracts.md 2 節）。
//!
//! # 画面の固定（他 Web 画面と同じ既知の制約）
//!
//! `mount` は `Screen<S>` の再構築に対応しないため、
//! `current_screen(&HttpFetchState::default())`（「取得」ボタン）で固定する。`Failed`
//! 時の「再試行」ボタンは表示されないが、「取得」ボタンの `start_fetch` は `Failed`
//! からも `Requested` へ進むため、`Failed` の表示のまま再度「取得」を押せば再試行できる。
//!
//! 取得先・応答本文は `console` 出力・保存・送信を一切行わない（security.md A09）。

/// 通信失敗・CORS 拒否時の案内（両者をブラウザ側で区別できないため同じ文言、
/// モジュール doc「ブラウザ側の失敗の畳み込み」節参照）。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`location.rs::failure_kind_from_code` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const NETWORK_FAILURE: &str = "通信に失敗しました（ネットワークまたは CORS）";
/// `AbortController` による期限切れの案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const TIMEOUT_MESSAGE: &str = "時間切れ（15 秒）";
/// 本文上限超過の案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const TOO_LARGE: &str = "応答が大きすぎます";

/// 2xx 以外の応答コードを失敗として畳む純関数（ブラウザ API を呼ばないため native からも
/// テストできる、`location.rs::failure_kind_from_code` と同じ理由で `wasm32` gate の
/// 外に置く）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn check_status(status: u16) -> Result<(), String> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}"))
    }
}

/// 本文の長さが上限（[`fandhe_device::http::MAX_BODY_BYTES`]）を超えていないかを
/// 確認する純関数。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn check_len(len: u64) -> Result<(), String> {
    if len > fandhe_device::http::MAX_BODY_BYTES {
        Err(TOO_LARGE.to_string())
    } else {
        Ok(())
    }
}

/// `url` が許可される取得先かを判定する純関数（ブラウザ API を呼ばないため
/// `check_status`/`check_len` と同じく native からもテストできる、`wasm32` gate の
/// 外に置く）。`fandhe_device::http::UreqHttp::get` のスキーム/ホスト検証（`https://`
/// 全般、またはループバック/私設 IPv4 宛ての `http://` のみ）と同じ方針を採る
/// （モジュール doc「取得先」節参照）。相対 URL（スキームを持たない値のうち、
/// プロトコル相対 `//host/...`・`\\host/...` 等ではないもの）は同一オリジンへの
/// 取得として許可する。
///
/// ブラウザの URL パーサは特別スキーム（`http`/`https` 等）ではスラッシュの欠落・
/// バックスラッシュ混在を許容してホストへ辿り着く（`http:/example.com` や
/// `http:\example.com` も `http://example.com` と同じ扱いになる）。厳密な
/// `"http://"`/`"https://"` 前方一致と `"://"` 部分一致だけを見ると、この寛容さの
/// 分だけ検証をすり抜けられる（P1 指摘対応、security.md A02）。[`scheme_of`] で
/// スキームを取り出してから判定することで、スラッシュの個数・種類に依存せず
/// ブラウザと同じ最終的な宛先を検証する。
///
/// 判定前に [`whatwg_preprocess`] で WHATWG URL の前処理（先頭・末尾の空白/制御文字
/// 除去、埋め込みの tab/CR/LF 除去）を再現する。これを省くと、`" http://example.com"`
/// のように先頭に空白を挟んだ値が `scheme_of` では英字始まりでないためスキームなし
/// （相対 URL）と誤判定される一方、ブラウザの `fetch` は前処理後に `http://example.com`
/// として解釈するため、公開ホストへの平文 HTTP がこの検証をすり抜ける
/// （PR #995 codex/cursor 指摘対応、security.md A02）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn is_allowed_items_url(url: &str) -> bool {
    let url = &whatwg_preprocess(url);
    match scheme_of(url) {
        Some((scheme, _)) if scheme.eq_ignore_ascii_case("https") => true,
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => {
            is_local_plain_http_host(extract_host(skip_authority_slashes(rest)))
        }
        // `ftp://` 等の他スキームは拒否する。
        Some(_) => false,
        // スキームを持たない値のうち、先頭 2 文字が `/`・`\` の組み合わせで始まる
        // ものはプロトコル相対参照（ブラウザは現在の文書のスキームを補って任意ホストへ
        // 解決する）とみなし拒否する。
        None => !is_protocol_relative(url),
    }
}

/// WHATWG URL の basic URL parser が入力を解釈する前に必ず行う前処理を再現する。
/// 仕様（<https://url.spec.whatwg.org/#concept-basic-url-parser>）:
/// 先頭・末尾の C0 制御文字（0x00〜0x1F）および space（0x20）を除去し、続けて
/// 文字列中のどこにあっても ASCII tab（0x09）・CR（0x0D）・LF（0x0A）を除去する。
/// `is_allowed_items_url` はこの正規化後の文字列に対して判定するため、判定結果が
/// 実際に `fetch` が解決する宛先と一致する。
fn whatwg_preprocess(url: &str) -> String {
    let is_c0_or_space = |c: char| matches!(c, '\u{0}'..='\u{1f}' | ' ');
    url.trim_matches(is_c0_or_space)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect()
}

/// 先頭が URL スキームとして解釈されるかを判定し、`(スキーム名, コロン以降)` を返す。
/// WHATWG URL の scheme start state 準拠: 英字で始まり、続く文字が英数字・`+`・`-`・`.`
/// の間はスキームの一部として消費し、`:` に達したらスキームが確定する。それより前に
/// 他の文字（`/`・`\` 等）へ当たった場合はスキームを持たない相対参照とみなす。
fn scheme_of(url: &str) -> Option<(&str, &str)> {
    let bytes = url.as_bytes();
    if bytes.first().is_none_or(|b| !b.is_ascii_alphabetic()) {
        return None;
    }
    let end = bytes.iter().position(|&b| b == b':')?;
    let is_scheme_byte = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.');
    if !bytes[..end].iter().all(|&b| is_scheme_byte(b)) {
        return None;
    }
    Some((&url[..end], &url[end + 1..]))
}

/// スキームのコロン直後にある `/`・`\` を任意個読み飛ばし、ホストの開始位置を返す
/// （`http:example.com`・`http:/example.com`・`http:\example.com` のいずれも
/// `http://example.com` と同じホストへ辿り着くブラウザの寛容さに合わせる）。
fn skip_authority_slashes(rest: &str) -> &str {
    rest.trim_start_matches(['/', '\\'])
}

/// スキームを持たない値が、先頭 2 文字とも `/`・`\` のいずれかで始まる
/// プロトコル相対参照かどうか。
fn is_protocol_relative(url: &str) -> bool {
    let mut chars = url.chars();
    matches!(chars.next(), Some('/' | '\\')) && matches!(chars.next(), Some('/' | '\\'))
}

/// スキーム以降のホスト候補文字列からホスト（ユーザー情報・ポート・パスを除く）を
/// 取り出す。`\` もホスト終端とみなす（ブラウザは authority 内でも `\` を `/` と
/// 同様に区切り文字として扱うため）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn extract_host(rest: &str) -> &str {
    let end = rest.find(['/', '\\', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or(authority)
    }
}

/// ループバック、または RFC 1918 私設 IPv4 リテラル宛てかどうか
/// （`fandhe_device::http::is_local_plain_http_host` と同じ判定基準。実機 LAN 検証用の
/// 許可、Issue #660 参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn is_local_plain_http_host(host: &str) -> bool {
    host == "127.0.0.1"
        || host == "::1"
        || host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::Ipv4Addr>()
            .is_ok_and(|addr| addr.is_private())
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::Cell;
    use std::rc::Rc;

    use fandhe_core::screens::http_fetch::{
        finish_fetch, mark_loading, FetchPhase, HttpFetchState,
    };
    use fandhe_device::http::{parse_items, TIMEOUT};
    use js_sys::Uint8Array;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        AbortController, AbortSignal, ReadableStreamDefaultReader, ReadableStreamReadResult,
        RequestCredentials, RequestInit, RequestRedirect, Response, Window,
    };

    use super::{check_len, check_status, is_allowed_items_url, NETWORK_FAILURE, TIMEOUT_MESSAGE};
    use crate::mount::Applier;

    /// 取得先 URL（モジュール doc「取得先」節参照）。実行時の利用者入力で変える経路は
    /// 作らない。
    const ITEMS_URL: &str = match option_env!("FANDHE_ITEMS_URL") {
        Some(url) => url,
        None => "items.json",
    };

    /// 取得先の設定が不正（許可されないスキーム/ホスト）なときの案内。応答本文と違い
    /// ビルド時定数の妥当性の話なので、`FANDHE_ITEMS_URL` の値自体は出さない（A09）。
    const INVALID_ITEMS_URL: &str = "取得先の設定が不正です";

    /// `mount_with_effect` へ渡す effect。`http_fetch` 画面の取得要求を処理する。
    ///
    /// `Requested` 以外では何もしない（`mount` モジュール doc の「effect フック」節の契約）。
    pub fn http_fetch_effect(state: &HttpFetchState, applier: &Applier<HttpFetchState>) {
        if state.phase != FetchPhase::Requested {
            return;
        }

        // `spawn_local` へ丸ごと逃がす理由はモジュール doc「二重起動防止」節参照。
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let started = Rc::new(Cell::new(false));
            {
                let started = started.clone();
                applier.apply(move |s: &mut HttpFetchState| {
                    if s.phase == FetchPhase::Requested {
                        mark_loading(s);
                        started.set(true);
                    }
                });
            }
            if !started.get() {
                // 別のタスクが先に mark_loading 済み（二重起動防止が働いた）。
                return;
            }

            let result = fetch_once().await;
            applier.apply(move |s| finish_fetch(s, result));
        });
    }

    /// 実際の `fetch` 呼び出し 1 回ぶん（期限つき）。成功時は [`parse_items`] を通した
    /// 文字列一覧を返す。
    async fn fetch_once() -> Result<Vec<String>, String> {
        let Some(window) = web_sys::window() else {
            return Err(NETWORK_FAILURE.to_string());
        };

        let controller = AbortController::new().map_err(|_| NETWORK_FAILURE.to_string())?;

        // TIMEOUT 経過で abort する（モジュール doc「期限」節参照）。
        let timeout_closure = {
            let controller = controller.clone();
            Closure::<dyn FnMut()>::new(move || controller.abort())
        };
        // 設定に失敗したら 15 秒 TIMEOUT 契約が効かないまま無応答取得が残りうるため、
        // `do_fetch` を呼ばずここで打ち切る（P1 指摘対応。`.ok()` で握りつぶさない）。
        let timeout_id = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                timeout_closure.as_ref().unchecked_ref(),
                TIMEOUT.as_millis() as i32,
            )
            .map_err(|_| NETWORK_FAILURE.to_string())?;

        let result = do_fetch(&window, &controller).await;

        // フェッチが先に終わった通常経路ではタイマーを解除して `Closure` を drop する
        // （`forget` しない、モジュール doc「期限」節参照）。
        window.clear_timeout_with_handle(timeout_id);
        drop(timeout_closure);

        result
    }

    /// `fetch` の呼び出し・応答検査・本文読み取りを行う（`fetch_once` から呼ぶ、
    /// タイマー管理を分離するための内部関数）。
    async fn do_fetch(
        window: &Window,
        controller: &AbortController,
    ) -> Result<Vec<String>, String> {
        // ネットワークへ出る前にスキーム/ホストを検証する（モジュール doc「取得先」節・
        // P1 指摘対応）。
        if !is_allowed_items_url(ITEMS_URL) {
            return Err(INVALID_ITEMS_URL.to_string());
        }

        let signal = controller.signal();

        let init = RequestInit::new();
        init.set_method("GET");
        // リダイレクトは失敗として扱う（デスクトップの `max_redirects(0)` と揃える、
        // モジュール doc・security.md A02 参照）。
        init.set_redirect(RequestRedirect::Error);
        // ダミー GET に Cookie は要らない（最小権限）。
        init.set_credentials(RequestCredentials::Omit);
        init.set_signal(Some(&signal));

        let response = JsFuture::from(window.fetch_with_str_and_init(ITEMS_URL, &init))
            .await
            .map_err(|_| network_or_timeout(&signal))?;
        let response: Response = response.unchecked_into();

        check_status(response.status())?;

        // Content-Length があれば本文を読む前に確認する（chunked 応答は本文読み取り後の
        // 再確認に委ねる、モジュール doc「本文上限」節参照）。
        if let Ok(Some(header)) = response.headers().get("content-length") {
            if let Ok(len) = header.parse::<u64>() {
                check_len(len)?;
            }
        }

        let bytes = read_body_limited(&response, controller, &signal).await?;

        parse_items(&bytes)
    }

    /// 本文を `ReadableStream` でチャンクごとに読み、蓄積量が `MAX_BODY_BYTES` を
    /// 超えた時点で打ち切る（モジュール doc「本文上限」節参照、P1 指摘対応）。
    /// `response.body()` が無い（本文なし）場合は空を返す。
    async fn read_body_limited(
        response: &Response,
        controller: &AbortController,
        signal: &AbortSignal,
    ) -> Result<Vec<u8>, String> {
        let Some(stream) = response.body() else {
            return Ok(Vec::new());
        };
        let reader: ReadableStreamDefaultReader = stream
            .get_reader()
            .dyn_into()
            .map_err(|_| NETWORK_FAILURE.to_string())?;

        let mut bytes: Vec<u8> = Vec::new();
        loop {
            let read_result = JsFuture::from(reader.read())
                .await
                .map_err(|_| network_or_timeout(signal))?;
            let read_result: ReadableStreamReadResult = read_result.unchecked_into();
            if read_result.get_done().unwrap_or(true) {
                break;
            }
            let chunk = Uint8Array::new(&read_result.get_value());
            // `Content-Length` の無い応答（chunked 等）で単一チャンクが上限を超えることが
            // あるため、コピー前に `chunk.length()` と既読量の合計を確認する。確保
            // （`to_vec`/`extend_from_slice`）してから上限判定すると、超過分を含む
            // バッファを確保してしまいメモリ枯渇防止として機能しない（P1 指摘対応、
            // security.md A05）。
            if let Err(err) = check_len(bytes.len() as u64 + chunk.length() as u64) {
                // 上限超過。reader を解放し `AbortController` も abort して基盤の
                // ネットワーク転送自体を打ち切る（結果は無視してよい。呼び出し元へは
                // 既に確定した `err` を返す）。
                let _ = JsFuture::from(reader.cancel()).await;
                controller.abort();
                return Err(err);
            }
            bytes.extend_from_slice(&chunk.to_vec());
        }
        Ok(bytes)
    }

    /// `fetch`/`array_buffer` の reject を、`abort` 済みなら期限切れ、それ以外は
    /// 通信失敗（モジュール doc「ブラウザ側の失敗の畳み込み」節参照）へ写す。
    fn network_or_timeout(signal: &AbortSignal) -> String {
        if signal.aborted() {
            TIMEOUT_MESSAGE.to_string()
        } else {
            NETWORK_FAILURE.to_string()
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::http_fetch_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_2xx_is_ok() {
        assert!(check_status(200).is_ok());
        assert!(check_status(299).is_ok());
    }

    #[test]
    fn status_outside_2xx_is_failure() {
        assert_eq!(check_status(199), Err("HTTP 199".to_string()));
        assert_eq!(check_status(300), Err("HTTP 300".to_string()));
        assert_eq!(check_status(404), Err("HTTP 404".to_string()));
        assert_eq!(check_status(500), Err("HTTP 500".to_string()));
    }

    #[test]
    fn len_within_limit_is_ok() {
        assert!(check_len(fandhe_device::http::MAX_BODY_BYTES).is_ok());
    }

    #[test]
    fn len_over_limit_is_failure() {
        assert_eq!(
            check_len(fandhe_device::http::MAX_BODY_BYTES + 1),
            Err(TOO_LARGE.to_string())
        );
    }

    #[test]
    fn relative_url_is_allowed() {
        assert!(is_allowed_items_url("items.json"));
    }

    #[test]
    fn https_is_allowed() {
        assert!(is_allowed_items_url("https://example.com/items.json"));
    }

    #[test]
    fn plain_http_to_public_host_is_rejected() {
        // P1 指摘の回帰テスト（security.md A02）。
        assert!(!is_allowed_items_url("http://example.com/items.json"));
    }

    #[test]
    fn plain_http_to_loopback_is_allowed() {
        assert!(is_allowed_items_url("http://127.0.0.1:8080/items.json"));
        assert!(is_allowed_items_url("http://localhost/items.json"));
    }

    #[test]
    fn plain_http_to_private_ipv4_is_allowed() {
        assert!(is_allowed_items_url("http://192.168.1.2/items.json"));
    }

    #[test]
    fn protocol_relative_url_is_rejected() {
        assert!(!is_allowed_items_url("//example.com/items.json"));
    }

    #[test]
    fn other_scheme_is_rejected() {
        assert!(!is_allowed_items_url("ftp://example.com/items.json"));
    }

    #[test]
    fn backslash_protocol_relative_url_is_rejected() {
        // PR #995 codex/cursor P1 指摘の回帰テスト。ブラウザはバックスラッシュを
        // スラッシュと同様に扱うため、`//` と同じ組み合わせをすべて拒否する
        // （security.md A02）。
        assert!(!is_allowed_items_url("\\\\example.com/items.json"));
        assert!(!is_allowed_items_url("/\\example.com/items.json"));
        assert!(!is_allowed_items_url("\\/example.com/items.json"));
    }

    #[test]
    fn scheme_with_missing_or_backslash_slashes_is_resolved_like_a_browser() {
        // PR #995 codex P1 指摘の回帰テスト。スラッシュが 1 個以下・バックスラッシュ
        // 混在でも、ブラウザは特別スキームのホストへ辿り着く（security.md A02）。
        assert!(is_allowed_items_url("https:/example.com/items.json"));
        assert!(is_allowed_items_url("https:example.com/items.json"));
        assert!(!is_allowed_items_url("http:/example.com/items.json"));
        assert!(!is_allowed_items_url("http:\\example.com/items.json"));
        assert!(is_allowed_items_url("http:/127.0.0.1/items.json"));
    }

    #[test]
    fn non_scheme_colon_relative_url_is_allowed() {
        // `:` を含んでいても英字始まり＋スキーム文字集合の規則に反する値
        // （相対パス中のコロン等）は相対 URL のまま扱う。
        assert!(is_allowed_items_url("a/b:c.json"));
    }

    #[test]
    fn leading_whitespace_before_public_http_scheme_is_rejected() {
        // PR #995 codex/cursor 指摘の回帰テスト。ブラウザは先頭の空白/制御文字を
        // 除去してから解釈するため、`scheme_of` が素の文字列を見て「英字始まりでない
        // ＝相対 URL」と誤判定してすり抜ける経路を塞ぐ（security.md A02）。
        assert!(!is_allowed_items_url(" http://example.com/items.json"));
        assert!(!is_allowed_items_url("\thttp://example.com/items.json"));
        assert!(!is_allowed_items_url("\nhttp://example.com/items.json"));
        assert!(!is_allowed_items_url("\u{0}http://example.com/items.json"));
    }

    #[test]
    fn embedded_tab_or_newline_in_public_http_scheme_is_rejected() {
        // ブラウザは文字列中のどこにあっても tab/CR/LF を除去してから解釈するため、
        // スキームの途中に紛れ込ませても検証をすり抜けさせない。
        assert!(!is_allowed_items_url("ht\ttp://example.com/items.json"));
        assert!(!is_allowed_items_url("http:/\n/example.com/items.json"));
        // 先頭のタブが除去されて `http://...` に解決される（cursor[bot] 指摘の例）。
        assert!(!is_allowed_items_url("\thttp://example.com/items.json"));
    }

    #[test]
    fn leading_whitespace_before_https_or_loopback_http_is_still_allowed() {
        // 正規化後に許可対象（https・ループバック http）へ解決される場合は
        // 引き続き許可する（過剰拒否の回帰防止）。
        assert!(is_allowed_items_url("  https://example.com/items.json"));
        assert!(is_allowed_items_url("\thttp://127.0.0.1/items.json"));
    }
}
