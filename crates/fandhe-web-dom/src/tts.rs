//! Phase 11（親 #864）型サンプル「読み上げ」の Web 経路（Issue #867、`wasm32` 専用）。
//!
//! `fandhe_core::screens::tts_speak`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、
//! `window.speechSynthesis` / `SpeechSynthesisUtterance`（Web Speech API）で
//! 発話・停止して結果を書き戻す。
//!
//! Web は `fandhe-device` の `Tts` トレイトを通らない（`clipboard.rs` と同じ理由。
//! `fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計、
//! `crates/fandhe-egui-proxy/src/device.rs` 参照）。そのため本モジュールが
//! DOM レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約
//!
//! - **Safari はユーザー操作起点が必須**: `speak()` はボタンの `click` ハンドラから
//!   始まる同期チェーンの中で呼ぶ必要がある。本モジュールの `speak()` 呼び出しは
//!   click → 状態更新 → 再描画 → effect という同期チェーンの中で行われるため要件を満たす
//!   （非同期処理を挟まない。`clipboard.rs` の Promise 経由とはこの点が異なる）。
//! - **非対応ブラウザは `Err`**: `speechSynthesis` が存在しない環境で黙って成功扱いに
//!   しない（親 #864 の再発防止点、`tts_speak` モジュール doc 参照）。
//! - **`IN_FLIGHT` 相当の二重実行防止は不要**: `fandhe_core::screens::tts_speak` は
//!   `speak_dispatched` / `stop_dispatched` フラグを状態側に持つ（`clipboard_text` には
//!   無い）。`applier.apply(mark_speak_dispatched)` を送った時点で次の再描画からは
//!   `take_speak_request` が `None` を返すため、`clipboard.rs` の `IN_FLIGHT` の
//!   ようなマウント単位の飛行中集合を Web 側に別途持つ必要がない。
//! - **`cancel()` はページ全体の発話を止める**: マウント単位の分離はない。本画面を
//!   同一ページに複数マウントする用途は無いため受容する。
//!
//! 発話テキストや例外の詳細（`message`）は `console` 出力・送信を一切行わない
//! （security.md A09。`clipboard.rs::to_message` と同じ方針）。

use std::cell::RefCell;

use fandhe_core::screens::tts_speak::{
    finish_speak, finish_stop, mark_speak_dispatched, mark_stop_dispatched, take_speak_request,
    take_stop_request, TtsSpeakState,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use super::mount::Applier;

/// 発話中の `Utterance` とそのイベントハンドラ 2 本をまとめて保持する。
///
/// `speak()` 呼び出し後にこれを解放してしまうと、Chrome 系ブラウザでは
/// `Utterance` が GC されて `onend`/`onerror` が発火しないことがある
/// （Bugbot 指摘）。加えて `Closure::once_into_js` は発火しなかった側の
/// `FnOnce` を解放しないため、`onend`/`onerror` どちらが発火しても
/// 未発火側を含めてここで明示的に解放する（codex-review 指摘）。
struct SpeakHandlers {
    // 発火はしないが、Drop されるまで JS 側から呼び出し可能な状態を保つために
    // 保持するだけのフィールド（未読で許容する）。
    #[allow(dead_code)]
    on_end: Closure<dyn FnMut(web_sys::Event)>,
    #[allow(dead_code)]
    on_error: Closure<dyn FnMut(web_sys::SpeechSynthesisErrorEvent)>,
    utterance: web_sys::SpeechSynthesisUtterance,
}

// ページ内で発話は同時に 1 本のみ（モジュール doc の「cancel() はページ全体の
// 発話を止める」前提と対応）。stop 経路・onend・onerror の 3 経路すべてが
// ここを共有先として参照できるよう、Rc サイクルではなく単一の thread_local に置く
// （wasm32 はシングルスレッドなので Send 不要）。Rc サイクルだったときは
// stop 分岐がこの箱へ到達する手段を持たず、cancel() が onend/onerror を
// 発火させない実装（Safari/Chrome 既知）で speak→stop のたびにリークしていた
// （Bugbot 指摘）。
thread_local! {
    static ACTIVE_HANDLERS: RefCell<Option<SpeakHandlers>> = const { RefCell::new(None) };
}

/// 発話終了・エラー・停止のいずれの経路でもここを通し、
/// ハンドラを外してから `SpeakHandlers` を破棄する（クロージャを解放する）。
fn release_speak_handlers() {
    ACTIVE_HANDLERS.with(|cell| {
        if let Some(handlers) = cell.borrow_mut().take() {
            handlers.utterance.set_onend(None);
            handlers.utterance.set_onerror(None);
        }
    });
}

/// 非対応ブラウザ・取得失敗時の案内（画面の `error` へ出る）。
const UNAVAILABLE: &str = "このブラウザでは読み上げを使えません（Web Speech API 非対応）";

/// `window.speechSynthesis` を取得する。非対応ブラウザでは getter 自体が無いか
/// `undefined` を返す。`clipboard()` と同じく、型に現れない JS 側の実態を
/// 境界で検証する（security.md A03 の方針）。
fn synthesis() -> Option<web_sys::SpeechSynthesis> {
    let synth = web_sys::window()?.speech_synthesis().ok()?;
    if JsValue::from(synth.clone()).is_undefined() {
        return None;
    }
    Some(synth)
}

/// `SpeechSynthesisErrorEvent::error()` を固定文言へ畳む。
///
/// エラーコードの詳細な出し分けは要件外（`tts_speak` 画面は `error` に文字列を
/// 1 本表示するのみ）で、例外オブジェクトの生 message は転送しない。
fn error_message(code: web_sys::SpeechSynthesisErrorCode) -> String {
    use web_sys::SpeechSynthesisErrorCode as E;
    match code {
        E::Canceled | E::Interrupted => "発話が中断されました".to_string(),
        E::AudioBusy | E::AudioHardware => "音声出力デバイスを使用できません".to_string(),
        E::SynthesisUnavailable | E::SynthesisFailed => {
            "この環境では読み上げを使えません".to_string()
        }
        _ => "読み上げに失敗しました".to_string(),
    }
}

/// `mount_with_effect` へ渡す effect。`tts_speak` 画面の発話・停止要求を処理する。
///
/// 停止 → 発話の順で見る（`fandhe-egui-proxy` 側 effect ドライバと同じ順序。
/// 同一フレームで両方が立つことは `tts_speak` の状態遷移上ないが、順序を固定して
/// おくことで将来の変更に対しても意味が変わらないようにする）。
pub fn tts_speak_effect(state: &TtsSpeakState, applier: &Applier<TtsSpeakState>) {
    if take_stop_request(state) {
        applier.apply(mark_stop_dispatched);
        let result = match synthesis() {
            Some(synth) => {
                synth.cancel();
                // cancel() が onend/onerror を発火させない環境（Safari/Chrome
                // 既知）でも、ここで明示的に解放して utterance と両 Closure の
                // リークを防ぐ（Bugbot 指摘）。onend/onerror 側が既に解放済みの
                // 場合は take() が None を返すだけで無害。
                release_speak_handlers();
                Ok(())
            }
            None => Err(UNAVAILABLE.to_string()),
        };
        applier.apply(move |s| finish_stop(s, result));
    }

    if let Some((generation, text)) = take_speak_request(state) {
        applier.apply(mark_speak_dispatched);

        let Some(synth) = synthesis() else {
            applier.apply(move |s| finish_speak(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        };
        let Ok(utterance) = web_sys::SpeechSynthesisUtterance::new_with_text(&text) else {
            applier.apply(move |s| finish_speak(s, generation, Err(UNAVAILABLE.to_string())));
            return;
        };

        // `Utterance` と両ハンドラは ACTIVE_HANDLERS（モジュール共有の thread_local）
        // へ集約する。onend/onerror/stop のどの経路が先に来ても
        // release_speak_handlers 経由で同じ箱を解放するため、他経路のクロージャが
        // リークしない（モジュール先頭のコメント参照）。
        let on_end = {
            let applier = applier.clone();
            Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                // release_speak_handlers はこのクロージャ自身が属する `SpeakHandlers`
                // （＝実行中の Closure 自体）を drop する（Bugbot 指摘）。drop 後に
                // このクロージャの capture 環境（applier/generation）を読むと
                // use-after-free になるため、drop 前に値をローカルへ退避しておく。
                let applier = applier.clone();
                let generation = generation;
                release_speak_handlers();
                applier.apply(move |s| finish_speak(s, generation, Ok(())));
            })
        };
        let on_error = {
            let applier = applier.clone();
            Closure::<dyn FnMut(web_sys::SpeechSynthesisErrorEvent)>::new(
                move |event: web_sys::SpeechSynthesisErrorEvent| {
                    let message = error_message(event.error());
                    // on_end と同じ理由（上記コメント参照）で drop 前に退避する。
                    let applier = applier.clone();
                    let generation = generation;
                    release_speak_handlers();
                    applier.apply(move |s| finish_speak(s, generation, Err(message)));
                },
            )
        };
        utterance.set_onend(Some(on_end.as_ref().unchecked_ref()));
        utterance.set_onerror(Some(on_error.as_ref().unchecked_ref()));

        // Utterance 自体もここで保持する。呼び出し元が参照を持たないと
        // ブラウザによっては発話中に GC されて onend/onerror が発火しない
        // （Bugbot 指摘）。前回分がまだ残っていた場合（通常は無いはずだが）は
        // 上書き前に解放し、Closure を取りこぼさない。
        release_speak_handlers();
        ACTIVE_HANDLERS.with(|cell| {
            *cell.borrow_mut() = Some(SpeakHandlers {
                on_end,
                on_error,
                utterance: utterance.clone(),
            });
        });

        // click の同期チェーン内での呼び出し（Safari のユーザー操作起点要件を満たす、
        // モジュール doc 参照）。
        synth.speak(&utterance);
    }
}
