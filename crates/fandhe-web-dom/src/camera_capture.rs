//! 親 #990 の (c)。型サンプル「カメラ撮影」（Issue #991）の Web 経路。
//!
//! `fandhe_core::screens::camera_capture`（端末依存ゼロ）の `state.phase ==
//! Requested` を [`super::mount::mount_with_effect`] の effect として観測し、
//! `navigator.mediaDevices.getUserMedia({video: true})` + 非表示 `<video>`/`<canvas>`
//! でフレームを 1 枚取得してサムネイル化し、結果を書き戻す。
//!
//! Web は `fandhe-device` の `Camera` トレインを通らない（`mic_record.rs`/`location.rs`
//! と同じ理由。`fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計、
//! `docs/device/capabilities/camera-capture.json` の web 欄 note 参照）。そのため本モジュール
//! が DOM レンダラ側の対応実装にあたる。サムネイル化（実画素へのブロック平均縮小）の不変条件だけは
//! [`fandhe_core::screens::camera_capture::thumbnail_from_rgba`] を共有する
//! （検証ロジックの SSOT を 1 箇所に保つ、`mic_record.rs::summary_from_bytes` と同じ方針）。
//!
//! # `fandhe_core::screens::camera_capture` に世代が無い
//!
//! `mic_record`/`location` の要求型と異なり、`CameraCaptureState` は `phase` フィールド
//! のみを持ち、要求ごとの世代番号を持たない（`finish_capture`/`cancel_capture` はいずれも
//! `phase` の照合のみで世代引数を取らない）。そのため世代の管理は本モジュールの
//! `thread_local`（`ACTIVE` の各エントリが持つ `generation`）に閉じる
//! （`location.rs::Entry::generation` と同じ方針だが、単調カウンタは `ACTIVE` の
//! エントリ自体に埋め込み、`allocate_generation` で払い出す）。
//!
//! # 期限は撮影開始後のフレーム待ちのみに張る（契約表「期限・キャンセル」節）
//!
//! 権限プロンプト表示中（`getUserMedia` の Promise 待ち）には期限を張らない
//! （利用者操作待ちであり、離脱は `pagehide` で回収する）。別タブへの切り替え
//! （`pagehide` を伴わない `document.hidden`）もこのフェーズでキャンセル対象にする
//! （`visibilitychange` 購読、Codex 指摘 #998・P1）。ストリーム取得後、
//! `<video>` にフレームが準備できるまでの待ち（[`FRAME_TIMEOUT_MS`]）にのみ期限を
//! 設ける。待機は `Closure` を新たに作らず、`js_sys::Promise::new` の executor が
//! 渡す `resolve`（`Function`）をそのまま `setTimeout` へ渡す（`Closure::forget()` を
//! ポーリングのたびに積み上げてリークしない、実装計画「純関数を wasm32 gate の外に
//! 置く理由」節・#963 の教訓）。
//!
//! # 成功・取消・失敗の三値（契約表「キャンセル・正常終了・失敗の三値」節）
//!
//! | 事象 | 報告 |
//! |------|------|
//! | フレーム取得成功 | `finish_capture(Ok(thumbnail))` → `Captured` |
//! | `pagehide`（bfcache 退避含む）・撮影中の `document.hidden` | `cancel_capture` → `Idle`（利用者の離脱。再開しない） |
//! | `NotAllowedError`/`PermissionDeniedError`（拒否） | `Failed(PermissionDenied)` |
//! | `NotFoundError`/`OverconstrainedError`/`NotSupportedError`/`SecurityError`・非 secure context・`mediaDevices` 無し | `Failed(Unsupported)` |
//! | `NotReadableError`/`AbortError`・その他・`play()` 失敗・フレーム待ちタイムアウト・画素データ不正 | `Failed(Other(固定文言))` |
//!
//! # リソース解放（契約表「失敗時のリソース解放・共有状態復元」節）
//!
//! [`web::release`] 1 箇所に集約する（成功・失敗・取消・`pagehide` の全経路がここを
//! 通る）。`<video>`/`<canvas>` は DOM に挿入しないため、`release` で参照を手放せば
//! ガベージコレクション対象になる。取得直後に世代不一致（既に破棄済み）と分かった
//! ストリームも、格納前にその場でトラックを止める（`mic_record.rs` と同じ方針）。
//!
//! # 秘密値・ログ（security.md A09）
//!
//! 例外オブジェクトの `message`・デバイス名・実画素は `console` へ出力・保存・送信しない
//! （`name` のみを見て固定文言へ畳む、`mic_record.rs` と同じ方針）。撮影結果の状態には
//! 生 RGBA を保持しない（`fandhe_core::screens::camera_capture` モジュール doc 参照）。
//!
//! # 既知の天井（対象外、実装計画「対象外」節）
//!
//! `demo.rs` の各エントリポイントは `current_screen(&CameraCaptureState::default())` で
//! 初期状態の `Screen` を固定するため（`mount_with_effect` が状態から `Screen` を
//! 再構築しない設計、`mic_record.rs`/`http_fetch.rs` と同型の既存の天井）、「撮影」
//! ボタンは常時表示される。`start_capture` は `Requested`/`Capturing` 中は no-op、
//! `Captured`/`Failed` からは再撮影へ進むため実害はない。

use fandhe_core::screens::camera_capture::{thumbnail_from_rgba, CaptureFailure, Thumbnail};

/// `NotSupportedError`/`SecurityError`・非 secure context・`mediaDevices` 無しのときの案内。
/// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
/// （`mic_record.rs::UNAVAILABLE` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const UNAVAILABLE: &str = "この環境ではカメラを利用できません";
/// 種別を特定できない失敗・タイムアウト・画素データ不正のときの汎用案内。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CAPTURE_FAILED: &str = "カメラの映像を取得できませんでした";
/// フレーム待ちの上限（ミリ秒）。権限プロンプト表示中には適用しない
/// （モジュール doc「期限は撮影開始後のフレーム待ちのみに張る」節参照）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const FRAME_TIMEOUT_MS: i32 = 5_000;
/// フレーム準備待ちのポーリング間隔（ミリ秒）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const POLL_INTERVAL_MS: i32 = 100;

/// 例外の `name`（`DOMException` の種別）→ [`CaptureFailure`] への変換純関数
/// （ブラウザ API を呼ばないため native からもテストできる、モジュール doc 参照）。
/// `message` は受け取らない・転送しない（security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn failure_from_error_name(name: Option<&str>) -> CaptureFailure {
    match name {
        Some("NotAllowedError") | Some("PermissionDeniedError") => CaptureFailure::PermissionDenied,
        Some("NotFoundError")
        | Some("OverconstrainedError")
        | Some("NotSupportedError")
        | Some("SecurityError") => CaptureFailure::Unsupported,
        _ => CaptureFailure::Other(CAPTURE_FAILED.to_string()),
    }
}

/// 縮小キャンバスから読み取った RGBA（`canvas_w` x `canvas_h`）をサムネイル化し、
/// 表示用の `width`/`height` を元映像のサイズ（`src_w`/`src_h`）で上書きする純関数
/// （ブラウザ API を呼ばないため native からもテストできる、モジュール doc 参照）。
///
/// `src_w`/`src_h` が 0、または [`thumbnail_from_rgba`] が `None`（`canvas_w`/`canvas_h`
/// が 0・`rgba` の長さ不一致）を返す場合は [`CaptureFailure::Other`] へ畳む
/// （0 バイト・不正長を成功へ畳まない、契約表「嘘の成功」節）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn thumbnail_from_frame(
    src_w: u32,
    src_h: u32,
    canvas_w: u32,
    canvas_h: u32,
    rgba: &[u8],
) -> Result<Thumbnail, CaptureFailure> {
    if src_w == 0 || src_h == 0 {
        return Err(CaptureFailure::Other(CAPTURE_FAILED.to_string()));
    }
    let mut thumbnail = thumbnail_from_rgba(canvas_w, canvas_h, rgba)
        .ok_or_else(|| CaptureFailure::Other(CAPTURE_FAILED.to_string()))?;
    thumbnail.width = src_w;
    thumbnail.height = src_h;
    Ok(thumbnail)
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::camera_capture::{
        cancel_capture, finish_capture, mark_capturing, thumbnail_size, CameraCaptureState,
        CapturePhase,
    };
    use js_sys::Reflect;
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        CanvasRenderingContext2d, HtmlCanvasElement, HtmlVideoElement, MediaStream,
        MediaStreamConstraints,
    };

    use super::{
        failure_from_error_name, thumbnail_from_frame, CAPTURE_FAILED, FRAME_TIMEOUT_MS,
        POLL_INTERVAL_MS, UNAVAILABLE,
    };
    use crate::mount::Applier;

    /// 1 回の撮影セッションぶんの状態。すべての終了経路（成功・失敗・キャンセル・
    /// `pagehide`）が [`release`] を通して解放する（モジュール doc 参照）。
    struct Session {
        applier: Applier<CameraCaptureState>,
        /// [`matches_current`] へ照合する世代番号（`location.rs::Entry::generation` と
        /// 同じ役割。古い世代の遅延コールバックを捨てる）。
        generation: u64,
        stream: Option<MediaStream>,
        /// DOM に挿入しない `<video>`（`srcObject` 経由でストリームを流し込む）。
        video: Option<HtmlVideoElement>,
    }

    thread_local! {
        /// マウント ID ごとの撮影セッション（`mic_record.rs::ACTIVE` と同じくマウント単位で
        /// 持つ。同一ページへ本画面を 2 つマウントしたときに一方が他方の撮影を
        /// 止めないため）。
        static ACTIVE: RefCell<HashMap<u64, Session>> = RefCell::new(HashMap::new());

        /// マウント ID ごとに払い出し済みの次の世代番号（`location.rs::NEXT_GENERATION`
        /// と同じ理由で `ACTIVE` とは独立して残り続ける）。
        static NEXT_GENERATION: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    }

    /// `pagehide`/`visibilitychange` 購読クロージャの型（clippy::type_complexity
    /// 回避のエイリアス）。
    type PagehideClosure = Closure<dyn FnMut(web_sys::Event)>;

    thread_local! {
        /// `pagehide` 購読を（未登録なら）1 度だけ登録するためのハンドル
        /// （`mic_record.rs::PAGEHIDE` と同型）。
        static PAGEHIDE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };

        /// `visibilitychange` 購読を（未登録なら）1 度だけ登録するためのハンドル
        /// （[`register_visibilitychange_once`] 参照）。
        static VISIBILITYCHANGE: RefCell<Option<PagehideClosure>> = const { RefCell::new(None) };
    }

    /// `window` を取得する（`web_sys::window()` の欠如を毎回 `Option` 判定しないための
    /// 下ごしらえ）。
    fn window() -> Option<web_sys::Window> {
        web_sys::window()
    }

    /// `document.hidden` を読む。取得できない場合は非表示ではない扱い
    /// （fail-open だが、直後のフレーム待ちループが引き続き `matches_current` で
    /// 世代照合するため実害はない）。
    fn is_hidden() -> bool {
        window()
            .and_then(|w| w.document())
            .map(|d| d.hidden())
            .unwrap_or(false)
    }

    /// `mount_id` の次の世代番号を払い出す（`location.rs::allocate_generation` と同じ方針）。
    fn allocate_generation(mount_id: u64) -> u64 {
        NEXT_GENERATION.with(|next| {
            let mut map = next.borrow_mut();
            let slot = map.entry(mount_id).or_insert(0);
            let generation = *slot;
            *slot = slot.wrapping_add(1);
            generation
        })
    }

    /// `mount_id`・`generation` が現在のセッションと一致するかを確認する
    /// （`mic_record.rs::matches_current` と同型）。
    fn matches_current(mount_id: u64, generation: u64) -> bool {
        ACTIVE.with(|active| {
            active
                .borrow()
                .get(&mount_id)
                .is_some_and(|s| s.generation == generation)
        })
    }

    /// `mount_id` のセッションを取り出して解放する（`video.pause()` で係属中の
    /// `play()` Promise を強制決着させる → ストリームの全トラック停止 → `video` の
    /// `srcObject` を外す）。全終了経路（成功・失敗・キャンセル・`pagehide`）が
    /// ここを通る（モジュール doc 参照）。見つからなければ何もしない（古い世代から
    /// の呼び出し・二重解放を無害化する）。
    ///
    /// `pause()` を呼ぶのは、[`play_or_timeout`] の `holder`（`Rc` の自己参照）を
    /// 確実に解くため（`play_or_timeout` doc 参照）。`TimedOut`/`Hidden`/`Stale` は
    /// この関数を経由する `fail`/`cancel_session` の前に決まるが、`play()` の
    /// Promise 自体はまだ未決着で `holder` を握ったまま残る。`pause()` は仕様上
    /// 「係属中の `play()` Promise を `AbortError` で拒否する」契約を持つため、
    /// `srcObject` を外すより先に呼び、`on_reject` を確実に発火させて `holder` を
    /// 自己解放させる（`srcObject` を外すだけでも通常は拒否されるが、`pause()` の
    /// 方が仕様上直接的で確実）。
    fn take_and_release(mount_id: u64) -> Option<Session> {
        let session = ACTIVE.with(|active| active.borrow_mut().remove(&mount_id));
        if let Some(session) = &session {
            if let Some(video) = &session.video {
                let _ = video.pause();
                video.set_src_object(None);
            }
            stop_stream(session.stream.as_ref());
        }
        session
    }

    /// ストリームの全トラックを止める（マイク/カメラ使用中インジケータが消える）。
    /// 未格納（`getUserMedia` 解決前に破棄された）場合は何もしない。
    fn stop_stream(stream: Option<&MediaStream>) {
        let Some(stream) = stream else {
            return;
        };
        for track in stream.get_tracks().iter() {
            if let Ok(track) = track.dyn_into::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
    }

    /// 要求を失敗として畳み、セッションを解放して `Applier::apply` へ渡す
    /// （`mic_record.rs::fail` と同型）。`await` を挟んだ後に呼ぶため、その間に
    /// `pagehide`/新規撮影でセッションが破棄・差し替えされている場合がある。
    /// 世代照合を再確認しないと、破棄済みの `mount_id` に対して `release`/
    /// `finish_capture` を呼び、`discard_all_sessions` が既に確定させた「中断」結果を
    /// 遅延到着した失敗で上書きしてしまう（PR #963 の教訓を踏襲）。
    fn fail(applier: &Applier<CameraCaptureState>, mount_id: u64, generation: u64, message: &str) {
        finish(
            applier,
            mount_id,
            generation,
            Err(fandhe_core::screens::camera_capture::CaptureFailure::Other(
                message.to_string(),
            )),
        );
    }

    /// 非セキュアコンテキスト・`mediaDevices`/`getUserMedia` 不在（契約表「成功・取消・
    /// 失敗の三値」節の `Unsupported` 経路）を反映する（Cursor Bugbot 指摘 #998）。
    /// `fail` は常に `CaptureFailure::Other` へ畳むため、専用の未対応文言
    /// （`status_text` の `Unsupported` 分岐）を出せなかった。
    fn fail_unsupported(applier: &Applier<CameraCaptureState>, mount_id: u64, generation: u64) {
        finish(
            applier,
            mount_id,
            generation,
            Err(fandhe_core::screens::camera_capture::CaptureFailure::Unsupported),
        );
    }

    /// 撮影結果（成功/失敗）を反映する。世代照合 → 解放 → `finish_capture` の順序を
    /// 1 箇所に集約する（`fail`・成功経路の双方の呼び出し元）。
    fn finish(
        applier: &Applier<CameraCaptureState>,
        mount_id: u64,
        generation: u64,
        result: Result<
            fandhe_core::screens::camera_capture::Thumbnail,
            fandhe_core::screens::camera_capture::CaptureFailure,
        >,
    ) {
        if !matches_current(mount_id, generation) {
            return;
        }
        take_and_release(mount_id);
        applier.apply(move |s| finish_capture(s, result));
    }

    /// 利用者操作によるキャンセル（`pagehide`・撮影中の `document.hidden`）を反映する。
    /// `cancel_capture` は `Capturing` 以外では no-op なので、`Requested` の間に届いても
    /// 実害はない（`fandhe_core::screens::camera_capture::cancel_capture` の doc 参照）。
    fn cancel_session(mount_id: u64) {
        if let Some(session) = take_and_release(mount_id) {
            session.applier.apply(cancel_capture);
        }
    }

    /// 全マウントの撮影セッションを破棄する。`pagehide`（タブを離れる・bfcache への
    /// 退避）から呼ばれる契約（`mic_record.rs::discard_all_sessions` と同型）。
    /// bfcache から復帰しても古い値を返し続けないよう、再開はしない
    /// （実装計画「対象外」節、`pageshow` の再開なし）。
    fn discard_all_sessions() {
        let mount_ids: Vec<u64> = ACTIVE.with(|active| active.borrow().keys().copied().collect());
        for mount_id in mount_ids {
            cancel_session(mount_id);
        }
    }

    /// ストリーム取得後（`play()`/フレーム待ち）のセッションだけを破棄する。
    /// `visibilitychange` から呼ばれる。購読はページ寿命ぶん残るため、2 回目以降の撮影で
    /// 権限プロンプト表示中（`stream` 未格納）に `document.hidden` へ遷移しても、
    /// そのセッションは取り消さない（PR #998 codex 指摘。モバイル Chromium は権限
    /// ダイアログ表示中に `hidden` になる、[`register_visibilitychange_once`] 参照）。
    fn discard_playing_sessions() {
        let mount_ids: Vec<u64> = ACTIVE.with(|active| {
            active
                .borrow()
                .iter()
                .filter(|(_, s)| s.stream.is_some())
                .map(|(id, _)| *id)
                .collect()
        });
        for mount_id in mount_ids {
            cancel_session(mount_id);
        }
    }

    /// `pagehide` 購読を（未登録なら）1 度だけ登録する。`ACTIVE` へセッションを登録した
    /// 直後・`getUserMedia` 呼び出し前に呼ぶ契約（`mic_record.rs::register_pagehide_once`
    /// と同じ理由。権限プロンプト表示中に離脱されても `discard_all_sessions` が確実に
    /// 効くようにするため）。戻り値は購読が有効かどうかで、呼び出し側はこれが `false`
    /// のときは fail-closed で撮影開始へ進んではならない。
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

    /// `visibilitychange` 購読を（未登録なら）1 度だけ登録する。`getUserMedia` の
    /// Promise 待ち（権限プロンプト表示中）より後、`start_playback` の先頭で呼ぶ契約
    /// （`mic_record.rs` がこの待機中は `pagehide` のみを監視する方針と揃える）。
    /// モバイル Chromium は権限ダイアログ表示中に `document.hidden` へ遷移するため、
    /// `getUserMedia` 呼び出し前にこの購読を有効にすると、許可待ちの間に
    /// `discard_all_sessions` が呼ばれ、許可されたストリームを直後に失う
    /// （Bugbot 指摘 #998）。`play()`/フレーム待ちフェーズでは `document.hidden` を
    /// 即座に検知したいため、ストリーム取得後にここで登録する。
    /// 戻り値は [`register_pagehide_once`] と同型（登録失敗時は呼び出し側が
    /// fail-closed で撮影開始へ進んではならない）。
    fn register_visibilitychange_once() -> bool {
        VISIBILITYCHANGE.with(|cell| {
            if cell.borrow().is_some() {
                return true;
            }
            let Some(document) = window().and_then(|w| w.document()) else {
                return false;
            };
            let closure =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    if is_hidden() {
                        discard_playing_sessions();
                    }
                });
            if document
                .add_event_listener_with_callback(
                    "visibilitychange",
                    closure.as_ref().unchecked_ref(),
                )
                .is_err()
            {
                return false;
            }
            *cell.borrow_mut() = Some(closure);
            true
        })
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する
    /// （`mic_record.rs::has_function` と同型。型に現れない JS 側の実態を境界で
    /// 検証する、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `ms` ミリ秒だけ待つ。`Closure` を新たに作らない（モジュール doc「期限は撮影開始後
    /// のフレーム待ちのみに張る」節参照）: `Promise::new` の executor が渡す `resolve`
    /// （既に `Function`）をそのまま `setTimeout` のコールバックへ渡す。
    async fn sleep_ms(ms: i32) {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let Some(w) = window() else {
                // `window` が無ければ即解決する（呼び出し元は直後に `window()` を
                // 再度必要とする経路のため、いずれにせよ後続で fail-closed になる）。
                let _ = resolve.call0(&JsValue::NULL);
                return;
            };
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        });
        let _ = JsFuture::from(promise).await;
    }

    /// [`play_or_timeout`] の結果種別。`Hidden`/`Stale` は `wait_for_frame` の
    /// [`WaitOutcome`] と同じ扱い（契約表「ライフサイクル境界」節の「非表示はキャンセル
    /// して Idle に戻す」契約を `play()` 待ちにも揃える、Codex 指摘 #998）。
    enum PlayOutcome {
        /// `play()` の Promise が解決した（再生開始）。
        Resolved,
        /// `play()` の Promise が拒否された。
        Rejected,
        /// `ms` 以内に `play()` が解決も拒否もしなかった。
        TimedOut,
        /// `play()` 待ち中にタブが非表示になった。呼び出し元は [`cancel_session`] を呼ぶ。
        Hidden,
        /// `play()` 待ち中に世代不一致になった（`pagehide`・新規撮影で既に破棄済み）。
        Stale,
    }

    /// [`play_or_timeout`] の `on_resolve`/`on_reject` を自己解放させるための保持先の型
    /// （`play_or_timeout` doc 参照）。
    type PlaySettleClosures = (Closure<dyn FnMut(JsValue)>, Closure<dyn FnMut(JsValue)>);

    /// `play_promise`（`video.play()` の戻り値）を `ms` ミリ秒の期限と競わせつつ、
    /// [`POLL_INTERVAL_MS`] 間隔で `document.hidden`・世代照合も見る
    /// （`wait_for_frame` と同じポーリング方針。`Promise::race` だけだと `play()` の
    /// 決着かタイムアウトしか検出できず、待機中の非表示をタイムアウトまで
    /// 見逃してしまう、Codex 指摘 #998）。`play_promise` 自体には `.then2` で
    /// 決着フラグを取り付け、ポーリングループから毎回覗く。
    ///
    /// `on_resolve`/`on_reject` は `holder`（`Rc<RefCell<Option<..>>>`）へ格納し、
    /// 自分自身が呼ばれた時点で `holder` を空にして自己解放する（wasm-bindgen 公式
    /// ガイドの「クロージャに自身を保持させ、用済みになったら take() で外す」パターン。
    /// `holder` は `on_resolve`/`on_reject` 自身にも clone を渡しているため、
    /// この関数が `TimedOut`/`Hidden`/`Stale` で先に return してローカル変数の
    /// `holder` を手放しても、`play_promise` が後から解決/拒否すれば呼び出しは届き、
    /// 届いた時点で `holder` の最後の参照ごと自然に解放される。関数スコープの Drop に
    /// 任せて早期に破棄すると、破棄後に決着したときに JS 側が破棄済みクロージャを
    /// 呼び出して panic する（Codex P2 / Cursor Bugbot 指摘 #998）。
    ///
    /// `TimedOut`/`Hidden`/`Stale` いずれの呼び出し元も直後に `fail`/`cancel_session`
    /// 経由で [`take_and_release`] を呼ぶ契約になっており、そこで `video.pause()` が
    /// 係属中の `play()` Promise を `AbortError` で強制的に拒否させるため、`holder` の
    /// 自己解放は待たせても長くは残らない（無期限にリークし続けるわけではない、
    /// Codex 指摘 #998 の再指摘・`take_and_release` doc 参照）。
    async fn play_or_timeout(
        mount_id: u64,
        generation: u64,
        play_promise: js_sys::Promise,
        ms: i32,
    ) -> PlayOutcome {
        let settled: Rc<RefCell<Option<Result<(), ()>>>> = Rc::new(RefCell::new(None));
        let holder: Rc<RefCell<Option<PlaySettleClosures>>> = Rc::new(RefCell::new(None));
        let on_resolve = {
            let settled = Rc::clone(&settled);
            let holder = Rc::clone(&holder);
            Closure::<dyn FnMut(JsValue)>::new(move |_| {
                *settled.borrow_mut() = Some(Ok(()));
                holder.borrow_mut().take();
            })
        };
        let on_reject = {
            let settled = Rc::clone(&settled);
            let holder = Rc::clone(&holder);
            Closure::<dyn FnMut(JsValue)>::new(move |_| {
                *settled.borrow_mut() = Some(Err(()));
                holder.borrow_mut().take();
            })
        };
        let _ = play_promise.then2(&on_resolve, &on_reject);
        *holder.borrow_mut() = Some((on_resolve, on_reject));

        let mut waited_ms = 0;
        loop {
            if !matches_current(mount_id, generation) {
                return PlayOutcome::Stale;
            }
            if is_hidden() {
                return PlayOutcome::Hidden;
            }
            if let Some(result) = *settled.borrow() {
                return match result {
                    Ok(()) => PlayOutcome::Resolved,
                    Err(()) => PlayOutcome::Rejected,
                };
            }
            if waited_ms >= ms {
                return PlayOutcome::TimedOut;
            }
            sleep_ms(POLL_INTERVAL_MS).await;
            waited_ms += POLL_INTERVAL_MS;
        }
    }

    /// [`wait_for_frame`] の結果種別。「失敗として報告すべきか」を種類ごとに区別する
    /// （契約表「キャンセル・正常終了・失敗の三値」節。`Stale`/`Hidden` は失敗として
    /// 表示しない）。
    enum WaitOutcome {
        /// フレーム準備完了。
        Ready,
        /// 世代不一致（`pagehide`・新規撮影で既に破棄済み）。呼び出し元は何もしない。
        Stale,
        /// 撮影中にタブが非表示になった。呼び出し元は [`cancel_session`] を呼ぶ。
        Hidden,
        /// [`FRAME_TIMEOUT_MS`] 到達。呼び出し元は失敗として報告する。
        Timeout,
    }

    /// `video` にフレームが準備できるまで [`POLL_INTERVAL_MS`] 間隔でポーリングする。
    /// 各回で世代照合・`document.hidden` 判定を行う（契約表「ポーリング・再描画」
    /// 「ライフサイクル境界」節）。
    async fn wait_for_frame(
        mount_id: u64,
        generation: u64,
        video: &HtmlVideoElement,
    ) -> WaitOutcome {
        let mut waited_ms = 0;
        loop {
            if !matches_current(mount_id, generation) {
                return WaitOutcome::Stale;
            }
            if is_hidden() {
                return WaitOutcome::Hidden;
            }
            // `HAVE_CURRENT_DATA`（2）以上、かつ実寸が確定していればフレームが読める
            // （`video_width() == 0` は寸法未確定の代表的な兆候）。
            if video.ready_state() >= 2 && video.video_width() > 0 {
                return WaitOutcome::Ready;
            }
            if waited_ms >= FRAME_TIMEOUT_MS {
                return WaitOutcome::Timeout;
            }
            sleep_ms(POLL_INTERVAL_MS).await;
            waited_ms += POLL_INTERVAL_MS;
        }
    }

    /// 縮小キャンバスへ描画し、RGBA を読み取ってサムネイル化する。
    /// `<canvas>`/2D コンテキストの構築失敗・画素取得失敗は [`CAPTURE_FAILED`] へ畳む
    /// （内部の JS 例外を画面へ漏らさない、security.md A09）。
    fn capture_thumbnail(
        video: &HtmlVideoElement,
    ) -> Result<
        fandhe_core::screens::camera_capture::Thumbnail,
        fandhe_core::screens::camera_capture::CaptureFailure,
    > {
        let fail = || {
            fandhe_core::screens::camera_capture::CaptureFailure::Other(CAPTURE_FAILED.to_string())
        };

        let document = window().and_then(|w| w.document()).ok_or_else(fail)?;
        let canvas: HtmlCanvasElement = document
            .create_element("canvas")
            .map_err(|_| fail())?
            .dyn_into()
            .map_err(|_| fail())?;
        // 縦横比を保った縮小寸法（長辺 THUMB_MAX_SIDE 以下）。映像サイズ未確定（0）は失敗。
        let (canvas_w, canvas_h) = thumbnail_size(video.video_width(), video.video_height());
        if canvas_w == 0 || canvas_h == 0 {
            return Err(fail());
        }
        canvas.set_width(canvas_w);
        canvas.set_height(canvas_h);

        let context: CanvasRenderingContext2d = canvas
            .get_context("2d")
            .map_err(|_| fail())?
            .ok_or_else(fail)?
            .dyn_into()
            .map_err(|_| fail())?;

        context
            .draw_image_with_html_video_element_and_dw_and_dh(
                video,
                0.0,
                0.0,
                canvas_w as f64,
                canvas_h as f64,
            )
            .map_err(|_| fail())?;

        let image_data = context
            .get_image_data(0.0, 0.0, canvas_w as f64, canvas_h as f64)
            .map_err(|_| fail())?;

        thumbnail_from_frame(
            video.video_width(),
            video.video_height(),
            image_data.width(),
            image_data.height(),
            &image_data.data().0,
        )
    }

    /// `getUserMedia` が成功したあとの続き（`<video>` の構築・再生・フレーム待ち・
    /// 縮小描画・結果反映）。
    async fn start_playback(
        mount_id: u64,
        generation: u64,
        applier: Applier<CameraCaptureState>,
        stream: MediaStream,
    ) {
        // 権限プロンプト表示中は監視しない（[`register_visibilitychange_once`]
        // 参照）。ストリーム取得後のこの時点から `document.hidden` への遷移を
        // 監視する。登録できなければ fail-closed でストリームを閉じる。
        if !register_visibilitychange_once() {
            stop_stream(Some(&stream));
            fail(&applier, mount_id, generation, UNAVAILABLE);
            return;
        }
        let Some(document) = window().and_then(|w| w.document()) else {
            stop_stream(Some(&stream));
            fail(&applier, mount_id, generation, UNAVAILABLE);
            return;
        };
        let video: HtmlVideoElement = match document
            .create_element("video")
            .ok()
            .and_then(|el| el.dyn_into().ok())
        {
            Some(video) => video,
            None => {
                stop_stream(Some(&stream));
                fail(&applier, mount_id, generation, UNAVAILABLE);
                return;
            }
        };
        video.set_muted(true);
        // iOS Safari はインライン再生に `playsinline` 属性を要求する（`autoplay` の
        // フルスクリーン誘導を避ける）。`web-sys` に専用アクセサが無いため属性で設定する。
        let _ = video.set_attribute("playsinline", "");
        video.set_src_object(Some(&stream));

        // `video`/`stream` を格納してから再照合できるよう、`ACTIVE` を更新する
        // （世代不一致なら格納直後にトラックを止めて破棄する、`mic_record.rs` の
        // `start_recorder` と同じ方針）。
        let stored = ACTIVE.with(|active| {
            if let Some(session) = active.borrow_mut().get_mut(&mount_id) {
                if session.generation == generation {
                    // `stream.clone()` は JS の `MediaStream.clone()`（トラックを複製する
                    // web-sys の固有メソッド）に解決され、停止しても元のトラックが生き残る
                    // （カメラが点いたままになる）。JS 参照の複製は `Clone::clone` で明示する。
                    session.stream = Some(Clone::clone(&stream));
                    session.video = Some(video.clone());
                    return true;
                }
            }
            false
        });
        if !stored {
            video.set_src_object(None);
            stop_stream(Some(&stream));
            return;
        }

        // `play()` は `Promise` を返す（同期例外だけでなく、再生不可の場合は
        // Promise の拒否として非同期に通知される、Codex 指摘 #998）。拒否を見逃すと
        // フレームが来ないまま `wait_for_frame` の期限（`FRAME_TIMEOUT_MS`）まで
        // トラックを保持し続けるため、ここで `await` して拒否を即座に検出する。
        //
        // 拒否だけでなく「いつまでも解決しない」場合も同じ期限で打ち切る
        // （Codex/Bugbot 指摘 #998 の再指摘）。`play()` の Promise は拒否も解決も
        // しないまま止まりうる（ブラウザ実装依存）ため、`wait_for_frame` と同じ
        // `FRAME_TIMEOUT_MS` を期限に使う。さらに、この待ち区間でタブが非表示に
        // なってもタイムアウトまでトラックを保持し続けないよう、`wait_for_frame` と
        // 同じ `POLL_INTERVAL_MS` 間隔で `document.hidden`・世代照合も見る
        // （Codex 指摘 #998 の 3 度目の再指摘。`play_or_timeout` の doc 参照）。
        match video.play() {
            Ok(promise) => {
                match play_or_timeout(mount_id, generation, promise, FRAME_TIMEOUT_MS).await {
                    PlayOutcome::Resolved => {}
                    PlayOutcome::Stale => return,
                    PlayOutcome::Hidden => {
                        cancel_session(mount_id);
                        return;
                    }
                    PlayOutcome::Rejected | PlayOutcome::TimedOut => {
                        fail(&applier, mount_id, generation, CAPTURE_FAILED);
                        return;
                    }
                }
            }
            Err(_) => {
                fail(&applier, mount_id, generation, CAPTURE_FAILED);
                return;
            }
        }

        match wait_for_frame(mount_id, generation, &video).await {
            WaitOutcome::Stale => {}
            WaitOutcome::Hidden => cancel_session(mount_id),
            WaitOutcome::Timeout => fail(&applier, mount_id, generation, CAPTURE_FAILED),
            WaitOutcome::Ready => {
                let result = capture_thumbnail(&video);
                finish(&applier, mount_id, generation, result);
            }
        }
    }

    /// `mount_with_effect` へ渡す effect。`camera_capture` 画面の撮影開始要求を処理する。
    ///
    /// 要求が無ければ何もしない（`mount.rs` モジュール doc の「effect フック」節の契約）。
    /// `state.phase == Requested` のときだけ処理し、既にセッションがあれば二重起動しない
    /// （`mic_record.rs::mic_record_effect` と同型の判定。`mark_capturing` の適用で
    /// `phase` が `Capturing` へ進めば、以降の再入はこのガードで no-op になる）。
    pub fn camera_capture_effect(
        state: &CameraCaptureState,
        applier: &Applier<CameraCaptureState>,
    ) {
        if !matches!(state.phase, CapturePhase::Requested) {
            return;
        }
        let mount_id = applier.mount_id();
        if ACTIVE.with(|active| active.borrow().contains_key(&mount_id)) {
            return;
        }

        applier.apply(mark_capturing);

        let generation = allocate_generation(mount_id);
        // 先に「起動不能」を仮登録してから前提検査に入る（`apply` が起こす再描画で
        // effect が再入しても「既にエントリがある」判定で no-op になるようにする、
        // `mic_record.rs::mic_record_effect` と同じ理由）。
        ACTIVE.with(|active| {
            active.borrow_mut().insert(
                mount_id,
                Session {
                    applier: applier.clone(),
                    generation,
                    stream: None,
                    video: None,
                },
            );
        });

        // `getUserMedia` の Promise 待ち（権限プロンプト表示中）にも `pagehide` を
        // 監視できるよう、`ACTIVE` 登録直後・`getUserMedia` 呼び出し前に登録する
        // （`mic_record.rs` と同じ理由）。登録できなければ離脱後の撮影開始を防げない
        // ため fail-closed で撮影そのものを始めない。
        if !register_pagehide_once() {
            fail(applier, mount_id, generation, UNAVAILABLE);
            return;
        }
        let Some(w) = window() else {
            fail(applier, mount_id, generation, UNAVAILABLE);
            return;
        };
        if !w.is_secure_context() {
            fail_unsupported(applier, mount_id, generation);
            return;
        }
        let navigator = w.navigator();
        let Ok(media_devices) = navigator.media_devices() else {
            fail_unsupported(applier, mount_id, generation);
            return;
        };
        // `MediaStreamConstraints` 自体は型として常に存在するため、境界検証は
        // `navigator.mediaDevices` の実態確認（直前の `media_devices()` の成功）で足りる
        // （`mic_record.rs` の同種コメント参照）。`getUserMedia` が関数として実在するかは
        // 念のため確認する（`share.rs::has_function` と同じ理由、security.md A03）。
        let media_devices_value: JsValue = media_devices.clone().into();
        if !has_function(&media_devices_value, "getUserMedia") {
            fail_unsupported(applier, mount_id, generation);
            return;
        }

        let constraints = MediaStreamConstraints::new();
        constraints.set_video_bool(true);

        let promise = match media_devices.get_user_media_with_constraints(&constraints) {
            Ok(promise) => promise,
            Err(err) => {
                let name = Reflect::get(&err, &JsValue::from_str("name"))
                    .ok()
                    .and_then(|v| v.as_string());
                let result = Err(failure_from_error_name(name.as_deref()));
                finish(applier, mount_id, generation, result);
                return;
            }
        };

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            match JsFuture::from(promise).await {
                Ok(stream_value) => {
                    if !matches_current(mount_id, generation) {
                        // `pagehide`/新規撮影で既に破棄済み。取得したストリームは
                        // すぐ閉じる（カメラ使用中インジケータを残さない）。
                        if let Ok(stream) = stream_value.dyn_into::<MediaStream>() {
                            stop_stream(Some(&stream));
                        }
                        return;
                    }
                    match stream_value.dyn_into::<MediaStream>() {
                        Ok(stream) => start_playback(mount_id, generation, applier, stream).await,
                        Err(_) => fail(&applier, mount_id, generation, CAPTURE_FAILED),
                    }
                }
                Err(err) => {
                    // 権限プロンプト表示中に `pagehide` でセッションが破棄され得る
                    // （`mic_record.rs` と同じ理由で再照合する）。
                    if !matches_current(mount_id, generation) {
                        return;
                    }
                    let name = Reflect::get(&err, &JsValue::from_str("name"))
                        .ok()
                        .and_then(|v| v.as_string());
                    let result = Err(failure_from_error_name(name.as_deref()));
                    finish(&applier, mount_id, generation, result);
                }
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::camera_capture_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_allowed_error_maps_to_permission_denied() {
        assert_eq!(
            failure_from_error_name(Some("NotAllowedError")),
            CaptureFailure::PermissionDenied
        );
        assert_eq!(
            failure_from_error_name(Some("PermissionDeniedError")),
            CaptureFailure::PermissionDenied
        );
    }

    #[test]
    fn not_found_and_unsupported_errors_map_to_unsupported() {
        assert_eq!(
            failure_from_error_name(Some("NotFoundError")),
            CaptureFailure::Unsupported
        );
        assert_eq!(
            failure_from_error_name(Some("OverconstrainedError")),
            CaptureFailure::Unsupported
        );
        assert_eq!(
            failure_from_error_name(Some("NotSupportedError")),
            CaptureFailure::Unsupported
        );
        assert_eq!(
            failure_from_error_name(Some("SecurityError")),
            CaptureFailure::Unsupported
        );
    }

    #[test]
    fn unknown_or_missing_name_is_generic_failure() {
        assert_eq!(
            failure_from_error_name(None),
            CaptureFailure::Other(CAPTURE_FAILED.to_string())
        );
        assert_eq!(
            failure_from_error_name(Some("AbortError")),
            CaptureFailure::Other(CAPTURE_FAILED.to_string())
        );
    }

    #[test]
    fn thumbnail_from_frame_rejects_zero_source_size() {
        let rgba = vec![0u8; 4 * 4 * 4];
        assert_eq!(
            thumbnail_from_frame(0, 480, 4, 4, &rgba),
            Err(CaptureFailure::Other(CAPTURE_FAILED.to_string()))
        );
        assert_eq!(
            thumbnail_from_frame(640, 0, 4, 4, &rgba),
            Err(CaptureFailure::Other(CAPTURE_FAILED.to_string()))
        );
    }

    #[test]
    fn thumbnail_from_frame_rejects_mismatched_rgba_length() {
        let rgba = vec![0u8; 4 * 4 * 4 - 1];
        assert_eq!(
            thumbnail_from_frame(640, 480, 4, 4, &rgba),
            Err(CaptureFailure::Other(CAPTURE_FAILED.to_string()))
        );
    }

    #[test]
    fn thumbnail_from_frame_overrides_size_with_source_dimensions() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let thumbnail = thumbnail_from_frame(1280, 720, 4, 4, &rgba).unwrap();
        assert_eq!(thumbnail.width, 1280);
        assert_eq!(thumbnail.height, 720);
        assert_eq!(
            (thumbnail.pixels.width(), thumbnail.pixels.height()),
            (4, 4)
        );
    }
}
