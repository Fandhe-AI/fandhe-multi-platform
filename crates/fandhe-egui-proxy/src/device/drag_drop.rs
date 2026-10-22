//! Phase 11 の拡充ループ（親 #1013）型サンプル「ドラッグ & ドロップ」（Issue #1014）の
//! effect ドライバ。`fandhe_core::screens::drag_drop`（端末依存ゼロ）へ、egui が
//! 観測したホバー/ドロップのイベントを橋渡しする（`crate::screens::registry` の
//! `effect` 区画から `fn(&S, &CompletionSender<S>)` として呼ばれる契約、`app::EffectFn`
//! 参照）。
//!
//! feature `device`（既定 on）かつ非 wasm32 でのみコンパイル対象になる
//! （`device.rs` モジュール doc と同じ理由）。ドロップ領域はウィンドウ全体
//! （egui の `ViewportBuilder::drag_and_drop` は `None` のまま winit の既定＝有効に
//! 任せている。`main.rs` の変更は不要）。領域外へ出た = ウィンドウ外 =
//! `HoveredFileCancelled` として扱われる。
//!
//! # デスクトップ（macOS/Windows）と iOS/Android で経路が分かれる理由
//!
//! iOS/Android では egui-winit がドラッグ & ドロップイベントを一切発火させないため
//! （[`crate::app::CompletionSender::file_drag`] は常に空を返す）、[`drive`]（本
//! モジュールの経路）をそのまま流用できない。[`drag_drop_effect`] は
//! `target_os` に応じて [`drive`]（デスクトップ）と [`drive_events`] +
//! [`super::drop_session`]（iOS は `super::ios_drag_drop` 経由で `UIDropInteraction`
//! を橋渡し、Issue #1015。Android は `super::android::request_repaint` 呼び出し込みの
//! JNI 受け口が直接 `drop_session` を駆動、Issue #1016）の経路に分かれる。デスクトップの
//! [`drive`] を iOS/Android で呼んではいけない（`file_drag()` が常に「ホバー無し」を
//! 返すため、ホバー中の間、毎フレーム誤って `cancel_hover` が送られてしまう）。Windows の
//! OLE ドロップ登録は winit の既定に任せている（本モジュールは何も登録しない）。
//! Web は後続の兄弟 issue の対象（実装計画 §8 スコープ外）。
//!
//! # egui-winit 0.33.3 の挙動（設計の前提）
//!
//! - `HoveredFile` はホバー一覧へ追加される
//! - `HoveredFileCancelled` はホバー一覧を空にする
//! - `DroppedFile` はホバー一覧を空にしてドロップ一覧へ 1 回だけ積む。
//!   `RawInput::take` がドロップ一覧を移動するため、2 フレーム目以降には現れない
//!
//! # ライフサイクル・並行性（契約表 1・2、Issue #1014 自己点検）
//!
//! デスクトップには Activity 再生成に相当する再生成が無い。ホバー中にウィンドウを
//! 閉じれば画面状態ごと破棄され、「最後に届いた値」を返し続ける経路も無い
//! （ドロップ一覧は 1 フレーム限りの push 型イベント）。
//!
//! 実ファイルの読み取り（最大 [`MAX_DROP_FILES`] 件 ×
//! `fandhe_device::drag_drop::MAX_FILE_BYTES`、合計最大 256 MiB）はワーカー
//! スレッドへ逃がす（低速ディスク・ネットワーク越しのファイルで UI スレッドが
//! 応答しなくなるのを防ぐ、codex 指摘 #1018 対応）。これにより「要求 → 完了」の
//! 往復が生じるため、`fandhe_core::screens::drag_drop::DragDropState::generation`
//! による最小限の世代照合（[`drive`] 参照）が必要になった。二重送信自体は
//! （生成した世代を確認する前段の）[`drive`] の分岐順（ドロップ優先 →
//! ホバー開始 → ホバー取り消し）と `state.hovering` の比較で防ぐ。
//!
//! 読み取りはドロップごとに専用スレッドで行い、同時に生きる読み取りスレッドを
//! [`MAX_READERS`] 本までに抑える（[`ReaderSlot`]）。codex 指摘 #1018 は
//! 「ドロップのたびにスレッドが無制限に増える」と「単一ワーカーだと応答しない
//! ファイル（低速なネットワークドライブ等）の読み取り中に新しいドロップが進まない」の
//! 両方を挙げたため、上限付きで並べる形にした。上限に達している間のドロップは
//! 待たせずに即座に失敗として返す（固まった表示にしない、契約表 2・6）。
//! 追い越された仕事は [`LATEST_GENERATION`] との照合で次のファイル 1 件
//! 読み終えた時点で切り上がり、完了通知も世代照合で捨てられる。
//!
//! ドロップ開始時は [`begin_drop_read`] が前回の結果表示（`items`/`status`/
//! `error`）を即座にクリアする。クリアしないと、読み取りに時間がかかる
//! ドロップの最中、完了まで前回の「受け取りました」という表示が残り続けて
//! 新しい操作の結果と誤認される（レビュー指摘 2 対応）。

use std::fs::File;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use fandhe_core::screens::drag_drop::{
    begin_drop_read, begin_hover, cancel_hover, finish_drop_with_overflow, is_current_generation,
    next_generation, DragDropState, DropItem, FAILED_PREFIX,
};
use fandhe_device::drag_drop::{read_dropped, read_dropped_from, DropError, MAX_DROP_FILES};

use crate::app::CompletionSender;

// `drop_session` はターゲット非依存（`device.rs` の doc 参照）なので、`DropEvent` は
// iOS に限らず常に import する。[`drive_events`] 自体もホストの `cargo test` から
// 常に呼べるようにするため（`drive` と同じ「ロジックはホストの `cargo test` で
// 検証する」方針、`android_gamepad.rs` と同型）。
use super::drop_session::DropEvent;

/// 直近に読み取りスレッドへ渡した世代番号（codex 指摘 #1018 対応）。
///
/// 読み取りスレッド（[`drive`] が立てる）は 1 ファイル読むたびにこの static と
/// 突き合わせ、既に新しいドロップへ差し替えられていれば残りを読まずに抜ける。
/// これにより、処理中に別のドロップで追い越された仕事は「次のファイルの
/// 読み取り 1 回分」で切り上がる（低速な共有ドライブ越しの連続ドロップで
/// 同一ジョブの I/O が積み上がるのを防ぐ）。
///
/// デスクトップ限定（[`drive`] 経路専用）。iOS では読み取りを起動側
/// （`ios_drag_drop.rs::fandhe_rust_drag_drop_item`）が同期に 1 件ずつ処理し、
/// 世代照合は [`super::drop_session::DropSessions`] のセッション id が兼ねるため
/// 参照されない（`cfg_attr` は iOS ビルドでの `dead_code` 誤検知を避けるため）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
static LATEST_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 同時に生きる読み取りスレッドの上限。応答しないファイルの読み取りが残っていても
/// 新しいドロップを進められる本数を確保しつつ、スレッドが無制限に増えないようにする
/// （モジュール doc 参照）。デスクトップ限定（上記 [`LATEST_GENERATION`] と同じ理由）。
// ponytail: 固定の本数上限。応答しない読み取りが上限まで溜まると、それらが返るまで
// 新しいドロップは失敗になる。I/O を中断できる非同期 API へ移すときに見直す。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
const MAX_READERS: usize = 4;

/// 生きている読み取りスレッドの数。デスクトップ限定（上記と同じ理由）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
static ACTIVE_READERS: AtomicUsize = AtomicUsize::new(0);

/// 読み取りスレッド 1 本ぶんの枠。[`ReaderSlot::acquire`] で確保し、drop で返す
/// （スレッドが panic で終わっても枠を返すため RAII にしている）。デスクトップ限定
/// （上記と同じ理由）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
struct ReaderSlot;

impl ReaderSlot {
    /// 上限未満なら枠を 1 つ確保する。上限に達していれば `None`。
    fn acquire() -> Option<Self> {
        ACTIVE_READERS
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < MAX_READERS).then_some(n + 1)
            })
            .ok()
            .map(|_| ReaderSlot)
    }
}

impl Drop for ReaderSlot {
    fn drop(&mut self) {
        ACTIVE_READERS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 読み取りスレッドが上限に達しているときのドロップ結果（1 件の失敗行）。デスクトップ
/// 限定（上記 [`LATEST_GENERATION`] と同じ理由）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
const BUSY_REASON: &str = "前の読み込みが終わっていないため受け取れません";

/// `registry.rs` の `effect` 区画から `drag_drop` 画面用に配線される effect ドライバ。
/// iOS/Android/デスクトップで経路が分かれる（モジュール doc「デスクトップと iOS/Android
/// で経路が分かれる理由」参照）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(unused_variables))]
pub fn drag_drop_effect(state: &DragDropState, sender: &CompletionSender<DragDropState>) {
    #[cfg(target_os = "ios")]
    {
        // 毎フレーム、install 成否に関わらず先に呼ぶ（画面が表示中である証跡を
        // 更新する。`ios_drag_drop::mark_active` doc・codex 指摘 #1019 参照）。
        super::ios_drag_drop::mark_active(sender);
        super::ios_drag_drop::ensure_installed(sender);
        drive_events(sender, super::drop_session::take_events());
    }
    #[cfg(target_os = "android")]
    {
        // Android は取り付け（`setOnDragListener`）を Java 側 `onCreate` が担うため、
        // iOS の `ensure_installed`/`mark_active` に相当する処理は無い（Issue #1016
        // 実装計画 §3-1）。JNI 受け口（`device/android.rs::nativeOnDragDrop*`）が
        // 毎フレームではなくイベント発生時に `super::drop_session` へ push し、そのたびに
        // `crate::android::request_repaint()` を呼ぶため、ここでは溜まったイベントを
        // drain するだけでよい（契約表 4）。
        drive_events(sender, super::drop_session::take_events());
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let (hovering, dropped) = sender.file_drag();
        drive(state, sender, hovering, dropped);
    }
}

/// [`DropEvent`] を状態機械（`fandhe_core::screens::drag_drop`）へ適用する、GUI にも
/// ObjC にも依存しない純粋な分岐部分（iOS 経路）。[`drive`]（デスクトップ経路）と
/// 同様、ホストの `cargo test` で検証できるようにするため、`sender`/`events` を
/// 引数で受け取る形にしている。
///
/// 判定はすべてこの関数の呼び出し時点（`events` を受け取った時点）の最新の `state`
/// に対して行う（契約表 2「並行性・世代」）。`events` は
/// [`super::drop_session::take_events`] が返す順序どおりに 1 件ずつ適用する。iOS/Android
/// 共通（`drag_drop_effect` の両分岐から呼ばれる）。
#[cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]
fn drive_events(sender: &CompletionSender<DragDropState>, events: Vec<DropEvent>) {
    for event in events {
        match event {
            DropEvent::Hover(true) => {
                let _ = sender.send(Box::new(|s: &mut DragDropState| {
                    begin_hover(s);
                }));
            }
            // 既に hovering でなければ何もしない（`cancel_hover` 自体が no-op、
            // `reading` 中は status を上書きしない: `fandhe_core::screens::
            // drag_drop::cancel_hover` の既存契約に委譲する）。
            DropEvent::Hover(false) => {
                let _ = sender.send(Box::new(|s: &mut DragDropState| {
                    cancel_hover(s);
                }));
            }
            DropEvent::Began(session) => {
                let _ = sender.send(Box::new(move |s: &mut DragDropState| {
                    begin_drop_read(s, session);
                }));
            }
            DropEvent::Finished {
                session,
                mut items,
                overflow,
            } => {
                // `finish_drop_with_overflow` は「超過分が 1 件の集約行として
                // 既に `items` に含まれている」契約（`read_all`/コア側 doc 参照）。
                // `drop_session::DropSessions::item` は個々の読み込み結果しか
                // 積まないため、ここで集約行を追加してから渡す。
                if let Some(item) = overflow_item(overflow) {
                    items.push(item);
                }
                let _ = sender.send(Box::new(move |s: &mut DragDropState| {
                    // 古い世代（差し替えられたセッション）の完了通知を捨てる
                    // （契約表 2、`drive` の `is_current_generation` 照合と同じ判断）。
                    if !is_current_generation(s, session) {
                        return;
                    }
                    finish_drop_with_overflow(s, items, overflow);
                }));
            }
        }
    }
}

/// GUI（`egui::Context`）に依存しない純粋な分岐部分。`hovering`/`dropped` を
/// 引数で受け取ることで、`device/tilt.rs::drive`/`device/text_scale.rs::drive` と
/// 同様プラットフォーム cfg 抜きでテストできるようにしている。
///
/// 評価順序が重要: ドロップされたフレームでは egui-winit が既にホバー一覧を
/// 空にしているため、1 の分岐（ドロップ）を最初に評価しないと 3 の分岐
/// （キャンセル）に化けてしまう。送ったフレームの次のフレームでは、先頭で
/// 完了が適用されてから本関数が呼ばれる（`app.rs::drain_completions` の順序）ため、
/// `state.hovering` は既に更新済みで二重送信は起きない。
///
/// ドロップ時は世代を即座に（同期の完了で）確定させてからワーカースレッドを
/// 起動する。次のフレームで別のドロップが来ても、その時点で `state.generation`
/// は既に確定済みなので新しい世代と衝突しない。ワーカーの読み取り完了時は
/// `state.generation` と照合し、後続のドロップに上書きされていれば
/// （契約表 2「古い世代の完了通知を照合して捨てているか」）結果を捨てる。
///
/// デスクトップ限定（上記 [`LATEST_GENERATION`] と同じ理由。iOS は [`drive_events`]）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
fn drive(
    state: &DragDropState,
    sender: &CompletionSender<DragDropState>,
    hovering: bool,
    dropped: Vec<Option<PathBuf>>,
) {
    if !dropped.is_empty() {
        // 枠を先に確保する。上限に達していれば世代を進めず（読み込み中のドロップを
        // 無効にしない、Cursor Bugbot 指摘 PR #1018）、表示中の結果にも触れずに
        // 失敗の文言だけを出す。読み込み中の分は完了すれば通常どおり反映される。
        let Some(slot) = ReaderSlot::acquire() else {
            let _ = sender.send(Box::new(|s: &mut DragDropState| {
                cancel_hover(s);
                s.error = format!("{FAILED_PREFIX}{BUSY_REASON}");
            }));
            return;
        };

        let generation = next_generation(state);
        // 新しいドロップを最新世代として即座に公開する。処理中のジョブは次に
        // 1 ファイル読み終えた時点でこれを見て自分が古いと気づき切り上げる。
        LATEST_GENERATION.store(generation, Ordering::SeqCst);
        let _ = sender.send(Box::new(move |s: &mut DragDropState| {
            begin_drop_read(s, generation);
        }));

        // ドロップごとに読み取りスレッドを立てる（同時に生きる本数は
        // [`MAX_READERS`] まで、モジュール doc 参照）。
        let sender = sender.clone();
        std::thread::spawn(move || {
            let (items, overflow) = read_all(dropped, generation);
            // 読み終えた時点で枠を返してから結果を送る（受け取った側が
            // 次のドロップを出したとき、この枠が埋まったままにならないように）。
            // panic で抜けた場合も `slot` の drop で枠は返る。
            drop(slot);
            let _ = sender.send(Box::new(move |s: &mut DragDropState| {
                if !is_current_generation(s, generation) {
                    return;
                }
                finish_drop_with_overflow(s, items, overflow);
            }));
        });
    } else if hovering && !state.hovering {
        let _ = sender.send(Box::new(|s: &mut DragDropState| {
            begin_hover(s);
        }));
    } else if !hovering && state.hovering {
        let _ = sender.send(Box::new(|s: &mut DragDropState| {
            cancel_hover(s);
        }));
    }
}

/// ドロップされたパスの一覧を [`DropItem`] へ変換する。[`MAX_DROP_FILES`] を
/// 超えた分は読まない上に [`DropItem`] へも変換せず、件数を 1 件へ集約して
/// `TooMany` にする（`fandhe_device::drag_drop` モジュール doc 「1 回のドロップで
/// 読む件数の上限」参照）。超過分を 1 件ずつ処理・保持すると、大量ドロップ時に
/// 入力件数に比例してメモリ確保・描画コストが増え続け上限の意味が無くなるため、
/// 入力件数そのものをここで打ち切る（codex 指摘 #1018 対応）。
///
/// 戻り値の第 2 要素は実際の超過件数（`(名前, 理由)` を 1 件へ畳んだ表示行とは
/// 別に保持する）。`finish_drop_with_overflow` へそのまま渡し、集約行 1 件では
/// 総件数・失敗件数の表示が実数より少なくなるのを防ぐ（codex 指摘 #1018 対応）。
///
/// 呼び出し元 [`drive`] がワーカースレッド上で呼ぶ（最大 [`MAX_DROP_FILES`] ×
/// `fandhe_device::drag_drop::MAX_FILE_BYTES` の同期読み取りを UI スレッドから
/// 追い出すため、codex 指摘 #1018 対応）。
///
/// `generation` は自分がこの読み取りを始めた時点の世代。1 ファイル読むたびに
/// [`LATEST_GENERATION`] と突き合わせ、既に新しいドロップに差し替えられていれば
/// 残りを読まずに打ち切る（完了通知は [`drive`] 側の [`is_current_generation`]
/// でどのみち捨てられるため UI には影響しない。ワーカー自体の寿命を縮めるための
/// キャンセルであり、codex 指摘 #1018「連続ドロップで読み取りワーカーが無制限に
/// 増える」対応）。
///
/// 名前・内容・パスをログへ出さない（security.md A09）。
///
/// デスクトップ限定（上記 [`LATEST_GENERATION`] と同じ理由）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
fn read_all(paths: Vec<Option<PathBuf>>, generation: u64) -> (Vec<DropItem>, usize) {
    let overflow = paths.len().saturating_sub(MAX_DROP_FILES);
    let mut items = Vec::new();
    for path in paths.into_iter().take(MAX_DROP_FILES) {
        if LATEST_GENERATION.load(Ordering::SeqCst) != generation {
            break;
        }
        items.push(match path {
            None => DropItem {
                name: display_name(None),
                result: Err(DropError::Unreadable.to_string()),
            },
            Some(path) => match read_dropped(&path) {
                Ok(picked) => DropItem {
                    name: picked.name,
                    result: Ok(picked.bytes.len()),
                },
                Err(e) => DropItem {
                    name: display_name(Some(&path)),
                    result: Err(e.to_string()),
                },
            },
        });
    }
    if let Some(item) = overflow_item(overflow) {
        items.push(item);
    }
    (items, overflow)
}

/// 上限（[`MAX_DROP_FILES`]）超過分を 1 件へ集約した表示行。`overflow == 0` なら
/// `None`。デスクトップ（[`read_all`]）と iOS（`ios_drag_drop.rs::
/// fandhe_rust_drag_drop_begin` が超過分を計算し、読み込み完了後に集約する側）の
/// 両方で使う共通部品（安易な複製を避ける、実装計画 §2.2「再利用」）。
pub(super) fn overflow_item(overflow: usize) -> Option<DropItem> {
    (overflow > 0).then(|| DropItem {
        name: format!("（他 {overflow} 件）"),
        result: Err(DropError::TooMany.to_string()),
    })
}

/// 失敗行に出す表示用の名前。パスから取れなければ「（名前なし）」にする
/// （フルパスは表示・ログに出さない、security.md A09）。デスクトップ限定
/// （上記 [`LATEST_GENERATION`] と同じ理由。iOS は `.m` 側の `suggestedName`/
/// ファイル名を使う）。
#[cfg_attr(any(target_os = "ios", target_os = "android"), allow(dead_code))]
fn display_name(path: Option<&std::path::Path>) -> String {
    path.and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "（名前なし）".to_string())
}

/// Android の JNI 受け口（`device/android.rs::nativeOnDragDropItem`）が fd から得た
/// 結果を [`DropItem`] へ変換する、FFI を含まない純粋な分岐部分（ホストの `cargo test`
/// で検証できるようにする切り出し、`ios_drag_drop.rs::build_item` と同じ方針）。
///
/// `code` の意味（`MainActivity.java` の `nativeOnDragDropItem` 宣言コメントと対）:
/// - `0`: 通常ファイル（`file` に fd から開いたハンドルが入る契約）
/// - `1`: フォルダだった（`ContentResolver#getType` が `Document.MIME_TYPE_DIR`）
/// - `2`: 読めなかった（URI 無し・上限到達による埋め・`openFileDescriptor` 失敗等、
///   Java 側 `deliverDropItem` 参照）
/// - それ以外、または `code == 0` なのに `file` が `None`（契約違反）: 未知の値を
///   成功へ丸めず読めない扱いにする（契約表 6「嘘の成功」）
///
/// 読み取り（[`read_dropped_from`]）は本関数の中で行う。呼び出し元
/// （`device/android.rs::nativeOnDragDropItem`）がロックの外（`super::drop_session::item`
/// 呼び出し前）で本関数を呼ぶ契約（`drop_session.rs` モジュール doc「判定と更新は同じ
/// ロックの内側」参照。ロック保持中に I/O を行わない）。
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(super) fn item_from_fd(code: i32, file: Option<File>, name: String) -> DropItem {
    match (code, file) {
        (0, Some(file)) => match read_dropped_from(file, name.clone()) {
            Ok(picked) => DropItem {
                name: picked.name,
                result: Ok(picked.bytes.len()),
            },
            Err(e) => DropItem {
                name,
                result: Err(e.to_string()),
            },
        },
        (1, _) => DropItem {
            name,
            result: Err(DropError::Folder.to_string()),
        },
        _ => DropItem {
            name,
            result: Err(DropError::Unreadable.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    /// `drive` のドロップ経路はプロセス全体で共有の [`LATEST_GENERATION`] と
    /// [`ACTIVE_READERS`] を使う。実運用ではドラッグ & ドロップ画面はプロセスに
    /// 1 つしか無いため競合しないが、`cargo test` の既定並列実行では別テストの
    /// ドロップが世代を追い越して読み取りを途中で打ち切らせ得る。ドロップ経路を
    /// 使うテストはこのロックで直列化する（ロック自体はテスト専用）。
    static DRIVE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_drive_test() -> std::sync::MutexGuard<'static, ()> {
        DRIVE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn sender_with_channel() -> (
        CompletionSender<DragDropState>,
        std::sync::mpsc::Receiver<crate::app::Completion<DragDropState>>,
    ) {
        let ctx = egui::Context::default();
        let (tx, rx) = std::sync::mpsc::channel();
        (CompletionSender::new(tx, ctx), rx)
    }

    fn write_temp_file(contents: &[u8]) -> std::path::PathBuf {
        // 並列実行のテスト同士が同じ時刻で同名ファイルを奪い合わないよう連番を含める。
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "fandhe-egui-proxy-drag-drop-test-{}-{}-{}",
            std::process::id(),
            seq,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::write(&path, contents).expect("一時ファイルを書ける");
        path
    }

    fn apply(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<DragDropState>>,
        state: &mut DragDropState,
    ) {
        let completion = rx.recv().expect("完了が送られているはず");
        completion(state);
    }

    /// ドロップ経路専用。[`drive`] はドロップ時、世代確定（同期）→ 読み取りスレッドの
    /// 読み取り完了（非同期）の 2 件を順に送る（モジュール doc・[`drive`] 参照）。
    /// 2 件目の `recv` はワーカースレッドの完了を待ってブロックする。
    fn apply_drop(
        rx: &std::sync::mpsc::Receiver<crate::app::Completion<DragDropState>>,
        state: &mut DragDropState,
    ) {
        apply(rx, state); // 世代確定
        apply(rx, state); // 読み取り結果の反映
    }

    /// 読み取りスレッドが上限まで埋まっている間のドロップは、待たせずに失敗を返す
    /// （応答しない読み取りが残っていても表示が「読み込んでいます…」で固まらない）。
    #[test]
    fn drop_while_readers_are_full_fails_immediately() {
        let _guard = lock_drive_test();
        let held: Vec<ReaderSlot> = (0..MAX_READERS)
            .map(|_| ReaderSlot::acquire().expect("上限までは確保できる"))
            .collect();
        assert!(ReaderSlot::acquire().is_none());

        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let before = LATEST_GENERATION.load(Ordering::SeqCst);
        let path = write_temp_file(b"hello");
        drive(&state, &sender, false, vec![Some(path.clone())]);
        apply(&rx, &mut state); // 失敗の文言だけが 1 件届く

        assert_eq!(state.error, format!("{FAILED_PREFIX}{BUSY_REASON}"));
        // 読み込み中のドロップを無効にしない（世代を進めない）。
        assert_eq!(LATEST_GENERATION.load(Ordering::SeqCst), before);
        assert!(rx.try_recv().is_err());

        drop(held);
        assert!(ReaderSlot::acquire().is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn regular_file_drop_reports_success_status() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let path = write_temp_file(b"hello");

        drive(&state, &sender, false, vec![Some(path.clone())]);
        apply_drop(&rx, &mut state);

        assert_eq!(state.status, "1 件受け取りました");
        assert!(state.error.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn folder_drop_reports_failure_with_reason() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let dir = std::env::temp_dir();

        drive(&state, &sender, false, vec![Some(dir)]);
        apply_drop(&rx, &mut state);

        assert!(!state.error.is_empty());
        assert!(state.items[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("フォルダ"));
    }

    #[test]
    fn oversized_file_drop_reports_size_limit_reason() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let path = write_temp_file(&vec![0u8; fandhe_device::drag_drop::MAX_FILE_BYTES + 1]);

        drive(&state, &sender, false, vec![Some(path.clone())]);
        apply_drop(&rx, &mut state);

        assert!(state.items[0].result.as_ref().unwrap_err().contains("MiB"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_path_or_none_reports_failure() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();

        drive(&state, &sender, false, vec![None]);
        apply_drop(&rx, &mut state);

        assert!(state.items[0].result.is_err());
    }

    #[test]
    fn mixed_success_and_failure_reports_partial_error_not_success() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let ok_path = write_temp_file(b"ok");

        drive(&state, &sender, false, vec![Some(ok_path.clone()), None]);
        apply_drop(&rx, &mut state);

        assert!(state.status.is_empty());
        assert!(state.error.contains("2 件中 1 件"));
        let _ = std::fs::remove_file(&ok_path);
    }

    #[test]
    fn only_the_file_past_the_limit_becomes_too_many() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        let paths: Vec<Option<std::path::PathBuf>> = (0..=MAX_DROP_FILES)
            .map(|_| Some(write_temp_file(b"")))
            .collect();

        drive(&state, &sender, false, paths.clone());
        apply_drop(&rx, &mut state);

        assert_eq!(state.items.len(), MAX_DROP_FILES + 1);
        assert!(state.items[MAX_DROP_FILES - 1].result.is_ok());
        assert!(state.items[MAX_DROP_FILES]
            .result
            .as_ref()
            .unwrap_err()
            .contains("一度に受け取れるのは"));

        for path in paths.into_iter().flatten() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn large_overflow_is_collapsed_into_a_single_too_many_item() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        // MAX_DROP_FILES を大きく超える件数でも、超過分を 1 件ずつ DropItem 化
        // せず 1 件へ集約することを確認する（codex 指摘 #1018 対応）。
        let overflow = 500;
        let paths: Vec<Option<std::path::PathBuf>> = (0..MAX_DROP_FILES + overflow)
            .map(|_| Some(write_temp_file(b"")))
            .collect();

        drive(&state, &sender, false, paths.clone());
        apply_drop(&rx, &mut state);

        assert_eq!(state.items.len(), MAX_DROP_FILES + 1);
        assert!(state.items[MAX_DROP_FILES - 1].result.is_ok());
        let overflow_item = &state.items[MAX_DROP_FILES];
        assert!(overflow_item
            .result
            .as_ref()
            .unwrap_err()
            .contains("一度に受け取れるのは"));
        assert!(overflow_item.name.contains(&overflow.to_string()));

        for path in paths.into_iter().flatten() {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn hovering_true_while_not_hovering_sends_begin() {
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();

        drive(&state, &sender, true, vec![]);
        apply(&rx, &mut state);

        assert!(state.hovering);
    }

    #[test]
    fn hovering_false_while_hovering_and_no_drop_sends_cancel() {
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        begin_hover(&mut state);

        drive(&state, &sender, false, vec![]);
        apply(&rx, &mut state);

        assert!(!state.hovering);
        assert_eq!(
            state.status,
            fandhe_core::screens::drag_drop::CANCELLED_STATUS
        );
    }

    #[test]
    fn hovering_false_while_hovering_with_drop_finishes_not_cancels() {
        let _guard = lock_drive_test();
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        begin_hover(&mut state);
        let path = write_temp_file(b"x");

        drive(&state, &sender, false, vec![Some(path.clone())]);
        apply_drop(&rx, &mut state);

        assert_ne!(
            state.status,
            fandhe_core::screens::drag_drop::CANCELLED_STATUS
        );
        assert!(!state.hovering);
        let _ = std::fs::remove_file(&path);
    }

    /// レビュー指摘（Issue #1018 High）再発防止: ドロップ直後の次フレームは
    /// 既にホバー一覧が空（`device/drag_drop.rs` モジュール doc の
    /// egui-winit 0.33.3 挙動）になる。読み取りワーカーがまだ完了していない
    /// 間にこのフレームが来ても、`begin_drop_read` が世代確定と同時に
    /// `state.hovering` を `false` へ倒し済みなら `drive` の「hovering=false
    /// かつ state.hovering=true」分岐（cancel_hover 送出）は発火しない。
    /// バックグラウンド読み取りの実時間に依存させないよう、`drive`（の
    /// 呼び出し）を経由せず `begin_drop_read` 適用直後の状態を直接組み立てて
    /// 検証する。
    #[test]
    fn drop_in_progress_next_frame_does_not_send_cancel() {
        let (sender, rx) = sender_with_channel();
        let mut state = DragDropState::default();
        begin_hover(&mut state);
        let generation = fandhe_core::screens::drag_drop::next_generation(&state);
        fandhe_core::screens::drag_drop::begin_drop_read(&mut state, generation);

        // 読み取りワーカーが終わる前の次フレーム: ホバー一覧は既に空。
        drive(&state, &sender, false, vec![]);

        assert!(!state.hovering);
        assert!(
            rx.try_recv().is_err(),
            "読み取り完了を待つ間に cancel_hover を送ってはいけない"
        );
    }

    #[test]
    fn no_change_sends_nothing() {
        let (sender, rx) = sender_with_channel();
        let state = DragDropState::default();

        drive(&state, &sender, false, vec![]);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn overflow_item_is_none_when_no_overflow() {
        assert_eq!(overflow_item(0), None);
    }

    #[test]
    fn overflow_item_reports_count_and_too_many_reason() {
        let item = overflow_item(3).expect("overflow > 0 は Some を返す");
        assert!(item.name.contains('3'));
        assert!(item.result.unwrap_err().contains("一度に受け取れるのは"));
    }

    /// Android 経路（[`item_from_fd`]）のテスト群。fd を模した一時ファイルを渡し、
    /// `code` ごとの分岐を検証する（`ios_drag_drop.rs::build_item` のテストと同じ方針）。
    mod item_from_fd_tests {
        use super::*;

        fn temp_file(contents: &[u8]) -> File {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "fandhe-egui-proxy-drag-drop-android-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            std::fs::write(&path, contents).expect("一時ファイルを書ける");
            let file = File::open(&path).expect("開ける");
            let _ = std::fs::remove_file(&path);
            file
        }

        #[test]
        fn code_zero_with_file_reads_content() {
            let file = temp_file(b"hello");
            let item = item_from_fd(0, Some(file), "a.txt".to_string());
            assert_eq!(item.name, "a.txt");
            assert_eq!(item.result, Ok(5));
        }

        #[test]
        fn code_one_is_folder_regardless_of_file() {
            let item = item_from_fd(1, None, "dir".to_string());
            assert_eq!(item.name, "dir");
            assert_eq!(item.result.unwrap_err(), DropError::Folder.to_string());
        }

        #[test]
        fn code_two_is_unreadable() {
            let item = item_from_fd(2, None, "".to_string());
            assert_eq!(item.result.unwrap_err(), DropError::Unreadable.to_string());
        }

        #[test]
        fn unknown_code_is_unreadable_not_success() {
            let file = temp_file(b"hello");
            let item = item_from_fd(99, Some(file), "a.txt".to_string());
            assert_eq!(
                item.result.unwrap_err(),
                DropError::Unreadable.to_string(),
                "契約表 6: 未知の code を成功へ丸めない"
            );
        }

        #[test]
        fn code_zero_without_file_is_unreadable() {
            // 契約違反（code=0 なのに fd 取得失敗）でも成功へ丸めない。
            let item = item_from_fd(0, None, "a.txt".to_string());
            assert_eq!(item.result.unwrap_err(), DropError::Unreadable.to_string());
        }
    }

    /// iOS 経路（[`drive_events`]）のテスト群。`super::drop_session` を経由せず、
    /// [`DropEvent`] を直接組み立てて渡す（イベント生成側の分岐網羅は
    /// `drop_session.rs` のテストが担う）。
    mod drive_events_tests {
        use super::*;

        #[test]
        fn began_then_finished_reports_success_count() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(
                &sender,
                vec![
                    DropEvent::Began(1),
                    DropEvent::Finished {
                        session: 1,
                        items: vec![DropItem {
                            name: "a.txt".to_string(),
                            result: Ok(10),
                        }],
                        overflow: 0,
                    },
                ],
            );
            apply(&rx, &mut state);
            apply(&rx, &mut state);

            assert_eq!(state.status, "1 件受け取りました");
        }

        #[test]
        fn partial_failure_reports_count_based_error() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(
                &sender,
                vec![
                    DropEvent::Began(1),
                    DropEvent::Finished {
                        session: 1,
                        items: vec![
                            DropItem {
                                name: "a.txt".to_string(),
                                result: Ok(10),
                            },
                            DropItem {
                                name: "b".to_string(),
                                result: Err("フォルダは受け取れません".to_string()),
                            },
                        ],
                        overflow: 0,
                    },
                ],
            );
            apply(&rx, &mut state);
            apply(&rx, &mut state);

            assert!(state.error.contains("2 件中 1 件"));
        }

        /// `overflow > 0` のとき、集約行 1 件を `items` へ追加してから
        /// `finish_drop_with_overflow` へ渡す（コア側の「超過分は集約行として
        /// 既に含まれている」契約、実装計画 §2.3 参照）。
        #[test]
        fn finished_with_overflow_appends_aggregate_row() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(
                &sender,
                vec![
                    DropEvent::Began(1),
                    DropEvent::Finished {
                        session: 1,
                        items: vec![DropItem {
                            name: "a.txt".to_string(),
                            result: Ok(10),
                        }],
                        overflow: 3,
                    },
                ],
            );
            apply(&rx, &mut state);
            apply(&rx, &mut state);

            assert_eq!(state.items.len(), 2, "集約行が 1 件追加される");
            assert!(state.error.contains("4 件中 3 件"));
        }

        /// 古い `Finished`（差し替えられたセッションの完了通知）は捨てられる
        /// （契約表 2、`drive` の `is_current_generation` 照合と同じ判断）。
        #[test]
        fn stale_session_finished_is_discarded() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(&sender, vec![DropEvent::Began(1), DropEvent::Began(2)]);
            apply(&rx, &mut state); // Began(1)
            apply(&rx, &mut state); // Began(2)
            assert!(is_current_generation(&state, 2));

            drive_events(
                &sender,
                vec![DropEvent::Finished {
                    session: 1,
                    items: vec![DropItem {
                        name: "stale.txt".to_string(),
                        result: Ok(1),
                    }],
                    overflow: 0,
                }],
            );
            apply(&rx, &mut state);

            assert!(state.items.is_empty(), "古い世代の結果は反映されない");
        }

        #[test]
        fn hover_true_sends_begin_hover() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(&sender, vec![DropEvent::Hover(true)]);
            apply(&rx, &mut state);

            assert!(state.hovering);
        }

        /// ドロップ無しのホバー取り消しはキャンセル表示になる。
        #[test]
        fn hover_false_without_drop_shows_cancelled_status() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();
            begin_hover(&mut state);

            drive_events(&sender, vec![DropEvent::Hover(false)]);
            apply(&rx, &mut state);

            assert!(!state.hovering);
            assert_eq!(
                state.status,
                fandhe_core::screens::drag_drop::CANCELLED_STATUS
            );
        }

        /// `Began` の直後に届く `Hover(false)`（ドロップ自体で領域外イベントが
        /// 追随するケース）では `CANCELLED_STATUS` にならない（`begin_drop_read` が
        /// 既に `reading=true` へ倒しているため、`cancel_hover` の既存契約
        /// （`state.reading` 中は status を上書きしない）に委譲する）。
        #[test]
        fn hover_false_after_began_does_not_overwrite_reading_status() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();
            begin_hover(&mut state);

            drive_events(&sender, vec![DropEvent::Began(1), DropEvent::Hover(false)]);
            apply(&rx, &mut state); // Began
            apply(&rx, &mut state); // Hover(false)

            assert!(state.reading);
            assert!(state.status.is_empty());
        }

        #[test]
        fn empty_finished_is_reported_as_failure() {
            let (sender, rx) = sender_with_channel();
            let mut state = DragDropState::default();

            drive_events(
                &sender,
                vec![
                    DropEvent::Began(1),
                    DropEvent::Finished {
                        session: 1,
                        items: vec![],
                        overflow: 0,
                    },
                ],
            );
            apply(&rx, &mut state);
            apply(&rx, &mut state);

            assert!(state.status.is_empty());
            assert!(!state.error.is_empty());
        }
    }
}
