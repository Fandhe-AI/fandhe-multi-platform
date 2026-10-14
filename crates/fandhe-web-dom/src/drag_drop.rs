//! 型サンプル「ドラッグ & ドロップ受け取り」の Web 経路（親 #1013、Issue #1017）。
//!
//! `fandhe_core::screens::drag_drop`（端末依存ゼロ）の状態を
//! [`super::mount::mount_with_effect`] の effect として観測し、`document` の
//! `dragenter`/`dragover`/`dragleave`/`drop`（HTML Drag and Drop API）から
//! ホバー開始・取り消し・受け取りを反映する。デスクトップ版
//! （`crates/fandhe-egui-proxy/src/device/drag_drop.rs`）と同じ画面状態機械を使うが、
//! egui の `RawInput::hovered_files`/`dropped_files` を経由しないため配線は別実装になる
//! （`tilt.rs`/`location.rs` と同じ理由。`fandhe-egui-proxy` の feature `device` は
//! wasm32 に依存を持ち込まない設計）。
//!
//! # ドロップ領域（文書全体）
//!
//! `fandhe_core::screens::drag_drop` の画面 doc（ウィンドウ全体がドロップ領域）に
//! 合わせ、リスナーは `document` に付ける。`mount.rs` は再描画のたびに root 配下を
//! 全再構築するため、子要素にリスナーを付けても再描画のたびに消えてしまう。
//! `document` に付けることで、領域外への drop でブラウザがファイルを開いて
//! ページを離れる既定動作も `preventDefault` で一緒に止まる。
//!
//! # ホバー中の抑止（`dragover` の高頻度発火）
//!
//! `dragover` はホバー中数十 ms ごとに発火するため、[`Entry::hovering`] が既に
//! `true` の間は [`fandhe_core::screens::drag_drop::begin_hover`] を呼ばない
//! （毎回呼ぶと全再構築が無駄に走る、契約表 4 相当）。
//!
//! # 取り消し判定（`dragleave`）
//!
//! `related_target()` が `None` であることに加え、`clientX`/`clientY` が
//! ビューポート範囲外であることを確認できたときだけ取り消しとみなす
//! （`web::is_outside_viewport`、`wasm32` gate 内のため doc link は張らない）。
//! `dragenter` の [`begin_hover`] は
//! `mount.rs` の全再構築を伴うため、ポインター下の要素がその再構築で
//! 消えると `relatedTarget` が `null` の `dragleave` が飛ぶ（ポインターは
//! 文書内に留まったまま）。`relatedTarget == null` だけで判定すると、この
//! 疑似 leave を実際の領域外移動と誤認し、直後の `dragover` で再び
//! `begin_hover` が呼ばれてちらつきと再構築の反復が起きる（PR #1020
//! レビュー指摘）。座標がビューポート内なら DOM 再構築由来とみなし無視する。
//! 子要素間の移動（`relatedTarget` が文書内の要素）も同様に無視する。
//! enter/leave のカウンタ方式は、全再構築で要素が消えると leave が来ずに
//! 値がずれるため採らない。
//! `relatedTarget` を常に `null` で送り、かつ実際の領域外移動でも
//! `clientX`/`clientY` を文書内の値のまま送る古い WebKit では、この座標判定でも
//! ホバーがちらつき得る（out-of-scope-tracking.md 対象、実機確認は人間担当）。
//!
//! # Esc によるドラッグ取り消し（`keydown`）
//!
//! Esc（キーボード操作）でドラッグを取り消すと、`dragleave` が飛ぶ座標は
//! （ポインターが移動していないため）ビューポート内のままで上記の座標判定に
//! 掛からず、`hovering` が取り消し後も残ってしまう（PR #1020 レビュー指摘）。
//! 当初は `dragend` を併用していたが、`dragend` は仕様上ドラッグ「元」の
//! ノードでのみ発火するイベントであり、Finder/エクスプローラ等 OS 側が起点の
//! 外部ドラッグでは文書側にドラッグ元が存在しないため `dragend` が一切届かない
//! （codex 指摘・cursor[bot] 指摘、PR #1020）。座標にもドラッグ元にも依存しない
//! 手段として、`document` の `keydown` で `Escape` を検知し、`hovering` なら
//! 直接 [`cancel_hover`] を送る（[`handle_keydown`]）。ドロップ成立時は
//! `drop`（`handle_drop`）が `hovering` を既に `false` に倒しているため、
//! ドロップ後に Esc 以外のキーが押されても再入しない（`hovering` フラグで防ぐ）。
//!
//! ただし OS 側が起点の外部ドラッグ中は、OS がキー入力を横取りして
//! `keydown` そのものが文書に届かないことがある（codex 指摘、PR #1020）。
//! 座標にもキー入力にも依存しない最終防波堤として、`dragover` が一定時間
//! 途切れたらセッション終了とみなす監視タイマー（[`handle_watchdog`]）を
//! 別途持つ。詳細はそのドキュメンテーションコメントを参照。
//!
//! # 世代管理（契約表 2、`fandhe-egui-proxy::device::drag_drop` と同じ考え方）
//!
//! `drop` イベントの同期ハンドラ内でのみ [`Entry::generation`] を書き換える
//! （`begin_drop_read` を呼ぶのもこのハンドラ）ため、ローカルの世代カウンタは
//! 常に `DragDropState::generation` と一致する。読み取り本体は `spawn_local` で
//! 1 件ずつ順に読み、各 `await` の後にローカルの世代と突き合わせて、後続の
//! ドロップに追い越されていれば残りを読まずに切り上げる。完了の反映は
//! [`fandhe_core::screens::drag_drop::is_current_generation`] で最終照合する
//! （古い世代の完了通知を捨てる、契約表 2）。
//!
//! # 登録失敗時（契約表 9）
//!
//! 4 本のリスナーのうちどれか 1 本でも `addEventListener` に失敗したら、
//! 登録済みの分を外して `Closure` を捨て、[`fandhe_core::screens::drag_drop::
//! FAILED_PREFIX`] 付きのエラーへ倒す（嘘の成功を出さない、契約表 6）。
//! このとき `ENTRIES` のエントリは削除せず終端状態として残す
//! （削除すると次の再描画で未初期化と誤認され、`register` を呼び直して
//! 同じ失敗を無限に繰り返す、契約表 4・9）。
//!
//! # 純関数を `wasm32` gate の外に置く理由
//!
//! [`precheck`]/[`check_len`]/[`assemble`] はブラウザ API を呼ばない判定・組み立て
//! ロジックのみを持つ。`tilt.rs`/`location.rs` と同じ観点で native の `cargo test`
//! から固定できるようにするため、ブラウザ API を直接使う部分（[`mod@web`]）だけを
//! `wasm32` へ gate する。

use fandhe_core::screens::drag_drop::DropItem;
use fandhe_device::drag_drop::{DropError, MAX_FILE_BYTES};

/// 名前が取れないときの表示名（`fandhe-egui-proxy::device::drag_drop::display_name` と
/// 同じ文言。フルパスは扱わないため衝突の心配はない、security.md A09）。
// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const NO_NAME: &str = "（名前なし）";

/// 非対応・リスナー登録失敗時の案内（`error` へ `FAILED_PREFIX` 付きで出す）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) const UNAVAILABLE: &str = "このブラウザではドラッグ & ドロップを使えません";

/// 1 件の事前判定（フォルダ・読む前のサイズ上限）。`file_picker.rs::read` の
/// 「読む前の事前判定」と同じ役割（TOCTOU 対策ではなく、巨大ファイルの
/// `ArrayBuffer` 確保を避ける最適化、file_picker.rs モジュール doc 参照）。
// native ビルドでは `mod web`（唯一の呼び出し元）が存在しないため未使用警告が出る
// （`tilt.rs`/`location.rs` と同じ理由）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn precheck(is_directory: bool, size: f64) -> Result<(), DropError> {
    if is_directory {
        return Err(DropError::Folder);
    }
    if size > MAX_FILE_BYTES as f64 {
        return Err(DropError::TooLarge);
    }
    Ok(())
}

/// 読んだ後の再確認（`ArrayBuffer::byte_length` の実測値）。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn check_len(len: usize) -> Result<usize, DropError> {
    if len > MAX_FILE_BYTES {
        Err(DropError::TooLarge)
    } else {
        Ok(len)
    }
}

/// [`fandhe_device::drag_drop::MAX_DROP_FILES`] 超過分を末尾へ 1 行（
/// `DropError::TooMany`）へ集約する（`fandhe-egui-proxy::device::drag_drop::read_all`
/// の集約と同じ形。置き場の共通クレートが無いため複製する、実装計画 §9 スコープ外）。
/// `overflow == 0` なら何もしない。
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) fn assemble(mut items: Vec<DropItem>, overflow: usize) -> Vec<DropItem> {
    if overflow > 0 {
        items.push(DropItem {
            name: format!("（他 {overflow} 件）"),
            result: Err(DropError::TooMany.to_string()),
        });
    }
    items
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use fandhe_core::screens::drag_drop::{
        begin_drop_read, begin_hover, cancel_hover, finish_drop_with_overflow,
        is_current_generation, DragDropState, DropItem, FAILED_PREFIX,
    };
    use fandhe_device::drag_drop::{DropError, MAX_DROP_FILES};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{DataTransfer, DragEvent, Event, File, KeyboardEvent, MouseEvent};

    use super::{assemble, check_len, precheck, NO_NAME, UNAVAILABLE};
    use crate::mount::Applier;

    type EventClosure = Closure<dyn FnMut(Event)>;
    /// [`schedule_raf_watchdog`] の自己再スケジュールクロージャを保持するスロット。
    type RafWatchdogSlot = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

    /// `dragover` が来なくなってから、ドラッグセッション終了（取り消し含む）と
    /// みなすまでの猶予（[`handle_watchdog`] 参照）。
    const DRAGOVER_STALE_MS: f64 = 700.0;
    /// 監視タイマーの実行間隔（[`handle_watchdog`] 参照）。
    const DRAGOVER_WATCHDOG_MS: i32 = 300;

    /// マウント単位の状態。`on_*` はページが生きている間保持し続ける
    /// （モジュール冒頭「登録失敗時」節参照。アンマウント API が無いため 1 マウント
    /// あたり最大 3 個で打ち止め、`tilt.rs`/`gamepad.rs` と共通の天井）。
    #[derive(Default)]
    struct Entry {
        /// ドラッグ中のファイルが文書上にあるか（`dragover` の高頻度発火を
        /// [`begin_hover`] へ間引くためだけに使う、モジュール doc 参照）。
        hovering: bool,
        /// 直近のドロップで払い出した世代番号。このハンドラだけが書き換えるため、
        /// `DragDropState::generation` と常に一致する（モジュール doc 参照）。
        generation: u64,
        /// 直近に `dragenter`/`dragover` を受け取った時刻（`Date::now()`、ms）。
        /// [`handle_watchdog`] が「`dragover` が一定時間来ない」＝セッション終了
        /// （取り消し含む）を検知するために使う。
        last_dragover_ms: f64,
        on_hover: Option<EventClosure>,
        on_leave: Option<EventClosure>,
        on_keydown: Option<EventClosure>,
        on_drop: Option<EventClosure>,
        /// [`handle_watchdog`] を一定間隔で呼ぶ `setInterval` のクロージャ
        /// （`on_*` と同じ理由でページが生きている間保持し続ける）。
        on_watchdog: Option<Closure<dyn FnMut()>>,
        /// `setInterval` の登録に失敗し、[`schedule_raf_watchdog`] による
        /// `requestAnimationFrame` 監視へ切り替える必要があるか（[`register`] 参照）。
        raf_watchdog_fallback: bool,
        /// [`schedule_raf_watchdog`] の自己再スケジュールループが現在回っているか。
        /// ホバー中だけ回す設計のため、二重起動防止とホバー再開時の再始動判定に使う
        /// （codex 指摘、PR #1020。「ドラッグが一度も開始されない/終了後も毎フレーム
        /// 回り続ける」の修正）。
        raf_watchdog_running: bool,
    }

    thread_local! {
        /// マウント ID ごとの登録状態（`tilt.rs::ENTRIES` と同じくマウント単位）。
        /// エントリの有無は「このマウントが初期化済みか」の判定にも使う
        /// （[`drag_drop_effect`] 参照）。
        static ENTRIES: RefCell<HashMap<u64, Entry>> = RefCell::new(HashMap::new());
    }

    /// ドロップされた 1 件の候補（`DataTransferItem` から同期的に取り出した情報）。
    /// `DataTransfer` の中身はイベントが終わると消えるため、`File`/フォルダ判定は
    /// `drop` ハンドラの同期処理内で確定させ、非同期の読み取り本体へはこの型で渡す。
    struct Candidate {
        is_directory: bool,
        /// `webkit_get_as_entry()` が返す `FileSystemEntry::name()`。フォルダは
        /// `get_as_file()` が `None` を返すため `file` から名前を取れないことが多く、
        /// この値が失敗行の表示名を埋める唯一の手段になる（cursor[bot] 指摘・codex
        /// 指摘、PR #1020。以前はここを捨てており、フォルダの失敗行が
        /// [`NO_NAME`] 固定になっていた）。
        entry_name: Option<String>,
        file: Option<File>,
    }

    /// 失敗行に出す表示用の名前。`File::name()` を優先し、空またはファイルが無い
    /// （フォルダ等）場合は `entry_name`（[`Candidate::entry_name`]）へ落ちる。
    /// どちらも取れなければ [`NO_NAME`]（フルパスは扱わない、security.md A09）。
    fn display_name(file: Option<&File>, entry_name: Option<&str>) -> String {
        if let Some(name) = file.map(|f| f.name()).filter(|name| !name.is_empty()) {
            return name;
        }
        if let Some(name) = entry_name.filter(|name| !name.is_empty()) {
            return name.to_string();
        }
        NO_NAME.to_string()
    }

    /// `dragleave` の座標がビューポート範囲外か（モジュール doc「取り消し判定」節
    /// 参照）。`window` / `inner_width`/`inner_height` が取れない場合は判定不能
    /// として「範囲外ではない」（= 取り消さない）側に倒す。DOM 再構築による疑似
    /// leave をホバー取り消しとして誤発火させるより、実際の領域外移動を 1 回
    /// 見逃す方が実害が小さいため（表示のちらつき防止を優先、契約表 4 相当）。
    fn is_outside_viewport(x: i32, y: i32) -> bool {
        // 取得不能（`window` 自体が無い、`inner_width`/`inner_height` が失敗・
        // 数値化できない）を幅・高さ 0 へ丸めると、ほぼ全ての座標が
        // `x >= width`/`y >= height` で「領域外」と誤判定される（cursor[bot]
        // 指摘、PR #1020）。判定不能はどちらも「範囲外ではない」側へ倒す
        // （関数冒頭のコメントと同じ方針、疑似 leave の誤発火を避ける）。
        let Some(window) = web_sys::window() else {
            return false;
        };
        let Some(width) = window.inner_width().ok().and_then(|v| v.as_f64()) else {
            return false;
        };
        let Some(height) = window.inner_height().ok().and_then(|v| v.as_f64()) else {
            return false;
        };
        let (x, y) = (x as f64, y as f64);
        x < 0.0 || y < 0.0 || x >= width || y >= height
    }

    /// `mount_id` の世代が `generation` と一致するか（追い越し検知、モジュール doc 参照）。
    fn is_latest(mount_id: u64, generation: u64) -> bool {
        ENTRIES.with(|cell| {
            cell.borrow()
                .get(&mount_id)
                .map(|e| e.generation == generation)
                .unwrap_or(false)
        })
    }

    /// `mount_id` で現在ドラッグがホバー中か（[`Entry::hovering`]、モジュール
    /// doc 参照）。[`handle_drop`] の完了クロージャが、先行ドロップの読み取り中に
    /// 割り込んだ別のドラッグのホバー状態を上書きしないための読み取り専用問い合わせ
    /// （codex 指摘、PR #1020）。
    fn is_hovering(mount_id: u64) -> bool {
        ENTRIES.with(|cell| {
            cell.borrow()
                .get(&mount_id)
                .map(|e| e.hovering)
                .unwrap_or(false)
        })
    }

    /// 1 件の候補を読み切る。フォルダ判定は `file` の有無より先に見る
    /// （フォルダは `get_as_file` が `None` を返すことが多く、判定順が逆だと
    /// `DropError::Folder` ではなく `Unreadable` を返してしまうため、契約表 6）。
    /// `file` が無い（`getAsFile` 失敗）場合は `Unreadable`。
    async fn read_candidate(candidate: Candidate) -> DropItem {
        let name = display_name(candidate.file.as_ref(), candidate.entry_name.as_deref());
        if candidate.is_directory {
            return DropItem {
                name,
                result: Err(DropError::Folder.to_string()),
            };
        }
        let Some(file) = candidate.file else {
            return DropItem {
                name,
                result: Err(DropError::Unreadable.to_string()),
            };
        };
        if let Err(e) = precheck(candidate.is_directory, file.size()) {
            return DropItem {
                name,
                result: Err(e.to_string()),
            };
        }
        let raw = match JsFuture::from(file.array_buffer()).await {
            Ok(raw) => raw,
            Err(_) => {
                return DropItem {
                    name,
                    result: Err(DropError::Unreadable.to_string()),
                }
            }
        };
        let Ok(buffer) = raw.dyn_into::<js_sys::ArrayBuffer>() else {
            return DropItem {
                name,
                result: Err(DropError::Unreadable.to_string()),
            };
        };
        match check_len(buffer.byte_length() as usize) {
            Ok(len) => DropItem {
                name,
                result: Ok(len),
            },
            Err(e) => DropItem {
                name,
                result: Err(e.to_string()),
            },
        }
    }

    /// `document` に文書全体をドロップ領域とするリスナー 4 本（`dragenter`/`dragover`
    /// は共有クロージャ）を 1 度だけ登録する。登録に失敗したら登録済みの分を外して
    /// `Closure` を捨て、失敗を表示する（契約表 9・6、モジュール doc 参照）。
    fn register(mount_id: u64, applier: Applier<DragDropState>) {
        ENTRIES.with(|cell| {
            cell.borrow_mut().insert(mount_id, Entry::default());
        });

        let Some(window) = web_sys::window() else {
            applier.apply(|s| s.error = format!("{FAILED_PREFIX}{UNAVAILABLE}"));
            return;
        };
        let Some(document) = window.document() else {
            applier.apply(|s| s.error = format!("{FAILED_PREFIX}{UNAVAILABLE}"));
            return;
        };

        let on_hover: EventClosure = {
            let applier = applier.clone();
            Closure::new(move |event: Event| handle_hover(mount_id, event, &applier))
        };
        let on_leave: EventClosure = {
            let applier = applier.clone();
            Closure::new(move |event: Event| handle_leave(mount_id, event, &applier))
        };
        // Esc によるドラッグ取り消しを拾うための 4 本目のリスナー
        // （モジュール doc「Esc によるドラッグ取り消し」節参照）。
        let on_keydown: EventClosure = {
            let applier = applier.clone();
            Closure::new(move |event: Event| handle_keydown(mount_id, event, &applier))
        };
        let on_drop: EventClosure = {
            let applier = applier.clone();
            Closure::new(move |event: Event| handle_drop(mount_id, event, &applier))
        };

        let registered = document
            .add_event_listener_with_callback("dragenter", on_hover.as_ref().unchecked_ref())
            .and_then(|()| {
                document
                    .add_event_listener_with_callback("dragover", on_hover.as_ref().unchecked_ref())
            })
            .and_then(|()| {
                document.add_event_listener_with_callback(
                    "dragleave",
                    on_leave.as_ref().unchecked_ref(),
                )
            })
            .and_then(|()| {
                document.add_event_listener_with_callback(
                    "keydown",
                    on_keydown.as_ref().unchecked_ref(),
                )
            })
            .and_then(|()| {
                document.add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref())
            })
            .is_ok();

        if !registered {
            let _ = document.remove_event_listener_with_callback(
                "dragenter",
                on_hover.as_ref().unchecked_ref(),
            );
            let _ = document
                .remove_event_listener_with_callback("dragover", on_hover.as_ref().unchecked_ref());
            let _ = document.remove_event_listener_with_callback(
                "dragleave",
                on_leave.as_ref().unchecked_ref(),
            );
            let _ = document.remove_event_listener_with_callback(
                "keydown",
                on_keydown.as_ref().unchecked_ref(),
            );
            let _ = document
                .remove_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref());
            // ENTRIES からは外さない（終端状態として保持する）。
            // ここで remove すると `drag_drop_effect` の
            // `already_initialized` 判定が次回の再描画で false に戻り、
            // `register` が呼び直されて同じ失敗を無限に繰り返す。
            applier.apply(|s| s.error = format!("{FAILED_PREFIX}{UNAVAILABLE}"));
            return;
        }

        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                entry.on_hover = Some(on_hover);
                entry.on_leave = Some(on_leave);
                entry.on_keydown = Some(on_keydown);
                entry.on_drop = Some(on_drop);
            }
        });

        // OS 起点のドラッグ取り消しを検知する監視タイマー（[`handle_watchdog`]
        // 参照）。4 本のリスナーとは異なり、登録に失敗しても本来のドラッグ &
        // ドロップ機能（受け取り・キャンセル自体）は成立するため、失敗時は
        // `FAILED_PREFIX` エラーへは倒さない（契約表 6 は「嘘の成功」を禁じる
        // ものであり、本体機能が実際に動く状態で補助機能の欠落をエラー扱いに
        // する必要はない）。ただし監視を諦めるだけだと、OS 側が起点の外部
        // ドラッグで keydown も届かない環境では hovering が取り消し後も残り
        // 続ける唯一の検知経路が失われる（codex 指摘、PR #1020）。
        // `requestAnimationFrame` は `setInterval` とは別の API（CSP 等の
        // 失敗要因を必ずしも共有しない）のため、代替の終了検知として使う。
        let on_watchdog: Closure<dyn FnMut()> = {
            let applier = applier.clone();
            Closure::new(move || handle_watchdog(mount_id, &applier))
        };
        if window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                on_watchdog.as_ref().unchecked_ref(),
                DRAGOVER_WATCHDOG_MS,
            )
            .is_ok()
        {
            ENTRIES.with(|cell| {
                if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                    entry.on_watchdog = Some(on_watchdog);
                }
            });
        } else {
            // ここでは開始しない。ホバー中しか意味の無い監視のため、
            // フラグだけ立てて実際の起動は [`handle_hover`] がホバー開始時に行う
            // （[`schedule_raf_watchdog`] 参照、codex 指摘 PR #1020）。
            ENTRIES.with(|cell| {
                if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                    entry.raf_watchdog_fallback = true;
                }
            });
        }
    }

    /// [`register`] の `setInterval` 登録が失敗した場合の代替監視ループ。
    /// [`handle_watchdog`] と同じ判定を `requestAnimationFrame` の自己再スケジュール
    /// で回す（登録失敗を代替の終了検知なしに黙って諦めない、codex 指摘 PR #1020）。
    ///
    /// ホバー中（[`Entry::hovering`]）だけ回す。ドラッグが一度も始まらない、
    /// または終了した後まで毎フレーム回り続けると無駄な `handle_watchdog` 呼び出しが
    /// 続くため（codex 指摘、PR #1020）、[`handle_hover`] がホバー開始のたびに
    /// この関数を呼んで起動し、tick 自身がホバー終了を検知したら自己再スケジュール
    /// をやめて [`Entry::raf_watchdog_running`] を下ろす。次にホバーが始まれば
    /// [`handle_hover`] が再びこの関数を呼んで起動し直す。[`Entry::raf_watchdog_running`]
    /// で二重起動を防ぐ。
    ///
    /// 自己再スケジュールのクロージャは `Rc<RefCell>` で自身を捕捉する定番の
    /// wasm-bindgen 手筋。`Entry` へは保持させず、この自己参照サイクルだけで
    /// 生存させる（アンマウント API が無く `on_*` もページ生存中ずっと保持する
    /// 設計、モジュール冒頭「登録失敗時」節参照）。`window`/`requestAnimationFrame`
    /// 自体が取れない場合はループを開始しない（多重で失敗を諦めるだけであり、
    /// 本体機能には影響しない）。ループ終了時（tick 内で自己再スケジュールを
    /// やめる分岐）に `slot` へ `None` を代入してこの循環を切る。切らないと
    /// `slot`（`Closure` と捕捉した `applier`）の参照カウントが 0 にならず、
    /// ドラッグを繰り返すたびに蓄積する（codex 指摘、PR #1020）。
    fn schedule_raf_watchdog(mount_id: u64, applier: Applier<DragDropState>) {
        let already_running = ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let Some(entry) = map.get_mut(&mount_id) else {
                return true;
            };
            std::mem::replace(&mut entry.raf_watchdog_running, true)
        });
        if already_running {
            return;
        }

        let slot: RafWatchdogSlot = Rc::new(RefCell::new(None));
        let slot_for_tick = slot.clone();
        *slot.borrow_mut() = Some(Closure::new(move || {
            handle_watchdog(mount_id, &applier);
            if is_hovering(mount_id) {
                if let Some(window) = web_sys::window() {
                    if let Some(tick) = slot_for_tick.borrow().as_ref() {
                        if window
                            .request_animation_frame(tick.as_ref().unchecked_ref())
                            .is_ok()
                        {
                            return;
                        }
                    }
                }
            }
            // ホバーが終わった、または `requestAnimationFrame` の再登録に
            // 失敗した場合はここでループを止める。次のホバー開始時に
            // `handle_hover` が改めて起動し直す。
            ENTRIES.with(|cell| {
                if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                    entry.raf_watchdog_running = false;
                }
            });
            // `slot` は自分自身（この `Closure`）を `slot_for_tick` 経由で
            // 捕捉する自己参照サイクルのため、ここで `None` を代入して切らないと
            // Rc の参照カウントが 0 にならず `Closure`（と捕捉した `applier`）が
            // 解放されない（codex 指摘、PR #1020）。ループ終了はここでしか
            // 判定しないため、切る場所もここに限定できる。呼び出し中の
            // `Closure` 自身をここで drop しても、この tick の呼び出しは
            // 既に開始済みで JS 側はこの後この `Closure` を参照しないため安全
            // （wasm-bindgen の自己再スケジュール定番手筋の後始末）。
            *slot_for_tick.borrow_mut() = None;
        }));
        let started = web_sys::window().is_some_and(|window| {
            slot.borrow().as_ref().is_some_and(|tick| {
                window
                    .request_animation_frame(tick.as_ref().unchecked_ref())
                    .is_ok()
            })
        });
        if !started {
            // 初回登録に失敗したらフラグを戻し、自己参照を切って `Closure` を解放する。
            // 戻さないと以後のホバーで二重起動とみなされ再試行できない（codex 指摘、PR #1020）。
            ENTRIES.with(|cell| {
                if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                    entry.raf_watchdog_running = false;
                }
            });
            *slot.borrow_mut() = None;
        }
    }

    /// `dt` が示すドラッグ対象にファイルが含まれるか。`dragenter`/`dragover`
    /// 時点では `types()`（`DataTransfer.types`）に `"Files"` が含まれるかで
    /// 判定する（[`handle_hover`] 参照）。
    ///
    /// `items()` の `kind()`/`type_()` を使わない理由: Safari は `drop` が
    /// 実際に起きるまで `DataTransferItem` の中身を伏せ（プライバシー対策）、
    /// `dragenter`/`dragover` の時点では `kind()` が常に空扱いになる。そのため
    /// `kind() == "file"` で判定すると Finder からの実ファイルドラッグでも
    /// 常に false になり、`prevent_default`/`begin_hover` がスキップされて
    /// ドロップしてもハイライトも着地もせずブラウザが遷移し得る
    /// （cursor[bot] 指摘、PR #1020）。`types` は仕様上どのブラウザでも
    /// `dragenter` の時点から確定しているため、ここでの判定に使う
    /// （`handle_drop` 側は `drop` 時点の `items()` を使うため対象外）。
    fn has_file_item(dt: &DataTransfer) -> bool {
        dt.types()
            .iter()
            .any(|t| t.as_string().as_deref() == Some("Files"))
    }

    /// `dragenter`/`dragover` 共有ハンドラ。既定動作を止め、ドロップ可能を示す
    /// カーソルにし、ホバー未開始のときだけ [`begin_hover`] を送る（モジュール doc
    /// 「ホバー中の抑止」節参照）。
    ///
    /// ファイルを含まないドラッグ（テキスト選択等）は `prevent_default` も
    /// [`begin_hover`] も行わず無視する。ここで `prevent_default` すると
    /// document がそのドラッグの有効な drop target になってしまい、離した際に
    /// `drop` イベントが発火する。`handle_drop` はファイル種別の項目のみを
    /// 候補にするため、ファイルが 1 件も無いドロップは 0 件の読み取り結果となり
    /// 「受け取れるファイルがありませんでした」という誤った失敗表示になる
    /// （PR #1020 レビュー指摘）。
    fn handle_hover(mount_id: u64, event: Event, applier: &Applier<DragDropState>) {
        let event: DragEvent = event.unchecked_into();
        let Some(dt) = event.data_transfer() else {
            return;
        };
        if !has_file_item(&dt) {
            return;
        }
        event.prevent_default();
        dt.set_drop_effect("copy");
        // 監視タイマー（[`handle_watchdog`]）が「セッション終了」と誤判定しない
        // よう、ホバー中かどうかに関わらず毎回時刻を更新する。
        let now = js_sys::Date::now();
        let (already_hovering, raf_watchdog_fallback) = ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let Some(entry) = map.get_mut(&mount_id) else {
                return (false, false);
            };
            entry.last_dragover_ms = now;
            (entry.hovering, entry.raf_watchdog_fallback)
        });
        if already_hovering {
            return;
        }
        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                entry.hovering = true;
            }
        });
        // `setInterval` 監視が使えない環境では、ホバー開始のこのタイミングで
        // `requestAnimationFrame` 監視ループを起動する（常時回さずホバー中だけ
        // 動かす、[`schedule_raf_watchdog`] 参照、codex 指摘 PR #1020）。
        if raf_watchdog_fallback {
            schedule_raf_watchdog(mount_id, applier.clone());
        }
        applier.apply(begin_hover);
    }

    /// `hovering` フラグを `false` へ倒し、直前の値を返す（[`handle_leave`]/
    /// [`handle_keydown`] 共通、モジュール doc「取り消し判定」節参照）。
    fn take_hovering(mount_id: u64) -> bool {
        ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let Some(entry) = map.get_mut(&mount_id) else {
                return false;
            };
            std::mem::take(&mut entry.hovering)
        })
    }

    /// `dragleave` ハンドラ。`relatedTarget` が無く、かつポインター座標が
    /// ビューポート外にあるときだけ取り消しとみなす（モジュール doc
    /// 「取り消し判定」節参照。再描画由来の疑似 leave を区別するため）。
    fn handle_leave(mount_id: u64, event: Event, applier: &Applier<DragDropState>) {
        let event: MouseEvent = event.unchecked_into();
        if event.related_target().is_some() {
            return;
        }
        if !is_outside_viewport(event.client_x(), event.client_y()) {
            return;
        }
        if take_hovering(mount_id) {
            applier.apply(cancel_hover);
        }
    }

    /// `keydown` ハンドラ。`Escape` のときだけ、`hovering` が残っていれば
    /// [`cancel_hover`] を送る（モジュール doc「Esc によるドラッグ取り消し」節参照）。
    /// `dragend` に頼らない理由は同節参照（OS 側が起点の外部ドラッグでは
    /// 文書にドラッグ元が無く `dragend` が届かない）。`drop` 成立時は
    /// `handle_drop` が既に `hovering` を `false` へ倒しているため、
    /// ドロップ後に Esc が押されても何もしない（再入防止）。
    fn handle_keydown(mount_id: u64, event: Event, applier: &Applier<DragDropState>) {
        let event: KeyboardEvent = event.unchecked_into();
        if event.key() != "Escape" {
            return;
        }
        if take_hovering(mount_id) {
            applier.apply(cancel_hover);
        }
    }

    /// `setInterval` から定期的に呼ばれる監視ハンドラ。OS 側が起点の外部ドラッグ
    /// （Finder/エクスプローラ等）を Esc で取り消すと、文書に `keydown` が届くとは
    /// 限らず（ネイティブドラッグ中は OS がキー入力を横取りすることがある）、
    /// `dragleave` もポインターが動いていなければビューポート内判定のまま無視される
    /// ため、[`handle_keydown`]/[`handle_leave`] のどちらにも掛からず `hovering` が
    /// 残り続け得る（codex 指摘、PR #1020）。`dragover` はホバー中 UA が
    /// 継続的に（数百 ms 間隔で）発火させ続ける仕様のため、一定時間
    /// （[`DRAGOVER_STALE_MS`]）途切れたら、経路に依存しない最終防波堤として
    /// セッション終了（取り消し含む）とみなし `hovering` を倒す。
    ///
    /// ponytail: 固定しきい値によるヒューリスティック。`DRAGOVER_STALE_MS` より
    /// `dragover` の発火間隔が長い環境では、実際はまだホバー中のドラッグを
    /// 誤って取り消し扱いにし得る（次の `dragover` で [`handle_hover`] が
    /// 再び `begin_hover` を送り直すため実害は表示のちらつきに留まる）。しきい値の
    /// 妥当性は実機確認で調整する（out-of-scope-tracking.md 対象、実機確認は
    /// 人間担当）。
    fn handle_watchdog(mount_id: u64, applier: &Applier<DragDropState>) {
        let stale = ENTRIES.with(|cell| {
            cell.borrow()
                .get(&mount_id)
                .map(|e| e.hovering && js_sys::Date::now() - e.last_dragover_ms > DRAGOVER_STALE_MS)
                .unwrap_or(false)
        });
        if stale && take_hovering(mount_id) {
            applier.apply(cancel_hover);
        }
    }

    /// `drop` ハンドラ。`DataTransfer` の中身が消える前に候補一覧を同期で確定させ、
    /// 世代を払い出してから非同期で 1 件ずつ読む（モジュール doc「世代管理」節参照）。
    fn handle_drop(mount_id: u64, event: Event, applier: &Applier<DragDropState>) {
        let event: DragEvent = event.unchecked_into();
        // ファイルを含まないドロップ（テキスト等）は受け取りの対象外。状態を変えず、
        // 既定動作も止めずに返す（失敗として表示しない、契約表 6・codex 指摘 PR #1020）。
        // `drop` 時点の `types` はどのブラウザでも確定している（[`has_file_item`] 参照）。
        if !event.data_transfer().is_some_and(|dt| has_file_item(&dt)) {
            if take_hovering(mount_id) {
                applier.apply(cancel_hover);
            }
            return;
        }
        event.prevent_default();
        ENTRIES.with(|cell| {
            if let Some(entry) = cell.borrow_mut().get_mut(&mount_id) {
                entry.hovering = false;
            }
        });

        // MAX_DROP_FILES の上限・超過件数はファイル種別（`kind() == "file"`）の
        // 項目数だけを基準に決める。`kind()` の判定自体は軽量なので全件に対して
        // 行ってよいが、重い処理（フォルダ判定・`get_as_file`）はファイル種別へ
        // 絞り込んだ先頭 MAX_DROP_FILES 件だけに限定する。絞り込み前の
        // `list.length()`（生の全件数、テキスト等の非ファイル項目を含む）を
        // 基準にすると、ファイル以外の項目が先行するドロップでファイルを
        // 一件も読めないまま「超過」と誤判定する（PR #1020 レビュー指摘）。
        // 件数の走査は全件に対して行うが、保持候補（`Vec` へ積む分）は
        // MAX_DROP_FILES 件までに絞る。走査前に一旦 `file_items` へ
        // 全件集めてから `take` すると、数千件ドロップされた際に読み捨てる分の
        // `DataTransferItem` まで一時的に保持してしまう（codex 指摘、PR #1020）。
        let (candidates, overflow): (Vec<Candidate>, usize) = event
            .data_transfer()
            .map(|dt| {
                let list = dt.items();
                let mut file_item_count = 0usize;
                let mut candidates = Vec::with_capacity(MAX_DROP_FILES);
                for item in (0..list.length())
                    .filter_map(|i| list.get(i))
                    .filter(|item| item.kind() == "file")
                {
                    file_item_count += 1;
                    if candidates.len() >= MAX_DROP_FILES {
                        // 件数の走査だけは全件に対して続けるが、保持候補は
                        // ここで打ち止め（`DataTransferItem` を読み捨てる分まで
                        // `Vec` へ積まない、codex 指摘、PR #1020）。
                        continue;
                    }
                    // `entry` は `is_directory` と `name` の両方に使うため、
                    // 判定用に 1 回だけ取り出して使い回す（entry_name を
                    // 捨てて `NO_NAME` 固定になっていた不具合の修正、
                    // cursor[bot] 指摘・codex 指摘、PR #1020）。
                    let entry = item.webkit_get_as_entry().ok().flatten();
                    let is_directory = entry.as_ref().map(|e| e.is_directory()).unwrap_or(false);
                    let entry_name = entry.map(|e| e.name()).filter(|name| !name.is_empty());
                    let file = item.get_as_file().ok().flatten();
                    candidates.push(Candidate {
                        is_directory,
                        entry_name,
                        file,
                    });
                }
                let overflow = file_item_count.saturating_sub(MAX_DROP_FILES);
                (candidates, overflow)
            })
            .unwrap_or_default();
        let generation = ENTRIES.with(|cell| {
            let mut map = cell.borrow_mut();
            let entry = map.entry(mount_id).or_default();
            entry.generation = entry.generation.wrapping_add(1);
            entry.generation
        });

        applier.apply(move |s| begin_drop_read(s, generation));

        let applier = applier.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut items = Vec::new();
            for candidate in candidates.into_iter().take(MAX_DROP_FILES) {
                items.push(read_candidate(candidate).await);
                if !is_latest(mount_id, generation) {
                    break;
                }
            }
            let items = assemble(items, overflow);
            applier.apply(move |s| {
                if is_current_generation(s, generation) {
                    finish_drop_with_overflow(s, items, overflow);
                    // finish_drop_with_overflow は s.hovering を無条件で false に
                    // 倒す。だがこの完了は非同期（読み取り中）に届くため、その間に
                    // 別のファイルが文書上へドラッグされ始めていると、Entry.hovering
                    // （dragover 間引き用のローカル真値）は既に true になっている。
                    // ここで再同期しないと、Entry.hovering == true のため以後の
                    // dragover が begin_hover を呼ばず（handle_hover の間引き）、
                    // 実際にはホバー中なのに s.hovering == false のまま固着する
                    // （codex 指摘、PR #1020）。
                    if is_hovering(mount_id) {
                        begin_hover(s);
                    }
                }
            });
        });
    }

    /// `mount_with_effect` へ渡す effect。`drag_drop` 画面のホバー・ドロップ検知を
    /// 継続駆動する。要求フラグを持たない画面（ホバー・ドロップはユーザー操作
    /// そのもの）のため、「[`ENTRIES`] に既にエントリがあれば以降は初期化しない」
    /// という形で「処理すべき要求が無ければ何もしない」契約を満たす
    /// （`tilt.rs::tilt_ball_effect` と同じ考え方）。
    pub fn drag_drop_effect(_state: &DragDropState, applier: &Applier<DragDropState>) {
        let mount_id = applier.mount_id();
        let already_initialized = ENTRIES.with(|cell| cell.borrow().contains_key(&mount_id));
        if already_initialized {
            return;
        }
        register(mount_id, applier.clone());
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::drag_drop_effect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precheck_rejects_directory() {
        assert_eq!(precheck(true, 0.0), Err(DropError::Folder));
    }

    #[test]
    fn precheck_accepts_file_exactly_at_limit() {
        assert_eq!(precheck(false, MAX_FILE_BYTES as f64), Ok(()));
    }

    #[test]
    fn precheck_rejects_file_over_limit() {
        assert_eq!(
            precheck(false, MAX_FILE_BYTES as f64 + 1.0),
            Err(DropError::TooLarge)
        );
    }

    #[test]
    fn check_len_accepts_exactly_at_limit() {
        assert_eq!(check_len(MAX_FILE_BYTES), Ok(MAX_FILE_BYTES));
    }

    #[test]
    fn check_len_rejects_over_limit() {
        assert_eq!(check_len(MAX_FILE_BYTES + 1), Err(DropError::TooLarge));
    }

    #[test]
    fn assemble_appends_overflow_row_when_positive() {
        let items = vec![DropItem {
            name: "a.txt".to_string(),
            result: Ok(1),
        }];
        let assembled = assemble(items, 1);
        assert_eq!(assembled.len(), 2);
        assert!(assembled[1].name.contains('1'));
        assert!(assembled[1].result.is_err());
    }

    #[test]
    fn assemble_adds_no_row_when_overflow_is_zero() {
        let items = vec![DropItem {
            name: "a.txt".to_string(),
            result: Ok(1),
        }];
        let assembled = assemble(items, 0);
        assert_eq!(assembled.len(), 1);
    }
}
