//! 複数 `TextFieldWidget` 間のフォーカス・IME 状態遷移の調停実装
//! （TASK-8.5-S24 / REQ-8、Issue #205）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）の sub-issue 24/30。単一フィールド内の
//! フォーカス・IME 配線は S8（#189、`text_field.rs` の `apply_input` / `apply_ime` /
//! `take_ime_requests`）が既に完了しているが、各フィールドは独立にキューを持つのみで、
//! **フィールド横断の調停者が存在しなかった**。呼び出し側（プラットフォーム層・アプリ層）が
//! 複数フィールドの `take_ime_requests()` を drain する順序を誤ると、
//! 「新フィールドの enable が旧フィールドの disable より先に出る」「2 フィールド同時に
//! IME が有効になる」状態を防げない。本モジュールはその調停をコア側に閉じて提供する。
//!
//! `docs/architecture/ime-architecture.md` §4.7 が定める実装契約:
//!
//! - 複数テキストフィールド間のフォーカス移動（A → B）では
//!   **A の `FocusLost` → A の `ImeRequest::SetEnabled(false)` → B の `FocusGained` →
//!   B の `ImeRequest::SetEnabled(true)`** の順序を保証する
//! - **同時に 2 つのフィールドが IME 有効になる状態を作らない**
//! - `set_disabled(true)` がフォーカスを奪う経路も同じ `FocusLost` 相当の
//!   IME 無効化契約に従う
//!
//! [`ImeFocusCoordinator`] はウィジェットの所有権を持たない（呼び出しごとに
//! `&mut [&mut TextFieldWidget<S>]` を受け取る）。フォーカスの実 SSOT は各ウィジェットの
//! `InteractionState::focused` のままであり、coordinator の内部記録はあくまで
//! 「次にどのフィールドを blur すべきか」を追跡する調停用の索引に過ぎない。
//!
//! **フォーカス遷移直後の遅延 `Commit` に対するガード**（PR #458 Bugbot 指摘
//! （High severity）対応）: `TextFieldWidget::apply_ime` 側のフォーカスガード
//! （`text-input-state-model.md` §4.3・§4.5 手順 0）は「フォーカスを失った
//! フィールド自身」しか保護しない。A → B のフォーカス移動後に A の IME セッション
//! 由来の遅延 `Commit` が届いた場合、B は `focused == true` であるためフィールド
//! 単体のガードはこれを破棄できず、B への誤った入力挿入になってしまう
//! （`dispatch_ime` は `WidgetId` のみでルーティングし、`ImeEvent` 自体はどの
//! IME セッション由来かを示すトークンを持たないため、フィールド側だけでは
//! 判別不能）。本モジュールはこれを [`ImeFocusCoordinator::armed`] という
//! **`Commit` 専用**のゲートで防ぐ: `fandhe-winit-ime` の既存契約
//! （「`Commit` の直前には必ず空文字列の `Preedit` が届く」、全プラットフォーム
//! ソース検証済み — `s9-macos-nstextinputclient-findings.md` §3 表 #5、
//! `s11-windows-imm32-tsf-findings.md` §3 表 #5、
//! `s13-linux-ibus-fcitx-findings.md` §3.1 表 #5・§3.2 表 #4）を利用し、
//! **現在のフォーカス先へ `Preedit` を転送済みの場合のみ**、直後に届く
//! `Commit` を転送する。ゲートは `focus()` によるフォーカス遷移（初回含む）と
//! `Disabled` 受信で「未 armed（閉）」に戻り、`Preedit` 転送のたびに「armed（開）」
//! になり、`Commit` を 1 件消費するたびに再び閉じる（次の `Commit` には新しい
//! `Preedit` を要求する 1 ショットゲート）。
//!
//! `Enabled` ではなく `Preedit` を起点に選んだのは、`Enabled` がプラットフォームに
//! よっては最初の `Preedit`/`Commit` より前に届くとは限らないため（macOS は
//! `Ime::Enabled` 自体が届かない場合すらある。
//! `crates/fandhe-winit-ime/tests/macos_japanese_input.rs` の
//! `enabled_event_may_arrive_late_or_be_skipped_before_first_preedit` 参照）。
//! `Enabled` を起点にすると macOS で正当な `Preedit`/`Commit` を丸ごと
//! 破棄しかねない。一方 `Preedit`（特に `Commit` 直前の空 `Preedit`）は
//! 全プラットフォームでソース検証済みの普遍的な前置契約であり、これを
//! 起点にすれば正当な入力を落とすリスクなく `Commit` のみを狙って保護できる。
//! `Preedit` 自体はフォーカス中フィールドへ無条件に転送する（`Commit` と異なり
//! 誤って転送されても一時的な変換中表示に留まり、`buffer` への確定挿入という
//! 不可逆な副作用を持たないため、多層防御の対象を絞った設計判断）。
//!
//! **セッション識別子照合 + `armed` ゲートの二段構え**（Issue #587）: 上記
//! `armed` ゲートは「旧セッションの空 `Preedit` → `Commit` がペアで遅延到着する」
//! ケースを防げない（旧セッションの `Preedit` 自体がゲートを開けてしまうため）。
//! [`ImeFocusCoordinator::dispatch_ime`] は `ImeSessionId`（`ime.rs`）による
//! 帰属照合を `armed` ゲートより前段に置き、`focus()` が払い出した現在の
//! セッションと一致しないイベント（`Enabled`/`Preedit`/`Commit` いずれも）を
//! 無条件に破棄する。`focus()` は新フィールドの `SetEnabled(true)` 等より前に
//! `ImeRequest::BeginSession` を発行し、プラットフォーム層（`fandhe-winit-ime`
//! 等）へ以降の `ImeEvent` の帰属先を伝える（付け替え規則・プラットフォーム別
//! 挙動は `ime-architecture.md` §4.8）。`armed` ゲートは撤去せず、識別子照合を
//! すり抜けうる残存経路（Wayland の `Enabled` 後 in-flight バッチ。winit
//! `zwp_text_input_v3` の `done` ハンドラが `ime_allowed`/`serial` を見ない
//! 実装上の限界、`fandhe-winit-ime` doc 参照）に対する多層防御として維持する
//! （残存限界は `ime-known-constraints.md` §3.6 に記録する）。
//!
//! 呼び出し元はプラットフォーム統合レイヤ（S7・S15・S18・S21 等、winit・AppKit・
//! Win32 IME・IBus/Fcitx 各実装）を想定する。本モジュール自体は特定プラットフォームへ
//! 依存せず、`fandhe-core` の `#![forbid(unsafe_code)]` を維持する。
//!
//! **機微情報の露出防止**（`.claude/rules/security.md`）: [`FocusTransition::events`] は
//! `Event::TextFieldChanged` を含みうる（確定文字列を運ぶ）。本モジュール・呼び出し側とも
//! これをログへそのまま出力しないこと。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠）: プラットフォーム層
//! への実配線（winit イベントループでの coordinator 使用）・網羅的な結合テスト
//! （S27 #208 / S28 #209 のスコープ）。

use crate::state::Event;

use super::ime::ImeEvent;
use super::ime::ImeRequest;
use super::ime::ImeSessionId;
use super::text_field::TextFieldWidget;
use super::widget::{InteractionInput, Widget, WidgetId};

/// 1 回のフォーカス遷移・IME ルーティングの結果。
///
/// 呼び出し側（プラットフォーム層）は `requests` をこの順序のまま処理する契約
/// （§4.7）。`events` は `StateStore::dispatch` へそのまま回す想定で、
/// `Composition` の内容と異なり確定値を含みうるためログへ出力しないこと
/// （モジュール doc 参照）。
#[derive(Debug, Default)]
pub struct FocusTransition {
    /// blur-commit 等で発生した `state::Event`（`StateStore::dispatch` へ回す）。
    pub events: Vec<Event>,
    /// §4.7 の順序を保証済みの `ImeRequest` 列（旧フィールドの `SetEnabled(false)` が
    /// 新フィールドの `SetEnabled(true)` より必ず前に来る）。
    pub requests: Vec<ImeRequest>,
}

impl FocusTransition {
    /// 空の遷移（no-op）を返す。
    fn empty() -> Self {
        Self::default()
    }
}

/// 複数 `TextFieldWidget` 間のフォーカス・IME 状態遷移の調停者（§4.7、S24）。
///
/// ウィジェットの所有はしない。呼び出しごとに対象フィールドのスライスを受け取り、
/// 各フィールドの既存公開 API（`apply_input` / `apply_ime` / `set_disabled` /
/// `take_ime_requests` / `Widget::id` / `Widget::interaction_state`）のみを介して
/// 操作する（`text_field.rs` / `ime.rs` 自体は変更しない設計判断）。
#[derive(Debug)]
pub struct ImeFocusCoordinator {
    /// 現在フォーカス中とみなすフィールドの `WidgetId`（コア側の単一フォーカスモデル）。
    /// 実 SSOT は各ウィジェットの `InteractionState::focused` であり、本フィールドは
    /// 調停用の記録に過ぎない（モジュール doc 参照）。
    focused: Option<WidgetId>,
    /// フォーカス遷移直後の遅延 `Commit` 対策ゲート（モジュール doc「フォーカス
    /// 遷移直後の遅延 `Commit` に対するガード」参照）。`true` の間のみ
    /// `dispatch_ime` は次の `Commit` を `focused` フィールドへ転送する
    /// （`Preedit` 自体は本ゲートの対象外で常に転送される）。`focus()` による
    /// フォーカス遷移（初回含む）・`Disabled` 受信で `false` に戻り、`Preedit` を
    /// `focused` フィールドへ転送するたびに `true` になり、`Commit` を 1 件
    /// 消費するたびに再び `false` に戻る（1 ショット）。`focused == None` の間は
    /// 本フィールドの値によらず `dispatch_ime` が早期 return するため、
    /// `blur_all()`/`set_field_disabled()` で `focused` が `None` になる経路では
    /// 本フィールドを明示的にリセットしない（次の `focus()` が必ず `false` へ
    /// 戻すため無害）。
    armed: bool,
    /// 現在フォーカス中フィールドに割り当てた IME セッション識別子（Issue #587）。
    /// `focus()` が新フィールドを有効化するたびに払い出し、`blur_all()` /
    /// `set_field_disabled()` でフォーカス記録をクリアする経路と併せてクリアする。
    /// `dispatch_ime` はこの値と一致しない `ImeEvent` を破棄する
    /// （モジュール doc「セッション識別子照合 + `armed` ゲートの二段構え」参照）。
    session: Option<ImeSessionId>,
    /// 次に払い出す IME セッション識別子（連番カウンタ）。
    next_session: ImeSessionId,
}

impl Default for ImeFocusCoordinator {
    /// `next_session` の初期値（`ImeSessionId::FIRST`）が `Default` を持たないため、
    /// `#[derive(Default)]` ではなく `new()` に委譲する手書き実装。
    fn default() -> Self {
        Self::new()
    }
}

impl ImeFocusCoordinator {
    /// フォーカスなしの状態で調停者を構築する。
    pub fn new() -> Self {
        Self {
            focused: None,
            armed: false,
            session: None,
            next_session: ImeSessionId::FIRST,
        }
    }

    /// 現在フォーカス中とみなすフィールドの `WidgetId`（調停者の記録）。
    pub fn focused(&self) -> Option<WidgetId> {
        self.focused
    }

    /// 現在フォーカス中フィールドに割り当てた IME セッション識別子（Issue #587）。
    /// プラットフォーム層の配線・テストでの観測用に公開する。
    pub fn session(&self) -> Option<ImeSessionId> {
        self.session
    }

    /// スライス内から `id` に一致するフィールドの位置を探す。
    fn position_of<S: 'static>(fields: &[&mut TextFieldWidget<S>], id: WidgetId) -> Option<usize> {
        fields.iter().position(|field| field.id() == id)
    }

    /// 単一フィールドへ `FocusLost` を適用し、blur-commit の `Event` と
    /// `SetEnabled(false)` を `transition` へ回収する。
    ///
    /// `apply_input` は `SetEnabled(false)` を当該フィールドの `pending_ime_requests`
    /// へ積むのみで返り値には含めないため、`take_ime_requests()` で明示的に
    /// drain して `transition.requests` へ連結する（§4.7 の順序契約を満たすため、
    /// この連結は各フィールドの `FocusGained` 処理より必ず先に行う）。
    fn blur_field<S: 'static>(field: &mut TextFieldWidget<S>, transition: &mut FocusTransition) {
        if let Some(event) = field.apply_input(InteractionInput::FocusLost) {
            transition.events.push(event);
        }
        transition.requests.extend(field.take_ime_requests());
    }

    /// 単一フィールドへ `FocusGained` を適用し、`SetEnabled(true)` を
    /// `transition` へ回収する。
    fn focus_field<S: 'static>(field: &mut TextFieldWidget<S>, transition: &mut FocusTransition) {
        // FocusGained は Event を返さない契約（`apply_input` 参照）。
        let _ = field.apply_input(InteractionInput::FocusGained);
        transition.requests.extend(field.take_ime_requests());
    }

    /// `target` へフォーカスを移す（§4.7 の中核 API）。
    ///
    /// 処理順:
    /// 1. `target` がスライス内に見つからない、または disabled → 何もせず空の遷移を
    ///    返す（現フォーカスを維持し、panic しない）。事前にここで弾くことで、
    ///    「旧フィールドを blur したのに新フィールドが受け付けず、どこにも
    ///    フォーカスがないまま IME だけ無効化される」中間状態を作らない。
    /// 2. `target` が既にフォーカス中 → no-op（冪等）。
    /// 3. 旧フィールド（`self.focused`）へ `FocusLost`（blur-commit の `Event` と
    ///    `SetEnabled(false)` を回収）。
    /// 4. 新フィールドへ `FocusGained`（`SetEnabled(true)` を回収）。
    ///
    /// → `requests` は必ず `[旧 disable, 新 enable]` の順になる（§4.7 順序契約）。
    pub fn focus<S: 'static>(
        &mut self,
        fields: &mut [&mut TextFieldWidget<S>],
        target: WidgetId,
    ) -> FocusTransition {
        let Some(target_index) = Self::position_of(fields, target) else {
            // 未知の WidgetId。現フォーカスを維持し、panic しない（coding-rust.md）。
            return FocusTransition::empty();
        };
        if fields[target_index].interaction_state().disabled {
            // disabled なターゲットへは移さない（旧フィールドを blur する前に検査する
            // ことが重要。先に blur してしまうと target が受け付けず、フォーカスが
            // どこにもないのに IME だけ無効化された中間状態が残る）。
            return FocusTransition::empty();
        }
        if self.focused == Some(target) {
            // 既にフォーカス中。二重に SetEnabled(true) を発行しない。
            return FocusTransition::empty();
        }

        let mut transition = FocusTransition::empty();

        if let Some(previous) = self.focused {
            let Some(previous_index) = Self::position_of(fields, previous) else {
                // 記録上フォーカス中のフィールドが今回のスライスに含まれない
                // （呼び出し側が部分スライスを渡した等）。ここで target を
                // enable してしまうと旧フィールドの SetEnabled(false) を
                // 発行できないまま §4.7「同時に 2 フィールド有効にしない」に
                // 反する。何もせず空の遷移を返し、記録も変更しない
                // （呼び出し側が正しいスライスで再試行できるようにする）。
                return FocusTransition::empty();
            };
            Self::blur_field(fields[previous_index], &mut transition);
        }

        // 新セッションを払い出し、新フィールドの SetPurpose/SetEnabled(true) より前に
        // BeginSession を積む（Issue #587。プラットフォーム層への付け替え規則は
        // `ime-architecture.md` §4.8）。
        let session = self.next_session;
        self.next_session = session.next();
        self.session = Some(session);
        transition.requests.push(ImeRequest::BeginSession(session));

        Self::focus_field(fields[target_index], &mut transition);
        self.focused = Some(target);
        // 新フィールドはまだ自身の Preedit を受け取っていないため、Commit ゲートを
        // 閉じる（モジュール doc「フォーカス遷移直後の遅延 Commit に対するガード」
        // 参照）。旧フィールド由来の遅延 `Commit` は次の `dispatch_ime` で破棄される。
        self.armed = false;

        transition
    }

    /// フォーカスを全解除する（背景クリック・ウィンドウフォーカス喪失時）。
    ///
    /// 現フォーカスフィールドへ `FocusLost` を送り、blur-commit と
    /// `SetEnabled(false)` を回収する。フォーカス中フィールドがなければ no-op。
    pub fn blur_all<S: 'static>(
        &mut self,
        fields: &mut [&mut TextFieldWidget<S>],
    ) -> FocusTransition {
        let mut transition = FocusTransition::empty();

        if let Some(previous) = self.focused {
            let Some(previous_index) = Self::position_of(fields, previous) else {
                // 記録上フォーカス中のフィールドが今回のスライスに含まれず blur
                // できなかった。ここで記録を無条件にクリアすると、実際には
                // まだ IME が有効なまま SetEnabled(false) を二度と発行できない
                // 恒久的な記録ずれに陥る（§4.7 違反）。記録を保持し、
                // 呼び出し側が正しいスライスで再試行できるようにする。
                return transition;
            };
            Self::blur_field(fields[previous_index], &mut transition);
            self.focused = None;
            self.session = None;
        }

        transition
    }

    /// フィールドを disabled にする経路の調停（§4.7 最終項）。
    ///
    /// `target` がフォーカス中なら `set_disabled(true)` の既存クリーンアップ
    /// （composition 破棄・`SetEnabled(false)` 要求・blur-commit、`text_field.rs`
    /// `set_disabled` の doc 参照）を回収し、coordinator の記録もクリアする。
    /// `disabled == false`（再有効化）の場合は `set_disabled` を素通しするのみで、
    /// フォーカスは自動では戻さない（再フォーカスは呼び出し側が明示的に
    /// `focus()` を呼ぶ設計。disabled 解除だけで暗黙にフォーカスが移ると
    /// 「同時に 2 フィールド有効」の契約に反するリスクがあるため）。
    pub fn set_field_disabled<S: 'static>(
        &mut self,
        fields: &mut [&mut TextFieldWidget<S>],
        target: WidgetId,
        disabled: bool,
    ) -> FocusTransition {
        let Some(target_index) = Self::position_of(fields, target) else {
            return FocusTransition::empty();
        };

        let mut transition = FocusTransition::empty();
        let field = &mut fields[target_index];
        let was_focused = field.interaction_state().focused;

        if let Some(event) = field.set_disabled(disabled) {
            transition.events.push(event);
        }
        transition.requests.extend(field.take_ime_requests());

        if disabled && was_focused && self.focused == Some(target) {
            self.focused = None;
            self.session = None;
        }

        transition
    }

    /// `ImeEvent` をルーティングする。
    ///
    /// `session` は呼び出し元（プラットフォーム層）が当該イベントの帰属セッションと
    /// して添える `ImeSessionId`（Issue #587、`ime.rs` `ImeSessionId` doc・
    /// `ime-architecture.md` §4.8 参照）。`Disabled` 以外は `self.session()` と
    /// 一致しない場合（`None` を含む）無条件に破棄する — これが本モジュールの
    /// 第一段の防御であり、フォーカス遷移直後に旧セッション由来の `Preedit` が
    /// 「`Commit` 直前の空 `Preedit`」契約を装って `armed` ゲートを開けてしまう
    /// 経路を、`Preedit` 自体を破棄することで塞ぐ（モジュール doc「セッション
    /// 識別子照合 + `armed` ゲートの二段構え」参照）。
    ///
    /// `Enabled` / `Preedit` / `Commit` はフォーカス中フィールドのみへ届ける
    /// （フォーカスなしなら破棄する。`apply_ime` 自身が持つフォーカスガードと
    /// 二重の防御になる）。`Disabled` は全フィールドへ配る（§4.6 の
    /// 「フォーカス状態によらず composition を必ずクリアする」冪等クリーンアップ
    /// 契約に一致する。フォーカスが記録とずれていても取りこぼさないための設計。
    /// セッション不問で必ず処理する — Wayland 等でセッション帰属が不確かな
    /// タイミングでも composition のクリーンアップだけは取りこぼさないため）。
    ///
    /// **`Commit` 専用ゲート**（モジュール doc「フォーカス遷移直後の遅延 `Commit`
    /// に対するガード」参照、セッション照合の第二段防御）: `Commit` は、直近で
    /// フォーカス中フィールドへ `Preedit` を転送済み（`armed == true`）の場合の
    /// みさらに転送する。`focus()` によるフォーカス遷移直後・`Disabled` 受信
    /// 直後はゲートが閉じており、旧フィールド由来の遅延 `Commit` を取りこぼす
    /// 前に破棄する。`Preedit` 自体はゲート対象外で（セッション照合を通過すれば）
    /// 常にフォーカス中フィールドへ転送し、転送するたびにゲートを開く副作用を
    /// 持つ。`Enabled` はゲートに影響しない（macOS では `Enabled` が最初の
    /// `Preedit`/`Commit` より前に届くとは限らない・届かないことすらあるため、
    /// `Enabled` を起点にすると正当な入力を丸ごと落としかねない。モジュール doc
    /// 参照）。
    pub fn dispatch_ime<S: 'static>(
        &mut self,
        fields: &mut [&mut TextFieldWidget<S>],
        session: Option<ImeSessionId>,
        event: ImeEvent,
    ) -> Vec<Event> {
        let mut events = Vec::new();

        if matches!(event, ImeEvent::Disabled) {
            // ゲートを閉じる: 次の Preedit まで Commit は転送しない
            // （`ime.rs` `ImeEvent::Disabled` doc「次の Enabled まで Preedit/Commit
            // は発生しない」契約に対応する再武装）。
            self.armed = false;
            for field in fields.iter_mut() {
                if let Some(dispatched) = field.apply_ime(event.clone()) {
                    events.push(dispatched);
                }
            }
            return events;
        }

        let Some(focused) = self.focused else {
            // フォーカスなし。Preedit/Commit/Enabled を届ける先がないため破棄する。
            return events;
        };
        let Some(focused_index) = Self::position_of(fields, focused) else {
            // 記録上はフォーカス中だが、スライスに対象が含まれない
            // （呼び出し側が部分スライスを渡した等）。破棄して panic しない。
            return events;
        };

        if session != self.session {
            // 旧セッション（または帰属不明 = None）由来のイベントは、フォーカス
            // 先が一致していても破棄する（Issue #587 の核心。この判定がなければ
            // 旧セッションの空 Preedit が armed ゲートを開けてしまう）。
            return events;
        }

        if let ImeEvent::Commit(_) = &event {
            if !self.armed {
                // ゲートが閉じている（フォーカス遷移直後・Disabled 直後、または
                // 既に 1 件消費済み）間に届いた Commit は旧セッション由来の遅延
                // イベントとみなして破棄する（Bugbot #458 指摘対応。フィールド
                // 単体のフォーカスガードでは `focused == true` の新フィールドを
                // 保護できないための coordinator 側の多層防御）。
                return events;
            }
            // 1 ショット: この Commit を消費したらゲートを再び閉じる
            // （次の Commit には新しい Preedit を要求する）。
            self.armed = false;
        } else if matches!(event, ImeEvent::Preedit { .. }) {
            // Preedit は無条件に転送し、転送のたびにゲートを開く（直後に届く
            // Commit の正当性の根拠となる「Commit 直前に必ず空 Preedit」契約、
            // モジュール doc 参照）。
            self.armed = true;
        }

        if let Some(dispatched) = fields[focused_index].apply_ime(event) {
            events.push(dispatched);
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text_field::TextFieldInput;
    use crate::ui::widget::{InteractionInput, Widget};

    /// テスト専用の状態型。フィールド間調停の検証には状態の中身自体は関与しない。
    struct FormState;

    fn field(raw: u64, label: &str, initial: &str) -> TextFieldWidget<FormState> {
        TextFieldWidget::new_static(WidgetId::from_raw(raw), label, initial)
    }

    #[test]
    fn focus_moves_between_two_fields_in_contract_order() {
        let mut a = field(1, "a", "hello");
        let mut b = field(2, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        // A へ直接フォーカスを与え（初回遷移の前提を作る）、続けてダーティな編集を行う。
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        a.handle_input(TextFieldInput::InsertText("!".to_string()));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        coordinator.focused = Some(WidgetId::from_raw(1));

        let transition = coordinator.focus(&mut fields, WidgetId::from_raw(2));

        // §4.7 順序契約: 旧フィールドの SetEnabled(false) が新フィールドの
        // SetEnabled(true) より必ず前に来ること。新フィールドの FocusGained は
        // `SetPurpose(現在の purpose)` → `SetEnabled(true)` の順で要求する
        // （S23 / #204、`text_field.rs::apply_input` の doc 参照。マージで
        // 取り込まれた挙動）。
        assert_eq!(
            transition.requests,
            vec![
                ImeRequest::SetEnabled(false),
                ImeRequest::BeginSession(ImeSessionId::FIRST),
                ImeRequest::SetPurpose(crate::ui::ImePurpose::Normal),
                ImeRequest::SetEnabled(true)
            ]
        );
        // A の blur-commit イベントが events に入っていること。
        assert_eq!(
            transition.events,
            vec![Event::TextFieldChanged {
                label: "a".to_string(),
                value: "hello!".to_string(),
            }]
        );
        assert!(!a.interaction_state().focused);
        assert!(b.interaction_state().focused);
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(2)));
    }

    #[test]
    fn focus_on_already_focused_field_is_noop() {
        let mut a = field(3, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        coordinator.focused = Some(WidgetId::from_raw(3));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let transition = coordinator.focus(&mut fields, WidgetId::from_raw(3));

        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
    }

    #[test]
    fn focus_on_unknown_widget_id_is_noop_and_keeps_current_focus() {
        let mut a = field(4, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        coordinator.focused = Some(WidgetId::from_raw(4));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let transition = coordinator.focus(&mut fields, WidgetId::from_raw(999));

        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(4)));
        assert!(a.interaction_state().focused);
    }

    #[test]
    fn focus_on_disabled_field_is_noop_and_does_not_blur_current_field() {
        let mut a = field(5, "a", "");
        let mut b = field(6, "b", "");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        b.set_disabled(true);
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(5));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        let transition = coordinator.focus(&mut fields, WidgetId::from_raw(6));

        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
        // A は blur されていない（disabled ターゲットへの遷移は事前検査で弾く）。
        assert!(a.interaction_state().focused);
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(5)));
    }

    #[test]
    fn blur_all_sends_focus_lost_to_current_field() {
        let mut a = field(7, "a", "hi");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        a.handle_input(TextFieldInput::InsertText("!".to_string()));
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(7));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let transition = coordinator.blur_all(&mut fields);

        assert_eq!(transition.requests, vec![ImeRequest::SetEnabled(false)]);
        assert_eq!(
            transition.events,
            vec![Event::TextFieldChanged {
                label: "a".to_string(),
                value: "hi!".to_string(),
            }]
        );
        assert!(!a.interaction_state().focused);
        assert_eq!(coordinator.focused(), None);
    }

    #[test]
    fn blur_all_without_focus_is_noop() {
        let mut a = field(8, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let transition = coordinator.blur_all(&mut fields);

        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
    }

    #[test]
    fn set_field_disabled_on_focused_field_clears_coordinator_record() {
        let mut a = field(9, "a", "hi");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        a.handle_input(TextFieldInput::InsertText("!".to_string()));
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(9));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let transition = coordinator.set_field_disabled(&mut fields, WidgetId::from_raw(9), true);

        assert_eq!(transition.requests, vec![ImeRequest::SetEnabled(false)]);
        assert_eq!(
            transition.events,
            vec![Event::TextFieldChanged {
                label: "a".to_string(),
                value: "hi!".to_string(),
            }]
        );
        assert!(a.interaction_state().disabled);
        assert_eq!(coordinator.focused(), None);
    }

    #[test]
    fn set_field_disabled_on_unfocused_field_does_not_affect_coordinator_record() {
        let mut a = field(10, "a", "");
        let mut b = field(11, "b", "");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(10));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        let transition = coordinator.set_field_disabled(&mut fields, WidgetId::from_raw(11), true);

        assert!(transition.events.is_empty());
        assert!(b.interaction_state().disabled);
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(10)));
    }

    #[test]
    fn dispatch_ime_routes_preedit_only_to_focused_field() {
        let mut a = field(12, "a", "");
        let mut b = field(13, "b", "");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(12));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        let session = coordinator.session();
        let events = coordinator.dispatch_ime(
            &mut fields,
            session,
            ImeEvent::Preedit {
                text: "あ".to_string(),
                cursor: None,
            },
        );

        assert!(events.is_empty());
        // A のみ preedit を反映していること（private な composition を直接
        // 観測できないため `is_composing()` で間接的に確認する）。`Preedit` は
        // ゲート対象外で常に転送される（モジュール doc 参照）。
        assert!(a.is_composing());
        assert_eq!(b.buffer(), "");
        assert!(!b.is_composing());
    }

    #[test]
    fn dispatch_ime_discards_stale_commit_after_focus_transition_before_preedit() {
        // Bugbot #458 指摘（High severity）の再現ケース: A → B へフォーカスが
        // 移った直後、B が自身の `Preedit` を受け取るより先に A の IME セッション
        // 由来の遅延 `Commit` が届いた場合、B へ誤って適用されないことを検証する。
        let mut a = field(24, "a", "");
        let mut b = field(25, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        coordinator.focus(&mut fields, WidgetId::from_raw(24));
        let session_a = coordinator.session();
        coordinator.dispatch_ime(
            &mut fields,
            session_a,
            ImeEvent::Preedit {
                text: "あ".to_string(),
                cursor: None,
            },
        );
        coordinator.focus(&mut fields, WidgetId::from_raw(25));

        // B はまだ自身の Preedit を受け取っていない（ゲートは focus() 遷移で
        // 再び閉じる）。A のセッション識別子を付けて送るため、まずセッション
        // 照合（第一段防御）で破棄される。
        let events = coordinator.dispatch_ime(
            &mut fields,
            session_a,
            ImeEvent::Commit("delayed".to_string()),
        );

        assert!(events.is_empty());
        assert_eq!(b.buffer(), "");
    }

    #[test]
    fn dispatch_ime_applies_commit_to_new_field_after_preedit_observed() {
        // ゲートが正当な入力まで飲み込まないことの回帰確認: focus() 遷移後、
        // 新フィールド自身の `Preedit` を受け取った後に届いた `Commit` は
        // 正しく適用される（`Commit` 直前に必ず空 `Preedit` が届く契約に従う
        // 実際の入力シーケンスを模す）。
        let mut a = field(26, "a", "");
        let mut b = field(27, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        coordinator.focus(&mut fields, WidgetId::from_raw(26));
        coordinator.focus(&mut fields, WidgetId::from_raw(27));
        let session = coordinator.session();
        coordinator.dispatch_ime(
            &mut fields,
            session,
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            },
        );
        let events =
            coordinator.dispatch_ime(&mut fields, session, ImeEvent::Commit("hi".to_string()));

        // `Commit` 自体は `Event` を返さない契約（buffer への反映のみ、
        // `TextFieldWidget::apply_ime` の `Commit` 分岐 doc 参照。`Event` は
        // blur-commit 等の別経路で発火する）。ここでは buffer への反映で
        // ゲートが正当な入力を通したことを検証する。
        assert!(events.is_empty());
        assert_eq!(b.buffer(), "hi");
    }

    #[test]
    fn dispatch_ime_disabled_rearms_gate_and_discards_subsequent_commit() {
        // Windows の高頻度 Disabled→Enabled 再サイクル契約（`ime.rs` doc）に対応:
        // Preedit でゲートが開いた後も、Disabled を受けると再び閉じ、次に自身の
        // 新しい Preedit が届くまで Commit を破棄する（Preedit 起点の 1 ショット
        // ゲート、モジュール doc 参照）。
        let mut a = field(28, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        coordinator.focus(&mut fields, WidgetId::from_raw(28));
        let session = coordinator.session();
        coordinator.dispatch_ime(
            &mut fields,
            session,
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            },
        );
        let applied =
            coordinator.dispatch_ime(&mut fields, session, ImeEvent::Commit("ok".to_string()));
        // `Commit` は `Event` を返さない契約（buffer 反映のみ）。
        assert!(applied.is_empty());
        assert_eq!(fields[0].buffer(), "ok");

        coordinator.dispatch_ime(&mut fields, session, ImeEvent::Disabled);
        let discarded =
            coordinator.dispatch_ime(&mut fields, session, ImeEvent::Commit("stray".to_string()));
        assert!(discarded.is_empty());
        assert_eq!(a.buffer(), "ok");
    }

    #[test]
    fn dispatch_ime_discards_preedit_when_no_field_focused() {
        let mut a = field(14, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        let events = coordinator.dispatch_ime(
            &mut fields,
            None,
            ImeEvent::Preedit {
                text: "あ".to_string(),
                cursor: None,
            },
        );

        assert!(events.is_empty());
        assert_eq!(a.buffer(), "");
    }

    #[test]
    fn dispatch_ime_disabled_clears_composition_on_all_fields_regardless_of_focus() {
        let mut a = field(15, "a", "");
        let mut b = field(16, "b", "");
        a.apply_input(InteractionInput::FocusGained);
        a.take_ime_requests();
        a.apply_ime(ImeEvent::Preedit {
            text: "あ".to_string(),
            cursor: None,
        });
        let mut coordinator = ImeFocusCoordinator::new();
        coordinator.focused = Some(WidgetId::from_raw(15));

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        let events = coordinator.dispatch_ime(&mut fields, None, ImeEvent::Disabled);

        assert!(events.is_empty());
        // Disabled は composition を破棄するのみで、buffer には影響しない
        // （§4.6）。破棄されたことは measure の幅変化で間接的に確認できるが、
        // ここでは buffer が変わっていないことのみ検証する（コンポジション自体は
        // private フィールドのため直接観測できない設計判断を尊重する）。
        assert_eq!(a.buffer(), "");
        assert_eq!(b.buffer(), "");
    }

    #[test]
    fn three_field_sequential_focus_never_has_two_enabled_at_once() {
        let mut a = field(17, "a", "");
        let mut b = field(18, "b", "");
        let mut c = field(19, "c", "");
        let mut coordinator = ImeFocusCoordinator::new();

        // 初回フォーカスは旧フィールドが存在しないため SetPurpose → SetEnabled(true)
        // のみ（S23 / #204、`text_field.rs::apply_input` の doc 参照）。
        {
            let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b, &mut c];
            let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(17));
            assert_eq!(
                t1.requests,
                vec![
                    ImeRequest::BeginSession(ImeSessionId::FIRST),
                    ImeRequest::SetPurpose(crate::ui::ImePurpose::Normal),
                    ImeRequest::SetEnabled(true)
                ]
            );
        }

        // A → B: [disable(A), BeginSession(B), SetPurpose(B), enable(B)] の順。
        let session_ab = ImeSessionId::FIRST.next();
        let requests_ab = {
            let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b, &mut c];
            coordinator
                .focus(&mut fields, WidgetId::from_raw(18))
                .requests
        };
        assert_eq!(
            requests_ab,
            vec![
                ImeRequest::SetEnabled(false),
                ImeRequest::BeginSession(session_ab),
                ImeRequest::SetPurpose(crate::ui::ImePurpose::Normal),
                ImeRequest::SetEnabled(true)
            ]
        );

        // B → C: [disable(B), BeginSession(C), SetPurpose(C), enable(C)] の順。
        let session_bc = session_ab.next();
        let requests_bc = {
            let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b, &mut c];
            coordinator
                .focus(&mut fields, WidgetId::from_raw(19))
                .requests
        };
        assert_eq!(
            requests_bc,
            vec![
                ImeRequest::SetEnabled(false),
                ImeRequest::BeginSession(session_bc),
                ImeRequest::SetPurpose(crate::ui::ImePurpose::Normal),
                ImeRequest::SetEnabled(true)
            ]
        );

        // 全遷移を通じて、enable の直前に必ず直前フィールドの disable が
        // 現れている（同時に 2 フィールド enable にならないことの検証）。
        for requests in [requests_ab, requests_bc] {
            assert_eq!(requests.len(), 4);
            assert_eq!(requests[0], ImeRequest::SetEnabled(false));
            assert_eq!(requests[3], ImeRequest::SetEnabled(true));
        }

        assert!(!a.interaction_state().focused);
        assert!(!b.interaction_state().focused);
        assert!(c.interaction_state().focused);
    }

    #[test]
    fn focus_with_previous_field_missing_from_slice_does_not_enable_target() {
        // 記録上のフォーカス先（a）を、実際の apply_input 経由でフォーカス済みに
        // した上でスライスから除外し、「呼び出し側が部分スライスを渡した」
        // ケースを再現する（記録と実ウィジェット状態の乖離を private フィールド
        // への直接代入に頼らず、focus() 経由で駆動する）。
        let mut a = field(20, "a", "");
        let mut b = field(21, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        {
            let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
            coordinator.focus(&mut fields, WidgetId::from_raw(20));
        }
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(20)));

        // a を含まない部分スライスで b へ focus しようとする。
        let mut partial: Vec<&mut TextFieldWidget<FormState>> = vec![&mut b];
        let transition = coordinator.focus(&mut partial, WidgetId::from_raw(21));

        // a の SetEnabled(false) を発行できないため、b の SetEnabled(true) も
        // 発行されない（§4.7: 同時に 2 フィールド有効にしない）。
        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
        assert!(!b.interaction_state().focused);
        // 記録は変更しない（呼び出し側が正しいスライスで再試行できるように）。
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(20)));
        assert!(a.interaction_state().focused);
    }

    #[test]
    fn blur_all_with_focused_field_missing_from_slice_keeps_record() {
        // a を focus() 経由でフォーカスさせた上でスライスから除外し、
        // blur_all がスライスに対象を見つけられないケースを再現する。
        let mut a = field(22, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();

        {
            let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
            coordinator.focus(&mut fields, WidgetId::from_raw(22));
        }
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(22)));

        let mut b = field(23, "b", "");
        let mut empty_of_a: Vec<&mut TextFieldWidget<FormState>> = vec![&mut b];
        let transition = coordinator.blur_all(&mut empty_of_a);

        assert!(transition.requests.is_empty());
        assert!(transition.events.is_empty());
        // 記録を無条件にクリアしない（恒久的な記録ずれを防ぐ）。
        assert_eq!(coordinator.focused(), Some(WidgetId::from_raw(22)));
        assert!(a.interaction_state().focused);
    }

    #[test]
    fn focus_issues_distinct_session_ids_and_emits_begin_session_before_enable() {
        // Issue #587 受入基準の前提: focus() が払い出す ImeSessionId はフォーカス
        // 遷移ごとに異なり、BeginSession は SetEnabled(false)（旧フィールドが
        // ある場合）の直後・SetPurpose/SetEnabled(true) より前に積まれる。
        let mut a = field(36, "a", "");
        let mut b = field(37, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        let t1 = coordinator.focus(&mut fields, WidgetId::from_raw(36));
        let session_a = coordinator.session().expect("focus 直後は session が Some");
        assert_eq!(t1.requests[0], ImeRequest::BeginSession(session_a));

        let t2 = coordinator.focus(&mut fields, WidgetId::from_raw(37));
        let session_b = coordinator.session().expect("focus 直後は session が Some");
        assert_ne!(session_a, session_b);
        assert_eq!(t2.requests[0], ImeRequest::SetEnabled(false));
        assert_eq!(t2.requests[1], ImeRequest::BeginSession(session_b));
    }

    #[test]
    fn dispatch_ime_discards_events_without_session() {
        // フォーカス中でも session が現在のセッションと一致しない（ここでは
        // None）イベントは破棄する（Issue #587。プラットフォーム層がセッション
        // 帰属を特定できなかったイベントを誤って適用しないための契約）。
        let mut a = field(38, "a", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a];
        coordinator.focus(&mut fields, WidgetId::from_raw(38));

        let events = coordinator.dispatch_ime(
            &mut fields,
            None,
            ImeEvent::Preedit {
                text: "x".to_string(),
                cursor: None,
            },
        );

        assert!(events.is_empty());
        assert!(!a.is_composing());
    }

    #[test]
    fn disabled_is_broadcast_regardless_of_session() {
        // Disabled は §4.6 の冪等クリーンアップ契約によりセッション不問で
        // 全フィールドへ配る（Issue #587 でもこの契約は変更しない）。
        let mut a = field(39, "a", "");
        let mut b = field(40, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        coordinator.focus(&mut fields, WidgetId::from_raw(39));
        fields[0].apply_ime(ImeEvent::Preedit {
            text: "あ".to_string(),
            cursor: None,
        });

        let events = coordinator.dispatch_ime(&mut fields, None, ImeEvent::Disabled);

        assert!(events.is_empty());
        assert_eq!(a.buffer(), "");
    }

    #[test]
    fn dispatch_ime_discards_batched_preedit_and_commit_from_previous_session() {
        // Issue #587 受入基準 1: 旧セッションの「空 Preedit → Commit」がフォーカス
        // 遷移直後にペアで遅延到着しても、旧来の armed ゲート単独では防げなかった
        // （旧 Preedit がゲートを開けてしまう）。セッション識別子照合が Preedit
        // 自体を破棄することで防ぐ（`ime-architecture.md` §4.8）。
        let mut a = field(41, "a", "");
        let mut b = field(42, "b", "");
        let mut coordinator = ImeFocusCoordinator::new();

        let mut fields: Vec<&mut TextFieldWidget<FormState>> = vec![&mut a, &mut b];
        coordinator.focus(&mut fields, WidgetId::from_raw(41));
        let session_a = coordinator.session();
        coordinator.focus(&mut fields, WidgetId::from_raw(42));

        // A のセッション由来の空 Preedit → Commit がバッチで遅延到着した想定。
        coordinator.dispatch_ime(
            &mut fields,
            session_a,
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            },
        );
        let stale = coordinator.dispatch_ime(
            &mut fields,
            session_a,
            ImeEvent::Commit("stale".to_string()),
        );
        assert!(stale.is_empty());
        assert_eq!(fields[1].buffer(), "");

        // B 自身の正当な Preedit → Commit は届く。
        let session_b = coordinator.session();
        coordinator.dispatch_ime(
            &mut fields,
            session_b,
            ImeEvent::Preedit {
                text: String::new(),
                cursor: None,
            },
        );
        coordinator.dispatch_ime(&mut fields, session_b, ImeEvent::Commit("ok".to_string()));
        assert_eq!(fields[1].buffer(), "ok");
    }
}
