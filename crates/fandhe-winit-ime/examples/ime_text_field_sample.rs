//! TASK-8.5-S30 使用ガイド用の IME 実配線サンプル（REQ-8 / M3、Issue #211）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）の最終 sub-issue（30/30）。既存の実機検証
//! ハーネス `macos_ime_probe.rs`（S10 / #191）は winit `Ime::*` → コア境界型変換の確認に
//! 特化したプローブであり、`TextFieldWidget` / `ImeFocusCoordinator` を含む実際のアプリ
//! 配線例ではなかった。本サンプルは `docs/ime/ime-usage-guide.md` の一次情報（ガイドの
//! コード例はここから抜粋する）として、以下をすべて 1 つのウィンドウで配線する:
//!
//! - 2 個のテキストフィールド（[`fandhe_core::ui::TextFieldWidget`]）と
//!   [`fandhe_core::ui::ImeFocusCoordinator`] によるフォーカス調停（Tab キーで切替）
//! - winit `WindowEvent::Ime` → [`fandhe_winit_ime::ime_event_from_winit`] →
//!   `ImeFocusCoordinator::dispatch_ime` → `take_ime_requests` →
//!   [`fandhe_winit_ime::apply_ime_request`] の往復
//! - [`fandhe_core::ui::ime_cursor_area`] による変換候補ウィンドウ位置の計算・通知
//! - 非 IME 経路の文字入力（`WindowEvent::KeyboardInput`）を
//!   [`fandhe_winit_ime::text_field_input_from_key`] 経由で編集反映
//!
//! 対応 REQ/TASK: REQ-8（IME 統合）/ TASK-8.5（親 #181）。設計文書は
//! `docs/architecture/ime-architecture.md` §4.7（複数フィールド調停）・
//! `docs/architecture/text-input-state-model.md`（コンポジション状態モデル）を参照。
//!
//! # コンポジション矩形計算の制約（本サンプル固有）
//!
//! [`fandhe_core::ui::TextFieldWidget`] はコンポジション内部状態（`Composition` の
//! `anchor`/`preedit` テキスト）を `is_composing()`（真偽値のみ）以外に公開しない
//! （`ime_cursor_area.rs` モジュール doc「`Composition` 型統合は TASK-8.5-S8 へ申し送り」
//! 参照）。そのため本サンプルは、フィールド側から届く
//! `ImeEvent::Preedit` の `text`/`cursor` をアプリ層で一時的に保持し、
//! [`fandhe_core::ui::ime_cursor_area`] の入力として直接渡す（`TextFieldWidget` の
//! `buffer`/`cursor` から得られる `anchor` と組み合わせる）。実アプリでレイアウトの
//! 変更に追随する場合も同様の橋渡しが必要になる点に注意する。
//!
//! # セキュリティ考慮（`.claude/rules/security.md` 準拠）
//!
//! 標準出力へは **イベント種別・文字数・カーソル範囲・フォーカス中フィールド ID
//! （"field_a"/"field_b"、機微情報を含まない）のみ**を記録する
//! （[`fandhe_core::ImeEvent`] の `Debug` 実装が既に文字数マスクを行うため、
//! `{:?}` へ委譲するだけで契約を満たす）。確定済みバッファ内容・未確定文字列・
//! それらから導出した値（ハッシュ等）は一切出力しない。手元実行時も検証入力には
//! ダミー文字列のみを使うこと（`macos_ime_probe.rs` と同じ方針）。
//!
//! 内容一致の検証は行わない（codex-review P0 指摘対応: 受信イベントが発生元の
//! どちらのフィールドに属するかはイベント受信時点の `coordinator.focused()`
//! でしか判別できず、遅延到着したイベント（Issue #587 の既知限界）では
//! `field_b`（[`fandhe_core::ui::ImePurpose::Password`]）由来の内容が `field_a`
//! 扱いで内容比較へ渡りうる。発生元を証明できないイベントに対して内容一致の
//! 真偽値を出力すると、それ自体が Password 欄の内容を推測する照合オラクルに
//! なるため、内容由来の比較・出力を実装しない。本ハーネスは文字描画を持たないため
//! （画面上にテキストが見えない）、確認は
//! ログの `Commit` イベント受信・`text_len`（確定文字数）によるイベント受信確認に
//! 限定し、フィールドへの実際の反映（表示）は確認しない）。
//!
//! `dispatch_ime` 直後、field_a（非 Password）に限り
//! `[ime] field_a buffer_len=<N> composing=<bool>` を追加出力する（codex-review P1・
//! Cursor 指摘対応、PR #682）。`buffer_len` は `TextFieldWidget::buffer()`、`composing` は
//! `TextFieldWidget::is_composing()`（いずれも文字数・真偽値のみ、内容は出力しない）から
//! 得る。アプリ層が保持する `preedit_text` のコピー長は使わない（キャンセル経路では
//! dispatch 前に必ずクリアされるため、ウィジェットが composition を保持していても
//! 常に 0 を示す同語反復になる）。実機確認手順の P1（確定後の
//! buffer 反映確認）・P2（Esc 後の composition 消去・buffer 不変確認）はこのログの
//! `buffer_len`/`composing` を判定に使う。field_b はこのログを一切出力しない。
//!
//! # 実行方法
//!
//! ```text
//! cargo run -p fandhe-winit-ime --example ime_text_field_sample
//! ```
//!
//! ウィンドウ内をクリックしてフォーカスし、日本語 IME 等で入力を確定する。
//! Tab キーで 2 つのフィールド間のフォーカスを切り替えられる（切替時に
//! `ImeFocusCoordinator` が §4.7 の順序契約で `SetEnabled(false)` →
//! `SetEnabled(true)` を発行する）。対話的な実行確認はディスプレイ・IME を要するため
//! CI では不可（コンパイル確認のみ）。実機での確認は人間担当とする
//! （既存 S10 検証ハーネスと同じ運用）。
//!
//! `field_b` は [`fandhe_core::ui::ImePurpose::Password`] に設定済みで、
//! [`fandhe_winit_ime::sensitive_input::SensitiveInputTracker`] の実配線例を兼ねる
//! （Issue #586）。macOS 実行時、`field_b` へフォーカスすると SecureEventInput が
//! 有効化され日本語入力ソースが使えなくなる（メニューバーの入力ソースがロック表示 /
//! かな入力が ASCII 直接入力になる）ことが実挙動確認になる。`field_a` へ戻ると
//! 日本語入力が復帰する。
#![forbid(unsafe_code)]

use fandhe_core::ui::{
    ime_cursor_area, ImeCursorAreaParams, ImeFocusCoordinator, ImePurpose, Point, Size,
    TextFieldWidget, Widget, WidgetId,
};
use fandhe_winit_ime::sensitive_input::{apply_sensitive_input, SensitiveInputTracker};
use fandhe_winit_ime::{
    apply_ime_request, ime_event_from_winit, text_field_input_from_key, ImeSessionTracker,
};

use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// アプリ状態 `S`。本サンプルは配線の確認が目的で、アプリ固有の状態を持たないため
/// ユニット型を使う（`TextFieldWidget<()>`）。
type AppState = ();

/// テキストフィールドのレイアウト矩形（論理ピクセル）。本サンプルはレイアウト
/// エンジンを持たないため固定値とする（実アプリでは layout パス（`ui::layout`）の
/// 結果を使う）。
struct FieldLayout {
    origin: Point,
    size: Size,
}

/// フィールド 1 個分の状態一式。`preedit_text`/`preedit_cursor` はこのフィールドが
/// 直近で受け取った `ImeEvent::Preedit` を一時保持する（モジュール doc「コンポジション
/// 矩形計算の制約」参照。`TextFieldWidget` 自体は preedit 内容を公開しないための橋渡し）。
struct FieldSlot {
    widget: TextFieldWidget<AppState>,
    layout: FieldLayout,
    preedit_text: String,
    preedit_cursor: Option<(usize, usize)>,
}

impl FieldSlot {
    fn new(id: u64, label: &str, origin: Point, size: Size) -> Self {
        Self {
            widget: TextFieldWidget::new_static(WidgetId::from_raw(id), label, ""),
            layout: FieldLayout { origin, size },
            preedit_text: String::new(),
            preedit_cursor: None,
        }
    }

    /// このフィールドの現在の候補ウィンドウ矩形（論理ピクセル）を計算する。
    fn cursor_area(&self) -> fandhe_core::ui::Rect {
        let params = ImeCursorAreaParams {
            field_origin: self.layout.origin,
            field_size: self.layout.size,
            buffer: self.widget.buffer(),
            anchor: self.widget.cursor(),
            preedit: &self.preedit_text,
            preedit_cursor: self.preedit_cursor,
        };
        ime_cursor_area(&params)
    }
}

/// サンプル本体。2 フィールド + フォーカス調停者 + ウィンドウを保持する。
struct ImeTextFieldSample {
    window: Option<Window>,
    field_a: FieldSlot,
    field_b: FieldSlot,
    coordinator: ImeFocusCoordinator,
    /// `coordinator` が払い出す `ImeSessionId` を winit の `Ime::*` イベント列へ帰属させる
    /// 状態機械（Issue #587）。フォーカス遷移直後に旧フィールドの IME セッション由来の
    /// 遅延イベントが新フィールドへ誤配置されないよう、`dispatch_ime` へ渡す帰属セッションを
    /// 算出する（`fandhe_winit_ime::session` モジュール doc 参照）。
    tracker: ImeSessionTracker,
    /// `field_b`（`ImePurpose::Password`）の学習・予測変換抑止の代替担保配線
    /// （Issue #586、モジュール doc「`field_b` は Password」参照）。
    sensitive_tracker: SensitiveInputTracker,
    /// `WindowEvent::Focused(false)` による `blur_all_fields` 直前にフォーカスしていた
    /// フィールドを退避する（Cursor Bugbot 指摘対応）。`blur_all_fields` は
    /// `coordinator.focused()` を必ず `None` にするため、これを退避しておかないと
    /// `WindowEvent::Focused(true)` でアプリへ復帰した際に `focus_other_field`（Tab）が
    /// 「現在フォーカスなし」を「field_b からの切替」と区別できず常に field_a を対象にし、
    /// またフォーカス自体も外れたままキー入力が落ちる。
    focus_before_app_blur: Option<WidgetId>,
}

impl Default for ImeTextFieldSample {
    fn default() -> Self {
        let mut field_b =
            FieldSlot::new(2, "field_b", Point::new(40.0, 84.0), Size::new(240.0, 24.0));
        // `set_ime_purpose` はフォーカス外呼び出しでは ImeRequest を積まない
        // （`text_field.rs::set_ime_purpose` 契約）。フォーカス時に `apply_requests` の
        // 通常経路（`SetPurpose` → `SetEnabled(true)`）が sensitive_tracker を駆動する。
        field_b.widget.set_ime_purpose(ImePurpose::Password);
        Self {
            window: None,
            field_a: FieldSlot::new(1, "field_a", Point::new(40.0, 40.0), Size::new(240.0, 24.0)),
            field_b,
            coordinator: ImeFocusCoordinator::new(),
            tracker: ImeSessionTracker::new(),
            sensitive_tracker: SensitiveInputTracker::new(),
            focus_before_app_blur: None,
        }
    }
}

impl ImeTextFieldSample {
    /// `requests` を順序どおりウィンドウへ適用する（§4.7 の
    /// 「旧 disable → 新 enable」順序契約を維持したまま、生成順に処理するだけでよい）。
    /// 各要求を `apply_ime_request` へ渡す前に `tracker.observe_request` へも通し、
    /// `ImeRequest::BeginSession`（Issue #587）を状態機械へ反映させる。
    ///
    /// `apply_ime_request`（winit への委譲）とは独立に、各要求を
    /// `sensitive_tracker`（`field_b` の Password 抑止、Issue #586）へも観測させる
    /// （両者は並列に呼ぶ設計で、どちらの呼び出し順でも構わない）。
    fn apply_requests(
        &mut self,
        window: &Window,
        requests: impl IntoIterator<Item = fandhe_core::ui::ImeRequest>,
    ) {
        for request in requests {
            self.tracker.observe_request(&request);
            if let Some(active) = self.sensitive_tracker.observe(&request) {
                let succeeded = apply_sensitive_input(active);
                // 有効化・無効化のいずれが実際に成功したかを tracker へ報告する
                // （codex-review P1 対応: 無効化（DisableSecureEventInput）の失敗を無視すると
                // プロセス全体の SecureEventInput ロックが残存したまま「無効化済み」の
                // 前提で処理が続行されてしまう、`record_apply_result` の doc 参照）。
                self.sensitive_tracker
                    .record_apply_result(active, succeeded);
            }
            apply_ime_request(window, &request);
        }
    }

    /// フォーカス中フィールドの候補ウィンドウ矩形を計算し、`SetCursorArea` を送る。
    fn update_cursor_area(&self, window: &Window) {
        let Some(focused) = self.coordinator.focused() else {
            return;
        };
        let slot = if focused == self.field_a.widget.id() {
            &self.field_a
        } else if focused == self.field_b.widget.id() {
            &self.field_b
        } else {
            return;
        };
        let rect = slot.cursor_area();
        let request = fandhe_core::ui::ImeRequest::SetCursorArea {
            origin: (rect.origin.x, rect.origin.y),
            size: (rect.size.width, rect.size.height),
        };
        apply_ime_request(window, &request);
    }

    /// Tab キーでフィールド間フォーカスを切り替える。
    fn focus_other_field(&mut self, window: &Window) {
        let current = self.coordinator.focused();
        let target = if current == Some(self.field_a.widget.id()) {
            self.field_b.widget.id()
        } else {
            self.field_a.widget.id()
        };
        // フォーカスを外すフィールドの preedit 状態をクリアする（モジュール doc「コンポジション
        // 矩形計算の制約」参照）。`TextFieldWidget` 側の composition は `FocusLost` でクリアされる
        // が、アプリ層がここで保持する preedit_text/preedit_cursor は別管理のブリッジ状態で
        // あり、`handle_ime_event` の Commit/Disabled 分岐でしかクリアされない。Tab 切替は
        // FocusLost 経由でも上記分岐を通らないため、ここで明示的にクリアしないと、再度
        // フォーカスが戻った際に update_cursor_area が古い preedit オフセットを候補ウィンドウ
        // 位置計算へ渡してしまう（Bugbot 指摘対応）。
        if current == Some(self.field_a.widget.id()) {
            self.field_a.preedit_text.clear();
            self.field_a.preedit_cursor = None;
        } else if current == Some(self.field_b.widget.id()) {
            self.field_b.preedit_text.clear();
            self.field_b.preedit_cursor = None;
        }
        let mut fields = [&mut self.field_a.widget, &mut self.field_b.widget];
        let transition = self.coordinator.focus(&mut fields, target);
        // blur-commit で発生しうる Event::TextFieldChanged はアプリ状態へのディスパッチ先を
        // 持たない本サンプルでは読み捨てる（実アプリでは StateStore::dispatch へ回す）。
        drop(transition.events);
        self.apply_requests(window, transition.requests);
        self.update_cursor_area(window);
    }

    /// アプリ全体がフォーカスを失った際（`WindowEvent::Focused(false)`、Cmd-Tab 等での
    /// 他アプリへの切替）にすべてのフィールドを blur する（Cursor Bugbot 指摘対応:
    /// `SensitiveInputTracker` は `SetEnabled(false)` か `Drop` でしか SecureEventInput を
    /// 無効化しないため、`field_b`（Password）にフォーカスしたまま本イベントを無視すると、
    /// アプリ全体が非アクティブになってもプロセス全体で SecureEventInput が有効なまま残り、
    /// 他アプリの IME をロックしたままになりうる。`ImeFocusCoordinator::blur_all` が
    /// `SetEnabled(false)` を回収するため、`apply_requests` 経由で sensitive_tracker が
    /// 正しく無効化される）。
    fn blur_all_fields(&mut self, window: &Window) {
        // Focused(true) 復帰時に同じフィールドへ戻せるよう退避する（struct doc
        // `focus_before_app_blur` 参照）。`WindowEvent::Focused(false)`
        // が連続して届く（多重 blur）場合、2 回目以降は `coordinator.focused()` が既に
        // `None`（1 回目の blur_all で外れている）ため、`Some` を保持したまま `None` で
        // 上書きしないよう `or` で既存の退避値を優先する（常時上書きすると、
        // 連続 blur で復元先が失われ `Focused(true)` 復帰後にキーボード入力が
        // 効かなくなる）。
        self.focus_before_app_blur = self.coordinator.focused().or(self.focus_before_app_blur);
        self.field_a.preedit_text.clear();
        self.field_a.preedit_cursor = None;
        self.field_b.preedit_text.clear();
        self.field_b.preedit_cursor = None;
        let mut fields = [&mut self.field_a.widget, &mut self.field_b.widget];
        let transition = self.coordinator.blur_all(&mut fields);
        drop(transition.events);
        self.apply_requests(window, transition.requests);
    }

    /// `WindowEvent::Focused(true)` でアプリへ復帰した際、`blur_all_fields` 直前に
    /// フォーカスしていたフィールドへ戻す（Cursor Bugbot 指摘対応。`focus_other_field`
    /// の doc 参照）。
    fn restore_focus_after_app_focus(&mut self, window: &Window) {
        let Some(target) = self.focus_before_app_blur.take() else {
            return;
        };
        let mut fields = [&mut self.field_a.widget, &mut self.field_b.widget];
        let transition = self.coordinator.focus(&mut fields, target);
        drop(transition.events);
        self.apply_requests(window, transition.requests);
        self.update_cursor_area(window);
    }

    /// `WindowEvent::Ime` を両フィールドへ配り、生成された `ImeRequest` を適用する。
    fn handle_ime_event(&mut self, window: &Window, ime: winit::event::Ime) {
        let event = ime_event_from_winit(ime);
        // 機微情報の露出防止（モジュール doc 参照）: Debug マスクへそのまま委譲する。
        // フィールド ID（"field_a"/"field_b"、機微情報を含まない）を付記する。付記しない
        // と、Tab 切替時にどちらのフィールドが本イベントを受け取ったか標準出力から判別
        // できない。ただしこの `field=` はイベント受信時点の `coordinator.focused()` を
        // 示すのみで、遅延到着したイベント（Issue #587 の既知限界）の実際の発生元を
        // 証明しない（codex-review P0 指摘対応。
        // そのためこの ID を根拠に内容一致の可否を判定する処理は実装しない）。
        let focused_field = match self.coordinator.focused() {
            Some(id) if id == self.field_a.widget.id() => "field_a",
            Some(id) if id == self.field_b.widget.id() => "field_b",
            _ => "none",
        };
        // dispatch_ime は `event` を消費するため、Debug 表示は dispatch 前にここで
        // 確定させる。
        let event_debug = format!("{event:?}");

        // tracker が観測済みの BeginSession/Enabled/Preedit/Disabled から、この event の
        // 帰属セッションを算出する（Issue #587。付け替え規則は `session` モジュール doc）。
        let session = self.tracker.stamp(&event);

        // Preedit の内容をアプリ層で一時保持する（候補ウィンドウ位置計算の入力、
        // モジュール doc「コンポジション矩形計算の制約」参照）。dispatch 前に保持する
        // ことで、dispatch 後の cursor_area 計算がこの値を使える。旧セッション由来の
        // Preedit（session が現在のセッションと不一致）は candidate window の位置計算
        // にも使わせない（Issue #587。dispatch_ime 側のセッション照合と揃える）。
        match &event {
            fandhe_core::ImeEvent::Preedit { text, cursor }
                if session == self.coordinator.session() =>
            {
                if let Some(focused) = self.coordinator.focused() {
                    if focused == self.field_a.widget.id() {
                        self.field_a.preedit_text = text.clone();
                        self.field_a.preedit_cursor = *cursor;
                    } else if focused == self.field_b.widget.id() {
                        self.field_b.preedit_text = text.clone();
                        self.field_b.preedit_cursor = *cursor;
                    }
                }
            }
            fandhe_core::ImeEvent::Commit(_) | fandhe_core::ImeEvent::Disabled => {
                self.field_a.preedit_text.clear();
                self.field_a.preedit_cursor = None;
                self.field_b.preedit_text.clear();
                self.field_b.preedit_cursor = None;
            }
            // 旧セッションの Preedit（session 不一致）・Enabled は候補ウィンドウ表示状態を
            // 変更しない（`dispatch_ime` 側で破棄されるため、アプリ層のブリッジ状態も
            // 追随させない）。
            fandhe_core::ImeEvent::Preedit { .. } | fandhe_core::ImeEvent::Enabled => {}
        }

        let mut fields = [&mut self.field_a.widget, &mut self.field_b.widget];
        // Event::TextFieldChanged（確定値を含みうる）はログへ出さず読み捨てる
        // （モジュール doc・`ime_focus.rs` モジュール doc の機微情報注意参照）。
        let _events = self.coordinator.dispatch_ime(&mut fields, session, event);

        // P1（Commit イベント受信の確認）は本ハーネスが文字描画を持たないため、
        // ログの `Commit` / `text_len`（確定文字数、`Debug` マスク由来）による
        // イベント受信確認に限定する。フィールドへの実際の反映（表示）は確認しない。
        println!("[ime] field={focused_field} {event_debug}");

        // 上記の `Commit`/`text_len` ログだけでは `dispatch_ime` 後に `TextFieldWidget` の
        // buffer 更新自体が失敗しても PASS 扱いになってしまう（PR #682）。field_a
        // （非 Password）に限り、確定・変換のたびに buffer の実際の長さを安全な
        // 読み取り専用照会（`buffer()` は文字数のみを扱い内容は出力しない、§3 の
        // 内容非記録方針に準拠）で記録する。field_b（Password）はこのログを一切
        // 出力しない。
        //
        // 出力条件を `coordinator.focused() == field_a` に限定する（PR #682）:
        // フォーカス判定を欠くと field_b 操作時・無フォーカス時にも field_a のログが出て
        // 「field_a に限り出力」の契約に反する。
        //
        // `preedit_len`（ハーネス側 `preedit_text` のコピー長）は持たない（PR #682）:
        // キャンセル経路では dispatch 前に必ずクリアされる（本関数冒頭
        // の match 参照）ため、ウィジェットが composition を保持していても常に 0 を示す
        // 同語反復になる。代わりにウィジェット自身の composition 状態
        // （`TextFieldWidget::is_composing`）を記録する。
        if self.coordinator.focused() == Some(self.field_a.widget.id()) {
            println!(
                "[ime] field_a buffer_len={} composing={}",
                self.field_a.widget.buffer().chars().count(),
                self.field_a.widget.is_composing()
            );
        }

        // apply_ime が積んだ ImeRequest を適用する。フォーカス中フィールドのみを drain する
        // （Cursor Bugbot 指摘対応、origin/main #681: 両フィールドを無条件に drain して結合
        // すると、未フォーカスの field_b が構築時に積んだままの SetPurpose(Password) が
        // 次の Ime イベント（field_a 側で発火）と同一バッチで sensitive_tracker へ渡り、
        // field_a の通常入力中に SecureEventInput が誤って有効化されうる。フォーカスなし
        // フィールドの pending_ime_requests は `ImeFocusCoordinator::focus` がそのフィールド
        // 自身の FocusGained/FocusLost 処理時に drain する契約のため、ここで触れなくてよい
        // ── `ime_focus.rs::blur_field`/`focus_field` 参照）。
        let requests: Vec<_> = match self.coordinator.focused() {
            Some(id) if id == self.field_a.widget.id() => self.field_a.widget.take_ime_requests(),
            Some(id) if id == self.field_b.widget.id() => self.field_b.widget.take_ime_requests(),
            _ => Vec::new(),
        };
        self.apply_requests(window, requests);

        self.update_cursor_area(window);
    }

    /// `WindowEvent::KeyboardInput` を、コンポジションを伴わない編集入力
    /// （ASCII 直接入力・Backspace）としてフォーカス中フィールドへ適用する。
    ///
    /// [`text_field_input_from_key`] の変換結果は `TextFieldWidget::handle_input` へ渡す。
    /// コンポジション中（`is_composing() == true`）は `handle_input` 側の既存ガード
    /// （`Commit`/`Cancel` 以外を無視する）により二重挿入は起きない
    /// （`text_field.rs::handle_input` 実装参照）。
    fn handle_key_event(&mut self, window: &Window, key_event: &KeyEvent) {
        if key_event.state == ElementState::Pressed
            && key_event.logical_key == Key::Named(NamedKey::Tab)
        {
            self.focus_other_field(window);
            return;
        }

        let Some(input) = text_field_input_from_key(
            key_event.state,
            &key_event.logical_key,
            key_event.text.as_ref().map(|t| t.as_str()),
        ) else {
            return;
        };

        let Some(focused) = self.coordinator.focused() else {
            return;
        };
        if focused == self.field_a.widget.id() {
            let _ = self.field_a.widget.handle_input(input);
        } else if focused == self.field_b.widget.id() {
            let _ = self.field_b.widget.handle_input(input);
        } else {
            return;
        }

        // ASCII 直接入力・Backspace で buffer/cursor が変化するため、候補ウィンドウ位置を
        // 再計算して通知する（Bugbot 指摘対応: handle_input 後に呼ばないと SetCursorArea が
        // 古い位置のまま残り、次の IME コンポジション開始時に候補ウィンドウがずれる）。
        self.update_cursor_area(window);
    }
}

impl ApplicationHandler for ImeTextFieldSample {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes().with_title("fandhe IME text field sample"))
            .expect("ウィンドウ生成に失敗した（サンプルのため unwrap を許容、coding-rust.md「例外的な初期化不変条件」）");

        // 起動直後は field_a へフォーカスする（§4.7 の FocusGained 経路）。
        let target = self.field_a.widget.id();
        let mut fields = [&mut self.field_a.widget, &mut self.field_b.widget];
        let transition = self.coordinator.focus(&mut fields, target);
        drop(transition.events);
        self.apply_requests(&window, transition.requests);
        self.update_cursor_area(&window);

        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.take() else {
            return;
        };

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Ime(ime) => self.handle_ime_event(&window, ime),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key_event(&window, &event),
            // Cmd-Tab 等でアプリ全体がフォーカスを失った際に全フィールドを blur する
            // （Cursor Bugbot 指摘対応、`blur_all_fields` の doc 参照）。
            WindowEvent::Focused(false) => self.blur_all_fields(&window),
            // アプリへ復帰した際、blur 前にフォーカスしていたフィールドへ戻す
            // （Cursor Bugbot 指摘対応、`restore_focus_after_app_focus` の doc 参照）。
            WindowEvent::Focused(true) => self.restore_focus_after_app_focus(&window),
            _ => {}
        }

        self.window = Some(window);
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("イベントループ生成に失敗した");
    let mut app = ImeTextFieldSample::default();
    event_loop
        .run_app(&mut app)
        .expect("イベントループ実行に失敗した");
}
