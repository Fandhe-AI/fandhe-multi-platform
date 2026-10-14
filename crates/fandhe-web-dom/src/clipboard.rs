//! Phase 11（親 #858）型サンプル「クリップボード」の Web 経路（Issue #859、`wasm32` 専用）。
//!
//! `fandhe_core::screens::clipboard_text`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、`navigator.clipboard`
//! （Clipboard API）で読み書きして結果を書き戻す。
//!
//! Web は `fandhe-device` の `Clipboard` トレイトを通らない（`fandhe-device` は wasm32 で
//! ureq/arboard の依存が解決できず、`fandhe-egui-proxy` の feature `device` も wasm32 には
//! 依存自体を持ち込まない設計。`crates/fandhe-egui-proxy/src/device.rs` のモジュール doc
//! 参照）。そのため本モジュールが DOM レンダラ側の対応実装にあたる。
//!
//! # ブラウザ側の制約（対応表 `docs/device/capabilities/clipboard.json` の web 欄と対）
//!
//! - **secure context 必須**: `https:` または `localhost` 以外では `navigator.clipboard`
//!   自体が `undefined` になる。本モジュールは取得失敗を画面のエラー文言へ落とす。
//! - **書き込みはユーザー操作起因が必要**: ボタンの `click` ハンドラから始まる一連の
//!   処理なので要件を満たす（effect は click → 状態更新 → 再描画の直後に走る）。
//! - **読み取りはブラウザごとに追加の許可が要る**: Safari はペースト確認の UI を出し、
//!   Firefox はユーザー操作を要求する。拒否された場合は Promise が reject するため、
//!   本モジュールは失敗として画面へ返す（内容を推測して埋めない）。
//!
//! # 二重実行の防止（[`IN_FLIGHT`]）
//!
//! effect は**再描画のたび**に呼ばれる。デスクトップ・iOS・Android の経路は
//! クリップボード操作が同期的に完了し、同じ effect 呼び出しの中で結果を書き戻す
//! （`pending` が下りる）ため、要求を観測する effect 呼び出しは 1 回で済む。
//! 一方 Web の `navigator.clipboard` は Promise を返す非同期 API で、解決前に別の
//! 再描画（入力欄への打鍵など）が起きると effect が同じ `pending` をもう一度観測し、
//! 2 回目の読み書きを始めてしまう。
//!
//! `fandhe-core` 側に `http_fetch` のような `Requested` → `Loading` の二段階を足すと
//! 同期で足りる他 3 経路にも不要な状態が増えるため、Web に閉じた飛行中フラグで抑える。
//!
//! フラグは**マウント単位**で持つ（[`Applier::mount_id`] をキーにした集合）。プロセス内で
//! 単一のフラグにすると、同じページへ本画面を 2 つマウントしたときに一方の飛行中状態が
//! もう一方の要求を握り潰し、`pending` が残ったまま復帰しなくなる
//! （codex-review 指摘 P1、PR #860）。wasm32 はシングルスレッドのため
//! `thread_local!` + `RefCell<HashSet>` で足りる。
//!
//! クリップボードの内容は画面へ表示するだけで、`console` 出力・保存・送信は一切行わない
//! （他アプリ・他タブのパスワード等が載り得る機密経路のため痕跡を残さない、security.md A09）。

use fandhe_core::screens::clipboard_text::{
    finish_copy, finish_paste, take_request, ClipboardRequest, ClipboardTextState,
};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;

use super::mount::Applier;
use std::cell::RefCell;
use std::collections::HashSet;

thread_local! {
    /// クリップボード操作が飛行中のマウント ID の集合
    /// （モジュール doc「二重実行の防止」節参照）。
    static IN_FLIGHT: RefCell<HashSet<u64>> = RefCell::new(HashSet::new());
}

/// `mount_id` の飛行中フラグを立てる。既に飛行中なら `false`（呼び出し側は何もしない）。
fn begin_flight(mount_id: u64) -> bool {
    IN_FLIGHT.with(|f| f.borrow_mut().insert(mount_id))
}

/// `mount_id` の飛行中フラグを下ろす（結果の書き戻し直前に呼ぶ）。
fn end_flight(mount_id: u64) {
    IN_FLIGHT.with(|f| {
        f.borrow_mut().remove(&mount_id);
    });
}

/// `navigator.clipboard` を取得する。secure context 外・非対応ブラウザでは `None`。
fn clipboard() -> Option<web_sys::Clipboard> {
    // `Navigator::clipboard()` は web-sys の型上は非 Option だが、secure context 外では
    // 実際には `undefined` が返る。`JsValue` として `undefined` かを確かめてから使う
    // （型に現れない JS 側の実態を境界で検証する、security.md A03 の方針）。
    let navigator = web_sys::window()?.navigator();
    let clipboard = navigator.clipboard();
    if JsValue::from(clipboard.clone()).is_undefined() {
        return None;
    }
    Some(clipboard)
}

/// secure context 外・非対応ブラウザ向けの案内（画面の `status` へ出る）。
const UNAVAILABLE: &str =
    "このブラウザ・接続ではクリップボードを使えません（https か localhost が必要）";

/// `JsValue` のエラーを画面へ渡す短い文言へ変換する。
///
/// 例外オブジェクトの `message` には拒否理由が入るが、ブラウザによってはクリップボードの
/// 内容が含まれ得るため転送しない（security.md A09）。種別が分かる固定文言に畳む。
fn to_message(_e: JsValue) -> String {
    "ブラウザがクリップボード操作を拒否しました".to_string()
}

/// `mount_with_effect` へ渡す effect。`clipboard_text` 画面の要求を処理する。
///
/// 要求が無ければ何もしない（再描画が止まらなくなるのを防ぐ、`mount` モジュール doc の
/// 「effect フック」節の契約）。
pub fn clipboard_text_effect(state: &ClipboardTextState, applier: &Applier<ClipboardTextState>) {
    let Some(request) = take_request(state) else {
        return;
    };

    // 非同期解決の前に別の再描画が起きても 2 回目を始めない
    // （モジュール doc「二重実行の防止」節参照）。判定はマウント単位。
    let mount_id = applier.mount_id();
    if !begin_flight(mount_id) {
        return;
    }

    let Some(clipboard) = clipboard() else {
        // 非対応環境。要求を下ろしつつ案内を出す（要求が残ると画面が操作不能になる）。
        end_flight(mount_id);
        let applier = applier.clone();
        match request {
            ClipboardRequest::Copy(_) => {
                applier.apply(|s| finish_copy(s, Err(UNAVAILABLE.to_string())))
            }
            ClipboardRequest::Paste => {
                applier.apply(|s| finish_paste(s, Err(UNAVAILABLE.to_string())))
            }
        }
        return;
    };

    let applier = applier.clone();
    match request {
        ClipboardRequest::Copy(text) => {
            // `write_text` はユーザー操作起因（ボタン click）の延長で呼ばれるため
            // 権限要件を満たす（モジュール doc 参照）。
            let promise = clipboard.write_text(&text);
            wasm_bindgen_futures::spawn_local(async move {
                let result = JsFuture::from(promise)
                    .await
                    .map(|_| ())
                    .map_err(to_message);
                end_flight(mount_id);
                applier.apply(move |s| finish_copy(s, result));
            });
        }
        ClipboardRequest::Paste => {
            let promise = clipboard.read_text();
            wasm_bindgen_futures::spawn_local(async move {
                let result = match JsFuture::from(promise).await {
                    // `read_text` は空のクリップボードでも空文字列で解決する。
                    // 画面側の「空」表示へ合わせて `None` へ畳む
                    // （`fandhe_device::clipboard` の desktop 実装と同じ規則）。
                    Ok(value) => Ok(value.as_string().filter(|text| !text.is_empty())),
                    Err(e) => Err(to_message(e)),
                };
                end_flight(mount_id);
                applier.apply(move |s| finish_paste(s, result));
            });
        }
    }
}
