//! Phase 11 の拡充ループ（親 #1023）型サンプル「音声認識」の Web 経路（Issue #1027）。
//!
//! `fandhe_core::screens::speech_recognize`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Web Speech API
//! （`SpeechRecognition`/`webkitSpeechRecognition`）で 1 回だけ聞き取って結果を書き戻す。
//!
//! Web は `fandhe-device` の `SpeechRecognizer` トレイトを通らない（`mic_record.rs`/
//! `biometric_auth.rs` と同じ理由。`fandhe_device::speech_recognition` モジュール doc の
//! 「対応不可プラットフォーム」節参照）。そのため本モジュールが DOM レンダラ側の
//! 対応実装にあたる。ただし結果文字列の不変条件（空白のみ → `NoMatch`、16 KiB 超過を
//! 許さない）だけは [`fandhe_device::speech_recognition::outcome_from_transcript`] を
//! 共有する（検証ロジックの SSOT を 1 箇所に保つ、`mic_record.rs::summary_from_bytes` と
//! 同じ方針）。
//!
//! # コンストラクタの取得方法（`SpeechRecognition::new()` を使わない理由）
//!
//! Chrome は接頭辞なしの `SpeechRecognition` を持たず `webkitSpeechRecognition` のみを
//! 公開する。`web_sys::SpeechRecognition::new()` は常に `SpeechRecognition` という
//! グローバル名を呼ぶため Chrome では失敗する。また `JsCast::dyn_into` は実体が
//! 接頭辞付きコンストラクタで作られたオブジェクトに対して `instanceof SpeechRecognition`
//! が偽になり得るため使わない。そのため [`web::construct_recognition`] は
//! `Reflect::get` で `SpeechRecognition`→`webkitSpeechRecognition` の順に実在する関数を
//! 探し、`Reflect::construct` で生成してから `unchecked_into` で型を付ける
//! （security.md A03、境界で実在を検証する）。
//!
//! # 期限は [`fandhe_core::screens::speech_recognize::MAX_LISTEN_MS`] の 1 定数で揃える
//!
//! Rust 側は [`fandhe_core::screens::speech_recognize::tick`] が同定数で
//! `StopRequested` へ進める。OS 側は `setTimeout` を同じ定数で張り、世代が一致していれば
//! `stop()`（`abort()` ではなく結果の確定を求める）を呼ぶ（`device-bridge-contracts.md`
//! 「期限・キャンセル」節）。
//!
//! # `pagehide` でセッションを破棄する
//!
//! タブを離れる・bfcache への退避（`pagehide`）で全セッションを [`web::release`]
//! （`abort()` を含む）してから [`ListenOutcome::Cancelled`] を反映する。`aborted` も
//! 同じ扱いにする（下記の結果の写像表参照）ため、離脱由来の終了を失敗として見せない
//! （`device-bridge-contracts.md` 「三値」節）。bfcache から復帰しても再開しない。
//!
//! # 結果の写像表（`onerror` の `event.error`、SSOT: [`outcome_from_error_code`]）
//!
//! | コード | 結果 |
//! |---|---|
//! | `not-allowed` / `service-not-allowed` | 権限拒否（`DeviceError::PermissionDenied`） |
//! | `no-speech` | [`ListenOutcome::NoMatch`] |
//! | `aborted` | [`ListenOutcome::Cancelled`] |
//! | `language-not-supported` | [`UNAVAILABLE`]（`fandhe_device` 側「言語未対応 →
//!   Unsupported」の表と揃える） |
//! | `network` | `DeviceError::Disabled`（`fandhe_device` 側「オフライン → Disabled」の
//!   表と揃える） |
//! | `audio-capture` / `bad-grammar` / 未知・`None` | `DeviceError::Internal` |
//!
//! `onresult` が結果を 0 件返した場合・`onnomatch`・先に確定せず `onend` に到達した場合は
//! すべて [`ListenOutcome::NoMatch`] に畳む。
//!
//! # Web 固有の天井（`mic_record.rs` との差分）
//!
//! `start()` は同期 API で、`getUserMedia` のような権限待ち `Promise` を介さない
//! （マイク権限の確認自体はブラウザが `start()` 呼び出し後に内部で行う）。そのため
//! `mic_record.rs::Session::cancel_requested` に相当するフラグは持たない。
//! `mount_with_effect` は状態から `Screen` を再構築しないため（`mic_record.rs`
//! モジュール doc「既知の天井」節と同型）、Web の検証エントリポイントには「停止」
//! ボタンが出ない。`continuous = false` によりブラウザが発話の終わりで自動終了するため、
//! 正常系はこれで観測する。
//!
//! # 秘密値・ログ（security.md A09）
//!
//! 認識結果の文字列・例外オブジェクトの `message` を `console` へ出力・保存・送信しない
//! （`onerror` は `error`（コード）のみを読む）。
//!
//! # Chrome の音声送信について
//!
//! Chrome の実装は音声をオンライン認識サービス（Google のサーバー）へ送信してから
//! 結果を返す（ブラウザ実装の仕様。対応表の note にも明記する）。独自の送信経路は
//! 追加しない。

use fandhe_core::screens::speech_recognize::ListenOutcome;
use fandhe_device::DeviceError;

/// 非対応ブラウザ・secure context 外・コンストラクタ未定義時の案内
/// （`fandhe-egui-proxy::device::speech_recognition::UNAVAILABLE` と同じ文言で揃える）。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`mic_record.rs::UNAVAILABLE` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNAVAILABLE: &str = "この環境では音声認識を使えません";

/// `onerror`/内部失敗時の汎用案内。`DeviceError::Internal` の `Display` を共有し、
/// 内部識別子（`"speech_recognition"`）は画面へ出さず固定文言に畳む
/// （`fandhe-egui-proxy::device::speech_recognition::to_message` と同じ方針を
/// Web 側の `DeviceError` 由来の値にも適用する）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn internal_message() -> String {
    DeviceError::Internal("speech_recognition").to_string()
}

/// `event.error`（`onerror` のエラーコード文字列）→ [`ListenOutcome`] への変換純関数
/// （ブラウザ API を呼ばないため native からもテストできる、モジュール doc の表参照）。
/// `message` は受け取らない・転送しない（security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_error_code(code: Option<&str>) -> Result<ListenOutcome, String> {
    match code {
        Some("not-allowed") | Some("service-not-allowed") => {
            Err(DeviceError::PermissionDenied("speech_recognition").to_string())
        }
        Some("no-speech") => Ok(ListenOutcome::NoMatch),
        Some("aborted") => Ok(ListenOutcome::Cancelled),
        Some("language-not-supported") => Err(UNAVAILABLE.to_string()),
        Some("network") => Err(DeviceError::Disabled("speech_recognition").to_string()),
        _ => Err(internal_message()),
    }
}

/// `SpeechRecognitionAlternative::transcript()` の文字列 → [`ListenOutcome`] への変換。
/// 不変条件の検証は [`fandhe_device::speech_recognition::outcome_from_transcript`] の
/// SSOT に一本化し、`DeviceError` の内部識別子は画面へ出さず [`internal_message`] の
/// 固定文言へ畳む（`mic_record.rs::summary_from_bytes` と同じ方針）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn outcome_from_transcript_text(text: String) -> Result<ListenOutcome, String> {
    use fandhe_device::speech_recognition::SpeechOutcome;
    match fandhe_device::speech_recognition::outcome_from_transcript(text) {
        Ok(SpeechOutcome::Recognized(t)) => Ok(ListenOutcome::Recognized(t)),
        Ok(SpeechOutcome::NoMatch) => Ok(ListenOutcome::NoMatch),
        Ok(SpeechOutcome::Cancelled) => Ok(ListenOutcome::Cancelled),
        Err(_) => Err(internal_message()),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use fandhe_core::screens::speech_recognize::{
        finish_listen, mark_listening, mark_stopping, take_start_request, take_stop_request, tick,
        SpeechRecognizeState, MAX_LISTEN_MS,
    };
    use js_sys::{Array, Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{
        SpeechRecognition, SpeechRecognitionEvent, SpeechRecognitionResult,
        SpeechRecognitionResultList,
    };

    use super::{outcome_from_error_code, outcome_from_transcript_text, UNAVAILABLE};
    use crate::mount::Applier;

    /// 1 回の聞き取りセッションぶんの状態。すべての終了経路（成功・失敗・キャンセル・
    /// `pagehide`）が [`release`] を通して解放する（`mic_record.rs::Session` と同型）。
    struct Session {
        /// `pagehide` からこのセッションを破棄するときに使う。
        applier: Applier<SpeechRecognizeState>,
        /// [`finish_listen`] へ照合する世代番号（古い世代の遅延コールバックを捨てる）。
        generation: u64,
        recognition: Option<SpeechRecognition>,
        timeout_id: Option<i32>,
        interval_id: Option<i32>,
        /// `onresult`/`onerror`/`onnomatch`/`onend` の `Closure`。`forget()` すると
        /// 聞き取りのたびに Wasm ヒープへ永久リークするため、`release` での通常の drop に
        /// 委ねる（`mic_record.rs` の PR #963 指摘と同じ回避策）。
        #[allow(dead_code)]
        closures: Vec<Closure<dyn FnMut(JsValue)>>,
        /// `MAX_LISTEN_MS` 自動停止用タイマーの `Closure`。
        #[allow(dead_code)]
        timeout_closure: Option<Closure<dyn FnMut()>>,
        /// 経過秒数反映タイマーの `Closure`。
        #[allow(dead_code)]
        interval_closure: Option<Closure<dyn FnMut()>>,
        /// 経過秒数の実測に使う開始時刻（`Date::now()`）。
        started_at_ms: f64,
    }

    thread_local! {
        /// マウント ID ごとの聞き取りセッション（`mic_record.rs::ACTIVE` と同じく
        /// マウント単位で持つ）。
        static ACTIVE: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());
    }

    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        /// `pagehide` 購読を（未登録なら）1 度だけ登録するためのハンドル
        /// （`mic_record.rs::PAGEHIDE` と同型）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する（`share.rs::
    /// has_function` と同型。型に現れない JS 側の実態を境界で検証する、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `SpeechRecognition`→`webkitSpeechRecognition` の順に実在するコンストラクタを
    /// 探して構築する（モジュール doc「コンストラクタの取得方法」節参照）。
    fn construct_recognition() -> Option<SpeechRecognition> {
        let global: JsValue = js_sys::global().into();
        let name = if has_function(&global, "SpeechRecognition") {
            "SpeechRecognition"
        } else if has_function(&global, "webkitSpeechRecognition") {
            "webkitSpeechRecognition"
        } else {
            return None;
        };
        let ctor: Function = Reflect::get(&global, &JsValue::from_str(name))
            .ok()?
            .dyn_into()
            .ok()?;
        let instance = Reflect::construct(&ctor, &Array::new()).ok()?;
        Some(instance.unchecked_into::<SpeechRecognition>())
    }

    /// `mount_id` のセッションを解放する（タイマー解除 → ハンドラを外す → `abort()` →
    /// `ACTIVE` から除去）。全終了経路（成功・失敗・キャンセル・`pagehide`）がここを通る
    /// （モジュール doc 参照）。ハンドラを外してから `abort()` するため、`abort()` 由来の
    /// 遅延イベントは届かない（`device-bridge-contracts.md` 「ライフサイクル境界」節）。
    fn release(mount_id: u64) {
        let Some(session) = ACTIVE.with(|active| active.borrow_mut().remove(&mount_id)) else {
            return;
        };
        if let Some(w) = window() {
            if let Some(id) = session.timeout_id {
                w.clear_timeout_with_handle(id);
            }
            if let Some(id) = session.interval_id {
                w.clear_interval_with_handle(id);
            }
        }
        if let Some(recognition) = &session.recognition {
            recognition.set_onresult(None);
            recognition.set_onerror(None);
            recognition.set_onnomatch(None);
            recognition.set_onend(None);
            recognition.abort();
        }
    }

    /// `mount_id`・`generation` が現在のセッションと一致するかを確認する
    /// （`mic_record.rs::matches_current` と同じ「古い世代の完了を照合して捨てる」判定）。
    fn matches_current(mount_id: u64, generation: u64) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .is_some_and(|s| s.generation == generation)
        })
    }

    /// 要求を失敗として畳み、セッションを解放して [`Applier::apply`] へ渡す
    /// （`mic_record.rs::fail` と同型）。再照合してから呼ぶのは呼び出し元の責務。
    fn fail(
        applier: &Applier<SpeechRecognizeState>,
        mount_id: u64,
        generation: u64,
        message: String,
    ) {
        release(mount_id);
        applier.apply(move |s| finish_listen(s, generation, Err(message)));
    }

    /// `generation` が現在のセッションと一致するときのみ結果を反映して解放する
    /// （`matches_current` の再照合込み。各ハンドラの反映直前の共通処理）。
    fn finish_if_current(
        applier: &Applier<SpeechRecognizeState>,
        mount_id: u64,
        generation: u64,
        result: Result<fandhe_core::screens::speech_recognize::ListenOutcome, String>,
    ) {
        if !matches_current(mount_id, generation) {
            // 古い世代（既に `stop`/`pagehide` で解放済み、または他ハンドラが
            // 先に確定済み）の遅延コールバック。
            return;
        }
        release(mount_id);
        applier.apply(move |s| finish_listen(s, generation, result));
    }

    /// 聞き取り中かどうかを `MAX_LISTEN_MS` のタイムアウト・1 秒間隔の経過秒数反映の
    /// 両方から照合するため、世代一致時のみ `stop()` を呼ぶ。
    fn stop_recognition(mount_id: u64) {
        let recognition = ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .and_then(|s| s.recognition.clone())
        });
        if let Some(recognition) = recognition {
            recognition.stop();
        }
    }

    /// `onresult`（結果確定）の継続。`Closure` を自分自身の中で drop しないよう
    /// `spawn_local` へ逃がす（`mic_record.rs::handle_stop` と同じ回避策）。
    fn handle_result(
        mount_id: u64,
        generation: u64,
        applier: Applier<SpeechRecognizeState>,
        event: JsValue,
    ) {
        wasm_bindgen_futures::spawn_local(async move {
            // SAFETY ではなく契約の注記: `onresult` ハンドラとしてしか呼ばれないため
            // ブラウザが渡す実体は常に結果イベントだが、接頭辞付き API では
            // `webkitSpeechRecognitionEvent` のようにクラス名が標準と異なり
            // `dyn_into`（`instanceof SpeechRecognitionEvent` 判定）が落ちて
            // 認識結果を常に `NoMatch` へ畳んでしまう（`construct_recognition` が
            // コンストラクタ名を可変にしているのと同じ理由）。`unchecked_into` で
            // プロパティ読み取りのみ行う。
            let event: SpeechRecognitionEvent = event.unchecked_into();
            let result_index = event.result_index();
            let transcript: Option<String> = event
                .results()
                .and_then(|list: SpeechRecognitionResultList| list.get(result_index))
                .and_then(|result: SpeechRecognitionResult| result.get(0))
                .map(|alt| alt.transcript());
            let result = match transcript {
                Some(text) => outcome_from_transcript_text(text),
                None => Ok(super::ListenOutcome::NoMatch),
            };
            finish_if_current(&applier, mount_id, generation, result);
        });
    }

    /// `onerror`（認識失敗）の継続。エラーオブジェクトの `message` は読まない
    /// （モジュール doc「秘密値・ログ」節）。`web-sys` 0.3.103 には実ブラウザが発火する
    /// `SpeechRecognitionErrorEvent` 用の型が無いため、`Reflect::get` で `error`
    /// フィールドのみを読む。
    fn handle_error(
        mount_id: u64,
        generation: u64,
        applier: Applier<SpeechRecognizeState>,
        event: JsValue,
    ) {
        wasm_bindgen_futures::spawn_local(async move {
            let code = Reflect::get(&event, &JsValue::from_str("error"))
                .ok()
                .and_then(|v| v.as_string());
            let result = outcome_from_error_code(code.as_deref());
            finish_if_current(&applier, mount_id, generation, result);
        });
    }

    /// `onnomatch`（結果 0 件）の継続。
    fn handle_nomatch(mount_id: u64, generation: u64, applier: Applier<SpeechRecognizeState>) {
        wasm_bindgen_futures::spawn_local(async move {
            finish_if_current(
                &applier,
                mount_id,
                generation,
                Ok(super::ListenOutcome::NoMatch),
            );
        });
    }

    /// `onend`（結果も失敗も確定せず終わった場合の最終フォールバック）の継続。
    /// `onresult`/`onerror`/`onnomatch` が先に `release` 済みなら `matches_current` が
    /// 偽になり no-op（モジュール doc 参照）。
    fn handle_end(mount_id: u64, generation: u64, applier: Applier<SpeechRecognizeState>) {
        wasm_bindgen_futures::spawn_local(async move {
            finish_if_current(
                &applier,
                mount_id,
                generation,
                Ok(super::ListenOutcome::NoMatch),
            );
        });
    }

    /// 全マウントの聞き取りセッションを破棄する。`pagehide`（タブを離れる・bfcache への
    /// 退避）から呼ばれる契約（`mic_record.rs::discard_all_sessions` と同型）。`aborted`
    /// と同じ [`ListenOutcome::Cancelled`] 扱いにする（モジュール doc「`pagehide` で
    /// セッションを破棄する」節）。bfcache から復帰しても再開しない。
    fn discard_all_sessions() {
        let entries: Vec<(u64, u64, Applier<SpeechRecognizeState>)> = ACTIVE.with(|active| {
            active
                .borrow()
                .iter()
                .map(|(mount_id, session)| (*mount_id, session.generation, session.applier.clone()))
                .collect()
        });
        for (mount_id, generation, applier) in entries {
            release(mount_id);
            applier
                .apply(move |s| finish_listen(s, generation, Ok(super::ListenOutcome::Cancelled)));
        }
    }

    /// `pagehide` 購読を（未登録なら）1 度だけ登録する（`mic_record.rs::
    /// register_pagehide_once` と同型）。戻り値は購読が有効か。呼び出し側はこれが
    /// `false` のときは fail-closed で聞き取りを始めない。
    fn register_pagehide_once() -> bool {
        PAGEHIDE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(w) = window() else {
                return false;
            };
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    discard_all_sessions();
                });
            if w.add_event_listener_with_callback("pagehide", closure.as_ref().unchecked_ref())
                .is_err()
            {
                return false;
            }
            *cell.borrow_mut() = Some(closure);
            true
        })
    }

    /// `mount_with_effect` へ渡す effect。`speech_recognize` 画面の開始・停止要求を
    /// 処理する。要求が無ければ何もしない（`mount.rs` モジュール doc の「effect フック」
    /// 節の契約）。
    pub fn speech_recognize_effect(
        state: &SpeechRecognizeState,
        applier: &Applier<SpeechRecognizeState>,
    ) {
        let mount_id = applier.mount_id();

        if let Some(request) = take_start_request(state) {
            // 既にセッションがあれば二重開始しない（`mic_record.rs::ACTIVE` と同じ判定）。
            if ACTIVE.with(|active| active.borrow().contains_key(&mount_id)) {
                return;
            }
            applier.apply(mark_listening);
            let generation = request.generation;

            // 先に仮登録してから前提検査に入る（`apply` が起こす再描画で effect が
            // 再入しても「既にエントリがある」判定で no-op になるようにする、
            // `mic_record.rs::ACTIVE` 登録と同じ理由）。
            ACTIVE.with(|active| {
                active.borrow_mut().insert(
                    mount_id,
                    Session {
                        applier: applier.clone(),
                        generation,
                        recognition: None,
                        timeout_id: None,
                        interval_id: None,
                        closures: Vec::new(),
                        timeout_closure: None,
                        interval_closure: None,
                        started_at_ms: 0.0,
                    },
                );
            });

            // `pagehide` の購読は実際に聞き取りを始める前に登録する契約
            // （`mic_record.rs::register_pagehide_once` の呼び出し位置と同じ理由）。
            // 登録できなければ離脱後の聞き取りを防げないため fail-closed で始めない。
            if !register_pagehide_once() {
                fail(applier, mount_id, generation, UNAVAILABLE.to_string());
                return;
            }

            let Some(w) = window() else {
                fail(applier, mount_id, generation, UNAVAILABLE.to_string());
                return;
            };
            if !w.is_secure_context() {
                fail(applier, mount_id, generation, UNAVAILABLE.to_string());
                return;
            }
            let Some(recognition) = construct_recognition() else {
                fail(applier, mount_id, generation, UNAVAILABLE.to_string());
                return;
            };

            if recognition.set_continuous(false).is_err() {
                fail(applier, mount_id, generation, super::internal_message());
                return;
            }
            recognition.set_interim_results(false);
            recognition.set_lang("ja-JP");

            let applier_result = applier.clone();
            let on_result = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                handle_result(mount_id, generation, applier_result.clone(), event);
            });
            recognition.set_onresult(Some(on_result.as_ref().unchecked_ref()));

            let applier_error = applier.clone();
            let on_error = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                handle_error(mount_id, generation, applier_error.clone(), event);
            });
            recognition.set_onerror(Some(on_error.as_ref().unchecked_ref()));

            let applier_nomatch = applier.clone();
            let on_nomatch = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
                handle_nomatch(mount_id, generation, applier_nomatch.clone());
            });
            recognition.set_onnomatch(Some(on_nomatch.as_ref().unchecked_ref()));

            let applier_end = applier.clone();
            let on_end = Closure::<dyn FnMut(JsValue)>::new(move |_event: JsValue| {
                handle_end(mount_id, generation, applier_end.clone());
            });
            recognition.set_onend(Some(on_end.as_ref().unchecked_ref()));

            if recognition.start().is_err() {
                // `set()` 済みのハンドラを明示的に外す（`mic_record.rs::start_recorder`
                // の `recorder.start()` 失敗時の後始末と同じ理由。構築した `recognition`
                // はこのローカル変数のまま drop するため `release` 任せにできない）。
                recognition.set_onresult(None);
                recognition.set_onerror(None);
                recognition.set_onnomatch(None);
                recognition.set_onend(None);
                fail(applier, mount_id, generation, super::internal_message());
                return;
            }

            let started_at_ms = js_sys::Date::now();

            // `MAX_LISTEN_MS` の自動停止（OS 側）。Rust 側の `tick` 自動停止と同じ定数
            // （モジュール doc「期限は 1 定数で揃える」節）。`abort()` ではなく `stop()`
            // で結果の確定を求める（`fandhe_device::speech_recognition` モジュール doc
            // 「上限」節と同じ判断）。
            let timeout_closure = Closure::<dyn FnMut()>::new(move || {
                if matches_current(mount_id, generation) {
                    stop_recognition(mount_id);
                }
            });
            let timeout_id = window().and_then(|w| {
                w.set_timeout_with_callback_and_timeout_and_arguments_0(
                    timeout_closure.as_ref().unchecked_ref(),
                    MAX_LISTEN_MS as i32,
                )
                .ok()
            });
            // `MAX_LISTEN_MS` の上限を保証するタイマーが登録できなければ、聞き取りを
            // 開始状態のまま進めない（device-bridge-contracts.md「期限・キャンセル」節の
            // fail-closed 分岐）。`setInterval`（経過秒数反映）の失敗は上限保証に関与
            // しないため許容するが、`setTimeout` の失敗だけは即座に中止する。
            // `set()` 済みのハンドラを明示的に外してから中止するのは、上の
            // `recognition.start().is_err()` 分岐と同じ理由（構築した `recognition` は
            // このローカル変数のまま drop するため `release` 任せにできない）。
            let Some(timeout_id) = timeout_id else {
                recognition.set_onresult(None);
                recognition.set_onerror(None);
                recognition.set_onnomatch(None);
                recognition.set_onend(None);
                recognition.abort();
                fail(applier, mount_id, generation, super::internal_message());
                return;
            };

            // 経過秒数の 1 秒間隔の反映（`mic_record.rs`「`setInterval` を使う理由」節と
            // 同じ判断）。
            let applier_tick = applier.clone();
            let interval_closure = Closure::<dyn FnMut()>::new(move || {
                if !matches_current(mount_id, generation) {
                    return;
                }
                let elapsed = (js_sys::Date::now() - started_at_ms).max(0.0) as u64;
                applier_tick.apply(move |s| {
                    let dt = elapsed.saturating_sub(s.elapsed_ms);
                    tick(s, dt);
                });
            });
            let interval_id = window().and_then(|w| {
                w.set_interval_with_callback_and_timeout_and_arguments_0(
                    interval_closure.as_ref().unchecked_ref(),
                    1_000,
                )
                .ok()
            });

            ACTIVE.with(|active| {
                if let Some(session) = active.borrow_mut().get_mut(&mount_id) {
                    session.recognition = Some(recognition);
                    session.timeout_id = Some(timeout_id);
                    session.interval_id = interval_id;
                    session.closures = vec![on_result, on_error, on_nomatch, on_end];
                    session.timeout_closure = Some(timeout_closure);
                    session.interval_closure = Some(interval_closure);
                    session.started_at_ms = started_at_ms;
                }
            });
            return;
        }

        if let Some(request) = take_stop_request(state) {
            applier.apply(mark_stopping);
            let matches = ACTIVE.with(|active| {
                active
                    .borrow()
                    .get(&mount_id)
                    .is_some_and(|s| s.generation == request.generation)
            });
            if matches {
                stop_recognition(mount_id);
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::speech_recognize_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_allowed_errors_map_to_permission_denied() {
        assert_eq!(
            outcome_from_error_code(Some("not-allowed")),
            Err(DeviceError::PermissionDenied("speech_recognition").to_string())
        );
        assert_eq!(
            outcome_from_error_code(Some("service-not-allowed")),
            Err(DeviceError::PermissionDenied("speech_recognition").to_string())
        );
    }

    #[test]
    fn no_speech_maps_to_no_match() {
        assert_eq!(
            outcome_from_error_code(Some("no-speech")),
            Ok(ListenOutcome::NoMatch)
        );
    }

    #[test]
    fn aborted_maps_to_cancelled() {
        assert_eq!(
            outcome_from_error_code(Some("aborted")),
            Ok(ListenOutcome::Cancelled)
        );
    }

    #[test]
    fn language_not_supported_maps_to_unavailable() {
        assert_eq!(
            outcome_from_error_code(Some("language-not-supported")),
            Err(UNAVAILABLE.to_string())
        );
    }

    #[test]
    fn network_maps_to_disabled() {
        assert_eq!(
            outcome_from_error_code(Some("network")),
            Err(DeviceError::Disabled("speech_recognition").to_string())
        );
    }

    #[test]
    fn unknown_or_missing_code_maps_to_internal() {
        assert_eq!(
            outcome_from_error_code(Some("audio-capture")),
            Err(internal_message())
        );
        assert_eq!(
            outcome_from_error_code(Some("bad-grammar")),
            Err(internal_message())
        );
        assert_eq!(
            outcome_from_error_code(Some("something-unknown")),
            Err(internal_message())
        );
        assert_eq!(outcome_from_error_code(None), Err(internal_message()));
    }

    #[test]
    fn transcript_text_recognizes_non_blank_text() {
        assert_eq!(
            outcome_from_transcript_text("こんにちは".to_string()),
            Ok(ListenOutcome::Recognized("こんにちは".to_string()))
        );
    }

    #[test]
    fn transcript_text_does_not_trim() {
        // 入力どおりに保存する（trim で前後の空白を意図せず除去しない、
        // device-bridge-contracts.md「秘密値」節と同じ方針）。
        assert_eq!(
            outcome_from_transcript_text(" こんにちは ".to_string()),
            Ok(ListenOutcome::Recognized(" こんにちは ".to_string()))
        );
    }

    #[test]
    fn transcript_text_empty_or_whitespace_is_no_match() {
        assert_eq!(
            outcome_from_transcript_text(String::new()),
            Ok(ListenOutcome::NoMatch)
        );
        assert_eq!(
            outcome_from_transcript_text("   \n\t".to_string()),
            Ok(ListenOutcome::NoMatch)
        );
    }

    #[test]
    fn transcript_text_rejects_oversized_text() {
        let text = "あ".repeat(fandhe_device::speech_recognition::MAX_TRANSCRIPT_BYTES + 1);
        assert_eq!(outcome_from_transcript_text(text), Err(internal_message()));
    }
}
