//! ドラッグ & ドロップ受け取り（親 #1013）iOS 経路（Issue #1015）向けの、ターゲット
//! 非依存な集約器。`UIDropInteraction` の delegate（`ios/fandhe_ios_drag_drop.m`）は
//! `NSItemProvider` の読み込み完了ごとに別々のバックグラウンドスレッドから
//! `fandhe_rust_drag_drop_item` を呼ぶため、「全アイテムの読み込みが揃うまで待つ」
//! 「重複・古い世代の完了を無視する」の状態機械をここへ切り出し、
//! `android_gamepad.rs::AndroidGamepadState` と同じく `cargo test` で検証できる形にする
//! （FFI・ObjC を経由しないロジック本体）。
//!
//! # 判定と更新は同じロックの内側（契約表 2）
//!
//! [`SESSIONS`] の `Mutex` の内側で「現在のセッションか」「その slot は空きか」
//! 「全 slot が埋まったか」をまとめて判定・更新する。ファイルの実読み取り
//! （`fandhe_device::drag_drop::read_dropped`）はロックの外（呼び出し元
//! `ios_drag_drop.rs::fandhe_rust_drag_drop_item`）で済ませてから [`DropSessions::item`]
//! を呼ぶ契約にし、ロック保持中に I/O を行わない。
//!
//! # イベントは push 型・`take_events` で drain
//!
//! [`DropEvent`] は `effect` 呼び出し（`device/drag_drop.rs::drag_drop_effect` の iOS
//! 分岐、毎フレーム pull）が [`take_events`] で取り出し、`fandhe_core::screens::
//! drag_drop` の状態遷移関数へ適用する（`drive_events`、`device/drag_drop.rs` 参照）。
//! 集約器自体は `fandhe_core` にも `egui`/ObjC にも依存しない。
//!
//! iOS/Android 以外のターゲットでは本モジュールは `#[cfg(test)]` からのみ参照されるため、
//! `dead_code` 誤検知を避けるためモジュール全体に `cfg_attr` で許可する
//! （`android_gamepad.rs` と同じ判断）。
//!
//! # Android 経路（Issue #1016）
//!
//! Android は `View.setOnDragListener`（`MainActivity.java` の `dragListener`）が
//! push する 1 件ずつの読み込み結果を、iOS と同じくこの集約器へ渡す
//! （`device/android.rs::nativeOnDragDropItem` 経由）。iOS の completion 側
//! （`NSItemProvider`）と異なり、Android の読み取りスレッド（`MainActivity.java`）は
//! 「このセッションはまだ現在のものか」を事前に問い合わせてから I/O を始めたいため、
//! [`wants`] を追加した（iOS は completion が既に非同期に開始済みで問い合わせる余地が
//! 無く、[`item`] の世代照合だけで十分だった）。Activity 再生成時は [`reset`] を呼び
//! （`android_main` から、モジュール doc 参照）、進行中のセッションと未適用イベントを
//! 破棄する。

#![cfg_attr(not(any(target_os = "ios", target_os = "android")), allow(dead_code))]

use std::sync::Mutex;

use fandhe_core::screens::drag_drop::DropItem;
use fandhe_device::drag_drop::MAX_DROP_FILES;

/// `take_events` が返す、状態機械へ適用すべき 1 件の変化。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropEvent {
    /// ホバー状態の変化（`true` = 領域内に入った、`false` = 出た/取り消された）。
    Hover(bool),
    /// 新しいドロップセッションを開始した（`session` は世代番号を兼ねる、モジュール doc 参照）。
    Began(u64),
    /// `session` の全アイテムの読み込みが完了した。
    Finished {
        session: u64,
        items: Vec<DropItem>,
        overflow: usize,
    },
}

/// 進行中の 1 セッション。`slots` は `begin` で払い出した `to_load` 件ぶんの枠で、
/// 各 slot は完了した [`DropItem`] が届くまで `None`。
struct InFlight {
    id: u64,
    slots: Vec<Option<DropItem>>,
    overflow: usize,
}

impl InFlight {
    /// 全 slot が埋まったか（`Finished` を積んでよいか）。
    fn is_complete(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }

    /// 埋まった slot を順序どおりの `Vec<DropItem>` へ取り出す。`is_complete` 確認後にのみ呼ぶ。
    fn take_items(self) -> Vec<DropItem> {
        self.slots.into_iter().flatten().collect()
    }
}

/// ドラッグ & ドロップの集約状態。
#[derive(Default)]
struct DropSessions {
    /// 直近に払い出したセッション id（= 世代番号）。0 は「未使用」を表す番兵。
    next_session: u64,
    /// 進行中のセッション（読み込み待ちの slot が残っている間だけ `Some`）。
    current: Option<InFlight>,
    /// 次の `take_events` で drain される、状態機械へ未適用のイベント列。
    events: Vec<DropEvent>,
}

impl DropSessions {
    /// ホバー状態の変化を積む。
    fn hover(&mut self, entered: bool) {
        self.events.push(DropEvent::Hover(entered));
    }

    /// 新しいドロップ（`total` 件）を開始する。上限 [`MAX_DROP_FILES`] を超えた分は
    /// 読み込み対象にせず `overflow` として集計する（デスクトップの `read_all` と
    /// 同じ「入力件数そのものを打ち切る」方針、`device/drag_drop.rs` 参照）。
    ///
    /// 進行中の旧セッションがあれば（読み込み完了を待たず）差し替える。旧セッションの
    /// `slots` を捨てるだけで `Finished` は積まない。以降に届く旧セッション宛ての
    /// `item` 呼び出しは [`item`] 側の世代照合で無視される（契約表 2）。
    ///
    /// `total == 0` の場合は、その場で空の `Finished` を積む（0 件 = 失敗、契約表 6）。
    /// 戻り値は `(session_id, to_load)`。
    fn begin(&mut self, total: usize) -> (u64, usize) {
        self.next_session += 1;
        let id = self.next_session;
        let to_load = total.min(MAX_DROP_FILES);
        let overflow = total.saturating_sub(to_load);

        self.events.push(DropEvent::Began(id));

        if to_load == 0 {
            self.current = None;
            self.events.push(DropEvent::Finished {
                session: id,
                items: Vec::new(),
                overflow,
            });
            return (id, to_load);
        }

        self.current = Some(InFlight {
            id,
            slots: vec![None; to_load],
            overflow,
        });
        (id, to_load)
    }

    /// 1 件ぶんの読み込み結果を反映する。次のいずれかに該当すれば無視する
    /// （completion の重複・古い世代の除去、契約表 2・5）:
    ///
    /// - `session` が現在進行中のセッションと異なる
    /// - `index` が範囲外
    /// - その slot が既に埋まっている
    ///
    /// 全 slot が埋まった時点で [`DropEvent::Finished`] を 1 回だけ積み、`current` を
    /// `None` に戻す。
    fn item(&mut self, session: u64, index: usize, result: DropItem) {
        let Some(in_flight) = self.current.as_mut() else {
            return;
        };
        if in_flight.id != session {
            return;
        }
        let Some(slot) = in_flight.slots.get_mut(index) else {
            return;
        };
        if slot.is_some() {
            return;
        }
        *slot = Some(result);

        if in_flight.is_complete() {
            // `current` を取り出してから消費する（`is_complete` 確認後の唯一の
            // 消費経路であり、以降このセッションへの `item` は `current` が
            // `None` になるため次の begin まで無条件に無視される）。
            let in_flight = self.current.take().expect("直前に確認済み");
            let overflow = in_flight.overflow;
            let session = in_flight.id;
            self.events.push(DropEvent::Finished {
                session,
                items: in_flight.take_items(),
                overflow,
            });
        }
    }

    /// 未適用のイベントをすべて取り出す（呼び出し元が空の `Vec` を毎フレーム
    /// 受け取っても安全なように、内部バッファは `Vec::new()` で置き換える）。
    fn take_events(&mut self) -> Vec<DropEvent> {
        std::mem::take(&mut self.events)
    }

    /// `session` が現在進行中のセッションで、かつ `index` が範囲内かつ未読み込みか
    /// （Android の読み取りスレッドが I/O を始める前に問い合わせる、モジュール doc
    /// 「Android 経路」参照）。`false` を返した場合、呼び出し元は読み取りを行わず
    /// 打ち切る（差し替え済みの旧セッション・上限超過分の両方をここで弾く）。
    fn wants(&self, session: u64, index: usize) -> bool {
        let Some(in_flight) = self.current.as_ref() else {
            return false;
        };
        in_flight.id == session
            && in_flight
                .slots
                .get(index)
                .is_some_and(|slot| slot.is_none())
    }

    /// 進行中のセッションと未適用イベントを破棄する（Activity 再生成時、モジュール doc
    /// 「Android 経路」参照）。`next_session` は単調増加を保つため触らない
    /// （差し替え後に旧セッション宛ての `item`/`wants` が新セッションと衝突しないため）。
    fn reset(&mut self) {
        self.current = None;
        self.events.clear();
    }
}

/// プロセス全体で共有する集約状態（`UIDropInteraction` はウィンドウに 1 つだけ）。
static SESSIONS: Mutex<DropSessions> = Mutex::new(DropSessions {
    next_session: 0,
    current: None,
    events: Vec::new(),
});

/// ロック取得の失敗（poison）は panic させず、内容を取り出して継続する
/// （coding-rust.md「panic を API 契約にしない」）。
fn lock() -> std::sync::MutexGuard<'static, DropSessions> {
    SESSIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`DropSessions::hover`] へ委譲する（`ios_drag_drop.rs::fandhe_rust_drag_drop_hover` から）。
pub fn hover(entered: bool) {
    lock().hover(entered);
}

/// [`DropSessions::begin`] へ委譲する（`ios_drag_drop.rs::fandhe_rust_drag_drop_begin` から）。
pub fn begin(total: usize) -> (u64, usize) {
    lock().begin(total)
}

/// [`DropSessions::item`] へ委譲する（`ios_drag_drop.rs::fandhe_rust_drag_drop_item` から。
/// 呼び出し元がロックの外で読み取りを済ませている契約、モジュール doc 参照）。
pub fn item(session: u64, index: usize, result: DropItem) {
    lock().item(session, index, result);
}

/// [`DropSessions::take_events`] へ委譲する（`device/drag_drop.rs::drag_drop_effect` の
/// iOS 分岐から毎フレーム呼ばれる）。
pub fn take_events() -> Vec<DropEvent> {
    lock().take_events()
}

/// [`DropSessions::wants`] へ委譲する（`device/android.rs::
/// Java_..._nativeDragDropWants` から、読み取りスレッドが I/O を始める前に呼ぶ、
/// モジュール doc「Android 経路」参照）。
pub fn wants(session: u64, index: usize) -> bool {
    lock().wants(session, index)
}

/// [`DropSessions::reset`] へ委譲する（`android_main` から Activity 再生成のたびに呼ぶ、
/// モジュール doc「Android 経路」参照）。iOS では呼ばれない（`UIDropInteraction` の
/// 登録が破棄されないため、`ensure_installed`/`INSTALLED` 側で別途扱う）。
pub fn reset() {
    lock().reset();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`SESSIONS`] はプロセス全体で共有の static であり、`cargo test` の既定並列
    /// 実行では別テストの `begin`/`item` が世代を進め合って干渉し得るため、本モジュール
    /// を直接叩くテストはこのロックで直列化する（`device/drag_drop.rs::DRIVE_TEST_LOCK`
    /// と同じ判断。テスト専用）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_test() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn ok_item(name: &str) -> DropItem {
        DropItem {
            name: name.to_string(),
            result: Ok(name.len()),
        }
    }

    #[test]
    fn begin_over_limit_caps_to_load_and_reports_overflow() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (_, to_load) = sessions.begin(MAX_DROP_FILES + 5);
        assert_eq!(to_load, MAX_DROP_FILES);
    }

    #[test]
    fn all_slots_filled_emits_finished_exactly_once() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, to_load) = sessions.begin(2);
        assert_eq!(to_load, 2);
        sessions.item(id, 0, ok_item("a"));
        sessions.item(id, 1, ok_item("b"));

        let events = sessions.take_events();
        let finished: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, DropEvent::Finished { .. }))
            .collect();
        assert_eq!(finished.len(), 1, "Finished はちょうど 1 回だけ積まれる");
    }

    #[test]
    fn duplicate_index_is_ignored() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, _) = sessions.begin(2);
        sessions.item(id, 0, ok_item("a"));
        sessions.item(id, 0, ok_item("a-dup"));
        sessions.item(id, 1, ok_item("b"));

        let events = sessions.take_events();
        let DropEvent::Finished { items, .. } = events
            .into_iter()
            .find(|e| matches!(e, DropEvent::Finished { .. }))
            .expect("Finished が積まれているはず")
        else {
            unreachable!()
        };
        assert_eq!(
            items[0].name, "a",
            "先に届いた結果を保持し、重複で上書きしない"
        );
    }

    #[test]
    fn out_of_range_index_is_ignored() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, _) = sessions.begin(1);
        sessions.item(id, 5, ok_item("out-of-range"));
        assert!(
            sessions.current.is_some(),
            "範囲外は無視され、セッションは進行中のまま"
        );
    }

    #[test]
    fn item_for_old_session_after_new_begin_is_discarded() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (first_id, _) = sessions.begin(1);
        let (second_id, _) = sessions.begin(1);
        assert_ne!(first_id, second_id);

        // 旧セッション宛ての完了通知は捨てられ、新セッションの slot に紛れ込まない。
        sessions.item(first_id, 0, ok_item("stale"));
        sessions.item(second_id, 0, ok_item("fresh"));

        let events = sessions.take_events();
        let DropEvent::Finished { session, items, .. } = events
            .into_iter()
            .rev()
            .find(|e| matches!(e, DropEvent::Finished { .. }))
            .expect("新セッションの Finished が積まれているはず")
        else {
            unreachable!()
        };
        assert_eq!(session, second_id);
        assert_eq!(items[0].name, "fresh");
    }

    #[test]
    fn zero_total_emits_empty_finished_immediately() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, to_load) = sessions.begin(0);
        assert_eq!(to_load, 0);

        let events = sessions.take_events();
        assert!(matches!(
            events.as_slice(),
            [DropEvent::Began(began_id), DropEvent::Finished { session, items, overflow: 0 }]
                if *began_id == id && *session == id && items.is_empty()
        ));
    }

    #[test]
    fn take_events_drains_and_leaves_buffer_empty() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        sessions.hover(true);
        assert_eq!(sessions.take_events().len(), 1);
        assert!(sessions.take_events().is_empty());
    }

    #[test]
    fn overflow_past_limit_is_reported_on_finish() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, to_load) = sessions.begin(MAX_DROP_FILES + 3);
        for i in 0..to_load {
            sessions.item(id, i, ok_item("a"));
        }
        let events = sessions.take_events();
        let DropEvent::Finished { overflow, .. } = events
            .into_iter()
            .find(|e| matches!(e, DropEvent::Finished { .. }))
            .expect("Finished が積まれているはず")
        else {
            unreachable!()
        };
        assert_eq!(overflow, 3);
    }

    /// プロセス全体で共有される公開 API（`hover`/`begin`/`item`/`take_events`）の
    /// 疎通確認。ロジック本体の分岐網羅は `DropSessions` を直接叩く上記テストで行う。
    #[test]
    fn public_api_round_trips_through_shared_static() {
        let _guard = lock_test();
        // 前のテストの残留イベントを洗い流す（共有 static のため）。
        let _ = take_events();

        hover(true);
        let (id, to_load) = begin(1);
        assert_eq!(to_load, 1);
        item(id, 0, ok_item("a"));

        let events = take_events();
        assert!(events
            .iter()
            .any(|e| matches!(e, DropEvent::Finished { .. })));
    }

    #[test]
    fn wants_true_only_for_current_session_unfilled_index_in_range() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, to_load) = sessions.begin(2);
        assert_eq!(to_load, 2);
        assert!(sessions.wants(id, 0));
        assert!(!sessions.wants(id, 5), "範囲外は false");
        assert!(!sessions.wants(id + 1, 0), "旧/未知のセッションは false");

        sessions.item(id, 0, ok_item("a"));
        assert!(!sessions.wants(id, 0), "既に埋まった slot は false");
        assert!(sessions.wants(id, 1), "残りの slot はまだ true");
    }

    #[test]
    fn wants_false_after_session_completes() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        let (id, _) = sessions.begin(1);
        sessions.item(id, 0, ok_item("a"));
        assert!(
            !sessions.wants(id, 0),
            "完了後は current が None になり false"
        );
    }

    #[test]
    fn reset_drops_current_session_and_clears_events() {
        let _guard = lock_test();
        let mut sessions = DropSessions::default();
        sessions.hover(true);
        let (id, _) = sessions.begin(2);

        sessions.reset();

        assert!(
            !sessions.wants(id, 0),
            "reset 後は旧セッションを wants しない"
        );
        assert!(
            sessions.take_events().is_empty(),
            "reset 後は未適用イベントも空"
        );

        // `next_session` は単調増加を保ち、reset 後の begin は新しい id を払い出す。
        let (new_id, _) = sessions.begin(1);
        assert_ne!(id, new_id);
    }
}
