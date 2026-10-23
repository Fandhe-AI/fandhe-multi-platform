//! Phase 11 の拡充ループ型サンプル「画面の向き」（親 #128、Issue #655）。
//! 現在の向き（縦・横と角度）を表示し、縦固定・横固定・解除を切り替える 1 画面。
//! `FANDHE_PROXY_APP=screen_orientation_show` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼ばれる
//! （`screens/mod.rs` の `pub mod screen_orientation_show;` と対）。
//!
//! **端末依存はゼロ**（`fandhe-device` を import しない、`docs/ai/SCHEMA.md`「新規画面ファイルの
//! 置き場」節の制約どおり）。OS への要求・読み戻しは `fandhe-egui-proxy` の effect ドライバ
//! （`device/screen_orientation.rs`）が [`take_request`] で要求を取り、[`mark_applying`] →
//! [`finish`] / [`apply_read`] で結果を注入する。Web（`fandhe-web-dom`、#658）も
//! [`lock_satisfied`] など本モジュールの純関数を共有する。
//!
//! # 嘘の成功を避ける
//!
//! 「固定しています」の表示は、読み戻しの `lock` が固定した種類で、かつ実際の向きもその種類の
//! ときだけ出す。要求が成功を返しても読み戻しが満たさなければ失敗として扱う。非対応
//! （macOS・Windows・Linux 等）では操作 UI を出さず、要求も立てない。
//!
//! # OS ごとの違い（全対象 OS 分をここで定義する。後続の OS 実装はこの画面を変えない）
//!
//! - iPad・Safari・デスクトップ Chrome: 取得だけ動き、固定は `Unsupported`。[`Reading::can_lock`]
//!   が偽、または固定の結果が `Unsupported` のとき [`LOCK_UNSUPPORTED_NOTE`] を出して操作を隠す
//! - Web: 固定で全画面になり、解除で全画面を抜ける（[`HINT`]）
//! - Android: 大画面・メーカー設定で固定が無視されることがあり、その場合は失敗と表示する
//!   （[`HINT`]）

use crate::{Screen, WidgetId};

/// 「縦に固定」ボタンの `WidgetId`。
pub const LOCK_PORTRAIT_ID: WidgetId = WidgetId::from_raw(1);
/// 「横に固定」ボタンの `WidgetId`。
pub const LOCK_LANDSCAPE_ID: WidgetId = WidgetId::from_raw(2);
/// 「固定を解除」ボタンの `WidgetId`。
pub const UNLOCK_ID: WidgetId = WidgetId::from_raw(3);
/// 「読み直す」ボタンの `WidgetId`。
pub const RELOAD_ID: WidgetId = WidgetId::from_raw(4);

/// 縦。
pub const PORTRAIT_LABEL: &str = "縦";
/// 横。
pub const LANDSCAPE_LABEL: &str = "横";
/// 「縦に固定」ボタンのラベル。
pub const LOCK_PORTRAIT_LABEL: &str = "縦に固定";
/// 「横に固定」ボタンのラベル。
pub const LOCK_LANDSCAPE_LABEL: &str = "横に固定";
/// 「固定を解除」ボタンのラベル。
pub const UNLOCK_LABEL: &str = "固定を解除";
/// 「読み直す」ボタンのラベル。
pub const RELOAD_LABEL: &str = "読み直す";

/// 初回の読み取り中の案内。
pub const READING_STATUS: &str = "読み取り中…";
/// 現在の向きの接頭辞（後ろに `縦（0°）` の形）。
pub const CURRENT_PREFIX: &str = "いまの向き: ";
/// 縦固定中の案内。
pub const LOCKED_PORTRAIT_STATUS: &str = "縦に固定しています";
/// 横固定中の案内。
pub const LOCKED_LANDSCAPE_STATUS: &str = "横に固定しています";
/// 固定していないときの案内。
pub const NOT_LOCKED_STATUS: &str = "向きは固定していません";
/// 切り替え中の案内。
pub const SWITCHING_STATUS: &str = "切り替えています";
/// 非対応環境の案内（macOS・Windows・Linux）。
pub const UNSUPPORTED_STATUS: &str = "この端末では画面の向きを扱いません";
/// 固定だけが使えない環境の案内（iPad・Safari・デスクトップ Chrome）。
pub const LOCK_UNSUPPORTED_NOTE: &str =
    "この環境では向きの固定はできません。向きの表示だけ行います";
/// 失敗の接頭辞。
pub const FAILED_PREFIX: &str = "できませんでした: ";
/// 固定が成功を返したのに固定できていないときの理由。
pub const NOT_LOCKED_REASON: &str = "向きを固定できていません";
/// 解除が成功を返したのに固定が残っているときの理由。
pub const NOT_UNLOCKED_REASON: &str = "固定を解除できていません";
/// 操作は成功を返したが、読み戻しに失敗して結果を確認できないときの理由。
pub const READBACK_FAILED_REASON: &str = "結果を確認できませんでした";
/// OS ごとの注記。
pub const HINT: &str = "Web では固定すると全画面になり、解除すると全画面を抜けます。\
Android の大画面やメーカー設定では固定が無視されることがあり、その場合は失敗と表示します";

/// 向きの種類（`fandhe_device::screen_orientation::Orientation` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// 縦。
    Portrait,
    /// 横。
    Landscape,
}

/// OS の読み戻し（`fandhe_device::screen_orientation::OrientationReading` の鏡像）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// 実際の向き。
    pub orientation: Orientation,
    /// 自然な向きからの角度（0・90・180・270）。
    pub angle: u16,
    /// OS 側の固定の希望。`None` は解除。
    pub lock: Option<Orientation>,
    /// 固定できる端末か。
    pub can_lock: bool,
}

/// 失敗の種別（`fandhe-egui-proxy` が `DeviceError` から写す。OS の詳細は載せない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrientationFailure {
    /// この環境では使えない。
    Unsupported,
    /// その他の失敗（固定文言のみ）。
    Failed(String),
}

/// effect ドライバが取り出す要求 1 件。`generation` は完了通知の照合に使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockRequest {
    /// 要求の世代。
    pub generation: u64,
    /// 固定する向き。`None` は解除。
    pub target: Option<Orientation>,
}

/// 読み取り状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ReadState {
    /// まだ読んでいない。
    #[default]
    Unread,
    /// 読めた。
    Read(Reading),
    /// 向きの取得自体が使えない（失敗ではない）。
    Unsupported,
    /// 読み取りに失敗した。
    Failed,
}

/// 「画面の向き」画面の状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenOrientationShowState {
    read: ReadState,
    pending: Option<Option<Orientation>>,
    in_flight: Option<Option<Orientation>>,
    applying: bool,
    generation: u64,
    /// 固定の実行時 `Unsupported`（Safari 等）を受けた。以後の読み取りでも `can_lock` を偽に保つ。
    lock_unsupported: bool,
    error: String,
    notice: String,
}

impl ScreenOrientationShowState {
    /// 読み取り状態。
    pub fn read_state(&self) -> &ReadState {
        &self.read
    }

    /// 現在の世代。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// OS へ要求を渡して完了待ちか（driver が古い確認待ちを捨てる判定に使う）。
    pub fn applying(&self) -> bool {
        self.applying
    }

    /// 要求の保留中または処理中か。
    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.applying
    }
}

/// 固定（または解除）が読み戻しで満たされているか。
/// 解除は `lock` が `None`、固定は `lock` が目標で、かつ実際の向きも目標に入っていること。
pub fn lock_satisfied(target: Option<Orientation>, reading: &Reading) -> bool {
    match target {
        None => reading.lock.is_none(),
        Some(t) => reading.lock == Some(t) && reading.orientation == t,
    }
}

fn request_lock(state: &mut ScreenOrientationShowState, target: Option<Orientation>) {
    let ReadState::Read(r) = &state.read else {
        return;
    };
    if !r.can_lock || state.busy() {
        return;
    }
    state.pending = Some(target);
    state.generation += 1;
    state.applying = false;
    state.error.clear();
    state.notice.clear();
}

/// 「縦に固定」。
pub fn lock_portrait(state: &mut ScreenOrientationShowState) {
    request_lock(state, Some(Orientation::Portrait));
}

/// 「横に固定」。
pub fn lock_landscape(state: &mut ScreenOrientationShowState) {
    request_lock(state, Some(Orientation::Landscape));
}

/// 「固定を解除」。
pub fn unlock(state: &mut ScreenOrientationShowState) {
    request_lock(state, None);
}

/// 「読み直す」。失敗後に読み取りをやり直す。
pub fn reload(state: &mut ScreenOrientationShowState) {
    state.read = ReadState::Unread;
    state.error.clear();
}

/// 保留中の要求を状態を変えずに返す。
pub fn take_request(state: &ScreenOrientationShowState) -> Option<LockRequest> {
    state.pending.map(|target| LockRequest {
        generation: state.generation,
        target,
    })
}

/// 要求を OS へ渡す直前に呼ぶ。世代が一致するときだけ保留を取り除く。
pub fn mark_applying(state: &mut ScreenOrientationShowState, generation: u64) {
    if state.generation != generation {
        return;
    }
    state.in_flight = state.pending.take();
    state.applying = true;
}

/// 固定の実行時 `Unsupported` を反映する（読み取りは残し、固定だけ使えない扱いにする）。
fn downgrade_lock(state: &mut ScreenOrientationShowState) {
    state.lock_unsupported = true;
    state.notice = LOCK_UNSUPPORTED_NOTE.to_string();
    if let ReadState::Read(r) = &mut state.read {
        r.can_lock = false;
    }
}

fn adopt_reading(
    state: &mut ScreenOrientationShowState,
    reading: Result<Reading, OrientationFailure>,
) {
    match reading {
        Ok(mut r) => {
            r.can_lock &= !state.lock_unsupported;
            state.read = ReadState::Read(r);
        }
        Err(OrientationFailure::Unsupported) => state.read = ReadState::Unsupported,
        Err(_) => state.read = ReadState::Failed,
    }
}

/// 要求の完了を反映する。`reading` は成否に関わらず OS から読み戻した値。
/// 世代が合わない（古い）完了は丸ごと捨てる。`Ok` でも読み戻しが満たさなければ失敗にする。
pub fn finish(
    state: &mut ScreenOrientationShowState,
    generation: u64,
    result: Result<(), OrientationFailure>,
    reading: Result<Reading, OrientationFailure>,
) {
    if state.generation != generation {
        return;
    }
    let target = state.in_flight.take();
    state.applying = false;

    let not_satisfied_reason = |t: Option<Orientation>| {
        if t.is_some() {
            NOT_LOCKED_REASON
        } else {
            NOT_UNLOCKED_REASON
        }
    };
    let failure = match (&result, &reading, target) {
        (Err(OrientationFailure::Unsupported), _, _) => None,
        (Err(f), _, _) => Some(f.clone()),
        (Ok(()), Ok(r), Some(t)) if !lock_satisfied(t, r) => Some(OrientationFailure::Failed(
            not_satisfied_reason(t).to_string(),
        )),
        (Ok(()), Err(e), _) if *e != OrientationFailure::Unsupported => Some(
            OrientationFailure::Failed(READBACK_FAILED_REASON.to_string()),
        ),
        _ => None,
    };

    adopt_reading(state, reading);
    if result == Err(OrientationFailure::Unsupported) {
        downgrade_lock(state);
        return;
    }
    if let Some(OrientationFailure::Failed(m)) = failure {
        state.error = format!("{FAILED_PREFIX}{m}");
    }
}

/// 定期読み取り（pull）の結果を反映する。
pub fn apply_read(
    state: &mut ScreenOrientationShowState,
    result: Result<Reading, OrientationFailure>,
) {
    match &result {
        Ok(_) => {}
        Err(OrientationFailure::Failed(m)) => state.error = format!("{FAILED_PREFIX}{m}"),
        Err(OrientationFailure::Unsupported) => {}
    }
    adopt_reading(state, result);
}

/// driver が OS を読み直すべきか。非対応・失敗では偽（失敗状態で再描画し続けない）。
pub fn wants_read(state: &ScreenOrientationShowState) -> bool {
    matches!(state.read, ReadState::Unread | ReadState::Read(_))
}

fn orientation_label(o: Orientation) -> &'static str {
    match o {
        Orientation::Portrait => PORTRAIT_LABEL,
        Orientation::Landscape => LANDSCAPE_LABEL,
    }
}

/// 現在の向きの案内。
pub fn status_text(state: &ScreenOrientationShowState) -> String {
    match &state.read {
        ReadState::Unread => READING_STATUS.to_string(),
        ReadState::Unsupported => UNSUPPORTED_STATUS.to_string(),
        ReadState::Failed => String::new(),
        ReadState::Read(r) => format!(
            "{CURRENT_PREFIX}{}（{}°）",
            orientation_label(r.orientation),
            r.angle
        ),
    }
}

/// 固定状態の案内。固定中は、読み戻しが固定した種類で、実際の向きもその種類のときだけ出す。
pub fn lock_text(state: &ScreenOrientationShowState) -> String {
    if state.applying {
        return SWITCHING_STATUS.to_string();
    }
    match &state.read {
        ReadState::Read(r) => match r.lock {
            Some(k) if r.orientation == k => match k {
                Orientation::Portrait => LOCKED_PORTRAIT_STATUS,
                Orientation::Landscape => LOCKED_LANDSCAPE_STATUS,
            }
            .to_string(),
            _ => NOT_LOCKED_STATUS.to_string(),
        },
        _ => String::new(),
    }
}

/// 固定だけ使えないときなどの補足。
pub fn notice_text(state: &ScreenOrientationShowState) -> String {
    match &state.read {
        ReadState::Read(r) if !r.can_lock => LOCK_UNSUPPORTED_NOTE.to_string(),
        _ => state.notice.clone(),
    }
}

/// エラーの案内。
pub fn error_text(state: &ScreenOrientationShowState) -> String {
    state.error.clone()
}

/// `effect` 区画（`registry.rs`）が要求する `fn(&S) -> Screen<S>` 形の入口。
/// 固定のボタンは、読み取り済みで固定でき、処理中でないときだけ出す。
pub fn current_screen(state: &ScreenOrientationShowState) -> Screen<ScreenOrientationShowState> {
    let mut screen = Screen::new("画面の向き")
        .dynamic_text(status_text)
        .dynamic_text(lock_text)
        .dynamic_text(notice_text)
        .error_text(error_text);
    match &state.read {
        ReadState::Read(r) if r.can_lock && !state.busy() => {
            screen = screen
                .button(LOCK_PORTRAIT_ID, LOCK_PORTRAIT_LABEL, lock_portrait)
                .button(LOCK_LANDSCAPE_ID, LOCK_LANDSCAPE_LABEL, lock_landscape)
                .button(UNLOCK_ID, UNLOCK_LABEL, unlock);
        }
        ReadState::Failed => screen = screen.button(RELOAD_ID, RELOAD_LABEL, reload),
        _ => {}
    }
    if matches!(state.read, ReadState::Read(_)) {
        screen = screen.text(HINT);
    }
    screen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_headless;

    fn reading(o: Orientation, lock: Option<Orientation>, can_lock: bool) -> Reading {
        Reading {
            orientation: o,
            angle: 0,
            lock,
            can_lock,
        }
    }

    fn ready() -> ScreenOrientationShowState {
        let mut s = ScreenOrientationShowState::default();
        apply_read(&mut s, Ok(reading(Orientation::Portrait, None, true)));
        s
    }

    fn failed(m: &str) -> OrientationFailure {
        OrientationFailure::Failed(m.to_string())
    }

    fn start(s: &mut ScreenOrientationShowState) -> u64 {
        let g = take_request(s).unwrap().generation;
        mark_applying(s, g);
        g
    }

    #[test]
    fn lock_success_updates_display() {
        let mut s = ready();
        lock_landscape(&mut s);
        let g = start(&mut s);
        assert_eq!(lock_text(&s), SWITCHING_STATUS);
        finish(
            &mut s,
            g,
            Ok(()),
            Ok(reading(
                Orientation::Landscape,
                Some(Orientation::Landscape),
                true,
            )),
        );
        assert_eq!(lock_text(&s), LOCKED_LANDSCAPE_STATUS);
        assert!(error_text(&s).is_empty());
        unlock(&mut s);
        let g = start(&mut s);
        finish(
            &mut s,
            g,
            Ok(()),
            Ok(reading(Orientation::Landscape, None, true)),
        );
        assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
        assert!(error_text(&s).is_empty());
    }

    #[test]
    fn ok_with_wrong_orientation_is_failure() {
        let mut s = ready();
        lock_landscape(&mut s);
        let g = start(&mut s);
        // 希望は横だが、実際の向きは縦のまま。
        finish(
            &mut s,
            g,
            Ok(()),
            Ok(reading(
                Orientation::Portrait,
                Some(Orientation::Landscape),
                true,
            )),
        );
        assert!(error_text(&s).contains(NOT_LOCKED_REASON));
        assert_ne!(lock_text(&s), LOCKED_LANDSCAPE_STATUS);
        assert_ne!(lock_text(&s), LOCKED_PORTRAIT_STATUS);
    }

    #[test]
    fn ok_with_lock_still_none_is_failure() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = start(&mut s);
        finish(
            &mut s,
            g,
            Ok(()),
            Ok(reading(Orientation::Portrait, None, true)),
        );
        assert!(error_text(&s).contains(NOT_LOCKED_REASON));
        assert_eq!(lock_text(&s), NOT_LOCKED_STATUS);
    }

    #[test]
    fn ok_unlock_with_lock_remaining_is_failure() {
        let mut s = ready();
        unlock(&mut s);
        let g = start(&mut s);
        finish(
            &mut s,
            g,
            Ok(()),
            Ok(reading(
                Orientation::Portrait,
                Some(Orientation::Portrait),
                true,
            )),
        );
        assert!(error_text(&s).contains(NOT_UNLOCKED_REASON));
    }

    #[test]
    fn stale_generation_is_dropped() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = start(&mut s);
        let before = s.clone();
        finish(
            &mut s,
            g + 1,
            Ok(()),
            Ok(reading(
                Orientation::Portrait,
                Some(Orientation::Portrait),
                true,
            )),
        );
        assert_eq!(s, before);
    }

    #[test]
    fn unsupported_read_has_no_buttons_and_no_request() {
        let mut s = ScreenOrientationShowState::default();
        apply_read(&mut s, Err(OrientationFailure::Unsupported));
        assert_eq!(s.read_state(), &ReadState::Unsupported);
        lock_portrait(&mut s);
        landscape_noop(&mut s);
        assert!(take_request(&s).is_none());
        assert!(!wants_read(&s));
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains(UNSUPPORTED_STATUS));
        assert!(!joined.contains(LOCK_PORTRAIT_LABEL));
    }

    fn landscape_noop(s: &mut ScreenOrientationShowState) {
        lock_landscape(s);
        unlock(s);
    }

    #[test]
    fn runtime_unsupported_lock_keeps_reading() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = start(&mut s);
        finish(
            &mut s,
            g,
            Err(OrientationFailure::Unsupported),
            Ok(reading(Orientation::Landscape, None, true)),
        );
        let ReadState::Read(r) = s.read_state() else {
            panic!("読み取りが残っていない");
        };
        assert!(!r.can_lock);
        assert_eq!(notice_text(&s), LOCK_UNSUPPORTED_NOTE);
        // 次の読み取りが can_lock = true を返しても戻らない。
        apply_read(&mut s, Ok(reading(Orientation::Landscape, None, true)));
        lock_portrait(&mut s);
        assert!(take_request(&s).is_none());
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(!joined.contains(LOCK_PORTRAIT_LABEL));
    }

    #[test]
    fn can_lock_false_makes_no_request() {
        let mut s = ScreenOrientationShowState::default();
        apply_read(&mut s, Ok(reading(Orientation::Portrait, None, false)));
        lock_portrait(&mut s);
        unlock(&mut s);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn readback_failure_is_not_success() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = start(&mut s);
        finish(&mut s, g, Ok(()), Err(failed("x")));
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(error_text(&s).contains(READBACK_FAILED_REASON));
        assert!(lock_text(&s).is_empty());
    }

    #[test]
    fn failure_message_is_shown() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = start(&mut s);
        finish(
            &mut s,
            g,
            Err(failed("画面の向きの操作")),
            Ok(reading(Orientation::Portrait, None, true)),
        );
        assert!(error_text(&s).starts_with(FAILED_PREFIX));
    }

    #[test]
    fn handlers_are_noop_while_busy() {
        let mut s = ready();
        lock_portrait(&mut s);
        let g = s.generation();
        lock_landscape(&mut s);
        unlock(&mut s);
        assert_eq!(s.generation(), g);
        assert_eq!(
            take_request(&s).unwrap().target,
            Some(Orientation::Portrait)
        );
        mark_applying(&mut s, g);
        lock_landscape(&mut s);
        assert_eq!(s.generation(), g);
        assert!(take_request(&s).is_none());
    }

    #[test]
    fn unread_makes_no_request() {
        let mut s = ScreenOrientationShowState::default();
        lock_portrait(&mut s);
        assert!(take_request(&s).is_none());
        assert_eq!(status_text(&s), READING_STATUS);
    }

    #[test]
    fn failed_read_shows_reload_and_stops_polling() {
        let mut s = ready();
        apply_read(&mut s, Err(failed("x")));
        assert_eq!(s.read_state(), &ReadState::Failed);
        assert!(!wants_read(&s));
        let joined = render_headless(&current_screen(&s), &s).join("\n");
        assert!(joined.contains(RELOAD_LABEL));
        reload(&mut s);
        assert!(wants_read(&s));
    }

    #[test]
    fn status_text_shows_orientation_and_angle() {
        let mut s = ScreenOrientationShowState::default();
        apply_read(
            &mut s,
            Ok(Reading {
                orientation: Orientation::Landscape,
                angle: 90,
                lock: None,
                can_lock: true,
            }),
        );
        assert_eq!(status_text(&s), "いまの向き: 横（90°）");
    }
}
