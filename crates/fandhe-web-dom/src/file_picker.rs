//! Phase 11（親 #871）型サンプル「ファイル選択」の Web 経路（Issue #875、`wasm32` 専用）。
//! Issue #981（親 #979）で「TSV 読み込み」画面（[`tsv_import_effect`]）とこの
//! `<input type="file">` 配線を共有する形へ拡張した。
//!
//! `fandhe_core::screens::file_pick`（端末依存ゼロ）の要求を
//! [`super::mount::mount_with_effect`] の effect として観測し、非表示の
//! `<input type="file">` を `click()` して OS の「開く」ダイアログを起動、
//! `change` イベントで得た `File` を読み切って結果を書き戻す。
//!
//! Web は `fandhe-device` の `FilePicker` トレイトを通らない（`tts.rs`/`clipboard.rs` と
//! 同じ理由。`fandhe-egui-proxy` の feature `device` は wasm32 に依存を持ち込まない設計）。
//! そのため本モジュールが DOM レンダラ側の対応実装にあたる。`fandhe_device::file_picker`
//! からは上限バイト数 [`fandhe_device::file_picker::MAX_FILE_BYTES`] のみを参照し
//! （上限の SSOT を複製しない）、`FilePicker` トレイト自体は使わない。
//!
//! # ダイアログ API に `File System Access API` ではなく `<input type="file">` を使う理由
//!
//! `showOpenFilePicker` は Safari が明示的に非対応（対応表 `docs/device/capabilities/
//! file-picker.json` の web 欄参照）。`<input type="file">` は全ブラウザで動く基本線。
//!
//! `<input>` は要求のたびに新規生成し `document.body` 直下へ `hidden` 属性付きで追加する
//! （`mount.rs` の全再構築で消えるマウント root 配下には置かない）。同じ要素を使い回すと
//! 同一ファイルの再選択で `change` が発火しないブラウザがあるため、使用後は
//! [`release`] で都度除去する。
//!
//! # ブラウザ側の制約
//!
//! - **ユーザー操作起因が必須**: `input.click()` は、ボタンの `click` ハンドラから
//!   始まる同期チェーン（click → 状態更新 → 再描画 → effect）の中で呼ぶ
//!   （`tts.rs` の `speak()` と同じ論拠）。
//! - **`cancel` イベントはキャンセル検知の努力目標**: Chrome 113+ / Firefox 91+ /
//!   Safari 16.4+ で発火するが、非対応の古い環境では発火しない。発火しない場合は
//!   固定タイムアウトで「失敗」に倒さず、`Picking` のまま（ボタンが出ない状態）を
//!   維持する。`ponytail:` 上限は下記 [`ACTIVE`] のコメント参照
//! - **サイズ上限は 2 回確認する**: `File::size()` で読む前に、読了後は
//!   `ArrayBuffer::byte_length()` で再確認する（`fandhe_device::file_picker::read_picked`
//!   の「読みながら上限超過を検知する」設計と同じ意図。ただしブラウザには TOCTOU の
//!   問題は無いため、事前判定は巨大ファイルの `ArrayBuffer` 確保を避けるための
//!   最適化に留まる）
//!
//! ファイル名・例外の詳細（`message`）は `console` 出力・送信を一切行わない
//! （security.md A09。`clipboard.rs::to_message` と同じ方針）。バイト列そのものも
//! `file_pick` 側の画面状態には保持しない（[`fandhe_core::screens::file_pick::
//! PickedSummary`] は名前とサイズのみ）。`tsv_import` 側はバイト列を
//! `fandhe_core::tsv::parse_tsv` へ渡すためだけに 1 回だけ複製し、画面状態には
//! 解析結果（行数・列数・プレビュー）しか残さない
//! （`fandhe_core::screens::tsv_import::TsvImportState` 参照）。
//!
//! # `file_pick`/`tsv_import` が `<input>` 周りを共有する理由
//!
//! `<input type="file">` の生成・イベント登録・解放（[`ACTIVE`]/[`release`]）は画面の
//! 型に依存しない。画面状態 `S` へのアクセスだけを [`WebPickHooks<S>`]（関数ポインタの
//! 束）へ切り出し、[`drive`] がジェネリックに処理する
//! （`fandhe-egui-proxy` の `device/file_pick.rs::PickHooks<S>` と同じ設計、
//! 契約表「使う部品」節参照）。`read` の戻り値もバイト列の解釈（要約に潰すか、
//! `Vec<u8>` として解析へ渡すか）を持たない [`Picked`]（`File` の名前と
//! `ArrayBuffer` のみ）にとどめ、変換は各画面の `finish` フックへ委ねる。

use std::cell::RefCell;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use fandhe_core::screens::file_pick::{
    finish_pick, mark_picking as file_pick_mark_picking, take_request as file_pick_take_request,
    FilePickState, PickedSummary,
};
use fandhe_core::screens::photo_pick::{
    finish_pick as photo_finish_pick, image_source as photo_image_source,
    mark_picking as photo_mark_picking, take_request as photo_take_request, PhotoPickState,
    ShownPhoto,
};
use fandhe_core::screens::tsv_import::{
    finish_import, mark_picking as tsv_mark_picking, take_request as tsv_take_request,
    TsvImportState,
};
use fandhe_device::file_picker::MAX_FILE_BYTES;
use fandhe_device::photo_picker::{
    dimensions_within_limits, image_dimensions, validate_photo, MAX_PHOTO_BYTES,
};
use fandhe_device::DeviceError;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, BlobPropertyBag, Event, File, HtmlInputElement, ImageBitmap, Url};

use super::mount::Applier;

/// 非対応ブラウザ・取得失敗時の案内（画面の `error` へ出る）。
const UNAVAILABLE: &str = "このブラウザではファイル選択を使えません";
/// 読み取り失敗（`array_buffer()` の reject・型変換失敗）時の案内。
const READ_FAILED: &str = "ファイルを読み取れませんでした";

/// サイズ超過時の案内（`fandhe-egui-proxy` 側 `device/file_pick.rs::to_message` と
/// 同文言。上限の SSOT は [`MAX_FILE_BYTES`] のみ）。
fn to_size_message() -> String {
    format!(
        "{} MiB を超えるファイルは選べません",
        MAX_FILE_BYTES / (1024 * 1024)
    )
}

/// 1 回の選択要求ぶんの `<input>` 要素とイベントハンドラ 2 本をまとめて保持する。
struct PickHandlers {
    input: HtmlInputElement,
    // 発火はしないが、Drop されるまで JS 側から呼び出し可能な状態を保つために
    // 保持するだけのフィールド（未読で許容する、`tts.rs::SpeakHandlers` と同型）。
    #[allow(dead_code)]
    on_change: Closure<dyn FnMut(Event)>,
    #[allow(dead_code)]
    on_cancel: Closure<dyn FnMut(Event)>,
}

thread_local! {
    /// 飛行中の選択要求を [`Applier::mount_id`] ごとに保持する。
    ///
    /// マウント単位で持つ理由は `clipboard.rs::IN_FLIGHT` と同じ（同じページへ本画面を
    /// 2 つマウントしたときに一方の飛行中状態がもう一方を止めない、codex-review 指摘 P1
    /// 再発防止）。`change`/`cancel`/構築失敗のいずれの経路でも [`release`] を通し、
    /// `<input>` 要素とハンドラを取りこぼさず解放する（`tts.rs` の P1 是正と同じ観点）。
    /// `file_pick`/`tsv_import` の 2 画面が同じマウント root へ同時に配線されることは
    /// ない（`demo.rs` のエントリポイントは 1 画面につき 1 つ）ため、`mount_id` の
    /// 衝突は起きない。
    ///
    /// ponytail: `cancel` 非対応ブラウザでは要求が `Picking` のまま残り続ける
    /// （固定タイムアウト禁止、モジュール doc 参照）。フォーカス復帰時に再確認する
    /// ヒューリスティックが要るなら、`window` の `focus` イベントを追加で監視する形へ
    /// 拡張する（本 issue のスコープ外、out-of-scope-tracking.md 対象）。
    static ACTIVE: RefCell<HashMap<u64, PickHandlers>> = RefCell::new(HashMap::new());
}

/// `mount_id` の飛行中エントリを解放する（リスナー解除 → `<input>` を DOM から除去 →
/// `Closure` を drop）。`change`/`cancel`/構築失敗のすべての経路がここを通る
/// （モジュール doc 参照）。
fn release(mount_id: u64) {
    ACTIVE.with(|cell| {
        let Some(handlers) = cell.borrow_mut().remove(&mount_id) else {
            return;
        };
        let _ = handlers.input.remove_event_listener_with_callback(
            "change",
            handlers.on_change.as_ref().unchecked_ref(),
        );
        let _ = handlers.input.remove_event_listener_with_callback(
            "cancel",
            handlers.on_cancel.as_ref().unchecked_ref(),
        );
        handlers.input.remove();
    });
}

/// 選択された `File` の名前と内容（`ArrayBuffer`）。バイト列の解釈
/// （要約に潰すか `Vec<u8>` として解析へ渡すか）は画面ごとに異なるため、
/// ここでは持たない（モジュール doc「`file_pick`/`tsv_import` が共有する理由」節参照）。
pub(crate) struct Picked {
    pub(crate) name: String,
    pub(crate) buffer: js_sys::ArrayBuffer,
}

/// `File` の内容を読み切り、上限を超えていないか確認する。
///
/// `array_buffer()` を選ぶ理由: `FileReader` のイベントベース API より `Closure` が
/// 2 本（`onload`/`onerror`）少なく、リークの面が小さい。読めることの確認と
/// サイズの確定を兼ねる（モジュール doc 参照）。
pub(crate) async fn read(file: File) -> Result<Picked, String> {
    // 読む前の事前判定（TOCTOU 対策ではなく、巨大ファイルの `ArrayBuffer` 確保を
    // 避けるための最適化、モジュール doc 参照）。
    if file.size() as usize > MAX_FILE_BYTES {
        return Err(to_size_message());
    }

    let raw = JsFuture::from(file.array_buffer())
        .await
        .map_err(|_| READ_FAILED.to_string())?;
    let buffer: js_sys::ArrayBuffer = raw.dyn_into().map_err(|_| READ_FAILED.to_string())?;

    if buffer.byte_length() as usize > MAX_FILE_BYTES {
        return Err(to_size_message());
    }

    Ok(Picked {
        name: file.name(),
        buffer,
    })
}

/// `file_pick`/`tsv_import` が画面状態 `S` へのアクセスを渡す関数ポインタの束
/// （`fandhe-egui-proxy` の `device/file_pick.rs::PickHooks<S>` と同じ設計、
/// モジュール doc参照）。すべて `fn` ポインタ（クロージャ環境キャプチャ不可）のため
/// `Copy`（`derive` は `S: Copy` を要求してしまうため手実装）。
pub(crate) struct WebPickHooks<S, R> {
    /// `<input>` の `accept` 属性（`None` なら付けない。写真選択は `image/*`）。
    pub(crate) accept: Option<&'static str>,
    /// 未発送の要求があれば世代番号を返す（画面の `take_request` 相当）。
    pub(crate) take_request: fn(&S) -> Option<u64>,
    /// `Requested` → `Picking` へ進める（画面の `mark_picking` 相当）。
    pub(crate) mark_picking: fn(&mut S),
    /// `change` で得た `File` を、画面へ渡す形 `R` へ非同期に変換する
    /// （`file_pick`/`tsv_import` は [`read_boxed`]、写真選択は [`read_photo_boxed`]）。
    /// 第 1 引数は [`Applier::mount_id`]（マウントごとの資源を持つ変換だけが使う）。
    pub(crate) convert: fn(u64, File) -> ConvertFuture<R>,
    /// 完了を反映する（画面の `finish_pick`/`finish_import` 相当）。
    pub(crate) finish: fn(&mut S, u64, Result<Option<R>, String>),
}

/// [`WebPickHooks::convert`] の戻り値（`wasm32` は単一スレッドのため `Send` 不要）。
pub(crate) type ConvertFuture<R> = Pin<Box<dyn Future<Output = Result<R, String>>>>;

impl<S, R> Clone for WebPickHooks<S, R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S, R> Copy for WebPickHooks<S, R> {}

/// `file_pick`/`tsv_import` 用の `convert`（既存の [`read`] を包むだけ）。
fn read_boxed(_mount_id: u64, file: File) -> ConvertFuture<Picked> {
    Box::pin(read(file))
}

/// 要求を失敗として畳み、[`Applier::apply`] へ渡す（各構築失敗の分岐から共通で呼ぶ）。
fn fail<S: 'static, R: 'static>(
    applier: &Applier<S>,
    generation: u64,
    message: &'static str,
    hooks: WebPickHooks<S, R>,
) {
    applier.apply(move |s| (hooks.finish)(s, generation, Err(message.to_string())));
}

/// `mount_with_effect` へ渡す effect の共通本体。`hooks` が指す画面の要求を処理する
/// （`file_pick_effect`/`tsv_import_effect` はそれぞれの `WebPickHooks` で本関数を
/// 呼ぶだけ、モジュール doc 参照）。
///
/// 要求が無ければ何もしない（`mount.rs` モジュール doc の「effect フック」節の契約）。
pub(crate) fn drive<S: 'static, R: 'static>(
    state: &S,
    applier: &Applier<S>,
    hooks: WebPickHooks<S, R>,
) {
    let Some(generation) = (hooks.take_request)(state) else {
        return;
    };

    let mount_id = applier.mount_id();
    // 飛行中（既に `<input>` を出している）なら二重に開かない（[`ACTIVE`] doc 参照）。
    if ACTIVE.with(|cell| cell.borrow().contains_key(&mount_id)) {
        return;
    }

    applier.apply(hooks.mark_picking);

    let Some(document) = web_sys::window().and_then(|w| w.document()) else {
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    };
    let Some(body) = document.body() else {
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    };
    let Ok(input) = document.create_element("input") else {
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    };
    let Ok(input) = input.dyn_into::<HtmlInputElement>() else {
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    };
    input.set_type("file");
    let _ = input.set_attribute("hidden", "");
    if let Some(accept) = hooks.accept {
        let _ = input.set_attribute("accept", accept);
    }
    if body.append_child(&input).is_err() {
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    }

    // `change`: 選択された `File`（無ければキャンセル扱い）を読み取り、結果を返す。
    // 実行中の `Closure` を自分自身の中で drop しない（`tts.rs` の P1 是正と異なる
    // 回避策）: 必要な値をここで取り出したうえで `spawn_local` の非同期継続へ移し、
    // その中で [`release`]（＝この `Closure` 自身の drop）を行う。`release` が走る
    // 時点では、このクロージャの同期呼び出し自体は既に完了・リターンしているため
    // use-after-free にならない（モジュール doc 参照）。
    let on_change = {
        let applier = applier.clone();
        Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            let file = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                .and_then(|input| input.files())
                .and_then(|list| list.get(0));
            let applier = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let result = match file {
                    Some(file) => (hooks.convert)(mount_id, file).await.map(Some),
                    None => Ok(None),
                };
                release(mount_id);
                applier.apply(move |s| (hooks.finish)(s, generation, result));
            });
        })
    };

    // `cancel`: 対応ブラウザ（モジュール doc 参照）でダイアログが閉じられたときに
    // 発火する。`on_change` と同じ理由で `spawn_local` へ逃がしてから解放する。
    let on_cancel = {
        let applier = applier.clone();
        Closure::<dyn FnMut(Event)>::new(move |_event: Event| {
            let applier = applier.clone();
            wasm_bindgen_futures::spawn_local(async move {
                release(mount_id);
                applier.apply(move |s| (hooks.finish)(s, generation, Ok(None)));
            });
        })
    };

    let listeners_ok = input
        .add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())
        .and_then(|()| {
            input.add_event_listener_with_callback("cancel", on_cancel.as_ref().unchecked_ref())
        });
    if listeners_ok.is_err() {
        input.remove();
        fail(applier, generation, UNAVAILABLE, hooks);
        return;
    }

    ACTIVE.with(|cell| {
        cell.borrow_mut().insert(
            mount_id,
            PickHandlers {
                input: input.clone(),
                on_change,
                on_cancel,
            },
        );
    });

    // click の同期チェーン内での呼び出し（ユーザー操作起点要件を満たす、モジュール doc 参照）。
    input.click();
}

/// `file_pick` 画面用の [`WebPickHooks`]。`finish` は [`Picked`] を名前とサイズだけの
/// `PickedSummary` へ潰す（バイト列は画面状態に残さない、モジュール doc 参照）。
const FILE_PICK_HOOKS: WebPickHooks<FilePickState, Picked> = WebPickHooks {
    accept: None,
    convert: read_boxed,
    take_request: file_pick_take_request,
    mark_picking: file_pick_mark_picking,
    finish: |s, generation, result| {
        finish_pick(
            s,
            generation,
            result.map(|opt| {
                opt.map(|picked| PickedSummary {
                    name: picked.name,
                    size: picked.buffer.byte_length() as usize,
                })
            }),
        );
    },
};

/// `tsv_import` 画面用の [`WebPickHooks`]。`finish` は [`Picked`] のバイト列を
/// `Vec<u8>` へ 1 回だけ複製し、解析自体は `fandhe_core::screens::tsv_import::
/// finish_import` 内部の `tsv::parse_tsv` に任せる（名前は使わない。
/// サイズ上限は 2 段: `MAX_FILE_BYTES`（選択時）と `fandhe_core::tsv::MAX_TSV_BYTES`
/// （解析時）、モジュール doc 参照）。
const TSV_IMPORT_HOOKS: WebPickHooks<TsvImportState, Picked> = WebPickHooks {
    accept: None,
    convert: read_boxed,
    take_request: tsv_take_request,
    mark_picking: tsv_mark_picking,
    finish: |s, generation, result| {
        finish_import(
            s,
            generation,
            result.map(|opt| opt.map(|picked| js_sys::Uint8Array::new(&picked.buffer).to_vec())),
        );
    },
};

/// `mount_with_effect` へ渡す effect。`file_pick` 画面の要求を処理する。
pub fn file_pick_effect(state: &FilePickState, applier: &Applier<FilePickState>) {
    drive(state, applier, FILE_PICK_HOOKS);
}

/// `mount_with_effect` へ渡す effect。`tsv_import` 画面の要求を処理する（Issue #981）。
pub fn tsv_import_effect(state: &TsvImportState, applier: &Applier<TsvImportState>) {
    drive(state, applier, TSV_IMPORT_HOOKS);
}

thread_local! {
    /// 写真選択で表示中の `blob:` URL を [`Applier::mount_id`] ごとに 1 件保持する
    /// （[`ACTIVE`] と同じ理由で、複数マウントが互いの URL を解放しない）。
    /// 解放は [`sync_photo_url`] が行う（契約表 1・9）。
    ///
    /// 画面の破棄は [`release_mount`]（`mount_with_effect` が同じ root への再マウント・
    /// root の DOM 離脱を検知して呼ぶ）で解放する。
    static PHOTO_URL: RefCell<HashMap<u64, String>> = RefCell::new(HashMap::new());
}

/// 破棄されたマウントの資源（`blob:` URL）を解放する。`mount` 層から呼ばれる。
///
/// 復号が飛行中のまま破棄された場合は、`read_photo` が URL を作る直前に
/// [`is_live`](super::mount::is_live) で気づいて作らない。
pub(crate) fn release_mount(mount_id: u64) {
    // 進行中の `<input>`・ハンドラも解放する（`cancel` 非対応ブラウザでも残さない）。
    release(mount_id);
    revoke_photo_url(mount_id);
}

/// `mount_id` の `blob:` URL を解放する。
fn revoke_photo_url(mount_id: u64) {
    if let Some(url) = PHOTO_URL.with(|cell| cell.borrow_mut().remove(&mount_id)) {
        let _ = Url::revoke_object_url(&url);
    }
}

/// 画面が今表示していない `blob:` URL を解放する（新しい要求・キャンセル・失敗・
/// 古い世代の完了で状態から写真が消えたとき）。再描画の直後に毎回呼ぶ。
fn sync_photo_url(state: &PhotoPickState, mount_id: u64) {
    let shown = photo_image_source(state);
    let stale = PHOTO_URL.with(|cell| cell.borrow().get(&mount_id).is_some_and(|u| *u != shown));
    if stale {
        revoke_photo_url(mount_id);
    }
}

/// 復号に失敗したときの案内（`createImageBitmap` の reject・Blob 構築失敗）。
const PHOTO_UNDISPLAYABLE: &str = "画像を表示できませんでした";
/// 画像の寸法（辺・総画素数）が復号前の上限を超えるときの案内。
const PHOTO_TOO_LARGE: &str = "画像のサイズが大きすぎるため選べません";
/// 1 段目（形式不明・0 バイト）で落ちたときの案内。
const PHOTO_UNREADABLE: &str = "画像として読み取れませんでした";

/// 写真選択の `convert`。契約（`fandhe_device::photo_picker`）の 2 段検査をここで行う:
/// 1 段目は [`validate_photo`]、2 段目は `createImageBitmap` での実復号。`blob:` URL は
/// 2 段目を通った後にだけ作る（失敗の経路で URL を作らない = 解放漏れが起きない）。
/// `file.type` は信用せず、判定した形式の MIME で Blob を作る（A03）。
async fn read_photo(mount_id: u64, file: File) -> Result<ShownPhoto, String> {
    let picked = read(file).await?;
    let bytes = js_sys::Uint8Array::new(&picked.buffer).to_vec();
    let photo = validate_photo(bytes).map_err(|e| match e {
        DeviceError::InvalidInput(_) => format!(
            "{} MiB を超える画像は選べません",
            MAX_PHOTO_BYTES / (1024 * 1024)
        ),
        _ => PHOTO_UNREADABLE.to_string(),
    })?;
    let undisplayable = || PHOTO_UNDISPLAYABLE.to_string();

    // 復号（`createImageBitmap`）の前にヘッダーの寸法で上限を検査する。展開後の
    // 確保量はバイト数上限では抑えられない（小さな巨大 PNG）ため（デスクトップの
    // `image::Limits` と同じ辺・確保量）。読めないヘッダーは復号不能として失敗にする。
    match image_dimensions(&photo) {
        Some((w, h)) if dimensions_within_limits(w, h) => {}
        Some(_) => return Err(PHOTO_TOO_LARGE.to_string()),
        None => return Err(undisplayable()),
    }

    let opts = BlobPropertyBag::new();
    opts.set_type(photo.format.mime());
    let parts = js_sys::Array::of1(&picked.buffer);
    let blob = Blob::new_with_buffer_source_sequence_and_options(&parts, &opts)
        .map_err(|_| undisplayable())?;

    let window = web_sys::window().ok_or_else(undisplayable)?;
    let promise = window
        .create_image_bitmap_with_blob(&blob)
        .map_err(|_| undisplayable())?;
    let bitmap: ImageBitmap = JsFuture::from(promise)
        .await
        .map_err(|_| undisplayable())?
        .dyn_into()
        .map_err(|_| undisplayable())?;
    let (width, height) = (bitmap.width(), bitmap.height());
    bitmap.close();

    // 復号待ちの間（開始前を含む）にマウントが破棄されていたら URL を作らない
    // （解放対象から漏れる）。結果の反映は `Applier::apply` が破棄済みなら捨てる。
    if !super::mount::is_live(mount_id) {
        return Err(undisplayable());
    }
    let url = Url::create_object_url_with_blob(&blob).map_err(|_| undisplayable())?;
    if let Some(old) = PHOTO_URL.with(|cell| cell.borrow_mut().insert(mount_id, url.clone())) {
        let _ = Url::revoke_object_url(&old);
    }
    Ok(ShownPhoto {
        source: url,
        format: format!("{:?}", photo.format).to_uppercase(),
        size: photo.bytes.len(),
        width,
        height,
    })
}

fn read_photo_boxed(mount_id: u64, file: File) -> ConvertFuture<ShownPhoto> {
    Box::pin(read_photo(mount_id, file))
}

/// `photo_pick` 画面用の [`WebPickHooks`]。前回の `blob:` URL の解放は
/// [`photo_pick_effect`] が [`sync_photo_url`] で行う。
const PHOTO_PICK_HOOKS: WebPickHooks<PhotoPickState, ShownPhoto> = WebPickHooks {
    accept: Some("image/*"),
    take_request: photo_take_request,
    mark_picking: photo_mark_picking,
    convert: read_photo_boxed,
    finish: photo_finish_pick,
};

/// `mount_with_effect` へ渡す effect。`photo_pick` 画面の要求を処理する（Issue #1103）。
pub fn photo_pick_effect(state: &PhotoPickState, applier: &Applier<PhotoPickState>) {
    sync_photo_url(state, applier.mount_id());
    drive(state, applier, PHOTO_PICK_HOOKS);
}
