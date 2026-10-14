//! システムトレイ（メニューバーアイコン / 通知領域アイコン）の抽象（Phase 11 の
//! 拡充ループ、親 #1038、Issue #1039）。`Notifier`/`Share` と同じ request/response
//! 型の操作（`show`/`hide`）に加え、表示中はメニュー選択を継続的に受け取る必要が
//! あるため、選択の受け取りは専用の [`SelectionQueue`] で管理する。
//!
//! # 契約（各実装・呼び出し元が守る不変条件）
//!
//! - **対象外 OS**: [`SystemTray::show`] は `Err(Unsupported("system_tray"))`、
//!   [`SystemTray::hide`] は `Ok(NotShown)`（後始末の経路で失敗させない）、
//!   [`SystemTray::take_selection`] は `None` を返す（[`UnsupportedSystemTray`]）
//! - **二重表示**: 表示中の `show` は 2 個目のアイコンを作らず `Ok(AlreadyShown)` を
//!   返す。メニューも差し替えない
//! - **未表示の非表示**: `Ok(NotShown)` を返す
//! - **失敗**: OS 側の追加に失敗したときは `Err(Internal("system_tray show"))` を
//!   返す。確保済みのウィンドウ・スレッドは解放してから返す
//!   （`device-bridge-contracts.md`「9. 失敗時のリソース解放」）
//! - **選択の受け取り**: `hide` の後と、前回 `show` 時と異なる世代の選択は返さない
//!   （[`SelectionQueue::end`]/[`SelectionQueue::push`] の世代照合）。範囲外の
//!   index は捨てる
//! - **終了時**: 呼び出し側（`fandhe-egui-proxy`）が `on_exit` で `hide` を呼ぶ
//!   （プロセス終了任せにしない。Windows は `NIM_DELETE` しないとアイコンが
//!   通知領域に残るため）
//! - 初期化に失敗した実装は [`UnsupportedSystemTray`] へ倒す（ダミーの成功実装へは
//!   倒さない、`device-bridge-contracts.md`「6. 嘘の成功」）

use std::sync::{Arc, Mutex};

use crate::DeviceError;

/// メニュー項目数の上限（`TrackPopupMenu`/`NSMenu` いずれも実用上十分な件数、
/// 際限のない入力による描画コスト増大を避ける事前検証）。
pub const MAX_MENU_ITEMS: usize = 8;
/// タイトルの最大文字数（メニューバーのラベル・Windows の `szTip` 両方の目安）。
pub const TITLE_MAX_CHARS: usize = 16;
/// メニュー項目ラベルの最大文字数。
pub const ITEM_MAX_CHARS: usize = 64;
/// 選択キューの保持上限（OS コールバックが短時間に大量の選択を送ってきても
/// 際限なく溜めない、DoS 対策）。
const QUEUE_CAP: usize = 16;

/// ラベル（タイトル・メニュー項目）の共通検証。空文字・NUL・その他の制御文字・
/// 文字数超過を拒否する（`feature` はエラーに含める固定識別子、`notification.rs`
/// の `NotificationRequest::new` と同じ方針）。
fn validate_label(value: &str, max_chars: usize, feature: &'static str) -> Result<(), DeviceError> {
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(DeviceError::InvalidInput(feature));
    }
    if value.chars().any(|c| c == '\0' || c.is_control()) {
        return Err(DeviceError::InvalidInput(feature));
    }
    Ok(())
}

/// 検証済みのメニュー内容。[`TrayMenu::new`] 以外で作れない（検証の SSOT、
/// `notification::NotificationRequest` と同じ方針）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenu {
    title: String,
    items: Vec<String>,
}

impl TrayMenu {
    /// `title`・`items` を検証して構築する。
    ///
    /// - `title`: 1〜[`TITLE_MAX_CHARS`] 文字。空文字・NUL・制御文字は
    ///   `InvalidInput("system_tray menu")`
    /// - `items`: 1〜[`MAX_MENU_ITEMS`] 件。件数超過・0 件も同エラー。各要素は
    ///   1〜[`ITEM_MAX_CHARS`] 文字、空文字・NUL・制御文字は同エラー
    pub fn new(title: &str, items: &[&str]) -> Result<Self, DeviceError> {
        validate_label(title, TITLE_MAX_CHARS, "system_tray menu")?;
        if items.is_empty() || items.len() > MAX_MENU_ITEMS {
            return Err(DeviceError::InvalidInput("system_tray menu"));
        }
        for item in items {
            validate_label(item, ITEM_MAX_CHARS, "system_tray menu")?;
        }
        Ok(Self {
            title: title.to_string(),
            items: items.iter().map(|s| (*s).to_string()).collect(),
        })
    }

    /// メニューバー・通知領域に出すタイトル（1〜[`TITLE_MAX_CHARS`] 文字を保証）。
    pub fn title(&self) -> &str {
        &self.title
    }

    /// メニュー項目一覧（1〜[`MAX_MENU_ITEMS`] 件を保証、各要素は
    /// 1〜[`ITEM_MAX_CHARS`] 文字を保証）。
    pub fn items(&self) -> &[String] {
        &self.items
    }
}

/// [`SystemTray::show`] の結果。モジュール doc の「二重表示」契約を参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowOutcome {
    /// 新規にアイコンを追加した。
    Added,
    /// 既に表示中だったため何もしなかった（エラーではない）。
    AlreadyShown,
}

/// [`SystemTray::hide`] の結果。モジュール doc の「未表示の非表示」契約を参照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HideOutcome {
    /// アイコンを削除した。
    Removed,
    /// 元々表示していなかったため何もしなかった（エラーではない）。
    NotShown,
}

/// システムトレイ（メニューバー / 通知領域アイコン）の契約（モジュール doc の
/// 「契約」節を参照）。`Send + Sync`: effect ドライバ側の `static` 実装
/// （`fandhe-egui-proxy::device::system_tray::platform_tray`）がスレッド間で
/// 共有されるため。
pub trait SystemTray: Send + Sync {
    /// アイコンを表示する（冪等、二重表示は `Ok(AlreadyShown)`）。
    fn show(&self, menu: &TrayMenu) -> Result<ShowOutcome, DeviceError>;
    /// アイコンを消す（冪等、未表示は `Ok(NotShown)`）。
    fn hide(&self) -> Result<HideOutcome, DeviceError>;
    /// 未読のメニュー選択を 1 件取り出す（無ければ `None`）。呼び出し側が
    /// 空になるまで繰り返し呼ぶ契約（`fandhe-egui-proxy::device::system_tray::drive`
    /// 参照）。
    fn take_selection(&self) -> Option<usize>;
}

/// 対応不可プラットフォーム用（Linux・Web、初期化失敗時のフォールバック）。
/// モジュール doc の「対象外 OS」契約をそのまま実装する。
pub struct UnsupportedSystemTray;

impl SystemTray for UnsupportedSystemTray {
    fn show(&self, _menu: &TrayMenu) -> Result<ShowOutcome, DeviceError> {
        Err(DeviceError::Unsupported("system_tray"))
    }

    fn hide(&self) -> Result<HideOutcome, DeviceError> {
        Ok(HideOutcome::NotShown)
    }

    fn take_selection(&self) -> Option<usize> {
        None
    }
}

/// 選択キューの内部状態（[`SelectionQueue`] が `Mutex` で包む）。
struct QueueState {
    /// `begin` のたびに進む世代。`push` の世代照合に使う。
    generation: u64,
    /// `begin`（表示開始）〜`end`（非表示）の間だけ `true`。
    active: bool,
    /// 現在のメニューの項目数（`push` の範囲検証に使う）。
    item_count: usize,
    /// 未読の選択（FIFO）。
    items: Vec<usize>,
}

/// macOS（`apple_system_tray.rs`）・Windows（`windows_system_tray.rs`）の両実装が
/// 共有する「OS コールバックから届く選択」の受け皿。OS 側コールバック（ObjC の
/// `onSelect:`・Win32 の `WndProc`）は任意スレッドから [`push`](Self::push) を
/// 呼べる一方、取り出しは effect ドライバの UI スレッドから行う想定（`Mutex` で
/// 両立させる）。
pub struct SelectionQueue(Mutex<QueueState>);

impl SelectionQueue {
    /// static 初期化用。
    pub const fn new() -> Self {
        Self(Mutex::new(QueueState {
            generation: 0,
            active: false,
            item_count: 0,
            items: Vec::new(),
        }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 表示開始の直前に呼ぶ。世代を進めて有効化し、戻り値の世代を OS 側へ渡す
    /// （`push` の照合に使わせる）。未読の選択は破棄する（古い表示の選択が新しい
    /// 表示へ紛れ込まないようにする、`device-bridge-contracts.md`「2. 並行性・世代」）。
    pub fn begin(&self, item_count: usize) -> u64 {
        let mut state = self.lock();
        state.generation = state.generation.wrapping_add(1);
        state.active = true;
        state.item_count = item_count;
        state.items.clear();
        state.generation
    }

    /// 未表示のときだけ [`begin`](Self::begin) する。表示中なら `None`。表示中かの
    /// 判定と開始を同じロック内で行い、2 スレッドが同時に表示を始めて二重表示に
    /// なるのを防ぐ（`is_active` と `begin` を別々に呼ぶと間に割り込まれる。PR #1041
    /// codex 指摘）。
    pub fn try_begin(&self, item_count: usize) -> Option<u64> {
        let mut state = self.lock();
        if state.active {
            return None;
        }
        state.generation = state.generation.wrapping_add(1);
        state.active = true;
        state.item_count = item_count;
        state.items.clear();
        Some(state.generation)
    }

    /// 非表示にする（無効化 + 未読の選択を破棄）。モジュール doc の「`hide` の後は
    /// 選択を返さない」契約の実体。
    pub fn end(&self) {
        let mut state = self.lock();
        state.active = false;
        state.items.clear();
    }

    /// 現在有効（`begin` 済み・`end` 未呼び出し）かどうか。macOS/Windows 実装が
    /// 「表示中かどうか」の判定そのものに流用する（二重表示判定を Rust 側の状態
    /// だけで完結させ、OS 側へ毎回問い合わせない設計、`apple_system_tray.rs`/
    /// `windows_system_tray.rs` のモジュール doc 参照）。
    pub fn is_active(&self) -> bool {
        self.lock().active
    }

    /// OS コールバックから選択を積む。有効で、`generation` が一致し、
    /// `0..item_count` の範囲内で、上限 [`QUEUE_CAP`] 未満のときだけ積んで `true`
    /// を返す。それ以外（無効・世代不一致・範囲外・上限超過）は黙って捨てて
    /// `false`（判定と更新を同じロック内で行う、`device-bridge-contracts.md`
    /// 「2. 並行性・世代」）。
    pub fn push(&self, generation: u64, index: i64) -> bool {
        let mut state = self.lock();
        if !state.active || generation != state.generation {
            return false;
        }
        let Ok(index) = usize::try_from(index) else {
            return false;
        };
        if index >= state.item_count || state.items.len() >= QUEUE_CAP {
            return false;
        }
        state.items.push(index);
        true
    }

    /// 未読の選択を 1 件取り出す（FIFO）。
    pub fn pop(&self) -> Option<usize> {
        let mut state = self.lock();
        if state.items.is_empty() {
            None
        } else {
            Some(state.items.remove(0))
        }
    }
}

impl Default for SelectionQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// テスト・ホスト環境（実デバイスが無い CI 等）向けのダミー実装。呼び出し回数と
/// 「作ったアイコン数」を記録し、[`DummySystemTray::select`] で選択を注入できる。
/// モジュール doc の契約（二重表示・未表示の非表示・選択の世代管理）どおりに
/// 振る舞う（`gamepad::DummyGamepad` と同型の位置づけ）。
#[derive(Clone)]
pub struct DummySystemTray(Arc<Mutex<DummyState>>);

struct DummyState {
    shown: bool,
    show_calls: u32,
    hide_calls: u32,
    created_icons: u32,
    queue: QueueState,
}

impl DummySystemTray {
    /// 初期状態は「非表示」。
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(DummyState {
            shown: false,
            show_calls: 0,
            hide_calls: 0,
            created_icons: 0,
            queue: QueueState {
                generation: 0,
                active: false,
                item_count: 0,
                items: Vec::new(),
            },
        })))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, DummyState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `show` が呼ばれた回数。
    pub fn show_calls(&self) -> u32 {
        self.lock().show_calls
    }

    /// `hide` が呼ばれた回数。
    pub fn hide_calls(&self) -> u32 {
        self.lock().hide_calls
    }

    /// 実際に新規作成したアイコンの個数（二重表示は増えない）。
    pub fn created_icons(&self) -> u32 {
        self.lock().created_icons
    }

    /// 表示中にメニュー項目 `index` が選択されたことにする。表示していない間は
    /// 無視する（OS コールバックの契約を模す）。
    pub fn select(&self, index: usize) {
        let mut state = self.lock();
        if !state.queue.active || index >= state.queue.item_count {
            return;
        }
        state.queue.items.push(index);
    }
}

impl Default for DummySystemTray {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTray for DummySystemTray {
    fn show(&self, menu: &TrayMenu) -> Result<ShowOutcome, DeviceError> {
        let mut state = self.lock();
        state.show_calls += 1;
        if state.shown {
            return Ok(ShowOutcome::AlreadyShown);
        }
        state.shown = true;
        state.created_icons += 1;
        state.queue.generation = state.queue.generation.wrapping_add(1);
        state.queue.active = true;
        state.queue.item_count = menu.items().len();
        state.queue.items.clear();
        Ok(ShowOutcome::Added)
    }

    fn hide(&self) -> Result<HideOutcome, DeviceError> {
        let mut state = self.lock();
        state.hide_calls += 1;
        if !state.shown {
            return Ok(HideOutcome::NotShown);
        }
        state.shown = false;
        state.queue.active = false;
        state.queue.items.clear();
        Ok(HideOutcome::Removed)
    }

    fn take_selection(&self) -> Option<usize> {
        let mut state = self.lock();
        if state.queue.items.is_empty() {
            None
        } else {
            Some(state.queue.items.remove(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_begin_refuses_while_active() {
        let q = SelectionQueue::new();
        let g = q.try_begin(2).expect("未表示なら開始できる");
        assert!(q.try_begin(2).is_none(), "表示中は二重に開始しない");
        q.end();
        assert!(q.try_begin(2).expect("終了後は再開できる") > g);
    }

    fn menu() -> TrayMenu {
        TrayMenu::new("Fandhe", &["朝のあいさつ", "夜のあいさつ", "消す"]).unwrap()
    }

    #[test]
    fn tray_menu_rejects_empty_title() {
        assert!(matches!(
            TrayMenu::new("", &["a"]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_rejects_title_over_max_chars() {
        let title = "a".repeat(TITLE_MAX_CHARS + 1);
        assert!(matches!(
            TrayMenu::new(&title, &["a"]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_accepts_title_at_max_chars() {
        let title = "a".repeat(TITLE_MAX_CHARS);
        assert!(TrayMenu::new(&title, &["a"]).is_ok());
    }

    #[test]
    fn tray_menu_rejects_empty_items() {
        assert!(matches!(
            TrayMenu::new("t", &[]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_rejects_items_over_max_count() {
        let items: Vec<&str> = (0..=MAX_MENU_ITEMS).map(|_| "a").collect();
        assert!(matches!(
            TrayMenu::new("t", &items),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_accepts_items_at_max_count() {
        let items: Vec<&str> = (0..MAX_MENU_ITEMS).map(|_| "a").collect();
        assert!(TrayMenu::new("t", &items).is_ok());
    }

    #[test]
    fn tray_menu_rejects_item_with_nul() {
        assert!(matches!(
            TrayMenu::new("t", &["a\0b"]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_rejects_item_over_max_chars() {
        let item = "a".repeat(ITEM_MAX_CHARS + 1);
        assert!(matches!(
            TrayMenu::new("t", &[&item]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn tray_menu_rejects_control_character() {
        assert!(matches!(
            TrayMenu::new("t\u{0007}", &["a"]),
            Err(DeviceError::InvalidInput("system_tray menu"))
        ));
    }

    #[test]
    fn unsupported_tray_show_is_unsupported() {
        assert!(matches!(
            UnsupportedSystemTray.show(&menu()),
            Err(DeviceError::Unsupported("system_tray"))
        ));
    }

    #[test]
    fn unsupported_tray_hide_is_not_shown_not_error() {
        assert_eq!(UnsupportedSystemTray.hide().unwrap(), HideOutcome::NotShown);
    }

    #[test]
    fn unsupported_tray_take_selection_is_none() {
        assert_eq!(UnsupportedSystemTray.take_selection(), None);
    }

    #[test]
    fn selection_queue_push_requires_matching_generation() {
        let queue = SelectionQueue::new();
        let generation = queue.begin(3);
        assert!(!queue.push(generation.wrapping_add(1), 0));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn selection_queue_push_rejects_out_of_range_index() {
        let queue = SelectionQueue::new();
        let generation = queue.begin(2);
        assert!(!queue.push(generation, 2));
        assert!(!queue.push(generation, -1));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn selection_queue_push_accepts_in_range_index() {
        let queue = SelectionQueue::new();
        let generation = queue.begin(2);
        assert!(queue.push(generation, 1));
        assert_eq!(queue.pop(), Some(1));
    }

    #[test]
    fn selection_queue_end_discards_unread_and_future_push() {
        let queue = SelectionQueue::new();
        let generation = queue.begin(2);
        assert!(queue.push(generation, 0));
        queue.end();
        assert_eq!(queue.pop(), None, "end 後は未読が破棄される");
        assert!(!queue.push(generation, 0), "end 後の push は無視される");
    }

    #[test]
    fn selection_queue_begin_discards_previous_unread() {
        let queue = SelectionQueue::new();
        let gen1 = queue.begin(2);
        assert!(queue.push(gen1, 0));
        let gen2 = queue.begin(2);
        assert_ne!(gen1, gen2);
        assert_eq!(queue.pop(), None, "begin で前回の未読は破棄される");
    }

    #[test]
    fn selection_queue_enforces_cap() {
        let queue = SelectionQueue::new();
        let generation = queue.begin(1);
        for _ in 0..QUEUE_CAP {
            assert!(queue.push(generation, 0));
        }
        assert!(!queue.push(generation, 0), "上限を超えた push は無視される");
    }

    #[test]
    fn dummy_tray_double_show_is_already_shown_with_single_icon() {
        let tray = DummySystemTray::new();
        assert_eq!(tray.show(&menu()).unwrap(), ShowOutcome::Added);
        assert_eq!(tray.show(&menu()).unwrap(), ShowOutcome::AlreadyShown);
        assert_eq!(tray.created_icons(), 1);
        assert_eq!(tray.show_calls(), 2);
    }

    #[test]
    fn dummy_tray_hide_without_show_is_not_shown() {
        let tray = DummySystemTray::new();
        assert_eq!(tray.hide().unwrap(), HideOutcome::NotShown);
        assert_eq!(tray.hide_calls(), 1);
    }

    #[test]
    fn dummy_tray_hide_after_show_removes_and_stops_returning_selection() {
        let tray = DummySystemTray::new();
        tray.show(&menu()).unwrap();
        tray.select(0);
        assert_eq!(tray.hide().unwrap(), HideOutcome::Removed);
        assert_eq!(tray.take_selection(), None, "hide 後は選択を返さない契約");
    }

    #[test]
    fn dummy_tray_select_while_hidden_is_ignored() {
        let tray = DummySystemTray::new();
        tray.select(0);
        assert_eq!(tray.take_selection(), None);
    }

    #[test]
    fn dummy_tray_select_out_of_range_is_ignored() {
        let tray = DummySystemTray::new();
        tray.show(&menu()).unwrap();
        tray.select(99);
        assert_eq!(tray.take_selection(), None);
    }

    #[test]
    fn dummy_tray_take_selection_returns_in_fifo_order() {
        let tray = DummySystemTray::new();
        tray.show(&menu()).unwrap();
        tray.select(0);
        tray.select(2);
        assert_eq!(tray.take_selection(), Some(0));
        assert_eq!(tray.take_selection(), Some(2));
        assert_eq!(tray.take_selection(), None);
    }
}
