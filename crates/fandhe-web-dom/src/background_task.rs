//! Phase 11（端末機能の拡充ループ、親 #1042）型サンプル「バックグラウンド処理」の
//! Web 経路（Issue #1047）。
//!
//! `fandhe_core::screens::background_task`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、Background
//! Synchronization API（Service Worker の `sync` イベント、`web/
//! background-task-sw.js`）で「処理を 1 件予約する → ブラウザの都合で最大 1 回
//! 発火する → 実行時刻を保存する → 次に画面を開いたとき表示する」を実現する。
//!
//! Web は `fandhe-device` の `BackgroundTask` trait を通らない（`notification.rs`/
//! `share.rs` と同じ理由、`fandhe_device::background_task` モジュール doc の
//! schedule は同期 API だが Web は Promise ベースで非同期のため実装できない）。
//! 保存形式（[`fandhe_device::background_task::RECORD_KEY`]/`encode_record`/
//! `decode_record`）だけは SSOT として共有する。
//!
//! # 保存先（IndexedDB、ページと Service Worker の両方が読む）
//!
//! `localStorage` は Service Worker から読めないため使わない。DB 名 [`web::DB_NAME`]、
//! object store [`web::STORE_NAME`]（out-of-line key）に 2 つの key を持つ。
//!
//! - [`web::PENDING_KEY`]: 予約中の印（値は予約した時刻、ms）
//! - [`fandhe_device::background_task::RECORD_KEY`]: 最後の実行記録
//!   （`encode_record` と同じ JSON 文字列）
//!
//! sync tag は固定の [`web::SYNC_TAG`]。`web/background-task-sw.js` が `sync`
//! イベントで `PENDING_KEY` を確認し、あれば `RECORD_KEY` を書いて `PENDING_KEY` を
//! 削除する（1 つの readwrite トランザクション内、取り消しと競合しても直列化される）。
//!
//! # 取り消し・二重予約の意味
//!
//! 取り消しはブラウザ側の sync 登録を消す API が無いため、`PENDING_KEY` を削除する
//! だけで対応する（SW 側は `PENDING_KEY` が無ければ無視する、べき等）。二重予約は
//! 同じ tag で `register` し直すことでブラウザ側が 1 件に合流させる（置き換え契約）。
//!
//! # 予約状態の判定（[`schedule_state`]）
//!
//! オーナー承認済みの方針（10/2、Issue #1047）で契約を単純化した。正確さは保証しない
//! 参考値として、`PENDING_KEY`（自前の「予約した」印）があれば `Scheduled`、無く実行
//! 記録があれば `Ran`、どちらも無ければ `NotScheduled` とする。sync tag
//! （`getTags()`）は取り消し後も残るため判定に使わない（PR #1051 codex 指摘）。
//! tag 消失を何らかの確定条件（連続観測回数等）で「放棄」と
//! 判定して `PENDING_KEY` を自動で片付ける仕組みは置かない（codex 指摘: 同期実行中の
//! 一時的な tag 消失を放棄と誤認し予約情報を消す／片付け失敗を成功扱いする／片付け
//! 失敗でポーリングが止まる／再予約が放棄カウンタを引き継ぐ、いずれも仕組み自体の
//! 削除で解消する）。`PENDING_KEY` の削除は [`web::do_cancel`] のベストエフォートの
//! 取り消し操作のみが行う。
//!
//! # 非対応判定
//!
//! secure context 外・`serviceWorker`/`SyncManager`/`indexedDB` のいずれかが無い
//! 環境では何も呼ばず `Unsupported` にする（[`web::is_supported`]、境界で実在を
//! 確認する、security.md A03）。
//!
//! # register の戻り値を使う（`ready()` は使わない）
//!
//! `do_schedule` は `container.register(SW_URL)` が返す
//! `ServiceWorkerRegistration` をそのまま保持し、それへ `sync.register()` する。
//! `container.ready()` は「現在ページを制御している」登録を返すため、既存の
//! 広い scope の Service Worker がページを制御していると、今回登録した
//! `background-task-sw.js` とは別の登録が返ってしまい、その `sync` ハンドラが
//! 一生発火しない（codex 指摘: PR #1051）。`register()` の Promise は
//! アクティベーションを待たずに登録が作られた時点で解決するため、`ready()` の
//! ような「インストールが失敗すると永久に解決しない」期限も不要になる。
//!
//! # register の scope を専用にする（既存 Service Worker を置き換えない）
//!
//! 既定（scope オプション省略）では `register(SW_URL)` が `SW_URL` と同じ
//! ディレクトリ（= ページと同じ scope）へ登録される。同じディレクトリで既存の
//! Service Worker（`fetch` 等のハンドラを持つ）が既にその scope を持っていると、
//! 新しいスクリプトがそれを置き換え、既存ハンドラが失われてしまう（codex 指摘:
//! PR #1051）。`do_schedule` は [`web::SW_SCOPE`]（ページ・既存 SW の既定 scope とは
//! 重ならない専用のサブパス）を指定して登録することで、既存の登録と共存する。
//! `sync.register()` はページを制御しているかに関係なく登録オブジェクトへ呼べる
//! ため、専用 scope で登録してもこの画面の用途は成立する（上記「register の
//! 戻り値を使う」と同じ理由）。
//!
//! 一方で `sync.register()` は `registration.active` が `null` の間
//! `InvalidStateError` で reject する（Bugbot 指摘: PR #1051）。`do_schedule` は
//! `sync.register()` の前に `active` になるまで短い間隔でポーリングし
//! （[`web::wait_until_active`]、有限回数で諦める）、`container.ready()` を使わずに
//! 同じ登録の activation だけを待つ。
//!
//! # ポーリング（[`web::background_task_effect`]）
//!
//! `fandhe-core` の `Screen<S>` は再構築されないため、予約状態・最後の実行結果は
//! effect の定期観測で画面へ反映する（`speech_recognition.rs`/`notification.rs` と
//! 違い結果が非同期に届き得るため）。Web の `WebEffect` は再描画のたびに呼ばれるが
//! `Applier::apply` 自体がさらに再描画を起こすため、素朴に「毎回ポーリング」すると
//! 自己再入になる。`phase == Idle` かつ飛行中でないときのみポーリングを始め
//! （[`web::POLLING`] でマウント単位に直列化、`clipboard.rs::IN_FLIGHT` と同型）、
//! 差分が無ければ何もしない。`Scheduled` の間だけ 1 秒後に `apply(|_| {})` で
//! 再描画し、次のポーリングへつなげる（送信を省略する分岐でも pull 型は次回を予約する
//! 契約、`device-bridge-contracts.md`「4. ポーリング・再描画」）。IDB の読み取り
//! 自体が失敗した回（[`web::PollOutcome::ReadFailed`]）は「予約状態が
//! 変わった」という情報を持たないため表示を一切書き換えない（cursor 指摘: 失敗時に
//! 固定文言へ上書きすると、直前の正しい表示 — 例えば実行済みの結果 — が「再試行
//! します」と言いながら実際には再試行しない終端の失敗表示に化けてしまう）。
//! 直前の表示が何であったかに関係なく次回ポーリングを予約する（codex 指摘:
//! 読み取り失敗を「未予約確定」へ畳むと `PENDING_KEY` が残っていても
//! `NotScheduled` と誤表示される。さらに初回表示（直前状態が `Unknown`）で失敗すると
//! `Scheduled` 限定の再試行では次回が予約されず固まる。失敗状態のまま再描画し続け
//! ないが再試行は毎回予約する、`device-bridge-contracts.md`「4.」）。
//!
//!
//! 直後の 1 回分だけ差分の有無にかかわらず冗長なポーリングが起き得る
//! （`apply_snapshot` の再描画が effect を再入させ、`POLLING` が既に空のため
//! もう 1 回ポーリングしてしまう）。2 回目は 1 回目と同じ値になり差分が無いため
//! `apply` は呼ばれず、そこで収束する（無限再入にはならない）。
//! `ponytail:` 専用のガード変数で初回だけ抑止する手もあるが、収束が保証されている
//! ため最小実装では見送る。
//!
//! # ポーリング開始後に操作が完了した場合の世代照合（codex 指摘: PR #1051）
//!
//! `do_poll` は非同期（IDB アクセスを挟む）ため、ポーリング開始後に利用者が
//! 「予約を取り消す」等を押して操作が完了していると、ポーリングが読んだ値は
//! 既に古い。古い値で [`fandhe_core::screens::background_task::apply_snapshot`]
//! を呼ぶと、例えば取り消し完了後の `NotScheduled` を取り消し前の `Scheduled` へ
//! 戻してしまう。`background_task_effect` はポーリング開始時点の
//! `state.generation` を控え、結果を反映する `applier.apply` のクロージャ内
//! （状態に触れる時点、他の操作がその間に割り込んでいないかを再確認できる
//! 唯一の場所）で、現在の世代が一致し `phase == Idle`（操作が進行中でない）
//! ことを確認してから [`apply_snapshot`] を呼ぶ。合わなければ結果を捨てる
//! （次回ポーリングの予約は従来どおり行う）。
//!
//! # 秘密値・ログ（security.md A09）
//!
//! 例外オブジェクトの `message` は console にも画面にも出さない（`name` のみ見る）。

use fandhe_core::screens::background_task::{LastRunView, RunResultView, ScheduleView};
use fandhe_device::background_task::{RunRecord, RunResult};
use fandhe_device::DeviceError;

/// `DeviceError` を画面へ渡す文言へ変換する。`Unsupported` は固定文言へ畳み、
/// それ以外は `Display` の文言のみを転送する（`fandhe-egui-proxy::device::
/// background_task::to_message` と同じ文言で揃える、security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_message(e: DeviceError) -> String {
    match e {
        DeviceError::Unsupported(_) => "この環境ではバックグラウンド処理を使えません".to_string(),
        other => other.to_string(),
    }
}

/// `sync.register()`/`serviceWorker.register()` が投げた例外の `name` を
/// [`DeviceError`] へ写す純関数（ブラウザ API を呼ばないため native からも
/// テストできる、モジュール doc「結果の写像表」参照）。`message` は受け取らない
/// （security.md A09）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn error_from_name(name: Option<&str>) -> DeviceError {
    match name {
        Some("SecurityError") => DeviceError::Unsupported("background task"),
        Some("NotAllowedError") => DeviceError::PermissionDenied("background sync"),
        _ => DeviceError::Internal("background task"),
    }
}

/// `Scheduled`/`NotScheduled`/`Ran` の判定純関数（モジュール doc「予約状態の判定」
/// 参照）。ブラウザ API を呼ばないため native からもテストできる。参考値。
/// 予約の印（`PENDING_KEY`）があれば `Scheduled`。sync tag は取り消し後も残るため
/// 判定に使わない（PR #1051 codex 指摘）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn schedule_state(has_pending: bool, has_record: bool) -> ScheduleView {
    if has_pending {
        return ScheduleView::Scheduled;
    }
    if has_record {
        return ScheduleView::Ran;
    }
    ScheduleView::NotScheduled
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn to_run_result_view(result: RunResult) -> RunResultView {
    match result {
        RunResult::Completed => RunResultView::Completed,
        RunResult::Expired => RunResultView::Expired,
    }
}

/// 最後の実行記録の読み取り結果を画面表示用の [`LastRunView`] へ写す
/// （`fandhe-egui-proxy::device::background_task::to_last_run_view` と同じ 3 値の
/// 区別。壊れた値を「未実行」として成功へ畳まない、`device-bridge-contracts.md`
/// 「6. 嘘の成功」）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn to_last_run_view(last_run: Result<Option<RunRecord>, DeviceError>) -> LastRunView {
    match last_run {
        Ok(None) => LastRunView::NeverRan,
        Ok(Some(record)) => LastRunView::Ran {
            at_ms: record.ran_at_unix_ms,
            result: to_run_result_view(record.result),
        },
        Err(e) => LastRunView::ReadFailed(to_message(e)),
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashSet;

    use fandhe_core::screens::background_task::{
        apply_snapshot, finish_action, mark_in_flight, take_request, Action, BackgroundTaskState,
        Phase, ScheduleView,
    };
    use fandhe_device::background_task::{decode_record, RunRecord, RECORD_KEY};
    use fandhe_device::DeviceError;
    use js_sys::{Function, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{
        IdbDatabase, IdbObjectStore, IdbRequest, IdbTransaction, IdbTransactionMode,
        RegistrationOptions, ServiceWorkerRegistration, Window,
    };

    use super::{error_from_name, schedule_state, to_last_run_view, to_message};
    use crate::mount::Applier;

    /// IndexedDB の DB 名（モジュール doc「保存先」参照）。
    pub(super) const DB_NAME: &str = "fandhe-background-task";
    /// object store 名（out-of-line key）。
    const STORE_NAME: &str = "kv";
    /// 予約中の印の key（値は予約した時刻、ms）。
    pub(super) const PENDING_KEY: &str = "background-task.pending";
    /// Background Sync の tag（固定）。
    const SYNC_TAG: &str = "fandhe-background-task";
    /// Service Worker 本体の URL（配布先ディレクトリ直下、`build-wasm-dom.sh` が
    /// `web/background-task-sw.js` をここへ cp する）。
    const SW_URL: &str = "background-task-sw.js";
    /// 登録 scope（モジュール doc「register の scope を専用にする」参照）。
    /// ページ・既存 Service Worker の既定 scope（配布先ディレクトリ直下）とは
    /// 重ならない専用のサブパスとし、既存登録を置き換えない。実在するディレクトリ
    /// である必要はない（scope は URL 前方一致の境界であり、配信される実体を
    /// 要求しない）。
    const SW_SCOPE: &str = "fandhe-background-task-scope/";
    /// `Scheduled` の間の次回ポーリング間隔。
    const POLL_INTERVAL_MS: i32 = 1_000;

    thread_local! {
        /// ポーリングが飛行中のマウント ID の集合（`clipboard.rs::IN_FLIGHT` と同型。
        /// マウント単位で持つ理由も同じ）。
        static POLLING: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
    }

    fn window() -> Option<Window> {
        web_sys::window()
    }

    /// `target[name]` が呼び出し可能な関数として実在するかを確認する
    /// （`speech_recognition.rs::has_function` と同型、security.md A03）。
    fn has_function(target: &JsValue, name: &str) -> bool {
        Reflect::get(target, &JsValue::from_str(name))
            .map(|value| value.is_function())
            .unwrap_or(false)
    }

    /// `target` に `name` プロパティが存在するかを確認する（関数以外のプロパティ
    /// 存在確認、`navigator.serviceWorker` 用）。
    fn has_property(target: &JsValue, name: &str) -> bool {
        Reflect::has(target, &JsValue::from_str(name)).unwrap_or(false)
    }

    /// 例外オブジェクトの `name` を読む（`message` は読まない、security.md A09）。
    fn js_error_name(e: &JsValue) -> Option<String> {
        Reflect::get(e, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
    }

    /// 非対応判定（モジュール doc 参照）。境界で実在を確認してから呼ぶ
    /// （security.md A03）。
    fn is_supported(window: &Window) -> bool {
        if !window.is_secure_context() {
            return false;
        }
        let navigator_value: JsValue = window.navigator().into();
        if !has_property(&navigator_value, "serviceWorker") {
            return false;
        }
        let global: JsValue = js_sys::global().into();
        if !has_function(&global, "SyncManager") {
            return false;
        }
        // API の有無だけを見る。`window.indexed_db()` は利用中にサイト設定等で保存先が
        // 使えなくなっても失敗するため、ここで呼ぶと「読めない」を「非対応 = 未予約・
        // 未実行」へ畳んでしまう。実際の利用失敗は `open_db` の失敗として読み取り失敗
        // （`PollOutcome::ReadFailed`）で扱う（PR #1051 codex 指摘）。
        has_property(&global, "indexedDB")
    }

    /// 1 回しか発火しないブラウザイベントハンドラ用の `Closure` の型。
    type EventClosure = Closure<dyn FnMut(JsValue)>;

    /// `IdbRequest`（`get`/`put`/`delete`/`open` 共通の非同期結果）を 1 回だけ解決する
    /// `Promise` へ変換する。`onsuccess`/`onerror` は IDB 仕様上どちらか一度だけ発火
    /// するが、`Closure::once_into_js` は呼ばれて初めて内部の `Closure` を解放する
    /// ため、発火しなかった側は `request` から参照を外しても Rust 側のメモリが
    /// 解放されない（Bugbot 指摘: PR #1051。`do_poll` は `Scheduled` の間毎秒
    /// [`read_snapshot`] 経由で複数発行するため放置すると蓄積する）。`Closure::wrap`
    /// で両方の `Closure` を呼び出し元へ返し、`Promise` が解決するまで保持させる
    /// ことで、発火・未発火にかかわらず関数を抜けるときに確実に drop される
    /// （`forget()` しない、`mic_record.rs` と同じ方針）。
    fn request_promise(request: IdbRequest) -> (Promise, Vec<EventClosure>) {
        let mut closures: Vec<EventClosure> = Vec::new();
        let promise = {
            let request = request.clone();
            let mut executor = |resolve: Function, reject: Function| {
                let success_req = request.clone();
                let clear_on_success = request.clone();
                let onsuccess = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_on_success.set_onerror(None);
                    let _ = resolve.call1(
                        &JsValue::NULL,
                        &success_req.result().unwrap_or(JsValue::UNDEFINED),
                    );
                }) as Box<dyn FnMut(JsValue)>);
                request.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
                closures.push(onsuccess);

                let clear_on_error = request.clone();
                let onerror = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_on_error.set_onsuccess(None);
                    let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("idb request failed"));
                }) as Box<dyn FnMut(JsValue)>);
                request.set_onerror(Some(onerror.as_ref().unchecked_ref()));
                closures.push(onerror);
            };
            Promise::new(&mut executor)
        };
        (promise, closures)
    }

    /// 開いた IndexedDB 接続を、成功・失敗のどの経路で抜けても閉じる
    /// （`do_poll` は約 1 秒ごとに開くため、閉じないと接続が溜まり後続の更新を
    /// 妨げる。PR #1051 codex 指摘）。
    struct CloseOnDrop(IdbDatabase);

    impl std::ops::Deref for CloseOnDrop {
        type Target = IdbDatabase;
        fn deref(&self) -> &IdbDatabase {
            &self.0
        }
    }

    impl Drop for CloseOnDrop {
        fn drop(&mut self) {
            self.0.close();
        }
    }

    /// DB を開く。初回（バージョン無し → 1）は `onupgradeneeded` で [`STORE_NAME`]
    /// を作る（ページ側・`web/background-task-sw.js` の両方が同じ手順で作るため、
    /// どちらが先に開いても整合する）。
    async fn open_db(window: &Window) -> Result<IdbDatabase, JsValue> {
        let factory = window
            .indexed_db()
            .ok()
            .flatten()
            .ok_or_else(|| JsValue::from_str("indexedDB unavailable"))?;
        let open_req = factory
            .open_with_u32(DB_NAME, 1)
            .map_err(|_| JsValue::from_str("idb open failed"))?;

        let mut closures: Vec<EventClosure> = Vec::new();
        let promise = {
            let open_req = open_req.clone();
            let mut executor = |resolve: Function, reject: Function| {
                // `onupgradeneeded` は発火してもしなくても `onsuccess`/`onerror` の
                // どちらかは必ず発火する。未発火ハンドラをその時点で外す（`request_promise`
                // と同じ理由）。発火・未発火にかかわらず 3 つの `Closure` すべてを
                // `closures` へ積み、呼び出し元が `Promise` の解決後に drop する
                // （Bugbot 指摘: PR #1051、`request_promise` のコメント参照）。
                let clear_upgrade_on_success = open_req.clone();
                let clear_upgrade_on_error = open_req.clone();
                let upgrade_req = open_req.clone();
                let onupgradeneeded = Closure::wrap(Box::new(move |_evt: JsValue| {
                    if let Ok(result) = upgrade_req.result() {
                        let db: IdbDatabase = result.unchecked_into();
                        if !db.object_store_names().contains(STORE_NAME) {
                            let _ = db.create_object_store(STORE_NAME);
                        }
                    }
                }) as Box<dyn FnMut(JsValue)>);
                open_req.set_onupgradeneeded(Some(onupgradeneeded.as_ref().unchecked_ref()));
                closures.push(onupgradeneeded);

                let success_req = open_req.clone();
                let clear_success_on_error = open_req.clone();
                let onsuccess = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_upgrade_on_success.set_onupgradeneeded(None);
                    clear_success_on_error.set_onerror(None);
                    let _ = resolve.call1(
                        &JsValue::NULL,
                        &success_req.result().unwrap_or(JsValue::UNDEFINED),
                    );
                }) as Box<dyn FnMut(JsValue)>);
                open_req.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
                closures.push(onsuccess);

                let clear_error_on_error = open_req.clone();
                let onerror = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_upgrade_on_error.set_onupgradeneeded(None);
                    clear_error_on_error.set_onsuccess(None);
                    let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("idb open failed"));
                }) as Box<dyn FnMut(JsValue)>);
                open_req.set_onerror(Some(onerror.as_ref().unchecked_ref()));
                closures.push(onerror);
            };
            Promise::new(&mut executor)
        };
        let value = JsFuture::from(promise).await?;
        drop(closures);
        Ok(value.unchecked_into())
    }

    /// トランザクションと、そこから開いた object store の両方を返す（readwrite の
    /// 書き込み確定を待つには `IdbObjectStore` だけでは `IdbTransaction` を取り出せ
    /// ないため）。
    fn store(
        db: &IdbDatabase,
        mode: IdbTransactionMode,
    ) -> Result<(IdbTransaction, IdbObjectStore), JsValue> {
        let tx = db.transaction_with_str_and_mode(STORE_NAME, mode)?;
        let os = tx.object_store(STORE_NAME)?;
        Ok((tx, os))
    }

    /// `IdbTransaction` の `complete`（全要求の書き込みが確定）を 1 回だけ解決する
    /// `Promise` へ変換する。個々の `IdbRequest` の `onsuccess` だけを見て成功を
    /// 返すと、その後トランザクションが `abort`（クオータ超過・ブラウザ都合等）で
    /// 打ち切られても呼び出し元には書き込み成功が伝わってしまう（codex 指摘:
    /// PR #1051、`device-bridge-contracts.md`「6. 嘘の成功」）。`error` は `abort` に
    /// 先立って転送されてくるため両方を拒否として扱う。[`request_promise`] と同じ
    /// 理由で 3 つの `Closure` すべてを呼び出し元へ返し、`Promise` の解決後に
    /// drop させる（Bugbot 指摘: PR #1051）。
    fn transaction_promise(tx: IdbTransaction) -> (Promise, Vec<EventClosure>) {
        let mut closures: Vec<EventClosure> = Vec::new();
        let promise = {
            let tx = tx.clone();
            let mut executor = |resolve: Function, reject: Function| {
                let clear_on_complete = tx.clone();
                let oncomplete = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_on_complete.set_onerror(None);
                    clear_on_complete.set_onabort(None);
                    let _ = resolve.call0(&JsValue::NULL);
                }) as Box<dyn FnMut(JsValue)>);
                tx.set_oncomplete(Some(oncomplete.as_ref().unchecked_ref()));
                closures.push(oncomplete);

                let clear_on_error = tx.clone();
                let reject_on_error = reject.clone();
                let onerror = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_on_error.set_oncomplete(None);
                    clear_on_error.set_onabort(None);
                    let _ = reject_on_error
                        .call1(&JsValue::NULL, &JsValue::from_str("idb transaction failed"));
                }) as Box<dyn FnMut(JsValue)>);
                tx.set_onerror(Some(onerror.as_ref().unchecked_ref()));
                closures.push(onerror);

                let clear_on_abort = tx.clone();
                let onabort = Closure::wrap(Box::new(move |_evt: JsValue| {
                    clear_on_abort.set_oncomplete(None);
                    clear_on_abort.set_onerror(None);
                    let _ = reject.call1(
                        &JsValue::NULL,
                        &JsValue::from_str("idb transaction aborted"),
                    );
                }) as Box<dyn FnMut(JsValue)>);
                tx.set_onabort(Some(onabort.as_ref().unchecked_ref()));
                closures.push(onabort);
            };
            Promise::new(&mut executor)
        };
        (promise, closures)
    }

    /// `PENDING_KEY`・`RECORD_KEY` を 1 回の readonly トランザクションで読む
    /// （モジュール doc「保存先」参照）。IDB 自体の失敗は外側の `Result` の `Err`、
    /// 実行記録の形式不正は内側の `Result` の `Err`（`ReadFailed` へ区別して畳む、
    /// `device-bridge-contracts.md`「6. 嘘の成功」）。
    async fn read_snapshot(
        db: &IdbDatabase,
    ) -> Result<(Option<u64>, Result<Option<RunRecord>, DeviceError>), JsValue> {
        let (_tx, s) = store(db, IdbTransactionMode::Readonly)?;
        let pending_req = s.get(&JsValue::from_str(PENDING_KEY))?;
        let record_req = s.get(&JsValue::from_str(RECORD_KEY))?;
        // `_closures` は `Promise` の解決（await の完了）まで保持し、関数末尾で
        // まとめて drop させる（`request_promise` のコメント参照）。
        let (pending_promise, _pending_closures) = request_promise(pending_req);
        let (record_promise, _record_closures) = request_promise(record_req);
        // 片方が失敗しても両方の完了を待ってから closure を drop する（先に `?` で抜ける
        // と、まだ完了していない要求のハンドラが解放済みの closure を呼び得る。PR #1051
        // codex 指摘）。
        let pending_res = JsFuture::from(pending_promise).await;
        let record_res = JsFuture::from(record_promise).await;
        let pending_val = pending_res?;
        let record_val = record_res?;
        let pending = pending_val.as_f64().map(|f| f as u64);
        // `undefined`（key が無い、正常な「未実行」）と、値はあるが文字列でない
        // （形式不正）を区別する（codex 指摘: 両方を `as_string() == None` へ畳むと
        // 形式不正が `NeverRan` に誤表示される、`device-bridge-contracts.md`
        // 「6. 嘘の成功」）。
        let last_run = if record_val.is_undefined() {
            Ok(None)
        } else {
            match record_val.as_string() {
                Some(text) => decode_record(text.as_bytes()).map(Some),
                None => Err(DeviceError::Internal("background task record")),
            }
        };
        Ok((pending, last_run))
    }

    /// `PENDING_KEY` を書く。要求の `onsuccess` だけでなく [`transaction_promise`]
    /// でトランザクションの `complete` まで待ってから返す（codex 指摘: PR #1051。
    /// `complete` 前に `abort` された場合は書き込みが確定しておらず、ここで `Ok`
    /// を返すと予約ボタンが成功表示のまま `PENDING_KEY` は残らない「嘘の成功」に
    /// なる、`device-bridge-contracts.md`「6.」）。
    async fn write_pending(db: &IdbDatabase, token: f64) -> Result<(), JsValue> {
        let (tx, s) = store(db, IdbTransactionMode::Readwrite)?;
        s.put_with_key(&JsValue::from_f64(token), &JsValue::from_str(PENDING_KEY))?;
        let (promise, _closures) = transaction_promise(tx);
        JsFuture::from(promise).await?;
        Ok(())
    }

    /// `PENDING_KEY` を消す。[`write_pending`] と同じ理由でトランザクションの
    /// `complete` まで待つ（`abort` された場合、取り消しボタンが成功表示でも印が
    /// 残ってしまう）。
    async fn delete_pending(db: &IdbDatabase) -> Result<(), JsValue> {
        let (tx, s) = store(db, IdbTransactionMode::Readwrite)?;
        s.delete(&JsValue::from_str(PENDING_KEY))?;
        let (promise, _closures) = transaction_promise(tx);
        JsFuture::from(promise).await?;
        Ok(())
    }

    /// `PENDING_KEY` が `token`（自分が [`write_pending`] で書いた値）のときだけ消す。
    /// 同一オリジンの別タブが同時に予約して印を書き換えていたら触らない（予約失敗の
    /// 後片付けで他タブの予約を消さない。PR #1051 codex 指摘）。読み出しと削除は 1 本の
    /// readwrite トランザクション内で行い（`onsuccess` 内で削除を積む）、IndexedDB が
    /// タブ間で直列化するため比較と削除の間に割り込まれない。`get` の `onsuccess`
    /// も [`Closure::wrap`] で保持し、関数末尾まで drop しない（`get` がエラーになり
    /// `onsuccess` が発火しなかった場合でも解放する、Bugbot 指摘: PR #1051）。
    async fn delete_pending_if(db: &IdbDatabase, token: f64) -> Result<(), JsValue> {
        let (tx, s) = store(db, IdbTransactionMode::Readwrite)?;
        let req = s.get(&JsValue::from_str(PENDING_KEY))?;
        let req_for_cb = req.clone();
        let onsuccess = Closure::wrap(Box::new(move |_evt: JsValue| {
            let current = req_for_cb.result().ok().and_then(|v| v.as_f64());
            if current == Some(token) {
                let _ = s.delete(&JsValue::from_str(PENDING_KEY));
            }
        }) as Box<dyn FnMut(JsValue)>);
        req.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
        let (promise, _tx_closures) = transaction_promise(tx);
        JsFuture::from(promise).await?;
        drop(onsuccess);
        Ok(())
    }

    /// 予約失敗時の後片付け（自分の予約の印を消す）を行い、返すエラーを決める。
    /// 後片付け自体が失敗したら、印が残って後から実行され得るため元のエラーでは
    /// なく「後片付けに失敗した」ことを返す（失敗確定と表示しない。PR #1051 codex 指摘）。
    async fn cleanup_then(db: &IdbDatabase, token: f64, original: DeviceError) -> DeviceError {
        match delete_pending_if(db, token).await {
            Ok(()) => original,
            Err(_) => DeviceError::Internal("background task schedule cleanup"),
        }
    }

    /// `registration.sync.register(tag)` を呼ぶ。`SyncManager` が無い（非対応
    /// ブラウザが `register()` の後に判明する稀な経路）場合は `Unsupported`。
    async fn sync_register(
        registration: &ServiceWorkerRegistration,
        tag: &str,
    ) -> Result<(), DeviceError> {
        let reg_value: JsValue = registration.clone().into();
        let sync_value = Reflect::get(&reg_value, &JsValue::from_str("sync"))
            .map_err(|_| DeviceError::Internal("background task"))?;
        if !has_function(&sync_value, "register") {
            return Err(DeviceError::Unsupported("background task"));
        }
        let register_fn: Function = Reflect::get(&sync_value, &JsValue::from_str("register"))
            .map_err(|_| DeviceError::Internal("background task"))?
            .unchecked_into();
        let promise_value = register_fn
            .call1(&sync_value, &JsValue::from_str(tag))
            .map_err(|e| error_from_name(js_error_name(&e).as_deref()))?;
        JsFuture::from(Promise::from(promise_value))
            .await
            .map(|_| ())
            .map_err(|e| error_from_name(js_error_name(&e).as_deref()))
    }

    /// `window.set_timeout` を 1 回分の `Promise` にする（[`wait_until_active`] の
    /// ポーリング間隔待ちに使う。`schedule_next_poll` の `set_timeout` と同じ API
    /// を Promise 化しただけ）。
    async fn sleep(window: &Window, ms: i32) {
        let promise = Promise::new(&mut |resolve, _reject| {
            let closure: Function = Closure::once_into_js(move || {
                let _ = resolve.call0(&JsValue::NULL);
            })
            .unchecked_into();
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&closure, ms);
        });
        let _ = JsFuture::from(promise).await;
    }

    /// [`do_schedule`] が渡す `registration` が `active` になるまで待つ
    /// （モジュール doc「register の戻り値を使う」参照）。`register()` の
    /// `Promise` はアクティベーションを待たずに解決するため、直後に
    /// `sync.register()` を呼ぶと `SyncManager` 仕様上 `active` が `null` の間は
    /// `InvalidStateError` で reject される（Bugbot 指摘: PR #1051）。
    /// `container.ready()` は別の登録を返し得るため使わず、同じ `registration` を
    /// 短い間隔でポーリングする（有限回数で諦め `Internal` を返す — 無限待ちで
    /// `do_schedule` が終わらなくならないようにする）。
    pub(crate) async fn wait_until_active(
        window: &Window,
        registration: &ServiceWorkerRegistration,
    ) -> Result<(), DeviceError> {
        const ACTIVATION_POLL_INTERVAL_MS: i32 = 50;
        const ACTIVATION_MAX_ATTEMPTS: u32 = 100; // 最大 5 秒

        if registration.active().is_some() {
            return Ok(());
        }
        for _ in 0..ACTIVATION_MAX_ATTEMPTS {
            sleep(window, ACTIVATION_POLL_INTERVAL_MS).await;
            if registration.active().is_some() {
                return Ok(());
            }
        }
        Err(DeviceError::Internal("background task"))
    }

    /// 「予約する」の OS 呼び出し（モジュール doc「取り消し・二重予約の意味」参照）。
    /// `register()` が返す登録をそのまま使う（`container.ready()` は別の登録を
    /// 返し得るため使わない、モジュール doc「register の戻り値を使う」参照）。
    /// `sync.register()` の前に [`wait_until_active`] で worker の activation を
    /// 待つ（Bugbot 指摘: active 前に呼ぶと reject される）。register が失敗したら
    /// `PENDING_KEY` を削除して戻す（契約表「9. 失敗時のリソース解放」）。
    async fn do_schedule(window: Window) -> Result<(), DeviceError> {
        let container = window.navigator().service_worker();
        let options = RegistrationOptions::new();
        options.set_scope(SW_SCOPE);
        let registration: ServiceWorkerRegistration =
            JsFuture::from(container.register_with_options(SW_URL, &options))
                .await
                .map_err(|e| error_from_name(js_error_name(&e).as_deref()))?
                .unchecked_into();

        let db = CloseOnDrop(
            open_db(&window)
                .await
                .map_err(|_| DeviceError::Internal("background task"))?,
        );
        // 予約ごとの印。ミリ秒に乱数の端数を足し、同じミリ秒に別タブが予約しても
        // 区別できるようにする（`read_snapshot` は整数部だけを読む）。
        let token = js_sys::Date::now().floor() + js_sys::Math::random();
        write_pending(&db, token)
            .await
            .map_err(|_| DeviceError::Internal("background task"))?;

        if let Err(e) = wait_until_active(&window, &registration).await {
            return Err(cleanup_then(&db, token, e).await);
        }

        match sync_register(&registration, SYNC_TAG).await {
            Ok(()) => Ok(()),
            Err(e) => Err(cleanup_then(&db, token, e).await),
        }
    }

    /// 「予約を取り消す」。非対応環境では `Unsupported`、対応環境では
    /// `PENDING_KEY` を削除してべき等に戻す（未予約での取り消しも契約上 `Ok`、
    /// `fandhe_device::background_task` モジュール doc 参照）。Background Sync に
    /// 取り消し API は無いためベストエフォート（オーナー承認済みの方針、10/2）。
    /// 直前に始まった実行は記録として残り得る。
    async fn do_cancel(window: Window) -> Result<(), DeviceError> {
        if !is_supported(&window) {
            return Err(DeviceError::Unsupported("background task"));
        }
        let db = CloseOnDrop(
            open_db(&window)
                .await
                .map_err(|_| DeviceError::Internal("background task"))?,
        );
        delete_pending(&db)
            .await
            .map_err(|_| DeviceError::Internal("background task"))?;
        Ok(())
    }

    /// ポーリング 1 回分の結果（モジュール doc「ポーリング」参照）。読み取りの失敗を
    /// [`ReadFailed`](PollOutcome::ReadFailed) として区別し、呼び出し元が表示を
    /// 書き換えずに済ませられるようにする（codex 指摘:
    /// `device-bridge-contracts.md`「4. ポーリング・再描画」「6. 嘘の成功」）。
    enum PollOutcome {
        /// 読み取りに成功した（非対応環境を含む）。第 3 要素は `PENDING_KEY` の
        /// 有無（呼び出し元はこれが消えるまで観測を続ける）。
        Observed(ScheduleView, super::LastRunView, bool),
        /// IndexedDB の読み取りに失敗した（一時的な障害として扱い、呼び出し元が
        /// 次回ポーリングで再観測する）。
        ReadFailed,
    }

    /// ポーリング 1 回分。非対応環境は呼ばずに固定のスナップショットへ畳む
    /// （`ponytail:` 非対応環境の表示は「未予約・未実行」に留める簡略化。対応表の
    /// 非対応ブラウザ一覧で案内済みのため、より丁寧な `Unsupported` 表示が要るなら
    /// `LastRunView::ReadFailed` 経由で足す）。IDB の読み取り自体が失敗した回は
    /// [`PollOutcome::ReadFailed`] を返す。sync tag（`getTags()`）は見ない
    /// （取り消し後も tag は残るため、tag で `Scheduled` を判定すると取り消しが
    /// 画面に反映されない。PR #1051 codex 指摘）。
    async fn do_poll(window: &Window) -> PollOutcome {
        if !is_supported(window) {
            return PollOutcome::Observed(
                ScheduleView::NotScheduled,
                super::LastRunView::NeverRan,
                false,
            );
        }
        let Ok(db) = open_db(window).await.map(CloseOnDrop) else {
            return PollOutcome::ReadFailed;
        };
        let Ok((pending, last_run_result)) = read_snapshot(&db).await else {
            return PollOutcome::ReadFailed;
        };
        let has_record = matches!(&last_run_result, Ok(Some(_)));
        let has_pending = pending.is_some();

        let view = schedule_state(has_pending, has_record);
        PollOutcome::Observed(view, to_last_run_view(last_run_result), has_pending)
    }

    /// `Scheduled` の間、1 秒後に `apply(|_| {}) ` で再描画し、次のポーリングへ
    /// つなげる（モジュール doc「ポーリング」参照）。
    fn schedule_next_poll(applier: &Applier<BackgroundTaskState>) {
        let Some(window) = window() else {
            return;
        };
        let applier = applier.clone();
        let closure: Function = Closure::once_into_js(move || {
            applier.apply(|_s| {});
        })
        .unchecked_into();
        let _ = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(&closure, POLL_INTERVAL_MS);
    }

    /// `mount_with_effect` へ渡す effect。`background_task` 画面のボタン要求・
    /// ポーリングを処理する（モジュール doc参照）。
    pub fn background_task_effect(
        state: &BackgroundTaskState,
        applier: &Applier<BackgroundTaskState>,
    ) {
        let mount_id = applier.mount_id();

        if let Some(request) = take_request(state) {
            applier.apply(mark_in_flight);
            let generation = request.generation;
            let action = request.action;
            let Some(window) = window() else {
                let applier = applier.clone();
                applier.apply(move |s| {
                    finish_action(
                        s,
                        generation,
                        action,
                        Err(to_message(DeviceError::Unsupported("background task"))),
                    )
                });
                return;
            };

            let applier = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = match action {
                    Action::Schedule => {
                        if !is_supported(&window) {
                            Err(DeviceError::Unsupported("background task"))
                        } else {
                            do_schedule(window).await
                        }
                    }
                    Action::Cancel => do_cancel(window).await,
                }
                .map_err(to_message);
                applier.apply(move |s| finish_action(s, generation, action, result));
            });
            return;
        }

        if state.phase != Phase::Idle {
            return;
        }
        // ポーリングはマウント単位で直列化する（モジュール doc「ポーリング」参照）。
        if !POLLING.with(|p| p.borrow_mut().insert(mount_id)) {
            return;
        }
        let Some(window) = window() else {
            POLLING.with(|p| {
                p.borrow_mut().remove(&mount_id);
            });
            return;
        };
        let prev_schedule_view = state.schedule_view;
        let prev_last_run_view = state.last_run_view.clone();
        // ポーリング開始時点の世代を控える（モジュール doc「ポーリング開始後に
        // 操作が完了した場合の世代照合」参照）。この後の `apply` クロージャ内で
        // 現在の世代と比較し、他の操作が割り込んでいたら結果を捨てる。
        let started_generation = state.generation;
        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let outcome = do_poll(&window).await;
            POLLING.with(|p| {
                p.borrow_mut().remove(&mount_id);
            });
            match outcome {
                PollOutcome::Observed(schedule_view, last_run_view, has_pending) => {
                    if schedule_view != prev_schedule_view || last_run_view != prev_last_run_view {
                        applier.apply(move |s| {
                            // 世代照合は状態に触れる時点（このクロージャ内）で行う。
                            // ポーリング開始後に新しい操作が始まっている（世代が
                            // 進んだ）、または操作がまだ進行中（`Idle` でない）なら、
                            // 読んだ値は既に古いため捨てる（codex 指摘: PR #1051）。
                            if s.generation == started_generation && s.phase == Phase::Idle {
                                apply_snapshot(s, schedule_view, last_run_view);
                            }
                        });
                    }
                    // `PENDING_KEY` が消える（実行済み・取り消し）まで観測を続ける。
                    if schedule_view == ScheduleView::Scheduled || has_pending {
                        schedule_next_poll(&applier);
                    }
                }
                PollOutcome::ReadFailed => {
                    // 保存先を読めないことを最後の実行欄に出す（未実行・古い値のまま
                    // 見せない。PR #1051 codex 指摘）。予約状態は情報が無いので触らず、
                    // 実際に毎回再試行するので「再試行中」と書いてよい。回復すれば次の
                    // `Observed` が差分として上書きする。世代・phase の照合は `Observed`
                    // と同じ。
                    // 表示が変わるときだけ反映する（毎回 apply すると再描画で effect が
                    // 即座に再入し、タイマーとは別にポーリングが増殖する。PR #1051 codex・
                    // Bugbot 指摘）。再試行は下の `schedule_next_poll` の 1 経路だけ。
                    let failed_view = super::LastRunView::ReadFailed(
                        "保存先を読み取れません（再試行中）".to_string(),
                    );
                    if prev_last_run_view != failed_view {
                        applier.apply(move |s| {
                            if s.generation == started_generation && s.phase == Phase::Idle {
                                s.last_run_view = failed_view;
                            }
                        });
                    }
                    // 直前の `prev_schedule_view` が
                    // `Scheduled` かどうかに関係なく次回ポーリングを予約する
                    // （codex 指摘: 初回表示（直前状態が `Unknown`）で IDB の
                    // open が一時失敗すると `Scheduled` 条件を満たさず
                    // 次回が予約されずポーリングが止まる。読み取り失敗は一時的な
                    // 障害として毎回再試行する契約にする、送信を省略する分岐でも
                    // 次回を予約する、`device-bridge-contracts.md`「4.」）。
                    schedule_next_poll(&applier);
                }
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::background_task_effect;
/// `background_download` が登録の activation 待ちを再利用する（Issue #393）。
#[cfg(target_arch = "wasm32")]
pub(crate) use web::wait_until_active;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_message_maps_unsupported_to_fixed_text() {
        assert_eq!(
            to_message(DeviceError::Unsupported("background task")),
            "この環境ではバックグラウンド処理を使えません"
        );
    }

    #[test]
    fn to_message_forwards_other_variants() {
        let e = DeviceError::PermissionDenied("background sync");
        assert_eq!(
            to_message(DeviceError::PermissionDenied("background sync")),
            e.to_string()
        );
    }

    #[test]
    fn error_from_name_maps_security_error_to_unsupported() {
        assert!(matches!(
            error_from_name(Some("SecurityError")),
            DeviceError::Unsupported("background task")
        ));
    }

    #[test]
    fn error_from_name_maps_not_allowed_to_permission_denied() {
        assert!(matches!(
            error_from_name(Some("NotAllowedError")),
            DeviceError::PermissionDenied("background sync")
        ));
    }

    #[test]
    fn error_from_name_maps_unknown_or_missing_to_internal() {
        assert!(matches!(
            error_from_name(Some("TypeError")),
            DeviceError::Internal("background task")
        ));
        assert!(matches!(
            error_from_name(None),
            DeviceError::Internal("background task")
        ));
    }

    #[test]
    fn schedule_state_is_scheduled_only_when_pending() {
        assert_eq!(schedule_state(true, false), ScheduleView::Scheduled);
        assert_eq!(schedule_state(true, true), ScheduleView::Scheduled);
    }

    #[test]
    fn schedule_state_is_ran_when_record_present_and_no_pending_or_tag() {
        assert_eq!(schedule_state(false, true), ScheduleView::Ran);
    }

    #[test]
    fn schedule_state_is_not_scheduled_without_pending_tag_or_record() {
        assert_eq!(schedule_state(false, false), ScheduleView::NotScheduled);
    }

    #[test]
    fn to_last_run_view_distinguishes_never_ran_ran_and_read_failed() {
        assert_eq!(to_last_run_view(Ok(None)), LastRunView::NeverRan);
        assert_eq!(
            to_last_run_view(Ok(Some(RunRecord {
                ran_at_unix_ms: 1,
                result: RunResult::Completed,
            }))),
            LastRunView::Ran {
                at_ms: 1,
                result: RunResultView::Completed,
            }
        );
        let failed = to_last_run_view(Err(DeviceError::Internal("background task record")));
        assert!(matches!(failed, LastRunView::ReadFailed(_)));
        assert_ne!(failed, LastRunView::NeverRan);
    }
}
