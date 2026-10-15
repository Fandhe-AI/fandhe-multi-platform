//! winit 経由で届く [`ImeEvent`] へ帰属セッション（[`ImeSessionId`]）を付ける状態機械
//! （Issue #587、`ime-architecture.md` §4.8「IME セッション識別子」）。
//!
//! `fandhe-core` の `ImeFocusCoordinator::focus` はフォーカス遷移のたびに新しい
//! `ImeSessionId` を払い出し、`ImeRequest::BeginSession` で本クレートへ通知する。
//! winit は識別子付きのイベントを持たないため、[`ImeSessionTracker`] が
//! `apply_ime_request`（要求の観測）と `ime_event_from_winit` の結果（イベントの観測）を
//! 橋渡しし、`ImeFocusCoordinator::dispatch_ime` へ渡すべき帰属セッションを算出する。
//!
//! **付け替え規則**（`ime-architecture.md` §4.8。winit 0.30.13 の各バックエンド挙動に
//! 基づく設計判断、`ime_event_from_winit` doc のプラットフォーム別挙動と対応）:
//! `BeginSession` を受けても即座には切り替えず、旧セッションの終了信号（`Disabled`）または
//! 新セッションの開始信号（`Enabled`。旧セッションが blur 未要求の間に限り非空
//! `Preedit` も開始信号とする、下記「Enabled ゲート」参照）を観測した時点で切り替える。
//! それまでに届く空 `Preedit`/`Commit` は旧セッションへ帰属させる。
//!
//! 1 回の遷移（旧セッション終了 → 新セッション開始）は `Disabled` と
//! `Enabled`/非空 `Preedit` の 2 信号が両方届き得る（Linux の同期発火ペア等）。
//! この 2 信号を独立した昇格契機として扱うと、`Disabled` で 1 つ昇格した直後に
//! 届く「同じ遷移の」`Enabled` でさらに 1 つ昇格してしまい、A→B→C 連続遷移で
//! B を飛び越えて C へ二重昇格する（Issue #680 レビュー指摘、PRRT_kwDOTl73E86deWWb）。
//! `awaiting_confirmation` で「直前に `Disabled` 経由で昇格した現在値がまだ
//! 自分自身の開始信号を受け取っていない」状態を保持し、その状態で届く
//! `Enabled`/非空 `Preedit` は昇格済みの現在値を確認するだけ（pending を消費しない）
//! に留める。
//! - macOS: `Commit` は必ず非空 `Preedit` が先行するため、`Disabled` を待たず非空
//!   `Preedit` でも昇格する（`Disabled` 自体が届かない入力ソース切替もあるため）。
//! - Windows: `Enabled`（`WM_IME_STARTCOMPOSITION`）が正当な `Commit` に必ず先行する。
//!   `Disabled` 到達前に旧セッションの「空 Preedit → Commit」が届く形は、
//!   IMM32 単独駆動（同期配送）の範囲ではこの規則で正しく旧セッションへ
//!   帰属する。**TSF 経由の非同期配送では区別できない残存制約が残る**（Issue #4、
//!   下記「Enabled ゲート」節の Windows 項参照）。
//! - Linux（X11・Wayland）: `Disabled`/`Enabled` が同期発火するため、`Disabled` の
//!   直後に昇格させれば十分。Wayland の `zwp_text_input_v3` `done` バッチが `Enabled`
//!   後に旧セッションの in-flight イベントを送る場合は本状態機械でも判別できない
//!   （winit 側が `ime_allowed`/`serial` を見ない実装上の限界。残存事項として
//!   `ime-known-constraints.md` §3.6・追跡 Issue #4 に記録する）。
//! - A→B→C と素早く連続遷移し、B が完全にアイドル（`Disabled`/`Enabled` のいずれも
//!   一度も観測されない）のまま C の最初の開始信号（`Enabled` または非空 `Preedit`）が
//!   届く場合、B はネイティブ状態を一度も持たなかったとみなし pending をまとめて
//!   読み捨てて C へ直接昇格する（`promote_or_confirm` 参照、Issue #680 レビュー指摘
//!   PRRT_kwDOTl73E86deasS）。
//! - `Disabled` で B へ昇格した「後」に `BeginSession(C)` が届いてから開始信号が来る
//!   場合（B は一切ネイティブイベントを持たないまま C に追い越された）も同様に C へ
//!   直接昇格する。`awaiting_confirmation` を立てた瞬間の `pending.len()`
//!   （`pending_len_at_promotion`）と比較し、その後 `pending` が増えていれば
//!   「同じ遷移の確認」ではなく「追い越し」と判別する（Issue #680 レビュー指摘 P1:
//!   PRRT_kwDOTl73E86defcd）。
//! - `current` が既に非空 `Preedit` を観測してコンポジション中（`current_composing`）
//!   の間に届く、さらなる非空 `Preedit` は新セッションの開始信号として扱わず、
//!   `current` 自身の継続更新とみなして `pending` を消費しない。フォーカス遷移直後に
//!   旧フィールドの「進行中の」コンポジション更新が遅延到着しても、それを新フィールド
//!   （`pending` 先頭）の開始信号として誤って stamp しない（Issue #680 レビュー指摘 P0:
//!   PRRT_kwDOTl73E86defcb）。`current_composing` はコンポジション終了を示す信号
//!   （空 `Preedit`、`Commit`、pending の有無を問わない `Disabled`）で必ず解除する。
//!   解除し損ねると、`Disabled` を経由しない次セッションの開始信号（macOS の非空
//!   `Preedit` 等）が「旧セッションの継続更新」と誤判定され、以降の非空 `Preedit` が
//!   すべて旧セッションへ stamp され続ける（Issue #680 レビュー指摘 P1:
//!   PRRT_kwDOTl73E86dejHr）。
//! - `current` が既にコア側から blur（`SetEnabled(false)`）を要求されている
//!   （`current_blur_requested`）間は、`Enabled` を観測するまで新セッションへ
//!   昇格しない。**非空 `Preedit` は昇格契機にせず**、`current` 自身へ帰属させ続ける
//!   （「Enabled ゲート」、Issue #680 レビュー指摘 P0:
//!   PRRT_kwDOTl73E86degYY の再設計）。旧設計では blur 要求後の非空 `Preedit` を
//!   即座に新セッションの開始信号として昇格させていたが、`SetEnabled(false)` 発行と
//!   native 側の実際の無効化完了の間に native がキューイング済みの旧フィールドの
//!   イベント（Preedit/Commit）が挟まるプラットフォーム（Windows の
//!   `WM_IME_STARTCOMPOSITION` を伴わない残留イベント等）では、その旧フィールドの
//!   非空 `Preedit` が新セッションの開始信号と誤認され、後続の旧フィールドの
//!   確定文字列が新セッションへ stamp されてしまっていた。
//!   - macOS: `set_ime_allowed(false)` は同期的に marked text を破棄し
//!     `ime_state` を `Disabled` にする（winit 0.30.13
//!     `src/platform_impl/macos/view.rs` の `set_ime_allowed`）。以降の IME
//!     イベントは `keyDown` → `interpretKeyEvents` の同期呼び出しでしか生成
//!     されないため、blur 後に旧フィールドの遅延イベントが届くことは構造的に
//!     ない。次の `setMarkedText:` は `ime_state == Disabled` を経由するため
//!     必ず `Enabled` を先に発行し（同ファイルの `setMarkedText:`）、本ゲートは
//!     正当な系列を素通しする。
//!
//!     **旧安全網の撤去**: 再設計前は「blur 要求済み・native の終了/開始信号が
//!     一切届かない・B 自身の非空 `Preedit` のみが届く」場合に備え、blur
//!     要求済みでも非空 `Preedit` で直接昇格する安全網を持っていた
//!     （Issue #680 レビュー指摘 P1: PRRT_kwDOTl73E86den0I）。上記の
//!     とおり macOS では `set_ime_allowed(false)` 後に旧フィールドの `Enabled`
//!     なしの非空 `Preedit` が届くことは構造的に起こり得ないため、この安全網は
//!     不要と判断し撤去した。`s9-macos-nstextinputclient-findings.md` 表 #4
//!     の「入力ソース切替でフォーカス継続中に `Disabled`→`Enabled` の再サイクル
//!     が起きうる（次の composition 開始＝`setMarkedText` まで `Enabled` は
//!     来ない）」は **同一フィールド内で blur を伴わない**入力ソース切替の所見
//!     であり、`current_blur_requested == false` の場合の非空 `Preedit` 昇格
//!     （下記参照）で引き続き扱う。**blur 後のフィールド間フォーカス遷移には
//!     当てはまらない**（blur 済みフィールドで `setMarkedText:` 自体が発火する
//!     契機がなくなるため）。
//!   - Windows（残存制約、Issue #4）: `set_ime_allowed` は `ImmAssociateContextEx`
//!     の切替のみで `ime_state` をリセットしない。キューに残った旧フィールドの
//!     `WM_IME_COMPOSITION`/`WM_IME_ENDCOMPOSITION` は `WM_IME_STARTCOMPOSITION`
//!     （`Enabled`）を伴わずに届きうる。IMM32 単独駆動の範囲
//!     では新フィールドの実際の変換が必ず `WM_IME_STARTCOMPOSITION` から始まる
//!     ため `Enabled` を待つことで区別できる。**ただし TSF（Text Services
//!     Framework）経由の通知タイミングを保証する一次情報がなく、旧フィールドの
//!     `ENDCOMPOSITION → STARTCOMPOSITION → COMPOSITION` という `Disabled→Enabled`
//!     再サイクルが `BeginSession` の後に届く可能性を否定できない**。届いた場合、
//!     その `Enabled` が新フィールド自身の確認として通過し、後続の旧フィールドの
//!     `Preedit`/`Commit` が新フィールドへ誤って stamp される。Enabled ゲート単体
//!     では「旧フィールドの再サイクル」と「新フィールドへの遷移」を区別できない
//!     構造的な限界であり、是正は winit 側（`set_ime_allowed(false)` での
//!     `ime_state` リセット）が必要（追跡 Issue #4、
//!     `ime-known-constraints.md` §3.6 参照）。winit 側のパッチは
//!     `patches/winit-0.30.13/`（本クレートの依存解決には未配線）。
//!   - Linux（X11）: フォーカス遷移で XIC が破棄・再生成され、新 XIC 生成時に
//!     `Enabled` が送出されるため概ね安全。
//!   - Linux（Wayland、残存制約）: winit は `set_ime_allowed` 呼び出し時に
//!     `Enabled`/`Disabled` を同期合成するのみで、`zwp_text_input_v3` の
//!     `done.serial` を検証しない。旧フィールドの遅延イベントバッチが
//!     `Enabled`（新フィールド分）の「後」に届く場合、winit のイベント列だけでは
//!     区別できない。根本解決は winit 側の serial 検証であり、本状態機械では
//!     解決できない（残存事項として `ime-known-constraints.md` §3.6 に記録する）。
//!   - blur 要求が無い場合（`current_blur_requested == false`）の非空 `Preedit`
//!     昇格（macOS の `Disabled` を経由しない入力ソース切替対策）は従来どおり維持
//!     する。ただし `ImeFocusCoordinator::focus` によるフィールド間フォーカス遷移は
//!     必ず旧フィールドの `SetEnabled(false)` を先に発行する（§4.7 順序契約）ため、
//!     この分岐が実際に効くのは「フォーカス遷移を伴わない、同一フィールド内の
//!     入力ソース切替」に限られる。フィールド間の遷移では常に上記の Enabled ゲート
//!     が適用される。
//!
//! 呼び出し元はプラットフォーム統合レイヤ（winit イベントループの `ApplicationHandler`
//! 実装、`examples/ime_text_field_sample.rs` 参照）を想定する。本モジュール自体は
//! `fandhe-core` の `#![forbid(unsafe_code)]` を維持する（本クレート全体の属性を継承）。

use std::collections::VecDeque;

use fandhe_core::ui::{ImeEvent, ImeRequest, ImeSessionId};

/// `pending` キューの 1 エントリ。まだ `current` になっていないセッションの blur
/// 履歴（`SetEnabled(false)` 要求済みか）を id と一緒に保持する（Issue #680 レビュー
/// 指摘 P1: `current_blur_requested` が `current` のみのスカラーで、pending 中に
/// 追い越されたセッションの blur 履歴を持たない設計ギャップの対応）。A→B→C と高速に
/// 連続遷移し、B が一度も `current` にならないまま blur 要求（focus(C) による
/// `SetEnabled(false)`）を受けるケースでは、B が pending から昇格した瞬間にこの
/// 履歴を `current_blur_requested` へ引き継がないと、B 由来の遅延非空 `Preedit` が
/// 誤って C の開始信号として扱われてしまう。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingSession {
    id: ImeSessionId,
    /// このセッションがまだ pending（`current` になる前）の間に、コア側から
    /// `ImeRequest::SetEnabled(false)` を要求済みなら `true`。
    blur_requested: bool,
}

/// winit 経由の `ImeEvent` へ帰属セッションを付ける状態機械（モジュール doc 参照）。
#[derive(Debug, Default)]
pub struct ImeSessionTracker {
    /// 現在イベントを帰属させているセッション。
    current: Option<ImeSessionId>,
    /// `BeginSession` で宣言済みだが、まだ開始信号（`Enabled`/非空 `Preedit`）または
    /// 旧セッションの終了信号（`Disabled`）を観測していないセッション群（blur 履歴付き、
    /// `PendingSession` doc 参照）。
    ///
    /// 単一の `Option` ではなく到着順キューで保持する（Issue #680 レビュー指摘）:
    /// イベント到着前に A→B→C と連続遷移すると `BeginSession(C)` が `BeginSession(B)` を
    /// 単純上書きしてしまい、A の `Disabled` で `current` が中間セッション B を飛ばして
    /// 直接 C へ昇格する。その結果、B が native 側で開始・終了要求を受けていたにも
    /// 関わらず届く B 由来の遅延 `Preedit`/`Commit`（機微情報を含み得る）が C として
    /// stamp されてしまう。キューで保持し `Disabled`/`Enabled`/非空 `Preedit` のたびに
    /// 先頭を 1 つだけ昇格させることで、各遷移サイクルを飛び越えさせない。
    pending: VecDeque<PendingSession>,
    /// `current` を `Disabled` 経由（pending 先頭の昇格）で切り替えた直後、その
    /// `current` 自身の開始信号（`Enabled`/非空 `Preedit`）をまだ観測していない間 `true`。
    /// この間に届く `Enabled`/非空 `Preedit` は「同じ遷移の確認」として消費するのみで、
    /// pending をさらに 1 つ昇格させない（モジュール doc 参照、Issue #680 レビュー指摘）。
    awaiting_confirmation: bool,
    /// `awaiting_confirmation` を立てた瞬間の `pending.len()`。以降に新規
    /// `BeginSession` が届いて `pending` が増えていれば、次に届く開始信号は
    /// 「同じ遷移の確認」ではなく「昇格させた current が結局アイドルのまま
    /// 追い越された」ケースだと判別できる（`promote_or_confirm` 参照、Issue #680
    /// レビュー指摘 P1: PRRT_kwDOTl73E86defcd）。
    pending_len_at_promotion: usize,
    /// `current` が現在アクティブな非空コンポジションを保持している間 `true`。
    /// この間に届く非空 `Preedit` は新セッションの開始信号ではなく `current` 自身の
    /// 継続更新とみなし、`pending` を消費せず `current` へ帰属させる（Issue #680
    /// レビュー指摘 P0: PRRT_kwDOTl73E86defcb。旧フィールドが遅延送出する
    /// 「継続中」の非空 Preedit を新フィールドの開始信号と誤認しない）。
    current_composing: bool,
    /// `current` に対してコア側から `ImeRequest::SetEnabled(false)` が要求済み（＝
    /// blur 済み）なら `true`。`current` が切り替わるたび（`Disabled` 経由の昇格・
    /// `promote_or_confirm` の直接昇格のいずれも）、pending 中に記録していた
    /// `PendingSession::blur_requested` を引き継ぐ（無条件に `false` へリセットしない。
    /// Issue #680 レビュー指摘 P1: pending 中に追い越されたセッションの blur 履歴を
    /// 落とさないための対応、`PendingSession` doc 参照）。
    ///
    /// `true` の間は非空 `Preedit` を新セッションの開始信号として扱わない
    /// （「Enabled ゲート」、モジュール doc 参照、Issue #680 レビュー指摘 P0:
    /// PRRT_kwDOTl73E86degYY）。`Enabled` の到着のみを開始信号として待つ。
    current_blur_requested: bool,
}

impl ImeSessionTracker {
    /// 帰属セッションなしの状態で構築する。
    pub fn new() -> Self {
        Self::default()
    }

    /// `apply_ime_request` へ渡す前に呼ぶ。`ImeRequest::BeginSession` のみを解釈し、
    /// それ以外の要求（`SetEnabled`/`SetCursorArea`/`SetPurpose`）は無視する。
    ///
    /// 旧セッション（`current`）がまだなければ閉じるべき対象がないため即座に
    /// `current` へ昇格する（初回フォーカス時）。既に `current` がある場合は
    /// `pending` に据え置き、`stamp` が終了/開始信号を観測した時点で昇格させる。
    ///
    /// `ImeRequest::SetEnabled(false)` は `current_blur_requested` を立てるためだけに
    /// 観測する。`ImeFocusCoordinator::focus` は旧フィールドの `SetEnabled(false)` を
    /// 常に `BeginSession` より先に発行するため（`ime_focus.rs` §4.7 順序契約）、この
    /// 時点の `current` に対する blur 要求として扱ってよい（モジュール doc 参照、
    /// Issue #680 レビュー指摘 P1: PRRT_kwDOTl73E86den0I）。
    pub fn observe_request(&mut self, request: &ImeRequest) {
        match request {
            ImeRequest::SetEnabled(false) => {
                // blur 対象は「今フォーカスされている（＝最後に BeginSession された）
                // セッション」。pending が非空ならその末尾（`current` はまだ以前の
                // セッションのまま）、空なら `current` 自身が対象（`PendingSession`
                // doc・Issue #680 レビュー指摘 P1 参照）。
                if let Some(back) = self.pending.back_mut() {
                    back.blur_requested = true;
                } else {
                    self.current_blur_requested = true;
                }
            }
            ImeRequest::BeginSession(id) => {
                if self.current.is_none() {
                    self.current = Some(*id);
                } else {
                    self.pending.push_back(PendingSession {
                        id: *id,
                        blur_requested: false,
                    });
                }
            }
            _ => {}
        }
    }

    /// `ime_event_from_winit` の変換結果に対して呼び、`ImeFocusCoordinator::dispatch_ime`
    /// へ渡すべき帰属セッションを返す（モジュール doc「付け替え規則」参照）。
    pub fn stamp(&mut self, event: &ImeEvent) -> Option<ImeSessionId> {
        match event {
            ImeEvent::Disabled => {
                // 旧セッション（現在の current）へ帰属させてから、pending キューの
                // 先頭（1 つだけ）を昇格する（Linux の同期 Disabled→Enabled 再サイクル、
                // Windows のコンポジション確定ごとの再サイクルのいずれも、Disabled の
                // 帰属先は常に「今終わろうとしているセッション」= 昇格前の current）。
                // 先頭のみ pop することで A→B→C 連続遷移時に B を飛び越えて C へ直接
                // 昇格させない（Issue #680 レビュー指摘）。昇格後の current はまだ
                // 自分自身の開始信号を受けていないため awaiting_confirmation を立てる。
                //
                // current_composing は pending の有無に関わらず必ず解除する（Issue #680
                // レビュー指摘 P1: PRRT_kwDOTl73E86dejHr）。`Disabled` はコンポジションの
                // 終了を意味するネイティブ信号であり、pending が空でここで解除し損ねると
                // 後続の `BeginSession` が届いた際に「current が既にコンポジション中」と
                // 誤判定され、次セッションの非空 Preedit が旧セッションの継続更新として
                // 誤帰属し続ける（macOS のように Disabled 単独でフォーカスが移る入力ソース
                // 切替で顕在化）。
                let attributed = self.current;
                self.current_composing = false;
                if let Some(pending) = self.pending.pop_front() {
                    self.current = Some(pending.id);
                    self.awaiting_confirmation = true;
                    self.pending_len_at_promotion = self.pending.len();
                    // pending 中の blur 履歴を引き継ぐ（無条件 false リセットをやめた、
                    // Issue #680 レビュー指摘 P1: PendingSession doc 参照）。
                    self.current_blur_requested = pending.blur_requested;
                }
                attributed
            }
            ImeEvent::Enabled => self.promote_or_confirm(),
            ImeEvent::Preedit { text, .. } if !text.is_empty() => {
                // current が blur 要求済み（`current_blur_requested`）なら、この非空
                // Preedit を新セッションの開始信号として扱わない（「Enabled ゲート」、
                // モジュール doc 参照、Issue #680 レビュー指摘 P0:
                // PRRT_kwDOTl73E86degYY）。SetEnabled(false) 発行後も native 側で
                // キューイング済みの旧セッションのイベントが Enabled を伴わずに届く
                // プラットフォーム（Windows 等）があり、それを新セッションの開始信号と
                // 誤認すると、後続の旧セッションの確定文字列が新セッションへ
                // 誤帰属する。blur 要求済みの間は current へ帰属させ続け、`Enabled`
                // の到着（`promote_or_confirm`）を待つ。
                //
                // blur 未要求なら、current が既にコンポジション中の場合はこの非空
                // Preedit を current 自身の継続更新（旧フィールドが遅延送出した
                // 「進行中」の更新）とみなし、pending を消費せず current へ留める
                // （P0 対策、Issue #680 レビュー指摘 PRRT_kwDOTl73E86defcb）。
                // まだコンポジション中でなければ、この信号自体が開始契機（macOS で
                // Enabled が届かない場合の代替昇格契機）。
                let attributed = if self.current_blur_requested || self.current_composing {
                    self.current
                } else {
                    self.promote_or_confirm()
                };
                self.current_composing = true;
                attributed
            }
            ImeEvent::Preedit { .. } | ImeEvent::Commit(_) => {
                // 空 Preedit・Commit は pending 中でも昇格せず、旧セッション
                // （current）へ帰属させる（Issue #587 の主対象: フォーカス遷移直後に
                // 旧セッションの「空 Preedit → Commit」が届いても新セッションを
                // 名乗らせない）。
                //
                // 空 Preedit・Commit はいずれもコンポジションの終了を示す信号のため
                // current_composing を解除する（Issue #680 レビュー指摘 P1:
                // PRRT_kwDOTl73E86dejHr）。通常の終了列（空 Preedit → Commit）で
                // 解除し損ねると、`Disabled` を経由しない次セッションの非空 Preedit
                // （macOS 等）が「current の継続更新」と誤判定され、以降の非空
                // Preedit がすべて旧セッションへ stamp され続ける。
                self.current_composing = false;
                self.current
            }
        }
    }

    /// `Enabled`/非空 `Preedit`（新セッションの開始信号）に共通の昇格処理。
    ///
    /// `awaiting_confirmation` が立っている場合、`pending.len()` が
    /// `pending_len_at_promotion`（昇格時点のスナップショット）を超えていなければ、
    /// 直前の `Disabled` で pending から昇格させた現在値がこの信号でようやく自分自身の
    /// 開始を確認したとみなし、pending をさらに消費せずフラグだけ下ろす（同一遷移を
    /// Disabled と Enabled の 2 信号で二重に昇格させない、Issue #680 レビュー指摘
    /// PRRT_kwDOTl73E86deWWb）。
    ///
    /// 逆に昇格後に新規 `BeginSession` が届いて `pending` が増えていれば、今回の信号は
    /// 「同じ遷移の確認」ではなく、昇格させた現在値がネイティブ状態を一切持たないまま
    /// さらに新しいセッションへ追い越された合図である（Issue #680 レビュー指摘 P1:
    /// PRRT_kwDOTl73E86defcd）。この場合は下の「pending 末尾への昇格」分岐へフォール
    /// スルーする。
    ///
    /// `awaiting_confirmation` が立っていない場合はこの信号自体が昇格契機（Linux の
    /// Enabled 単独発火、macOS の非空 Preedit 等）である。この時点で pending に複数件
    /// 溜まっているなら、それらは一度も `Disabled`/`Enabled` を観測されないまま次の
    /// `BeginSession` に superseded された中間セッション（A/B が素早くフォーカスされ、
    /// ネイティブ側が一切イベントを発火しなかったケース）であり、実質ネイティブ状態を
    /// 持たなかったとみなしてまとめて読み捨てる。今受け取った信号は pending 末尾
    /// （最後に `BeginSession` された = 現在ネイティブ側が実際にイベントを発火している
    /// セッション）に帰属させ、先頭だけを pop すると中間セッションへ誤って昇格して
    /// しまう（Issue #680 レビュー指摘 PRRT_kwDOTl73E86deasS）。
    fn promote_or_confirm(&mut self) -> Option<ImeSessionId> {
        // current が既に blur 要求済みの場合、この Enabled/非空 Preedit を current
        // 自身の「同じ遷移の確認」として消費してはならない: blur 要求済みの
        // フィールドは native 側で無効化されているはずで、自力で正当な開始信号を
        // 再送出できない（Enabled ゲートと同じ前提）。pending の長さが一致していても
        // 「pending に blur 履歴を持つセッションが既に current へ昇格していた」だけで
        // あり、この信号は実際には pending 末尾（最新の追い越し先）の開始信号である
        // （Issue #680 レビュー指摘 P1: A→B→C 高速連続遷移で B が blur 済みのまま
        // 一度も自身の開始信号を送らず、C の Enabled だけが届くケースの対応）。
        if self.awaiting_confirmation
            && !self.current_blur_requested
            && self.pending.len() <= self.pending_len_at_promotion
        {
            self.awaiting_confirmation = false;
        } else if let Some(latest) = self.pending.pop_back() {
            self.pending.clear();
            self.current = Some(latest.id);
            self.awaiting_confirmation = false;
            self.current_composing = false;
            // pending 中の blur 履歴を引き継ぐ（無条件 false リセットをやめた、
            // Issue #680 レビュー指摘 P1: PendingSession doc 参照）。
            self.current_blur_requested = latest.blur_requested;
        }
        self.current
    }

    /// 現在の帰属先セッション（結合テスト・アプリ層の観測用）。
    pub fn current(&self) -> Option<ImeSessionId> {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preedit(text: &str) -> ImeEvent {
        ImeEvent::Preedit {
            text: text.to_string(),
            cursor: None,
        }
    }

    #[test]
    fn first_begin_session_is_current_immediately() {
        let mut tracker = ImeSessionTracker::new();
        tracker.observe_request(&ImeRequest::BeginSession(ImeSessionId::FIRST));
        assert_eq!(tracker.current(), Some(ImeSessionId::FIRST));
    }

    #[test]
    fn windows_style_batched_stale_events_before_disabled_stay_attributed_to_previous_session() {
        // Windows 形: `Disabled` に先行して「空 Preedit → Commit」が旧セッションの
        // フィールドの遅延イベントとして届く。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&preedit("")), Some(session_a));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("stale".to_string())),
            Some(session_a)
        );
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));

        // B 自身の Enabled 以降は B へ帰属する。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
        assert_eq!(tracker.stamp(&preedit("")), Some(session_b));
    }

    #[test]
    fn macos_style_promotes_on_non_empty_preedit_without_disabled() {
        // macOS 形: `Disabled` が届かない入力ソース切替でも、非空 Preedit で昇格する。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));
    }

    #[test]
    fn linux_style_promotes_on_disabled_then_enabled_sync_pair() {
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
    }

    #[test]
    fn idle_previous_session_without_disabled_still_promotes_on_enabled() {
        // A がアイドル（未変換）のまま B へ遷移すると Disabled が届かないことがある
        // （macOS/Windows いずれも「コンポジション未開始なら Disabled も出ない」契約）。
        // それでも B の Enabled で正しく昇格し、デッドロックしない。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));
    }

    #[test]
    fn non_begin_session_requests_are_ignored() {
        let mut tracker = ImeSessionTracker::new();
        tracker.observe_request(&ImeRequest::SetEnabled(true));
        assert_eq!(tracker.current(), None);
    }

    #[test]
    fn intermediate_session_in_triple_transition_is_not_skipped_by_latest_session() {
        // A→B→C とイベント到着前に連続遷移した後、A の Disabled → 中間セッション B
        // 由来の遅延「空 Preedit → Commit」が届くケース（Issue #680 レビュー指摘）。
        // B が飛び越えられて C 扱いにならないことを確認する。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        // A の終了信号。current は B へ昇格し、C はまだ pending のまま。
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));

        // B 由来の遅延「空 Preedit → Commit」は B へ帰属し、C へ漏れない。
        assert_eq!(tracker.stamp(&preedit("")), Some(session_b));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("stale-from-b".to_string())),
            Some(session_b)
        );

        // B の終了信号でようやく C へ昇格する。
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_b));
        assert_eq!(tracker.current(), Some(session_c));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("from-c".to_string())),
            Some(session_c)
        );
    }

    #[test]
    fn linux_disabled_then_enabled_pair_does_not_double_promote_across_pending() {
        // PRRT_kwDOTl73E86deWWb（Issue #680 レビュー指摘）: A→B→C と連続遷移した後、
        // Linux の同期イベント列 Disabled(A)→Enabled(B)→Disabled(B)→Enabled(C) が届く
        // ケース。Disabled(A) と Enabled(B) は同じ「A→B」遷移の 2 信号であり、
        // Enabled(B) は pending をさらに消費して C へ二重昇格してはならない。
        // Enabled(B) 後に届く B 由来の空 Preedit/Commit が正しく B へ帰属することも
        // 併せて検証する（C として stamp されると機微文字列が誤帰属する）。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));

        // 同一遷移の確認信号。C へ飛び越えてはならない。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));

        // B 由来の遅延イベントは B へ帰属する（C に混入しない）。
        assert_eq!(tracker.stamp(&preedit("")), Some(session_b));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("stale-from-b".to_string())),
            Some(session_b)
        );

        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_b));
        assert_eq!(tracker.current(), Some(session_c));
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_c));
        assert_eq!(tracker.current(), Some(session_c));
    }

    #[test]
    fn idle_intermediate_sessions_without_any_native_event_are_skipped_on_latest_start_signal() {
        // PRRT_kwDOTl73E86deasS（Issue #680 レビュー指摘）: A→B→C とイベント到着前に
        // 連続遷移し、A・B がいずれも完全にアイドル（Disabled も Enabled も一度も
        // 届かない）のまま、C の最初の開始信号（Enabled）だけが届くケース。
        // pending 先頭（B）へ誤って昇格せず、C へ直接昇格しなければならない。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        // A・B に対する Disabled/Enabled は一切発生せず、C の Enabled のみ届く。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_c));
        assert_eq!(tracker.current(), Some(session_c));

        // C 由来の Commit は正しく C へ帰属し、B に誤帰属しない。
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("from-c".to_string())),
            Some(session_c)
        );
    }

    #[test]
    fn idle_intermediate_sessions_without_any_native_event_are_skipped_on_latest_non_empty_preedit()
    {
        // 同上だが開始信号が非空 Preedit（macOS 形）の場合。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_c));
        assert_eq!(tracker.current(), Some(session_c));
    }

    #[test]
    fn composing_previous_session_delayed_preedit_stays_attributed_to_it() {
        // PRRT_kwDOTl73E86defcb（Issue #680 レビュー指摘 P0）: current（A）が既に
        // コンポジション中に、フォーカス遷移で BeginSession(B) が pending へ積まれた後、
        // A 自身の「進行中の」非空 Preedit 更新が遅延到着するケース。これを B の開始
        // 信号として誤って昇格・stamp してはならない（機微文字列の新フィールド混入）。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        // A がまずコンポジションを開始する。
        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_a));

        // フォーカスが B へ移り、BeginSession(B) が pending へ積まれる。
        tracker.observe_request(&ImeRequest::BeginSession(session_b));

        // A 自身の継続更新（遅延到着）は B へ誤帰属せず A のまま。
        assert_eq!(tracker.stamp(&preedit("あい")), Some(session_a));
        assert_eq!(tracker.current(), Some(session_a));

        // A の終了信号でようやく B へ昇格する。
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));
        assert_eq!(tracker.stamp(&preedit("い")), Some(session_b));
    }

    #[test]
    fn idle_promoted_session_is_overtaken_by_later_begin_session_before_confirmation() {
        // PRRT_kwDOTl73E86defcd（Issue #680 レビュー指摘 P1）: Disabled(A) で B へ
        // 昇格し awaiting_confirmation が立った「後」に BeginSession(C) が届き、B が
        // 一度もネイティブイベントを持たないまま C の開始信号が最初に届くケース。
        // B の確認信号と誤認して足止めせず、C へ直接昇格しなければならない。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));

        // A の終了信号で B へ昇格。この時点では C はまだ pending に積まれていない。
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));

        // B が一切イベントを発火しないまま、フォーカスが C へ移る。
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        // C の開始信号は B の確認として足止めされず、C へ直接昇格する。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_c));
        assert_eq!(tracker.current(), Some(session_c));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("from-c".to_string())),
            Some(session_c)
        );
    }

    #[test]
    fn composing_session_ending_without_disabled_releases_composing_flag_for_next_session() {
        // PRRT_kwDOTl73E86dejHr（Issue #680 レビュー指摘 P1）: A がコンポジションを
        // 完了させ（非空 Preedit → 空 Preedit → Commit の通常終了列）、`Disabled` を
        // 経由せずに B へフォーカスが移る（macOS 等）ケース。`current_composing` を
        // 終了列で解除し損ねると、B の非空 Preedit が「A の継続更新」と誤判定され、
        // 以降の非空 Preedit がすべて A へ stamp され続けてしまう。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        // A がコンポジションを開始し、通常の終了列（空 Preedit → Commit）で完了する。
        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_a));
        assert_eq!(tracker.stamp(&preedit("")), Some(session_a));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("あ".to_string())),
            Some(session_a)
        );

        // Disabled を経由せずフォーカスが B へ移り、B の非空 Preedit が届く
        // （macOS で入力ソース切替時に Disabled が届かない挙動）。
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&preedit("い")), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));
    }

    #[test]
    fn blurred_previous_session_non_empty_preedit_does_not_promote_until_enabled() {
        // PRRT_kwDOTl73E86degYY（Issue #680 レビュー指摘 P0、再設計）: A が
        // コンポジション中にフォーカスが B へ移り（コア側は SetEnabled(false) で A を
        // blur 済み）、A の旧セッション由来の非空 Preedit が遅延到着するケース。
        // 「Enabled ゲート」設計では blur 要求済みの間は非空 Preedit を昇格契機に
        // せず、`Enabled` が届くまで A へ帰属させ続ける（旧設計は blur 要求済みの
        // 非空 Preedit を即座に昇格契機として扱っていたため、native がキューイング
        // 済みの旧セッションのイベントを新セッションの開始信号と誤認しうる不具合が
        // あった）。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        // A がコンポジションを開始する。
        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_a));

        // フォーカスが B へ移る: コア側は常に旧フィールドの SetEnabled(false) を
        // BeginSession より先に発行する（ime_focus.rs §4.7 順序契約）。
        tracker.observe_request(&ImeRequest::SetEnabled(false));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));

        // blur 要求済みの間に届く非空 Preedit は B の開始信号として昇格させず、
        // A へ帰属させ続ける。
        assert_eq!(tracker.stamp(&preedit("あい")), Some(session_a));
        assert_eq!(tracker.current(), Some(session_a));

        // B 自身の開始信号（Enabled）が届いてようやく B へ昇格する。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));

        // B の後続の非空 Preedit・Commit も正しく B へ帰属し続ける。
        assert_eq!(tracker.stamp(&preedit("い")), Some(session_b));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("い".to_string())),
            Some(session_b)
        );
    }

    #[test]
    fn blurred_previous_session_delayed_preedit_does_not_leak_commit_into_next_session() {
        // Issue #680 P0（codex 指摘、PRRT_kwDOTl73E86degYY）の再現列そのもの:
        // A: 非空 Preedit → focus(B)/SetEnabled(false) → 遅延した A 由来の
        // 非空 Preedit → 空 Preedit → Commit、という順序で届いても、A の確定文字列が
        // B へ誤って stamp されないことを検証する。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        // A: 非空 Preedit（コンポジション開始）。
        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_a));

        // フォーカスが B へ移る（blur 要求 → BeginSession の順序契約）。
        tracker.observe_request(&ImeRequest::SetEnabled(false));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));

        // native 側にキューイング済みだった A の遅延イベント列。すべて A へ帰属し、
        // B（session_b）として stamp されてはならない。
        assert_eq!(tracker.stamp(&preedit("あい")), Some(session_a));
        assert_eq!(tracker.stamp(&preedit("")), Some(session_a));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("あい".to_string())),
            Some(session_a)
        );
        assert_eq!(tracker.current(), Some(session_a));

        // その後 B 自身の開始信号（Enabled）が届いた時点で B へ昇格し、以降の
        // イベントが正しく B へ帰属する。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_b));
        assert_eq!(tracker.stamp(&preedit("う")), Some(session_b));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("う".to_string())),
            Some(session_b)
        );
    }

    #[test]
    fn fast_triple_transition_blurred_intermediate_field_promotes_to_latest_field() {
        // Issue #680 レビュー指摘 P1: `current_blur_requested` が `current` のみの
        // スカラーで、pending 中に追い越されたセッションの blur 履歴を持たない設計
        // ギャップの対応（`PendingSession` doc 参照）。A→B→C と高速に連続遷移し
        // （focus(B) → focus(C) → Disabled(A) → B 由来の非空 Preedit → 空 Preedit →
        // Commit → Enabled(C)）、B は一度も自身の開始信号を受け取らないまま blur
        // 要求も受けるケース。B の Commit が C へ誤って stamp されないこと、かつ
        // 正当系列（C 自身の Enabled）で C へ昇格できることを検証する。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        let session_c = session_b.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        // focus(B): 旧フィールド A を blur してから B を宣言する（§4.7 順序契約）。
        tracker.observe_request(&ImeRequest::SetEnabled(false));
        tracker.observe_request(&ImeRequest::BeginSession(session_b));

        // focus(C): B も一度も current にならないまま blur される。
        tracker.observe_request(&ImeRequest::SetEnabled(false));
        tracker.observe_request(&ImeRequest::BeginSession(session_c));

        // A の終了信号。pending 先頭の B へ昇格し、B の blur 履歴（true）を引き継ぐ。
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));
        assert_eq!(tracker.current(), Some(session_b));

        // B 由来の遅延イベント列（非空 Preedit → 空 Preedit → Commit）はすべて B へ
        // 帰属し、C へ漏れない（Enabled ゲート: B は blur 要求済みのため非空 Preedit
        // では昇格しない）。
        assert_eq!(tracker.stamp(&preedit("あい")), Some(session_b));
        assert_eq!(tracker.stamp(&preedit("")), Some(session_b));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("stale-from-b".to_string())),
            Some(session_b)
        );
        assert_eq!(tracker.current(), Some(session_b));

        // C 自身の開始信号（Enabled）が届いて正しく C へ昇格する。
        assert_eq!(tracker.stamp(&ImeEvent::Enabled), Some(session_c));
        assert_eq!(tracker.current(), Some(session_c));
        assert_eq!(
            tracker.stamp(&ImeEvent::Commit("from-c".to_string())),
            Some(session_c)
        );
    }

    #[test]
    fn disabled_without_pending_releases_composing_flag_for_next_session() {
        // 同上だが、A の終了信号が `Disabled`（pending が空の状態）で届くケース。
        // pending が無いと `Disabled` 分岐の昇格処理自体が走らないため、
        // `current_composing` を pending の有無に関わらず解除する必要がある。
        let mut tracker = ImeSessionTracker::new();
        let session_a = ImeSessionId::FIRST;
        let session_b = session_a.next();
        tracker.observe_request(&ImeRequest::BeginSession(session_a));

        assert_eq!(tracker.stamp(&preedit("あ")), Some(session_a));
        assert_eq!(tracker.stamp(&ImeEvent::Disabled), Some(session_a));

        // BeginSession(B) は Disabled の「後」に届く（pending は空だった）。
        tracker.observe_request(&ImeRequest::BeginSession(session_b));
        assert_eq!(tracker.stamp(&preedit("い")), Some(session_b));
        assert_eq!(tracker.current(), Some(session_b));
    }
}
