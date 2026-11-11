//! `DomNode` を `web-sys` 経由で実 DOM へ構築するマウント層（`wasm32` 専用、TASK-4.4-S2 / REQ-4）。
//!
//! [`super::dom_tree::render`] が返す [`super::dom_tree::Rendered`]（DOM 中間表現 +
//! ハンドラ関数ポインタ一式）を実 DOM へ反映し、`Button` の `click` / `TextField` の
//! `input` イベントを状態 `S` の更新へ配線する。イベント発火のたびに
//! `Screen<S>` 全体を `render` し直して**全再構築**する最小実装であり、差分更新
//! （仮想 DOM diff）は扱わない（TASK-4.4-S2 のスコープ外、out-of-scope-tracking.md 対象）。
//!
//! 全再構築のたびに `document.activeElement` を含む入力要素も作り直されるため、
//! 対策なしでは `input` イベントのたびにフォーカスが失われ連続入力ができない。
//! これを避けるため、再構築の直前に `active_element` が本レンダラの入力要素
//! （`handler_index` 順に保持する [`InputElements`]）のいずれかと同一かを
//! 参照同一性（`Element` の `PartialEq`、JS 参照比較）で照合し、一致すれば
//! カーソル位置（`selection_start` / `selection_end`）を保存する。再構築後は
//! 同じ `handler_index` に対応する新しい入力要素へフォーカスとカーソル位置を
//! 復元する（`focus()` / `set_selection_range` はいずれも失敗を無視する
//! fail-soft 方針。panic を API 契約にしない、coding-rust.md 準拠）。
//!
//! # セキュリティ（OWASP A03 インジェクション対策）
//!
//! HTML 文字列の組み立て（[`super::html`]）とは異なり、本モジュールは
//! `create_element` + `set_text_content` + `set_attribute` のみで実 DOM を構築し、
//! `innerHTML` / `insertAdjacentHTML` を一切使用しない。これにより、状態由来の
//! 文字列（`Node::Text` / `TextField::label` / `get(state)` の戻り値）が
//! ブラウザの HTML パーサへ再解釈される経路自体が存在しない。
//!
//! # effect フック（Issue #859）
//!
//! 端末機能（クリップボード等）を使う画面のために、再描画の直後に呼ばれる
//! [`WebEffect`] を [`mount_with_effect`] で受け取れる。`fandhe-core` の画面は
//! 端末依存ゼロ（ボタンハンドラは `fn(&mut S)` で I/O を持てない）ため、
//! 「ボタンが状態へ要求を立てる → effect が観測して実際の I/O を行う →
//! [`Applier::apply`] で結果を書き戻す」という一巡にする。
//! `fandhe-egui-proxy` 側の `app::EffectFn` + `CompletionSender` と同じ役割分担で、
//! 本モジュールはその Web（DOM レンダラ）版にあたる。
//!
//! effect は再描画の完了後に呼ばれ、[`Applier::apply`] はさらに再描画を起こすため、
//! **effect は「処理すべき要求が無ければ何もしない」契約を守る**こと（守らないと
//! 再描画が止まらない）。`navigator.clipboard` のように非同期な API は
//! `spawn_local` で逃がすため同期再帰は起きない。
//!
//! `#![forbid(unsafe_code)]`（クレートルート）: `wasm-bindgen` の `Closure` は
//! safe API であり、本モジュールも `unsafe` を要求しない。

use std::cell::RefCell;
use std::rc::Rc;

use fandhe_core::ui::ImeEvent;
use fandhe_core::{Screen, Widget, WidgetValue};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CompositionEvent, Document, Element, HtmlElement, HtmlInputElement, InputEvent};

use super::dom_tree::{render, DomNode, EventHook, WidgetValueKind};
use super::ime::{composition_phase_from_event_type, ime_event_from_composition};

// `Applier`・マウントの生存管理・`query_permission` は `fandhe-platform-web`（Web 共通層、#1185）へ
// 切り出した。未移行の機能モジュールが `crate::mount::…` のまま使えるよう再公開する。
pub(crate) use fandhe_platform_web::{is_live, query_permission, root_of};
use fandhe_platform_web::{on_mount_release, register_mount, RenderTrigger};
pub use fandhe_platform_web::{Applier, WebEffect};

/// `handler_index` 順に並んだ `Node::Button::on_click` の共有ベクタ
/// （`clippy::type_complexity` 対策の型エイリアス、実体は [`super::dom_tree::Rendered`] と同じ）。
type ClickHandlers<S> = Rc<Vec<fn(&mut S)>>;

/// `handler_index` 順に並んだ `Node::TextField::set` の共有ベクタ
/// （`clippy::type_complexity` 対策の型エイリアス、実体は [`super::dom_tree::Rendered`] と同じ）。
type InputHandlers<S> = Rc<Vec<fn(&mut S, String)>>;

/// `handler_index` 順に並んだ、`sync_from_state` 済みの `Checkbox`/`ToggleSwitch`/
/// `Slider`/`RadioButton` 複製の共有ベクタ（Issue #993、実体は
/// [`super::dom_tree::Rendered::widget_handlers`] と同じ）。[`attach_change`] が
/// `change` イベントでこの複製の [`Widget::apply_value`] を直接呼ぶ。
type WidgetHandlers<S> = Rc<Vec<Box<dyn Widget<S>>>>;

/// 直近の再構築で生成された `input` 要素を `handler_index` で直接引ける共有ベクタ
/// （`elements[handler_index]` が対応する要素、未使用の添字は `None`）。
///
/// [`build_element`] が `EventHook::Input { handler_index }`（[`InputElements`]）/
/// `EventHook::Change { handler_index, .. }`（[`WidgetElements`]）を持つノードを
/// 構築するたびに、その `handler_index` の位置へ直接代入する（走査順に依存しない）。
/// フォーカス・カーソル位置の保存（[`capture_focus_state`]）・復元
/// （[`restore_focus_state`]）の双方が、このベクタを介して「同じ論理的な入力欄」を
/// 世代をまたいで対応付ける。
type InputElements = Rc<RefCell<Vec<Option<Element>>>>;

/// [`Node::Widget`][fandhe_core::Node::Widget]（`Checkbox`/`ToggleSwitch`/`Slider`/
/// `RadioButton`）が構築した `input` 要素の共有ベクタ（[`InputElements`] と同じ構造。
/// `TextField` の入力欄と型を分けているのは、フォーカス対象を
/// [`FocusTarget::Input`]/[`FocusTarget::Widget`] で区別するため）。
type WidgetElements = Rc<RefCell<Vec<Option<Element>>>>;

/// 再構築前にフォーカスされていた要素が、[`InputElements`]（`TextField`）と
/// [`WidgetElements`]（`Checkbox`/`ToggleSwitch`/`Slider`/`RadioButton`）の
/// どちらの `handler_index` に対応するか（Issue #993。全再構築のたびに両方の
/// 要素が作り直されるため、`TextField` と同じ理由でウィジェット側にもフォーカス
/// 復元を広げないと、キーボード操作（Space・矢印キー）が 1 回ごとに途切れる）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusTarget {
    Input(usize),
    Widget(usize),
}

/// 再構築前に保存した、フォーカスされていた要素の位置とカーソル位置。
/// `selection_start`/`selection_end` はテキスト選択を持つ `TextField` 入力欄
/// （`FocusTarget::Input`）でのみ意味を持ち、`FocusTarget::Widget` では常に `None`。
struct FocusState {
    target: FocusTarget,
    selection_start: Option<u32>,
    selection_end: Option<u32>,
}

/// 再構築の直前に、現在フォーカスされている要素が本レンダラの入力欄
/// （`input_elements`）またはウィジェット要素（`widget_elements`）のいずれかと
/// 参照同一（`Element` の `PartialEq`、JS 参照比較）かを調べ、一致すれば
/// [`FocusState`] として保存する。どちらとも一致しない場合（フォーカスが本レンダラ外の
/// 要素にある等）は `None` を返す。
fn capture_focus_state(
    document: &Document,
    input_elements: &[Option<Element>],
    widget_elements: &[Option<Element>],
) -> Option<FocusState> {
    let active = document.active_element()?;
    if let Some(handler_index) = input_elements
        .iter()
        .position(|element| element.as_ref() == Some(&active))
    {
        let input = active.dyn_ref::<HtmlInputElement>();
        return Some(FocusState {
            target: FocusTarget::Input(handler_index),
            selection_start: input.and_then(|i| i.selection_start().ok().flatten()),
            selection_end: input.and_then(|i| i.selection_end().ok().flatten()),
        });
    }
    let handler_index = widget_elements
        .iter()
        .position(|element| element.as_ref() == Some(&active))?;
    Some(FocusState {
        target: FocusTarget::Widget(handler_index),
        selection_start: None,
        selection_end: None,
    })
}

/// [`capture_focus_state`] で保存した [`FocusState`] を、再構築後の新しい
/// `input_elements`/`widget_elements`（同じ `handler_index` で引ける）へ復元する。
/// `focus()` / `set_selection_range` の失敗はいずれも無視する fail-soft 方針とする
/// （panic を API 契約にしない、coding-rust.md 準拠。フォーカス復元の失敗は致命的
/// ではなく、再描画自体を失敗させるべきではない）。
fn restore_focus_state(
    input_elements: &[Option<Element>],
    widget_elements: &[Option<Element>],
    focus_state: Option<FocusState>,
) {
    let Some(focus_state) = focus_state else {
        return;
    };
    let element = match focus_state.target {
        FocusTarget::Input(handler_index) => input_elements.get(handler_index),
        FocusTarget::Widget(handler_index) => widget_elements.get(handler_index),
    };
    let Some(Some(element)) = element else {
        return;
    };
    if let Some(html_element) = element.dyn_ref::<HtmlElement>() {
        let _ = html_element.focus();
    }
    if let Some(input) = element.dyn_ref::<HtmlInputElement>() {
        if let (Some(start), Some(end)) = (focus_state.selection_start, focus_state.selection_end) {
            let _ = input.set_selection_range(start, end);
        }
    }
}

/// `screen` を `root` 配下へマウントし、`initial_state` を状態として実 DOM 描画を開始する。
///
/// 呼び出し元（wasm32 側のエントリポイント、`#[wasm_bindgen(start)]` 等）が
/// `document.get_element_by_id(...)` 等で取得したマウントポイント要素を `root` として渡す
/// 想定。戻り値の `Result` はマウント直後の初回描画までのエラーを表し、以降の
/// イベント経由の再描画エラーはブラウザコンソールへ出力するに留める（panic を
/// API 契約にしない方針、coding-rust.md 準拠）。
pub fn mount<S: 'static>(
    root: Element,
    screen: Screen<S>,
    initial_state: S,
) -> Result<(), JsValue> {
    // effect 無し（端末機能を使わない画面）。`|_state, _applier| {}` の no-op を渡す。
    mount_with_effect(root, screen, initial_state, |_state, _applier| {})
}

/// [`mount`] に再描画直後の [`WebEffect`] を加えた版（Issue #859）。
/// 端末機能（クリップボード等）を使う画面はこちらを使う
/// （モジュール doc「effect フック」節参照）。
pub fn mount_with_effect<S: 'static>(
    root: Element,
    screen: Screen<S>,
    initial_state: S,
    effect: WebEffect<S>,
) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    // 未移行の機能の解放関数を登録してから、マウントの採番・旧マウントの解放を行う
    // （登録が後だと、この呼び出しで破棄される旧マウントの解放が漏れる）。
    on_mount_release("fandhe-web-dom", release_features);
    let mount_id = register_mount(&root);

    let document = web_sys::window()
        .ok_or_else(|| JsValue::from_str("window が取得できない"))?
        .document()
        .ok_or_else(|| JsValue::from_str("document が取得できない"))?;

    let root = Rc::new(root);
    let screen = Rc::new(screen);
    let state = Rc::new(RefCell::new(initial_state));

    // 自己参照クロージャの標準的な組み方: `render_slot` に「自分自身を呼び出す
    // クロージャ」を後から詰める。イベントハンドラは `render_slot` 経由で
    // 間接的に再レンダリングを呼び出すため、この時点では中身は空でよい。
    let render_slot: Rc<RefCell<Option<RenderTrigger>>> = Rc::new(RefCell::new(None));

    // 直近世代の入力欄一覧（フォーカス・カーソル位置の保存/復元用、モジュール
    // コメント参照）。初回描画時点ではまだ入力欄が存在しないため空で始める。
    let input_elements: InputElements = Rc::new(RefCell::new(Vec::new()));
    // 直近世代のウィジェット要素一覧（`Checkbox`/`ToggleSwitch`/`Slider`/`RadioButton`
    // の `input`、フォーカス復元用、Issue #993）。
    let widget_elements: WidgetElements = Rc::new(RefCell::new(Vec::new()));

    let render_once: RenderTrigger = {
        let document = document.clone();
        let root = root.clone();
        let screen = screen.clone();
        let state = state.clone();
        let render_slot = render_slot.clone();
        let input_elements = input_elements.clone();
        let widget_elements = widget_elements.clone();

        Rc::new(move || -> Result<(), JsValue> {
            // 全再構築で入力欄も作り直されフォーカスが失われるため、除去前に
            // フォーカス状態を保存しておく（モジュールコメント参照）。
            let focus_state = capture_focus_state(
                &document,
                &input_elements.borrow(),
                &widget_elements.borrow(),
            );

            // 全再構築（差分更新は本 sub-issue のスコープ外）。既存の子要素をまず除去する。
            while let Some(child) = root.first_child() {
                root.remove_child(&child)?;
            }

            let rendered = {
                let state_ref = state.borrow();
                render(&screen, &state_ref, mount_id)
            };
            let handlers = Handlers {
                click: Rc::new(rendered.click_handlers),
                input: Rc::new(rendered.input_handlers),
                widget: Rc::new(rendered.widget_handlers),
                render_slot: render_slot.clone(),
            };

            let new_elements = FocusElements {
                input: Rc::new(RefCell::new(Vec::new())),
                widget: Rc::new(RefCell::new(Vec::new())),
            };
            let built = build_element(&document, &rendered.dom, &state, &handlers, &new_elements)?;
            root.append_child(&built)?;
            let (new_input_elements, new_widget_elements) =
                (new_elements.input, new_elements.widget);

            *input_elements.borrow_mut() = new_input_elements.borrow().clone();
            *widget_elements.borrow_mut() = new_widget_elements.borrow().clone();
            restore_focus_state(
                &input_elements.borrow(),
                &widget_elements.borrow(),
                focus_state,
            );

            // 再描画の完了後に effect を呼ぶ（モジュール doc「effect フック」節参照）。
            // effect には借用中の `&S` を渡すため、effect 内から `Applier::apply` が
            // 同期的に呼ばれた場合は `apply` 側がマイクロタスクへ退避して二重借用を
            // 避ける（`Applier::apply` の doc 参照）。
            let applier = Applier::new(
                state.clone(),
                render_slot
                    .borrow()
                    .clone()
                    .ok_or_else(|| JsValue::from_str("render_slot が未初期化"))?,
                mount_id,
            );
            {
                let state_ref = state.borrow();
                effect(&state_ref, &applier);
            }
            Ok(())
        })
    };

    *render_slot.borrow_mut() = Some(render_once.clone());
    render_once()
}

/// 未移行の機能（`fandhe-web-dom` に実装が残っているもの）の破棄時の解放をまとめて呼ぶ。
/// 機能を crate へ移すときは、その行をここから消し、機能側の effect の入口で
/// `fandhe_platform_web::on_mount_release` に登録する。
fn release_features(mount_id: u64) {
    super::file_picker::release_mount(mount_id);
    super::network_status::release_mount(mount_id);
    super::compass::release_mount(mount_id);
    super::ar::release_mount(mount_id);
    super::screen_wake_lock::release_mount(mount_id);
    super::flashlight::release_mount(mount_id);
    super::power_state::release_mount(mount_id);
    super::device_info::release_mount(mount_id);
    super::audio_devices::release_mount(mount_id);
    super::local_fonts::release_mount(mount_id);
    super::eye_dropper::release_mount(mount_id);
    super::color_picker::release_mount(mount_id);
    super::camera_preview_stream::release_mount(mount_id);
    super::multi_display::release_mount(mount_id);
    super::window_management::release_mount(mount_id);
    super::accessibility_prefs::release_mount(mount_id);
    super::system_accent_color::release_mount(mount_id);
    super::safe_area_insets::release_mount(mount_id);
    super::foldable_posture::release_mount(mount_id);
    super::file_association::release_mount(mount_id);
    super::fullscreen_immersive::release_mount(mount_id);
    super::screen_orientation::release_mount(mount_id);
    super::picture_in_picture::release_mount(mount_id);
    super::media_cast::release_mount(mount_id);
    super::screen_capture::release_mount(mount_id);
    super::app_badge::release_mount(mount_id);
    super::app_lifecycle::release_mount(mount_id);
    super::memory_pressure::release_mount(mount_id);
    super::motion_raw_sensors::release_mount(mount_id);
    super::compose_mail_sms::release_mount(mount_id);
    super::external_app_launch::release_mount(mount_id);
    super::display_refresh_hdr::release_mount(mount_id);
    super::print_page::release_mount(mount_id);
    super::passkey::release_mount(mount_id);
    super::digital_credentials::release_mount(mount_id);
    super::nfc::release_mount(mount_id);
    super::autofill_otp::release_mount(mount_id);
    super::payment_request::release_mount(mount_id);
    super::file_watch::release_mount(mount_id);
    super::midi::release_mount(mount_id);
    super::media_playback::release_mount(mount_id);
    super::now_playing::release_mount(mount_id);
    super::media_transcode::release_mount(mount_id);
    super::video_transcode::release_mount(mount_id);
    super::on_device_llm::release_mount(mount_id);
    super::translation::release_mount(mount_id);
    super::pen_stylus::release_mount(mount_id);
    super::background_download::release_mount(mount_id);
    super::user_idle::release_mount(mount_id);
    super::sleep_lock_events::release_mount(mount_id);
    super::serial_port::release_mount(mount_id);
    super::video_capture::release_mount(mount_id);
}

/// [`build_element`] が使うハンドラ一式（`clippy::too_many_arguments` 対策の
/// 引数まとめ、Issue #993 で `widget` を追加した際に導入）。値そのものは
/// [`super::dom_tree::Rendered`] の各フィールドを `Rc` で共有したもの。
struct Handlers<S> {
    click: ClickHandlers<S>,
    input: InputHandlers<S>,
    widget: WidgetHandlers<S>,
    render_slot: Rc<RefCell<Option<RenderTrigger>>>,
}

/// [`build_element`] が直近世代の要素を書き込む先（[`Handlers`] と同じ理由の
/// 引数まとめ）。[`FocusState`] の保存・復元は呼び出し元（[`mount_with_effect`]）が
/// このまとまり越しに参照する。
struct FocusElements {
    input: InputElements,
    widget: WidgetElements,
}

/// [`DomNode`] を実 DOM 要素へ再帰的に構築する。`event_hook` を持つノードには
/// [`attach_click`] / [`attach_input`] / [`attach_change`] でイベントリスナーを配線する。
fn build_element<S: 'static>(
    document: &Document,
    node: &DomNode,
    state: &Rc<RefCell<S>>,
    handlers: &Handlers<S>,
    elements: &FocusElements,
) -> Result<Element, JsValue> {
    let element = document.create_element(&node.tag)?;

    for (name, value) in &node.attrs {
        // `set_attribute` は値をそのまま属性値として設定するのみで HTML として
        // 再解釈しない（`innerHTML` 系 API を使わないため A03 対策になる、モジュール
        // コメント参照）。
        element.set_attribute(name, value)?;
    }

    if let Some(text) = &node.text {
        element.set_text_content(Some(text));
    }

    for child in &node.children {
        let child_element = build_element(document, child, state, handlers, elements)?;
        element.append_child(&child_element)?;
    }

    match node.event_hook {
        Some(EventHook::Click { handler_index }) => {
            attach_click(
                &element,
                handler_index,
                state.clone(),
                handlers.click.clone(),
                handlers.render_slot.clone(),
            )?;
        }
        Some(EventHook::Input { handler_index }) => {
            // `handler_index` の位置へ直接代入する（走査順には依存しない、
            // [`InputElements`] のドキュメンテーションコメント参照）。
            register_at(&elements.input, handler_index, &element);
            attach_input(
                &element,
                handler_index,
                state.clone(),
                handlers.input.clone(),
                handlers.render_slot.clone(),
            )?;
        }
        Some(EventHook::Change {
            handler_index,
            kind,
        }) => {
            // 上と同じ理由で `handler_index` の位置へ直接代入する
            // （[`WidgetElements`] のドキュメンテーションコメント参照）。
            register_at(&elements.widget, handler_index, &element);
            attach_change(
                &element,
                handler_index,
                kind,
                state.clone(),
                handlers.widget.clone(),
                handlers.render_slot.clone(),
            )?;
        }
        None => {}
    }

    Ok(element)
}

/// `elements[handler_index] = Some(element)` を、必要なら `resize` してから行う
/// （[`InputElements`]/[`WidgetElements`] 共通の登録処理、走査順に依存しない）。
fn register_at(
    elements: &Rc<RefCell<Vec<Option<Element>>>>,
    handler_index: usize,
    element: &Element,
) {
    let mut elements = elements.borrow_mut();
    if elements.len() <= handler_index {
        elements.resize(handler_index + 1, None);
    }
    elements[handler_index] = Some(element.clone());
}

/// `click` イベントで `click_handlers[handler_index]`（`Node::Button::on_click`）を
/// 状態へ適用し、適用後に `render_slot` 経由で再レンダリングをトリガーする。
fn attach_click<S: 'static>(
    element: &Element,
    handler_index: usize,
    state: Rc<RefCell<S>>,
    click_handlers: ClickHandlers<S>,
    render_slot: Rc<RefCell<Option<RenderTrigger>>>,
) -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |_event: web_sys::Event| {
        if let Some(handler) = click_handlers.get(handler_index) {
            handler(&mut state.borrow_mut());
        }
        trigger_rerender(&render_slot);
    }) as Box<dyn FnMut(web_sys::Event)>);

    element.add_event_listener_with_callback("click", closure.as_ref().unchecked_ref())?;
    // マウント期間中はイベントリスナーを保持し続ける必要があるため `forget` する
    // （ページ遷移までリークするが、本 sub-issue の最小実装では許容する。
    // 全再構築のたびに新しい Closure を生成するため、長時間の操作でリークが
    // 積み上がる点は S3 以降の差分更新導入時に解消する想定、out-of-scope-tracking.md 対象）。
    closure.forget();
    Ok(())
}

/// `input` イベントで `input_handlers[handler_index]`（`Node::TextField::set`）へ
/// 入力欄の現在値を書き込み、適用後に `render_slot` 経由で再レンダリングをトリガーする。
///
/// IME 変換中（CJK 入力等）は `InputEvent::is_composing()` が `true` を返す。
/// この間に state 反映・全再構築を行うと、変換中の入力要素が破棄・再生成されて
/// 変換セッションが中断されてしまう（`compositionstart`〜`compositionend` の
/// ブラウザ内部状態は再生成後の要素には引き継がれない）。そのため変換中はこの
/// `input` イベントを無視する。
///
/// 変換確定の commit は `compositionend` イベントで行う（このイベントは仕様上
/// 必ず発火する。`isComposing: false` の追従 `input` イベントで commit すると、
/// Chromium / Safari ではその追従 `input` イベントが必ず発火するとは限らず、
/// 確定済みの IME 変換結果が state へ反映されないまま次の全再構築でフィールドから
/// 消える（PR #341 のレビュー指摘・TASK-4.4-S2 / REQ-4）。`compositionend` と
/// 後続 `input`（isComposing: false）が
/// 両方発火するブラウザでは commit が 2 回走るが、同一値の再代入・再描画で
/// 副作用はなく安全側に倒している。
///
/// `compositionstart`/`compositionupdate`/`compositionend` の 3 イベントはいずれも
/// [`super::ime::ime_event_from_composition`] を単一の変換経路として経由させる
/// （TASK-8.5-S21 / REQ-8、Issue #202、`ime-architecture.md` §4.5）。ただし本レンダラは
/// DOM `<input>` へブラウザの IME を素通しする方式であり、`<input>` 要素自体が
/// preedit をネイティブ表示するため、`ImeEvent::Enabled`（`compositionstart`）・
/// `ImeEvent::Preedit`（`compositionupdate`）はコア描画へ反映せず no-op とする
/// （コア側で `Preedit` を描画すると二重表示になる、`ime.rs` モジュール doc「二重表示の
/// 回避」参照）。`ImeEvent::Commit`（`compositionend`）到達時のみ既存の `commit`
/// クロージャを呼ぶが、その際に運ぶ値は `ImeEvent::Commit` が保持する合成断片
/// （`CompositionEvent.data`）ではなく、従来どおり `input.value()`（`Node::TextField::set`
/// が全文置換型のため全文が必要、`ime.rs` モジュール doc「commit 値の扱い」参照）。
/// この配線判断により、既存の観測可能な挙動（`isComposing` スキップ + `compositionend`
/// commit）は変更しない。
fn attach_input<S: 'static>(
    element: &Element,
    handler_index: usize,
    state: Rc<RefCell<S>>,
    input_handlers: InputHandlers<S>,
    render_slot: Rc<RefCell<Option<RenderTrigger>>>,
) -> Result<(), JsValue> {
    let commit = {
        let state = state.clone();
        let input_handlers = input_handlers.clone();
        let render_slot = render_slot.clone();
        move |event: &web_sys::Event| {
            let value = event
                .target()
                .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
                .map(|input| input.value())
                .unwrap_or_default();

            if let Some(handler) = input_handlers.get(handler_index) {
                handler(&mut state.borrow_mut(), value);
            }
            trigger_rerender(&render_slot);
        }
    };

    let input_commit = commit.clone();
    let input_closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        let is_composing = event
            .dyn_ref::<InputEvent>()
            .map(InputEvent::is_composing)
            .unwrap_or(false);
        if is_composing {
            return;
        }
        input_commit(&event);
    }) as Box<dyn FnMut(web_sys::Event)>);
    element.add_event_listener_with_callback("input", input_closure.as_ref().unchecked_ref())?;
    input_closure.forget();

    // `compositionstart`/`compositionupdate`: `ime_event_from_composition` を経由させて
    // `Enabled`/`Preedit` を得るが、二重表示回避の設計判断（上のドキュメントコメント参照）
    // により、いずれもコア描画へは反映しない（no-op）。将来 preedit のデザイントークン化・
    // 装飾等でコア側の反応が必要になった場合の配線点として、変換自体はここで確定させておく。
    for event_type in ["compositionstart", "compositionupdate"] {
        let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
            let Some(phase) = composition_phase_from_event_type(&event.type_()) else {
                return;
            };
            let data = event
                .dyn_ref::<CompositionEvent>()
                .and_then(|composition_event| composition_event.data())
                .unwrap_or_default();
            match ime_event_from_composition(phase, &data) {
                ImeEvent::Enabled | ImeEvent::Preedit { .. } => {
                    // 二重表示回避のため no-op（上のドキュメントコメント参照）。
                }
                ImeEvent::Commit(_) | ImeEvent::Disabled => {
                    // compositionstart/compositionupdate からは到達しない
                    // （`ime_event_from_composition` の対応表、`ime.rs` 参照）。
                }
            }
        }) as Box<dyn FnMut(web_sys::Event)>);
        element.add_event_listener_with_callback(event_type, closure.as_ref().unchecked_ref())?;
        closure.forget();
    }

    // `compositionend`: IME 変換確定を確実に state へ commit する（上のドキュメント
    // コメント参照）。`isComposing: false` の `input` イベントの発火有無に依存しない。
    // `ime_event_from_composition` は `CompositionPhase::End` を必ず `ImeEvent::Commit`
    // へ変換する（`ime.rs` の対応表参照。`End` から到達しうる分岐は他にないため、ここでの
    // `match` によるバリアント判定は行わない）。実際に反映する値は合成断片ではなく
    // `input.value()`（全文、上のドキュメントコメント「commit 値の扱い」参照）とするため、
    // `commit` クロージャへは素通しする。
    let composition_closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        commit(&event);
    }) as Box<dyn FnMut(web_sys::Event)>);
    element.add_event_listener_with_callback(
        "compositionend",
        composition_closure.as_ref().unchecked_ref(),
    )?;
    composition_closure.forget();

    Ok(())
}

/// `change` イベントで `widget_handlers[handler_index]`（`sync_from_state` 済みの
/// `Checkbox`/`ToggleSwitch`/`Slider`/`RadioButton` 複製、Issue #993）の
/// [`Widget::apply_value`] へ `HtmlInputElement` の現在値を適用し、適用後に
/// `render_slot` 経由で再レンダリングをトリガーする。
///
/// **嘘の成功を作らない**（`.claude/rules/device-bridge-contracts.md` 6 節）:
/// `disabled`・数値の解析失敗・非有限値（`NaN`/`inf`）はいずれも `apply_value` を
/// 呼ばずに握りつぶし、状態を変更しない。適用の成否にかかわらず**必ず**
/// `trigger_rerender` する（拒否された操作は、状態から作り直した DOM で
/// 元の見た目に戻る。ブラウザのネイティブ部品は操作直後に見た目が先に変わるため、
/// 再描画を省くと拒否時に見た目だけ変わったまま残ってしまう）。
fn attach_change<S: 'static>(
    element: &Element,
    handler_index: usize,
    kind: WidgetValueKind,
    state: Rc<RefCell<S>>,
    widget_handlers: WidgetHandlers<S>,
    render_slot: Rc<RefCell<Option<RenderTrigger>>>,
) -> Result<(), JsValue> {
    let closure = Closure::wrap(Box::new(move |event: web_sys::Event| {
        if let Some(widget) = widget_handlers.get(handler_index) {
            // disabled ガード: `fandhe_core::ui::set_widget_value_at` が `path` 解決後に
            // 行うのと同じ確認を、`handler_index` 直引きの本経路でも複製の
            // `interaction_state()` から再現する（PR #808 codex-review P1 と同じ観点）。
            if !widget.interaction_state().disabled {
                let input = event
                    .target()
                    .and_then(|target| target.dyn_into::<HtmlInputElement>().ok());
                let value = input.and_then(|input| match kind {
                    WidgetValueKind::Bool => Some(WidgetValue::Bool(input.checked())),
                    WidgetValueKind::Number => {
                        // 解析失敗・非有限値（`NaN`/`inf`）は信頼境界の入力検証として
                        // 適用しない（A03 対策、`Slider::apply_value` の `NaN` ガードと
                        // 同じ設計判断をブラウザ入力の解析時点でも行う）。
                        let parsed: f32 = input.value().parse().ok()?;
                        parsed.is_finite().then_some(WidgetValue::Number(parsed))
                    }
                    WidgetValueKind::Index(index) => Some(WidgetValue::Index(index)),
                });
                if let Some(value) = value {
                    widget.apply_value(&mut state.borrow_mut(), &value);
                }
            }
        }
        trigger_rerender(&render_slot);
    }) as Box<dyn FnMut(web_sys::Event)>);

    element.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref())?;
    // マウント期間中はイベントリスナーを保持し続ける必要があるため `forget` する
    // （`attach_click`/`attach_input` と同じ設計判断、モジュールコメント参照）。
    closure.forget();
    Ok(())
}

/// `render_slot` に詰まっている再レンダリング関数を呼び出す。エラーはブラウザ
/// コンソールへ出力するのみに留める（イベントハンドラのシグネチャ上、呼び出し元へ
/// `Result` を伝播できないため。panic を API 契約にしない方針、coding-rust.md 準拠）。
fn trigger_rerender(render_slot: &Rc<RefCell<Option<RenderTrigger>>>) {
    let trigger = render_slot.borrow().clone();
    if let Some(trigger) = trigger {
        if let Err(err) = trigger() {
            web_sys::console::error_1(&format!("再レンダリングに失敗した: {err:?}").into());
        }
    }
}
