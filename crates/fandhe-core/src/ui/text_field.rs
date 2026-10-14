//! テキストフィールドウィジェットの状態・イベント処理実装
//! （TASK-8.3-S14 / REQ-8、Issue #109）。
//!
//! `super::label`（ラベル・テキスト、Issue #106）が採った判断（既存 main の
//! 公開 API のみに依存する自己完結実装とし、`text_input::TextInput` との型統合は
//! 別作業に委ねる）を踏襲し、[`Widget<S>`] を実装する最小のテキストフィールド
//! ウィジェット型 [`TextFieldWidget`] として実装する（`measure` / `paint` は
//! 暫定実装）。
//!
//! テキストフィールドはラベル・テキストと異なりインタラクティブ要素であり、
//! 以下を設計判断とする。
//!
//! - 編集中の一時的 UI 状態（編集バッファ・カーソル位置）はアプリ状態 `S` には含めない
//!   （設計文書 §3 二層モデル。`super::widget` モジュール doc 参照）。確定値のみ
//!   [`crate::state::Event::TextFieldChanged`] 経由で `S` へブリッジする。
//! - カーソル操作・挿入・削除はすべて **char 単位**で行う（byte index を使わず、
//!   「こんにちは」等マルチバイト文字列で panic しない。panic を API 契約にしない、
//!   `.claude/rules/coding-rust.md` 準拠）。
//! - `InsertText` は制御文字（`char::is_control`、改行含む）を除去してから挿入する
//!   （単一行フィールドの意味論・OWASP A03 系の防御的入力検証、`.claude/rules/security.md`）。
//! - `Commit`（Enter 相当）は `Event::TextFieldChanged { label, value }` を返すのみで、
//!   widget 層は `&mut S` に一切触れない（[`super::pointer::PointerDispatcher`] と
//!   同じブリッジ設計。状態書き換えの単一ソースを `StateStore::dispatch` に保つ）。
//! - disabled 中・非フォーカス中はすべての編集入力を無視する（`InteractionState` から
//!   意味論を継承し、独自の入力遮断ロジックを再実装しない）。
//! - フォーカス喪失（`FocusLost`）時、`buffer` がユーザー編集で変化しており
//!   （`dirty`）かつ確定値と実際に異なる場合のみ blur-commit として
//!   `Event::TextFieldChanged` を返す（`buffer != committed_value` の単純な差分
//!   比較だけでは、`sync_from_state` がフォーカス中も `committed_value` を進める
//!   ため外部由来の更新とユーザー編集を区別できず、無編集フィールドが古い
//!   `buffer` で外部更新を上書きしてしまう。`commit_if_changed` の doc 参照）。
//! - `set_disabled(true)` はフォーカスを奪う前に上記と同じ blur-commit を実行する
//!   （`InteractionState::set_disabled` が `focused` を無条件でクリアするため、
//!   先に commit しないとダーティな編集が `TextFieldChanged` を一度も発火せずに
//!   破棄される）。
//!
//! **機微情報の露出防止**（`.claude/rules/security.md`）: `Debug` 実装は編集バッファ・
//! 確定値の**内容を出力しない**（文字数のみ）。テキストフィールドはパスワード等の
//! 機微情報を保持しうるため、ログ経由の情報漏えいを防ぐ。
//!
//! **未配線の契約**（[`super::widget`] モジュール doc 参照）: reconcile パス
//! （`&mut self` を `Node::Widget` へ届ける経路）は S3 以降も未確立であり、本モジュールの
//! `sync_from_state` は現時点でユニットテストからのみ駆動される。
//!
//! **スコープ外事項**（`.claude/rules/out-of-scope-tracking.md` 準拠。PR 本文に記録する）:
//! テキスト選択範囲・`Node::TextField::set` の `WriteFn` 統合・S13 マージ後の型統合。
//! IME 変換合成（composition）は本 issue（TASK-8.5-S8 / #189）で配線済み（下記参照）。
//! **grapheme cluster 単位のカーソル処理は TASK-8.5-S26（#207）で実装済み**
//! （`DeleteBackward` / `DeleteForward` / `MoveCursorLeft` / `MoveCursorRight` の doc
//! コメント・`text_selection.rs` の grapheme 境界ヘルパー参照）。
//!
//! **S15（Issue #110）の消化状況**: テスト網羅拡充・`WidgetCatalog` への登録は S15 で
//! 完了した。`catalog.rs` の `text_field` エントリは本モジュールの兄弟型
//! [`super::text_input::TextInput`] へ接続済み（`TextFieldWidget<S>` 自体との型統合は
//! 上記スコープ外のまま引き続き未実施）。結合テストは `tests/text_input_widget.rs` に
//! `TextFieldWidget<S>` の E2E（フォーカス → 入力 → Commit → `StateStore::dispatch`）を
//! 追加し、本ファイル内ユニットテストと重複する軸は代表ケースに絞った
//! （モジュール doc・`tests/text_input_widget.rs` 冒頭コメント参照）。
//!
//! ## IME 統合配線（TASK-8.5-S8 / REQ-8、Issue #189）
//!
//! `docs/architecture/text-input-state-model.md`（#183）§4.2〜§4.9 が確定した
//! コンポジション状態モデルを本ウィジェットへ配線する。`composition_display.rs`
//! （#186）と同じ判断（設計文書 §4.2 のシグネチャ案を一字一句変えずに実装する）を
//! 踏襲し、[`Composition`] 型を本モジュール内に自己完結で実装している
//! （設計文書 §4.2〜§4.9・§5 不変条件 1〜9 と 1 対 1 で対応。[`Composition`] 型・
//! [`apply_ime`][TextFieldWidget::apply_ime]・フォーカスガード・`sync_from_state` の
//! Composing 中据え置き（§4.9）はいずれも設計契約どおりに実装されている。
//! 別モジュールへの切り出しは行わない判断。詳細は `text-input-state-model.md` §6 を
//! 参照）。
//!
//! - [`Composition`][]: 進行中のコンポジション（未確定文字列）。`text-input-state-model.md`
//!   §4.2 の型案をそのまま実装（`Debug` は文字数のみ出力、機微情報マスク）。
//! - [`TextFieldWidget::apply_ime`][]: `ImeEvent` を受け付ける公開 API（同文書 §4.3〜§4.6
//!   の状態遷移の実体）。プラットフォーム層（`fandhe-winit-ime` 等、S7・S9〜S22）が
//!   ネイティブ IME イベントを `ImeEvent` へ変換して呼び出す想定（`ime.rs` モジュール doc
//!   参照）。
//! - [`TextFieldWidget::take_ime_requests`][]: コアからプラットフォーム層への要求
//!   （`ImeRequest::SetEnabled`）を drain する最小のキュー（`ime-architecture.md` §4.2
//!   が実装形態をプラットフォーム層に委ねる要求チャネルのコア側最小実装）。
//! - `apply_input` / `set_disabled` / `handle_input` / `sync_from_state` / `paint` /
//!   `paint_styled` / `measure` への配線は各メソッドの doc コメントを参照。
//!
//! **フォーカスガード**（`text-input-state-model.md` §4.3・§4.5 手順 0・不変条件 8）:
//! `interaction.focused == false` の間に届いた `ImeEvent::Preedit` / `ImeEvent::Commit` は
//! 破棄する（`buffer` / `cursor` / `dirty` / `composition` 不変）。`ImeEvent::Disabled` の
//! みこのガードの対象外（フォーカス状態によらず `composition` を必ずクリアする）。
//!
//! ## IME オン/オフ・入力目的の公開 API（TASK-8.5-S23 / REQ-8、Issue #204）
//!
//! 親 issue #181 の sub-issue 23/30。`ime-architecture.md` §4.2 の入力目的ヒント契約
//! （`SetPurpose` はコアの公開 API 経由でアプリ側が設定する）を実装する。
//!
//! - [`TextFieldWidget::set_ime_purpose`]: `ImePurpose` を設定する公開 API。設計文書の
//!   例示名は `set_purpose` だが、本モジュールの `ime` 接頭辞系命名（`apply_ime` /
//!   `take_ime_requests`）に合わせ `set_ime_purpose` とする（意図的な逸脱）。
//! - [`TextFieldWidget::ime_purpose`] / [`TextFieldWidget::ime_enabled`] /
//!   [`TextFieldWidget::is_composing`]: 現在の IME 状態を読み取る公開 API。
//!   `is_composing` は `composition.is_some()` を返すのみで、preedit 内容自体は
//!   公開しない（機微情報の露出防止方針を維持、`.claude/rules/security.md`）。
//!
//! **Password 時のポリシー**（`ime.rs` の `ImeRequest::SetPurpose` doc・
//! `ime-architecture.md` §9 参照。Bugbot #457 レビュー指摘 1・2 を受け S23 時点の
//! 実装から変更した）: winit 0.30 系では `set_ime_purpose` が Wayland 以外で no-op の
//! ため、`ImePurpose::Password` のヒントだけでは IME の学習・予測変換抑止を担保
//! できない。**S23 時点ではこれを `ImeRequest::SetEnabled(false)` の併送で補っていたが、
//! `SetEnabled` は Android/iOS ではソフトキーボードの表示/非表示に直結するため、
//! パスワードフィールドにフォーカスした瞬間にキーボードごと隠れて入力できなくなる
//! 欠陥があった（Bugbot 指摘 1）。** そのため本モジュールは `ImePurpose::Password` の
//! フィールドでも `SetEnabled` は他の purpose と同様に扱う（`SetEnabled(false)` は
//! `FocusLost` / `set_disabled(true)` 等、フィールドが入力対象でなくなる経路でのみ
//! 発行する）。学習・予測変換の抑止は `SetPurpose(Password)` の意図表明のみに委ね、
//! ヒントが no-op なプラットフォームでの実現手段（`ime-architecture.md` の各
//! プラットフォーム節が指す `set_ime_editor_info` 等）はプラットフォーム層に委譲する
//! （REQ-2 の依存方向 = コアは意図を表明し、機構はプラットフォーム層が選ぶ、を
//! 強める判断）。
//!
//! 進行中のコンポジションはフォーカス中の `Password` 切替時も引き続き（確定せず）
//! ローカルに破棄する。ただし `SetEnabled(false)` を送らなくなったことで、破棄した
//! preedit がその後 `Commit` として遅延到着する可能性が残る（Windows の
//! `WM_IME_ENDCOMPOSITION` 等、Bugbot 指摘 2）。これを防ぐため
//! `ime_purpose_switch_guard`（フィールド doc・`apply_ime` の doc 参照）という
//! ワンショットガードを導入し、破棄直後に届く最初の `Commit` のみを飲み込む。
//! 複数フィールド間の調停は S24（#205）のスコープであり本 issue では扱わない。
//!
//! **スコープ外**（本 issue、`out-of-scope-tracking.md` 準拠）: 兄弟型 `TextInput` への
//! IME 配線・`ImeRequest::SetCursorArea`（S6 #187）・winit `Ime::*` との実接続
//! （S7 #188）・複数フィールド調停（S24 #205）・preedit 装飾（下線・背景色）の
//! デザイントークン化。grapheme cluster・サロゲートペアの網羅検証・編集操作の
//! grapheme 対応は TASK-8.5-S26（#207）で実装済み（上記モジュール doc 参照）。

use crate::state::{Event, ReadFn};

use super::composition_display::CompositionDisplay;
use super::ime::{ImeEvent, ImePurpose, ImeRequest};
use super::text_selection::{
    CharOffset, is_grapheme_boundary, next_grapheme_boundary, prev_grapheme_boundary,
};
use super::widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, VisualState,
    Widget, WidgetId,
};

/// IME コンポジション（未確定文字列）の状態（`text-input-state-model.md` §4.2）。
///
/// [`TextFieldWidget`] が保持する一時的 UI 状態であり、アプリ状態 `S` には含めない
/// （二層モデル、REQ-13）。プラットフォーム層から届く [`ImeEvent::Preedit`]
/// （`docs/architecture/ime-architecture.md` §4.1）を受けて更新される。
#[derive(Clone, PartialEq)]
struct Composition {
    /// 現在の未確定文字列全体。
    text: String,
    /// `text` 内でのカーソル範囲（`text` の先頭を 0 とした相対 [`CharOffset`]、
    /// `ImeEvent::Preedit.cursor` と同じ座標系）。`None` はカーソルを表示しない
    /// ことを表す。
    cursor: Option<(CharOffset, CharOffset)>,
    /// コンポジション開始時点の確定バッファ内カーソル位置（char 単位、
    /// `TextFieldWidget::cursor` と同じ座標系）。
    ///
    /// `cursor` フィールド（`text` 内の相対範囲）と `anchor`（`buffer` 内の絶対位置）は
    /// **別の座標系**であり、混同しないこと（§4.2 参照）。表示合成
    /// （[`CompositionDisplay::synthesize`]）・確定挿入位置（[`TextFieldWidget::apply_ime`]）
    /// はいずれもこの `anchor` を起点とする。
    anchor: CharOffset,
}

impl std::fmt::Debug for Composition {
    /// 未確定文字列の内容を出力しない（文字数のみ）。既存 `TextFieldWidget` の
    /// Debug マスク方針を継承する（機微情報の露出防止、`.claude/rules/security.md`）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composition")
            .field("text_len", &self.text.chars().count())
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .finish()
    }
}

/// テキストフィールドへの編集入力を表す列挙（本 issue の核心）。
///
/// 外部由来の生バイト列やキーコードを直接扱わず、意味づけ済みの列挙のみで構成することで
/// OWASP A03（インジェクション）対策を型レベルで担保する（`widget::InteractionInput` と
/// 同じ設計判断、`.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFieldInput {
    /// カーソル位置へ文字列を挿入する（制御文字は挿入前に除去される）。
    InsertText(String),
    /// カーソル直前の 1 文字を削除する（char 単位、境界では no-op）。
    DeleteBackward,
    /// カーソル直後の 1 文字を削除する（char 単位、境界では no-op）。
    DeleteForward,
    /// カーソルを 1 文字左へ移動する（境界では no-op）。
    MoveCursorLeft,
    /// カーソルを 1 文字右へ移動する（境界では no-op）。
    MoveCursorRight,
    /// カーソルを先頭へ移動する。
    MoveCursorToStart,
    /// カーソルを末尾へ移動する。
    MoveCursorToEnd,
    /// 編集内容を確定する（Enter 相当）。`Event::TextFieldChanged` を発火させる。
    Commit,
    /// 編集内容を破棄し、直近の確定値へ巻き戻す（Esc 相当）。
    Cancel,
}

/// テキストフィールドウィジェット（TASK-8.3-S14 で新設する自己完結の最小実装）。
///
/// `measure` / `paint` は S13（#108）の基本描画実装がスコープであるため、本 issue では
/// 状態・イベント処理を単体で検証可能にするための暫定実装（[`super::label::LabelWidget`]
/// と同様、文字数に応じた素朴な計測・`PaintCommand` への変換のみ）を提供する。
pub struct TextFieldWidget<S> {
    id: WidgetId,
    /// `Event::TextFieldChanged` へのブリッジに使うラベル（宣言層 `Node::TextField` と
    /// 共有するキー。`WidgetId` とは独立し、状態ディスパッチ側の識別に用いる）。
    label: String,
    /// 状態 `S` から確定値を読み取るための関数ポインタ（`None` の場合は静的な
    /// 初期値のみを保持し、`sync_from_state` は no-op になる。REQ-13 の関数ポインタ
    /// 限定方針に従い、`ReadFn` のみを介して `S` に触れる）。
    value_source: Option<ReadFn<S, String>>,
    /// 直近の `sync_from_state`（または構築時の初期値）で確定した表示値。
    committed_value: String,
    /// 編集中バッファ（`S` には含めない一時的 UI 状態、二層モデル準拠）。
    buffer: String,
    /// カーソル位置（**char 単位**。`buffer.chars().count()` を上限にクランプする）。
    cursor: usize,
    /// `buffer` が直近の同期以降にユーザー編集で変化したかを表すフラグ。
    ///
    /// `commit_if_changed` の「編集されたか」の判定を `buffer != committed_value` から
    /// 分離するために導入する（Bugbot 指摘 #362 review comment 3683619206 対応）。
    /// `sync_from_state` はフォーカス中でも `committed_value` を進めるため、
    /// 単純な差分比較では「外部由来の `committed_value` 更新」と「ユーザー編集」を
    /// 区別できず、無編集のフォーカス中フィールドが古い `buffer` で外部更新を
    /// 上書きしてしまう（stale blur commit）。実際に `handle_input` が `buffer` を
    /// 変更した場合にのみ `true` にし、`Commit` / `Cancel` / 非編集中の
    /// `sync_from_state` で `false` へ戻す。
    dirty: bool,
    interaction: InteractionState,
    /// 進行中のコンポジション。`None` は非コンポジション中（Idle）を表す
    /// （`text-input-state-model.md` §4.3 のステートマシン、`apply_ime` 参照）。
    composition: Option<Composition>,
    /// コアからプラットフォーム層への `ImeRequest` を保持する最小のキュー。
    /// プラットフォーム層（S7 以降）が [`Self::take_ime_requests`] で drain する
    /// 想定（`ime-architecture.md` §4.2 の要求チャネルのコア側最小実装）。
    pending_ime_requests: Vec<ImeRequest>,
    /// プラットフォーム層から届いた最後の `ImeEvent::Enabled`/`Disabled` を反映する
    /// **観測値**（S23 / #204）。要求（`ImeRequest::SetEnabled`）を発行したかどうかとは
    /// 独立しており、プラットフォームからの応答が届くまで反映されない（`apply_ime`
    /// 参照）。Windows は変換確定のたびに `Disabled`→`Enabled` を高頻度に再サイクル
    /// しうる契約（`ime.rs` の `ImeEvent::Enabled`/`Disabled` doc 参照）ため、本フィールドは
    /// 直近 1 件の値のみを保持する単純な観測値である。
    ime_enabled: bool,
    /// 入力目的ヒント（S23 / #204）。既定は `ImePurpose::Normal`。
    /// [`Self::set_ime_purpose`] でのみ変更される。
    ime_purpose: ImePurpose,
    /// Bugbot #457 指摘 2 対応: フォーカス中に `Password` へ切り替えてローカルの
    /// `composition` を破棄した直後、次に届く `ImeEvent` を 1 件だけガードする
    /// ワンショットフラグ（`set_ime_purpose` の doc 参照）。
    ///
    /// `FocusLost` のフォーカスガード（`interaction.focused == false`）と同じ動機だが、
    /// Password 切替はフォーカスを保ったまま起こるため既存ガードでは防げない。
    /// プラットフォームによっては（特に Windows の `WM_IME_ENDCOMPOSITION`）切替前の
    /// preedit がその後 `Commit` として届く可能性があり、`composition` を `None` に
    /// 戻していても `apply_ime` の `Commit` 腕は `self.cursor` へフォールバックして
    /// 挿入してしまう（防げないと機微情報がパスワードバッファへ混入する）。
    ///
    /// 常に 1 件で解除されるバウンデッドガードとする（`ime_purpose_switch_guard` を
    /// 参照する `apply_ime` 側の各腕の doc 参照）。`ImeEvent` に相関トークンがないため
    /// 「本当に stale か」を厳密には区別できないが、デッドロックしない
    /// （ガードが無期限に残り続けて以降の正当な入力まで飲み込む）ことを優先する。
    ime_purpose_switch_guard: bool,
}

/// `buffer` の char 単位カーソル位置に対応する byte index を求める。
///
/// `cursor` は本モジュールが `chars().count()` 以下にクランプし続けるため、
/// 呼び出し側が不正な値を渡さない限り必ず有効な境界を返す（境界外の場合は
/// 末尾へフォールバックし、panic を API 契約にしない）。
fn byte_index_at(buffer: &str, char_index: usize) -> usize {
    buffer
        .char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(buffer.len())
}

/// 挿入直後のカーソル位置を `buffer` の grapheme cluster 境界へスナップする
/// （Bugbot #461 指摘対応・TASK-8.5-S26 / #207）。
///
/// `InsertText` / `ImeEvent::Commit` はカーソルを挿入した char 数分だけ前進させるが、
/// 挿入文字列の末尾が `buffer` 内の後続文字（結合文字・地域指標記号・肌色トーン
/// 修飾子等）と結合して 1 つの grapheme cluster を形成する場合、素朴な char 数
/// 前進では新しく形成されたクラスタの途中にカーソルが位置してしまい、「カーソルは
/// 常に grapheme 境界上にある」という本モジュールの不変条件を破る。破ったまま
/// `DeleteBackward` が呼ばれると `prev_grapheme_boundary` がそのクラスタの開始位置
/// までしか戻らず、削除範囲がクラスタの一部（プレフィックス）のみとなり、宙に
/// 浮いた修飾子が残る。
///
/// 挿入直後のカーソルは「これから続く入力の位置」であり、形成されたクラスタは
/// 挿入操作の一部とみなせるため、境界でなければ手前ではなく **後方** の直近境界へ
/// スナップする（`next_grapheme_boundary`）。既に境界上にある場合は no-op とし、
/// 余分な前進を避ける。
fn snap_cursor_forward_to_grapheme_boundary(buffer: &str, cursor: CharOffset) -> CharOffset {
    if is_grapheme_boundary(buffer, cursor) {
        cursor
    } else {
        next_grapheme_boundary(buffer, cursor)
    }
}

impl<S> TextFieldWidget<S> {
    /// 静的な初期値のみを持つテキストフィールドを構築する（`sync_from_state` は no-op）。
    pub fn new_static(id: WidgetId, label: impl Into<String>, initial: impl Into<String>) -> Self {
        let committed_value = initial.into();
        let cursor = committed_value.chars().count();
        Self {
            id,
            label: label.into(),
            value_source: None,
            buffer: committed_value.clone(),
            committed_value,
            cursor,
            dirty: false,
            interaction: InteractionState::default(),
            composition: None,
            pending_ime_requests: Vec::new(),
            ime_enabled: false,
            ime_purpose: ImePurpose::Normal,
            ime_purpose_switch_guard: false,
        }
    }

    /// 状態 `S` から動的に確定値を読み取るテキストフィールドを構築する。
    ///
    /// `initial` は最初の `sync_from_state` 呼び出し前に `buffer()` / `paint()` が
    /// 参照する初期表示値（未同期状態でも空文字を返さないための保険。
    /// `super::label::LabelWidget::new_dynamic` と同じ判断）。
    pub fn new_dynamic(
        id: WidgetId,
        label: impl Into<String>,
        initial: impl Into<String>,
        value_source: ReadFn<S, String>,
    ) -> Self {
        let committed_value = initial.into();
        let cursor = committed_value.chars().count();
        Self {
            id,
            label: label.into(),
            value_source: Some(value_source),
            buffer: committed_value.clone(),
            committed_value,
            cursor,
            dirty: false,
            interaction: InteractionState::default(),
            composition: None,
            pending_ime_requests: Vec::new(),
            ime_enabled: false,
            ime_purpose: ImePurpose::Normal,
            ime_purpose_switch_guard: false,
        }
    }

    /// `Event::TextFieldChanged` の発火対象を特定するラベル。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 現在の編集中バッファ文字列。
    pub fn buffer(&self) -> &str {
        &self.buffer
    }

    /// 現在のカーソル位置（char 単位）。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// 直近の確定値（`sync_from_state` または `Commit` で更新される）。
    pub fn committed_value(&self) -> &str {
        &self.committed_value
    }

    /// disabled フラグを切り替える。
    ///
    /// `true` へ切り替える場合、[`InteractionState::set_disabled`] が `focused` を
    /// 無条件でクリアするため、その前に blur-commit（[`Self::commit_if_changed`]）を
    /// 実行してダーティな編集を確定させる。これを行わないと、フォーカス喪失後に
    /// `FocusLost` が来なくなり（`was_focused` が既に `false`）、後続の
    /// `sync_from_state` が編集内容を `TextFieldChanged` を一度も発火せずに
    /// 破棄してしまう（Bugbot 指摘 #362 review comment 3683619228 対応）。
    ///
    /// フォーカスを奪う経路は `FocusLost` 相当のクリーンアップに従い（IME の
    /// コンポジションを確定せず先に破棄し、`ImeRequest::SetEnabled(false)` を
    /// 要求する）、`text-input-state-model.md` §4.6 末尾の契約を満たす。
    pub fn set_disabled(&mut self, disabled: bool) -> Option<Event> {
        let event = if disabled && self.interaction.focused {
            self.composition = None;
            let event = self.commit_if_changed();
            self.pending_ime_requests
                .push(ImeRequest::SetEnabled(false));
            event
        } else {
            None
        };
        self.interaction.set_disabled(disabled);
        event
    }

    /// 見た目解決に使う優先順位付き視覚状態（[`InteractionState::visual_state`] への委譲）。
    pub fn visual_state(&self) -> VisualState {
        self.interaction.visual_state()
    }

    /// ポインタ・フォーカス入力を適用する。
    ///
    /// [`InteractionState::apply`] への委譲に加え、本ウィジェット固有の意味論として
    /// `FocusLost` 時の **blur-commit**（buffer が確定値と異なる場合に
    /// `Event::TextFieldChanged` を返す）と、`FocusGained` 時のカーソル末尾配置を行う。
    /// disabled 中は `InteractionState::apply` が入力そのものを無視するため、
    /// フォーカス遷移自体が起きず blur-commit も発火しない（意図した設計）。
    ///
    /// IME 統合配線（`ime-architecture.md` §4.7、`text-input-state-model.md` §4.6）:
    /// `FocusGained` はプラットフォーム層へ `ImeRequest::SetPurpose(self.ime_purpose)` →
    /// `ImeRequest::SetEnabled(true)` の順で要求する（S23 / #204。purpose が
    /// プラットフォームへ伝わってから有効化する順序を契約化する）。
    /// `Password` の場合も含めて常に `SetEnabled(true)` を要求する（Bugbot #457 指摘 1
    /// 対応。`SetEnabled` は Android/iOS ではソフトキーボードの表示/非表示に直結する
    /// （`ime-architecture.md` §4.7・§4.9 Android/iOS 節）ため、`Password` フィールドで
    /// `SetEnabled(false)` を要求するとキーボードごと隠れて入力できなくなる。
    /// 学習・予測変換の抑止はコアが `SetEnabled` を乗っ取るのではなく `SetPurpose(Password)`
    /// の意図表明のみで行い、ヒントが no-op なプラットフォームでの実現手段（無効化する
    /// か等）はプラットフォーム層に委ねる、モジュール doc「Password 時のポリシー」参照）。
    /// `FocusLost` はコンポジションを**確定せずローカルに破棄**してから既存の
    /// blur-commit を実行し（フォーカスが外れたフィールドへ preedit を残さない、
    /// §4.6 のフォーカスガードと対になる防御）、`ImeRequest::SetEnabled(false)` を
    /// 要求する（フィールド自体が入力対象でなくなるため、こちらは引き続き無効化する）。
    pub fn apply_input(&mut self, input: InteractionInput) -> Option<Event> {
        let was_focused = self.interaction.focused;
        self.interaction.apply(input);

        if matches!(input, InteractionInput::FocusGained) && self.interaction.focused {
            self.cursor = self.buffer.chars().count();
            self.pending_ime_requests
                .push(ImeRequest::SetPurpose(self.ime_purpose));
            self.pending_ime_requests.push(ImeRequest::SetEnabled(true));
        }

        if matches!(input, InteractionInput::FocusLost) && was_focused && !self.interaction.focused
        {
            self.composition = None;
            let event = self.commit_if_changed();
            self.pending_ime_requests
                .push(ImeRequest::SetEnabled(false));
            return event;
        }
        None
    }

    /// プラットフォーム層（S7・S9〜S22）へ送出すべき `ImeRequest` を drain する。
    ///
    /// コアはコールバック・イベントキュー等の実装形態を持たないため、要求は本キューに
    /// 蓄積され、プラットフォーム層が描画ループ等で定期的に本メソッドを呼び出して
    /// 取り出す想定（`ime-architecture.md` §4.2、`out-of-scope-tracking.md` 準拠で
    /// 具体的な配信機構は S9 以降に委ねる）。
    pub fn take_ime_requests(&mut self) -> Vec<ImeRequest> {
        std::mem::take(&mut self.pending_ime_requests)
    }

    /// 入力目的ヒントを設定する（S23 / #204、`ime-architecture.md` §4.2）。
    ///
    /// `ime_purpose` を更新し、変更があった場合のみ `ImeRequest::SetPurpose` を
    /// キューへ積む（同値再設定では要求を積まない）。**フォーカス中に呼ばれた場合**は
    /// 追加で以下を行う（モジュール doc「Password 時のポリシー」参照。Bugbot #457
    /// 指摘 1・2 対応で S23 時点の実装から変更した）。
    ///
    /// - `Password` へ切替時: 進行中のコンポジションを（確定せず）破棄する。
    ///   `ImeRequest::SetEnabled(false)` は**もう要求しない**（`SetEnabled` は
    ///   Android/iOS でソフトキーボードの表示/非表示に直結するため、これを要求すると
    ///   パスワードフィールドにフォーカスした瞬間にキーボードごと隠れて入力できなく
    ///   なる、Bugbot 指摘 1）。学習・予測変換の抑止は `ImeRequest::SetPurpose(Password)`
    ///   の意図表明のみに委ね、ヒントが no-op なプラットフォームでの実現手段は
    ///   プラットフォーム層に委譲する（REQ-2 の依存方向を強める判断）。
    ///   代わりに `ime_purpose_switch_guard` を立て、破棄した preedit がその後
    ///   `Commit` として遅延到着しても `apply_ime` が挿入しないようにする
    ///   （Bugbot 指摘 2。`FocusLost` の非フォーカスガードでは、フォーカスを保った
    ///   ままの purpose 切替を防げないため専用ガードが必要、フィールド doc 参照）。
    /// - `Password` から `Normal`/`Terminal` へ復帰時: 追加の `ImeRequest` は発行しない
    ///   （`SetEnabled(false)` を送っていないため、対称的な再有効化要求も不要）。
    ///
    /// フォーカス外の呼び出しは `ime_purpose` の更新のみを行い、次回 `FocusGained`
    /// （`apply_input`）が `SetPurpose` → `SetEnabled(true)` を要求する。
    pub fn set_ime_purpose(&mut self, purpose: ImePurpose) {
        if self.ime_purpose == purpose {
            return;
        }
        self.ime_purpose = purpose;
        self.pending_ime_requests
            .push(ImeRequest::SetPurpose(purpose));

        if !self.interaction.focused {
            return;
        }
        if purpose == ImePurpose::Password {
            // 機微情報の入力目的へ切り替わった時点で、確定していない preedit を
            // 残さない（フォーカス喪失時のガードと同じ防御方針、§4.6 参照）。
            self.composition = None;
            // Bugbot #457 指摘 2: 破棄した preedit の遅延 Commit を次の 1 件に限り
            // 無視する（フィールド doc・`apply_ime` の各腕の doc 参照）。
            self.ime_purpose_switch_guard = true;
        }
    }

    /// 現在の入力目的ヒント。
    pub fn ime_purpose(&self) -> ImePurpose {
        self.ime_purpose
    }

    /// プラットフォーム層から届いた最後の `ImeEvent::Enabled`/`Disabled` を反映する
    /// 観測値（`ime_enabled` フィールドの doc 参照）。IME 有効化の要求
    /// （`ImeRequest::SetEnabled(true)`）を発行したことそのものではなく、
    /// プラットフォームからの応答（`apply_ime`）が届いた結果を表す。
    pub fn ime_enabled(&self) -> bool {
        self.ime_enabled
    }

    /// コンポジション（未確定文字列）が進行中かどうか。
    ///
    /// preedit の内容自体は公開しない（機微情報の露出防止、モジュール doc 参照）。
    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }

    /// プラットフォーム層から届く IME コンポジションの状態遷移を適用する
    /// （本 issue の核心。`text-input-state-model.md` §4.3〜§4.6 の配線）。
    ///
    /// フォーカスガード（§4.3・§4.5 手順 0・不変条件 8）: `interaction.focused == false`
    /// の間に届いた `Preedit` / `Commit` は破棄し、`buffer` / `cursor` / `dirty` /
    /// `composition` を一切変更しない。`Disabled` のみこのガードの対象外（フォーカス
    /// 状態によらず `composition` を必ずクリアする、§4.6）。
    ///
    /// `ime_purpose_switch_guard`（Bugbot #457 指摘 2 対応、フィールド doc・
    /// `set_ime_purpose` の doc 参照）: `Password` へのフォーカス中切替で破棄した
    /// preedit の遅延到着を捕まえるワンショットガード。デッドロック（ガードが
    /// 解除されず以降の正当な入力まで飲み込む）を避けるため、届いた `ImeEvent` の
    /// 種類を問わず**最初の 1 件で必ず解除する**（`Enabled` は素通し、`Disabled` は
    /// 元々の処理のみ、`Preedit` は新しいコンポジション開始とみなして素通し、
    /// `Commit` のみ「破棄すべき遅延応答」として飲み込んで解除する）。
    pub fn apply_ime(&mut self, event: ImeEvent) -> Option<Event> {
        match event {
            // ime_enabled 観測値を true にする（S23 / #204、`ime_enabled` フィールドの
            // doc 参照）。プラットフォームによっては即時応答ではない（`ImeEvent::Enabled`
            // の doc 参照）。
            ImeEvent::Enabled => {
                self.ime_purpose_switch_guard = false;
                self.ime_enabled = true;
                None
            }
            ImeEvent::Preedit { text, cursor } => {
                if !self.interaction.focused {
                    // フォーカスガード（§4.3）: 非フォーカス中の Preedit は破棄する。
                    return None;
                }
                // 新しいコンポジションの開始は、直前の Password 切替で破棄した
                // preedit とは無関係な入力とみなしガードを解除する（指摘 2 対応）。
                self.ime_purpose_switch_guard = false;
                if text.is_empty() {
                    // 空 Preedit はコンポジションのクリアを表す契約（§4.3 表）。
                    self.composition = None;
                } else {
                    match &mut self.composition {
                        Some(composition) => {
                            // Composing 中は text/cursor のみ更新し、anchor は不変
                            // （§4.3、コンポジション開始時にのみ確定する値）。
                            composition.text = text;
                            composition.cursor = cursor;
                        }
                        None => {
                            // Idle → Composing への遷移。anchor は現在の確定カーソル位置
                            // で固定する（§4.2・§4.3）。
                            self.composition = Some(Composition {
                                text,
                                cursor,
                                anchor: self.cursor,
                            });
                        }
                    }
                }
                None
            }
            ImeEvent::Commit(text) => {
                if !self.interaction.focused {
                    // フォーカスガード（§4.5 手順 0）: 非フォーカス中の Commit は破棄する。
                    return None;
                }
                if self.ime_purpose_switch_guard {
                    // Bugbot #457 指摘 2: Password 切替で破棄した preedit の遅延 Commit
                    // （Windows の WM_IME_ENDCOMPOSITION 等）を 1 件だけ飲み込み、
                    // パスワードバッファへの混入を防ぐ。ワンショットのため即座に解除する。
                    self.ime_purpose_switch_guard = false;
                    return None;
                }
                // 手順 1: 既存 InsertText と同一の制御文字除去（OWASP A03 対策の継承）。
                let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
                if sanitized.is_empty() {
                    // 手順 2: 除去後が空なら挿入せず composition のみクリアする
                    // （dirty は変更しない、Bugbot #439 指摘対応・§4.5 手順 2）。
                    self.composition = None;
                    return None;
                }
                // 手順 3: composition が Some なら anchor、None なら現在の cursor へ挿入する。
                let insert_at = self.composition.as_ref().map_or(self.cursor, |c| c.anchor);
                let byte_index = byte_index_at(&self.buffer, insert_at);
                self.buffer.insert_str(byte_index, &sanitized);
                // 手順 4: カーソルを挿入した文字列の char 数だけ前進させ、後続文字との
                // 結合で生じた grapheme cluster の途中に落ちないよう境界へスナップする
                // （Bugbot #461 指摘対応）。
                let advanced = insert_at + sanitized.chars().count();
                self.cursor = snap_cursor_forward_to_grapheme_boundary(&self.buffer, advanced);
                // 手順 5: dirty を立て、既存 commit_if_changed 経路に乗せる
                // （新しい Event バリアントを追加しない、§4.5 手順 5）。
                self.dirty = true;
                // 手順 6: composition をクリアする。
                self.composition = None;
                None
            }
            ImeEvent::Disabled => {
                // §4.6: composition のクリアのみを行い、buffer/cursor/dirty は
                // 再適用・巻き戻ししない（フォーカス状態によらず冪等に適用できる）。
                // ime_enabled 観測値も false にする（S23 / #204）。
                self.composition = None;
                self.ime_enabled = false;
                // Bugbot #457 指摘 2: Disabled はセッション終了を意味し、以降に
                // 届く Commit はこの解除より後の新しいセッションのものとみなせる。
                self.ime_purpose_switch_guard = false;
                None
            }
        }
    }

    /// 現在の `composition` を表示合成する（`paint` / `measure` の共通経路）。
    ///
    /// `composition_display::CompositionDisplay::synthesize` へ委譲するのみで、
    /// `display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]`
    /// （`text-input-state-model.md` §4.4）を構築する。
    fn composition_display(&self, composition: &Composition) -> CompositionDisplay {
        CompositionDisplay::synthesize(
            &self.buffer,
            composition.anchor,
            &composition.text,
            composition.cursor,
        )
    }

    /// `buffer` がユーザー編集でダーティであり、かつ確定値と実際に異なる場合のみ
    /// `Event::TextFieldChanged` を返す（blur-commit・`Commit`・disable 前 commit の
    /// 共通処理）。
    ///
    /// `dirty` と値の差分比較の**両方**を必要とする。`dirty` のみで判定すると、
    /// 挿入して削除するなど正味の変化がない編集列でも `Commit` のたびに
    /// no-op イベントを発火し `StateStore::dispatch` の version が空更新で
    /// 進んでしまう。差分比較のみで判定すると（旧実装）、`sync_from_state` が
    /// フォーカス中も `committed_value` を進める設計のため、外部由来の更新と
    /// ユーザー編集を区別できず stale blur-commit を起こす（`dirty` フィールドの
    /// doc 参照）。両方を要求することで両方の不整合を防ぐ。
    fn commit_if_changed(&mut self) -> Option<Event> {
        if !self.dirty || self.buffer == self.committed_value {
            // 編集済みでも確定値と一致した（挿入後に取り消す等）場合はダーティ状態を
            // 解消するのみで、no-op イベントは発火しない。
            self.dirty = false;
            return None;
        }
        self.dirty = false;
        self.committed_value = self.buffer.clone();
        Some(Event::TextFieldChanged {
            label: self.label.clone(),
            value: self.buffer.clone(),
        })
    }

    /// 編集入力を処理する（本 issue の核心）。
    ///
    /// disabled 中・非フォーカス中はすべての入力を無視する（フォーカス保持中のみ
    /// 編集可能という設計判断）。`Commit` は `Event::TextFieldChanged` を返し、
    /// `Cancel` は buffer を確定値へ巻き戻してカーソルを末尾へ置く。
    ///
    /// IME 統合配線（`text-input-state-model.md` §4.7）: Composing 中
    /// （`composition.is_some()`）は編集入力（`InsertText` / `DeleteBackward` /
    /// `DeleteForward` / カーソル移動系）を no-op とする（IME がキー入力を占有する
    /// ため、プラットフォーム層の実装差異に対する防御）。`Commit` / `Cancel` は
    /// 既存処理をそのまま実行するが、**既存処理より先に `composition` をクリアする**
    /// （クリア → 既存処理の順序。`anchor` が新しい `buffer` の意図しない位置を
    /// 指し続けることを防ぐ、Bugbot #439 指摘対応）。
    pub fn handle_input(&mut self, input: TextFieldInput) -> Option<Event> {
        if self.interaction.disabled || !self.interaction.focused {
            return None;
        }

        if self.composition.is_some() {
            match input {
                TextFieldInput::Commit | TextFieldInput::Cancel => {
                    self.composition = None;
                }
                _ => return None,
            }
        }

        match input {
            TextFieldInput::InsertText(text) => {
                let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
                if sanitized.is_empty() {
                    return None;
                }
                let byte_index = byte_index_at(&self.buffer, self.cursor);
                self.buffer.insert_str(byte_index, &sanitized);
                // 挿入した char 数だけ前進させ、後続文字との結合で生じた grapheme
                // cluster の途中に落ちないよう境界へスナップする（Bugbot #461 指摘
                // 対応・TASK-8.5-S26 / #207）。
                let advanced = self.cursor + sanitized.chars().count();
                self.cursor = snap_cursor_forward_to_grapheme_boundary(&self.buffer, advanced);
                self.dirty = true;
                None
            }
            TextFieldInput::DeleteBackward => {
                if self.cursor == 0 {
                    return None;
                }
                // TASK-8.5-S26（#207）: char 1 個ではなく直前の grapheme cluster
                // 全体を削除する（肌色修飾子つき絵文字・ZWJ 連結絵文字・国旗等の
                // 分断防止、text_selection.rs のモジュール doc 参照）。
                let boundary = prev_grapheme_boundary(&self.buffer, self.cursor);
                let start = byte_index_at(&self.buffer, boundary);
                let end = byte_index_at(&self.buffer, self.cursor);
                self.buffer.replace_range(start..end, "");
                self.cursor = boundary;
                self.dirty = true;
                None
            }
            TextFieldInput::DeleteForward => {
                let char_count = self.buffer.chars().count();
                if self.cursor >= char_count {
                    return None;
                }
                // TASK-8.5-S26（#207）: 直後の grapheme cluster 全体を削除する
                // （`DeleteBackward` と対称の是正）。
                let boundary = next_grapheme_boundary(&self.buffer, self.cursor);
                let start = byte_index_at(&self.buffer, self.cursor);
                let end = byte_index_at(&self.buffer, boundary);
                self.buffer.replace_range(start..end, "");
                self.dirty = true;
                None
            }
            TextFieldInput::MoveCursorLeft => {
                // TASK-8.5-S26（#207）: char 1 個ではなく直前の grapheme cluster
                // 境界へ移動する（カーソルが grapheme 内部に立たない不変条件）。
                self.cursor = prev_grapheme_boundary(&self.buffer, self.cursor);
                None
            }
            TextFieldInput::MoveCursorRight => {
                // TASK-8.5-S26（#207）: 直後の grapheme cluster 境界へ移動する。
                self.cursor = next_grapheme_boundary(&self.buffer, self.cursor);
                None
            }
            TextFieldInput::MoveCursorToStart => {
                self.cursor = 0;
                None
            }
            TextFieldInput::MoveCursorToEnd => {
                self.cursor = self.buffer.chars().count();
                None
            }
            TextFieldInput::Commit => self.commit_if_changed(),
            TextFieldInput::Cancel => {
                self.buffer = self.committed_value.clone();
                self.cursor = self.buffer.chars().count();
                self.dirty = false;
                None
            }
        }
    }
}

impl<S> std::fmt::Debug for TextFieldWidget<S> {
    /// `label.rs` の `LabelWidget` と同じ理由（derive が `S: Debug` の不要な境界を
    /// 課すのを避けるため）で手動実装する。**buffer / committed_value の内容は出力せず、
    /// 文字数のみ記録する**（テキストフィールドが機微情報を保持しうるための防御、
    /// モジュール doc 参照）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextFieldWidget")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("buffer_len", &self.buffer.chars().count())
            .field("committed_value_len", &self.committed_value.chars().count())
            .field("cursor", &self.cursor)
            .field("dirty", &self.dirty)
            .field("interaction", &self.interaction)
            .field("has_value_source", &self.value_source.is_some())
            // composition は文字数のみを出力する Composition 自身の Debug に委譲する
            // （未確定文字列の内容を漏らさない、機微情報の露出防止）。
            .field("composition", &self.composition)
            .field("pending_ime_requests_len", &self.pending_ime_requests.len())
            .field("ime_enabled", &self.ime_enabled)
            .field("ime_purpose", &self.ime_purpose)
            .field("ime_purpose_switch_guard", &self.ime_purpose_switch_guard)
            .finish()
    }
}

impl<S> Clone for TextFieldWidget<S> {
    /// `label.rs` の `LabelWidget` と同じ理由で手動実装する（derive が `S: Clone` の
    /// 不要な境界を課すのを避ける。関数ポインタは `Copy` であり複製にコストがかからない）。
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            label: self.label.clone(),
            value_source: self.value_source,
            committed_value: self.committed_value.clone(),
            buffer: self.buffer.clone(),
            cursor: self.cursor,
            dirty: self.dirty,
            interaction: self.interaction,
            composition: self.composition.clone(),
            pending_ime_requests: self.pending_ime_requests.clone(),
            ime_enabled: self.ime_enabled,
            ime_purpose: self.ime_purpose,
            ime_purpose_switch_guard: self.ime_purpose_switch_guard,
        }
    }
}

impl<S: 'static> Widget<S> for TextFieldWidget<S> {
    fn id(&self) -> WidgetId {
        self.id
    }

    fn sync_from_state(&mut self, state: &S) {
        let Some(read) = self.value_source else {
            return;
        };
        let new_value = read(state);
        self.committed_value = new_value.clone();
        // buffer を追随させるのは「非編集中」または「フォーカス中だがユーザーが
        // まだ何も編集していない（!dirty）」場合のみ。ユーザー編集済み（dirty）の
        // buffer は上書きしない（ユーザー入力を状態同期で破壊しない、本 issue の
        // 設計判断）。逆に dirty でなければ、フォーカス中でも安全に外部更新へ
        // 追随させることで、無編集フィールドが古い buffer を保持し続け
        // stale blur-commit を起こす不整合を防ぐ（Bugbot 指摘 #362 review comment
        // 3683619206 対応、`dirty` フィールドの doc 参照）。
        //
        // `composition.is_some()`（Composing 中）の間は `dirty` の値によらず
        // `buffer` / `cursor` を変更しない（`text-input-state-model.md` §4.9、
        // Bugbot #439 指摘対応）。`ImeEvent::Preedit` 自体は `dirty` を立てないため、
        // この条件がないと固定された `anchor` が外部 sync 後の新しい `buffer` の
        // 意図しない位置を指し、表示合成（§4.4）が破壊されるおそれがある。
        if (!self.interaction.focused || !self.dirty) && self.composition.is_none() {
            self.dirty = false;
            // `new_value` が現在の buffer と同一なら書き換えない。カーソル移動
            // （`MoveCursorLeft` 等）は `dirty` を立てないため、無条件に
            // `cursor = len` へ飛ばすとフォーカス中にカーソルを中央へ置いた
            // ユーザーが、値の変わらない reconcile のたびにカーソル位置を
            // 失ってしまう（advisor 指摘の再発防止）。
            if new_value != self.buffer {
                self.buffer = new_value;
                self.cursor = self.buffer.chars().count();
            }
        }
    }

    fn measure(&self, available: Measurement) -> Measurement {
        // 暫定計測（S13 が正式なテキストレイアウトへ置き換える想定、`LabelWidget` を踏襲）。
        // Composing 中は表示合成後（preedit 込み）の char 数で暫定計測する
        // （preedit 分の幅を反映、`text-input-state-model.md` §4.4）。
        let char_count = match &self.composition {
            Some(composition) => self
                .composition_display(composition)
                .display_string()
                .chars()
                .count(),
            None => self.buffer.chars().count(),
        };
        let approx_width = (char_count as f32) * 8.0;
        Measurement {
            width: approx_width.min(available.width),
            height: available.height.min(16.0),
        }
    }

    fn paint(&self, _origin: (f32, f32), _size: Measurement) -> Vec<PaintCommand> {
        // Composing 中は CompositionDisplay 経由で PaintCommand::CompositionText を
        // 出力する（`composition_display.rs` モジュール doc が S8 に委ねていた配線、
        // `text-input-state-model.md` §4.4）。Idle 時は従来の PaintCommand::Text を維持する。
        match &self.composition {
            Some(composition) => vec![
                PaintCommand::Rect,
                self.composition_display(composition).into_paint_command(),
            ],
            None => vec![PaintCommand::Rect, PaintCommand::Text(self.buffer.clone())],
        }
    }

    fn hit_test(&self, point: (f32, f32), origin: (f32, f32), size: Measurement) -> bool {
        // 設計判断: テキストフィールドはラベルと異なりインタラクティブ要素であり、
        // 矩形内判定を行う（フォーカス取得のためのポインタターゲットとして機能する）。
        // 右端・下端は半開区間 `[origin, origin + size)` として扱う
        // （`Rect::contains` / `hit_test::hit_test` が採用する共通のヒットジオメトリ規約に
        // 揃え、隣接するウィジェットが共有エッジ上の点を両方とも自分のものと
        // 判定してしまう不整合を防ぐ、Bugbot 指摘 #362 review comment 3683619239 対応）。
        //
        // `interaction.disabled` の間は `false` を返し、無効化されたテキストフィールドを
        // ポインタ当たり判定の対象から外す（兄弟実装 `TextInput::hit_test` と同じ設計）。
        // これを怠ると `handle_input` / `apply_input` 側で無効化入力を無視していても、
        // hit_test の時点で下に重なる他ウィジェットへのヒットを奪ってしまう
        // （Bugbot 指摘 #362 review, disabled field still receives hits, 対応）。
        if self.interaction.disabled {
            return false;
        }
        point.0 >= origin.0
            && point.0 < origin.0 + size.width
            && point.1 >= origin.1
            && point.1 < origin.1 + size.height
    }

    fn accessibility_role(&self) -> AccessibilityRole {
        AccessibilityRole::TextInput
    }

    /// 入力欄のラベル文言をアクセシブルネームとして公開する（TASK-9.1-S2、REQ-9）。
    fn accessibility_label(&self) -> Option<String> {
        Some(self.label.clone())
    }

    fn interaction_state(&self) -> InteractionState {
        self.interaction
    }

    fn clone_box(&self) -> Box<dyn Widget<S>> {
        Box::new(self.clone())
    }

    /// トークン解決込みの描画列（TASK-8.4-S14）。Input カテゴリの既定マッピング
    /// （`style_tokens::input_style_refs`）を用い、`paint()` と同じ構造を保った
    /// まま色・タイポグラフィのみを解決済み具体値へ差し替える。解決失敗時は
    /// `paint()` の無スタイル列へフォールバックする。
    fn paint_styled(
        &self,
        origin: (f32, f32),
        size: Measurement,
        resolver: &crate::token::TokenResolver,
    ) -> Vec<PaintCommand> {
        let Ok(refs) = super::style_tokens::input_style_refs() else {
            return self.paint(origin, size);
        };
        let Ok(resolved) = super::style_tokens::resolve_widget_style(
            resolver,
            &refs,
            self.interaction_state().visual_state(),
        ) else {
            return self.paint(origin, size);
        };
        match super::style_tokens::style_paint_commands(self.paint(origin, size), &resolved) {
            Some(styled) => styled,
            None => self.paint(origin, size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateStore;
    use crate::ui::Screen;

    /// テスト専用の状態型。動的な確定値読み取り・`StateStore` 結合を検証するために使う。
    struct FormState {
        name: String,
    }

    fn read_name(state: &FormState) -> String {
        state.name.clone()
    }

    fn set_name(state: &mut FormState, value: String) {
        state.name = value;
    }

    fn focus(field: &mut TextFieldWidget<FormState>) {
        field.apply_input(InteractionInput::FocusGained);
    }

    #[test]
    fn insert_text_moves_cursor_and_updates_buffer_with_multibyte_chars() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(1), "greeting", "");
        focus(&mut field);

        field.handle_input(TextFieldInput::InsertText("こんにちは".to_string()));

        assert_eq!(field.buffer(), "こんにちは");
        assert_eq!(field.cursor(), 5);
    }

    #[test]
    fn insert_text_strips_control_characters() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(2), "line", "");
        focus(&mut field);

        field.handle_input(TextFieldInput::InsertText("a\nb\tc".to_string()));

        assert_eq!(field.buffer(), "abc");
    }

    #[test]
    fn insert_at_cursor_position_inserts_in_the_middle() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(3), "greeting", "こんにちは");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);
        field.handle_input(TextFieldInput::MoveCursorRight);

        field.handle_input(TextFieldInput::InsertText("!".to_string()));

        assert_eq!(field.buffer(), "こん!にちは");
        assert_eq!(field.cursor(), 3);
    }

    #[test]
    fn delete_backward_and_forward_operate_on_char_boundaries() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(4), "greeting", "こんにちは");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);
        field.handle_input(TextFieldInput::MoveCursorRight);

        field.handle_input(TextFieldInput::DeleteBackward);
        assert_eq!(field.buffer(), "こにちは");
        assert_eq!(field.cursor(), 1);

        field.handle_input(TextFieldInput::DeleteForward);
        assert_eq!(field.buffer(), "こちは");
        assert_eq!(field.cursor(), 1);
    }

    // --- TASK-8.5-S26（#207）: grapheme cluster 単位の編集操作 ---

    #[test]
    fn delete_backward_removes_whole_skin_tone_modifier_emoji_cluster() {
        // 👍🏽 = 👍 + 肌色修飾子の 2 char。DeleteBackward 1 回でクラスタ全体が
        // 消え、修飾子だけが残って分断されないことを確認する（S8/S23 申し送りの是正）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(40), "emoji", "a👍🏽b");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToEnd);
        field.handle_input(TextFieldInput::MoveCursorLeft); // 'b' の前へ

        field.handle_input(TextFieldInput::DeleteBackward);

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn delete_forward_removes_whole_zwj_family_emoji_cluster() {
        // 👨‍👩‍👧‍👦 = 4 人 + ZWJ 3 個 = 7 char で 1 クラスタ。DeleteForward 1 回で
        // クラスタ全体が消えることを確認する。
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(41), "emoji", format!("a{family}b"));
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight); // family の先頭へ

        field.handle_input(TextFieldInput::DeleteForward);

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn delete_backward_removes_whole_regional_indicator_flag_cluster() {
        // 🇯🇵 = Regional Indicator 2 個で 1 クラスタ。片割れだけが残らないことを確認する。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(42), "emoji", "a🇯🇵b");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToEnd);
        field.handle_input(TextFieldInput::MoveCursorLeft);

        field.handle_input(TextFieldInput::DeleteBackward);

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn delete_backward_removes_whole_combining_accent_cluster() {
        // 'e' + U+0301（結合鋭アクセント）= é の 2 char で 1 クラスタ。基底文字と
        // 結合記号が分離しないことを確認する。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(43), "combining", "a\u{65}\u{301}b");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToEnd);
        field.handle_input(TextFieldInput::MoveCursorLeft);

        field.handle_input(TextFieldInput::DeleteBackward);

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn move_cursor_left_and_right_skip_whole_emoji_cluster_at_once() {
        // 👍🏽 を挟んだカーソル移動は 1 回で通過する（クラスタ内部にカーソルが
        // 立たない不変条件）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(44), "emoji", "a👍🏽b");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);

        field.handle_input(TextFieldInput::MoveCursorRight); // 'a' の後ろ（クラスタ直前）
        assert_eq!(field.cursor(), 1);
        field.handle_input(TextFieldInput::MoveCursorRight); // クラスタ全体を通過
        assert_eq!(field.cursor(), 3);
        field.handle_input(TextFieldInput::MoveCursorLeft); // クラスタ全体を逆方向に通過
        assert_eq!(field.cursor(), 1);
    }

    #[test]
    fn insert_text_preserves_zwj_and_variation_selector_in_emoji_sequences() {
        // ZWJ（U+200D）・異体字セレクタ（U+FE0F）は Unicode カテゴリ Cf であり
        // `char::is_control`（Cc のみ）が対象とする制御文字ではない。既存の
        // 制御文字除去（改行・タブ等）が絵文字合成列を破壊しないことを固定化する。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(45), "emoji", "");
        focus(&mut field);

        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        field.handle_input(TextFieldInput::InsertText(family.to_string()));
        assert_eq!(field.buffer(), family);

        field.handle_input(TextFieldInput::InsertText("\u{2764}\u{fe0f}".to_string()));
        assert_eq!(field.buffer(), format!("{family}\u{2764}\u{fe0f}"));
    }

    #[test]
    fn insert_text_snaps_cursor_past_grapheme_cluster_formed_with_following_char() {
        // Bugbot #461 指摘の固定化テスト。カーソル位置 0 の直後には結合文字
        // （U+0301 COMBINING ACUTE ACCENT）+ "x" が続く状態で "e" を挿入すると、
        // "e" + U+0301 が 1 個の grapheme cluster（"é" 相当）を形成する。素朴な
        // char 数前進ではカーソルがそのクラスタの途中（char index 1）に落ちるが、
        // 本修正により境界（char index 2）へスナップされることを検証する。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(46), "combining", "\u{0301}x");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);

        field.handle_input(TextFieldInput::InsertText("e".to_string()));

        assert_eq!(field.buffer(), "e\u{0301}x");
        // クラスタ途中の char index 1 ではなく、境界である 2 に位置すること。
        assert_eq!(field.cursor(), 2);

        // 不変条件が保たれていれば DeleteBackward はクラスタ全体を削除し、
        // 修飾子だけが宙に浮いて残ることはない。
        field.handle_input(TextFieldInput::DeleteBackward);
        assert_eq!(field.buffer(), "x");
    }

    #[test]
    fn ime_commit_snaps_cursor_past_grapheme_cluster_formed_with_following_char() {
        // 上記テストの `ImeEvent::Commit` 版（#207 の指摘箇所 L587-588）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(47), "combining", "\u{0301}x");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);

        field.apply_ime(ImeEvent::Commit("e".to_string()));

        assert_eq!(field.buffer(), "e\u{0301}x");
        assert_eq!(field.cursor(), 2);

        field.handle_input(TextFieldInput::DeleteBackward);
        assert_eq!(field.buffer(), "x");
    }

    #[test]
    fn delete_at_boundaries_is_no_op_and_does_not_panic() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(5), "greeting", "あ");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);

        field.handle_input(TextFieldInput::DeleteBackward);
        assert_eq!(field.buffer(), "あ");

        field.handle_input(TextFieldInput::MoveCursorToEnd);
        field.handle_input(TextFieldInput::DeleteForward);
        assert_eq!(field.buffer(), "あ");
    }

    #[test]
    fn move_cursor_clamps_at_boundaries() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(6), "greeting", "ab");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);

        field.handle_input(TextFieldInput::MoveCursorLeft);
        assert_eq!(field.cursor(), 0);

        field.handle_input(TextFieldInput::MoveCursorToEnd);
        field.handle_input(TextFieldInput::MoveCursorRight);
        assert_eq!(field.cursor(), 2);
    }

    #[test]
    fn disabled_field_ignores_edit_input() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(7), "greeting", "ab");
        focus(&mut field);
        let event = field.set_disabled(true);

        assert_eq!(event, None);
        field.handle_input(TextFieldInput::InsertText("x".to_string()));

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn disable_while_focused_commits_dirty_edit_before_dropping_focus() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(25), "greeting", "ab");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("x".to_string()));

        let event = field.set_disabled(true);

        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "abx".to_string(),
            })
        );
        assert_eq!(field.committed_value(), "abx");
        assert!(!field.interaction_state().focused);
        assert!(field.interaction_state().disabled);

        // 直後の FocusLost は was_focused == false のため何も発火しない
        // （disable 時点で既に commit 済みのため、二重発火しないのが正しい）。
        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(event, None);
    }

    #[test]
    fn unfocused_field_ignores_edit_input() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(8), "greeting", "ab");

        field.handle_input(TextFieldInput::InsertText("x".to_string()));

        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn commit_returns_text_field_changed_event() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(9), "greeting", "");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("太郎".to_string()));

        let event = field.handle_input(TextFieldInput::Commit);

        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "太郎".to_string(),
            })
        );
        assert_eq!(field.committed_value(), "太郎");
    }

    #[test]
    fn commit_without_change_returns_none() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(10), "greeting", "太郎");
        focus(&mut field);

        let event = field.handle_input(TextFieldInput::Commit);

        assert_eq!(event, None);
    }

    #[test]
    fn commit_after_edit_that_nets_back_to_committed_value_returns_none() {
        // dirty のみで判定すると、挿入して打ち消す等の正味の変化がない編集列でも
        // Commit のたびに no-op イベントを発火してしまう（advisor 指摘の再発防止。
        // `commit_if_changed` は `dirty` と値の差分比較の両方を要求する）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(29), "greeting", "ab");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("x".to_string()));
        field.handle_input(TextFieldInput::DeleteBackward);
        assert_eq!(field.buffer(), "ab");

        let event = field.handle_input(TextFieldInput::Commit);
        assert_eq!(event, None);

        // Commit 後は dirty も解消されているため、直後の blur も発火しない。
        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(event, None);
    }

    #[test]
    fn cancel_reverts_buffer_to_committed_value() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(11), "greeting", "太郎");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("さん".to_string()));

        let event = field.handle_input(TextFieldInput::Cancel);

        assert_eq!(event, None);
        assert_eq!(field.buffer(), "太郎");
        assert_eq!(field.cursor(), 2);
    }

    #[test]
    fn focus_lost_triggers_blur_commit_when_buffer_changed() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(12), "greeting", "");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("花子".to_string()));

        let event = field.apply_input(InteractionInput::FocusLost);

        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "花子".to_string(),
            })
        );
        assert_eq!(field.committed_value(), "花子");
    }

    #[test]
    fn focus_lost_without_change_returns_none() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(13), "greeting", "花子");
        focus(&mut field);

        let event = field.apply_input(InteractionInput::FocusLost);

        assert_eq!(event, None);
    }

    #[test]
    fn focus_gained_places_cursor_at_end() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(14), "greeting", "ab");

        field.apply_input(InteractionInput::FocusGained);

        assert_eq!(field.cursor(), 2);
    }

    #[test]
    fn disabled_field_ignores_focus_transitions_and_never_blur_commits() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(15), "greeting", "");
        let event = field.set_disabled(true);
        assert_eq!(event, None);

        focus(&mut field);
        assert!(!field.interaction_state().focused);

        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(event, None);
    }

    #[test]
    fn sync_from_state_static_field_is_no_op() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(16), "greeting", "固定値");
        let state = FormState {
            name: "無視される".to_string(),
        };

        field.sync_from_state(&state);

        assert_eq!(field.buffer(), "固定値");
        assert_eq!(field.committed_value(), "固定値");
    }

    #[test]
    fn sync_from_state_dynamic_field_tracks_state_when_unfocused() {
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(17), "greeting", "", read_name);
        let state = FormState {
            name: "一郎".to_string(),
        };

        field.sync_from_state(&state);

        assert_eq!(field.buffer(), "一郎");
        assert_eq!(field.committed_value(), "一郎");
    }

    #[test]
    fn sync_from_state_does_not_overwrite_buffer_while_focused() {
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(18), "greeting", "", read_name);
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("編集中".to_string()));

        let state = FormState {
            name: "外部更新".to_string(),
        };
        field.sync_from_state(&state);

        // 編集中の buffer はユーザー入力を優先し、状態同期で上書きしない。
        assert_eq!(field.buffer(), "編集中");
        // committed_value は reconcile 対象のため追随する。
        assert_eq!(field.committed_value(), "外部更新");
    }

    #[test]
    fn sync_from_state_while_focused_but_not_dirty_tracks_external_update() {
        // フォーカス中でもユーザーがまだ何も編集していない（dirty == false）場合は
        // 外部更新へ追随してよい（Bugbot 指摘 #362 review comment 3683619206 の
        // 再発防止テスト）。
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(26), "greeting", "", read_name);
        focus(&mut field);

        let state = FormState {
            name: "外部更新".to_string(),
        };
        field.sync_from_state(&state);

        assert_eq!(field.buffer(), "外部更新");
        assert_eq!(field.committed_value(), "外部更新");
    }

    #[test]
    fn stale_blur_commit_does_not_overwrite_external_update_when_not_dirty() {
        // 修正前の再現手順: フォーカス中に何も編集せず（dirty == false）、
        // sync_from_state で committed_value のみが外部更新へ進んだ状態で
        // FocusLost が来ても、古い buffer で TextFieldChanged を発火して
        // 外部更新を上書きしてはならない（Bugbot 指摘 #362 review comment
        // 3683619206 の再発防止テスト）。
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(27), "greeting", "初期値", read_name);
        focus(&mut field);
        // 未編集のままフォーカス中に外部状態が更新される（dirty は false のまま）。
        let state = FormState {
            name: "外部更新".to_string(),
        };
        field.sync_from_state(&state);
        assert_eq!(field.buffer(), "外部更新");

        let event = field.apply_input(InteractionInput::FocusLost);

        assert_eq!(event, None);
        assert_eq!(field.committed_value(), "外部更新");
    }

    #[test]
    fn state_store_dispatch_applies_commit_event_through_declarative_node() {
        let screen: Screen<FormState> =
            Screen::new("form").text_field("greeting", read_name, set_name);
        let mut store = StateStore::new(FormState {
            name: String::new(),
        });
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(19), "greeting", "");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("次郎".to_string()));

        let event = field
            .handle_input(TextFieldInput::Commit)
            .expect("編集内容が変更されているため Commit は Some を返す");
        let handled = store.dispatch(&screen, &event);

        assert!(handled);
        assert_eq!(store.state().name, "次郎");
        assert_eq!(store.version(), 1);
    }

    #[test]
    fn hit_test_is_true_inside_rect_and_false_outside() {
        let field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(20), "greeting", "");
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(field.hit_test((5.0, 5.0), (0.0, 0.0), size));
        assert!(!field.hit_test((-10.0, -10.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_returns_false_when_disabled_even_if_point_inside_rect() {
        // Bugbot 指摘 #362 review（disabled field still receives hits）の再発防止テスト。
        // 無効化中は矩形内の点でも hit_test が false を返し、下に重なる別ウィジェットへ
        // ヒットを譲る（兄弟実装 `TextInput` と同じ設計）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(20), "greeting", "");
        field.set_disabled(true);
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(!field.hit_test((5.0, 5.0), (0.0, 0.0), size));
    }

    #[test]
    fn hit_test_excludes_right_and_bottom_closed_edge() {
        // `Rect::contains`（`hit_test::hit_test` が使う共通のヒットジオメトリ）は
        // 半開区間 `[origin, origin + size)` のため、本ウィジェットの矩形内判定も
        // 右端・下端を含めてはならない（Bugbot 指摘 #362 review comment 3683619239
        // の再発防止テスト。含めると隣接ウィジェットが共有エッジ上の点を
        // 両方とも自分のものと判定してしまう）。
        let field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(28), "greeting", "");
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert!(!field.hit_test((100.0, 5.0), (0.0, 0.0), size));
        assert!(!field.hit_test((5.0, 20.0), (0.0, 0.0), size));
        assert!(field.hit_test((99.9, 19.9), (0.0, 0.0), size));
    }

    #[test]
    fn accessibility_role_is_text_input() {
        let field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(21), "greeting", "");

        assert_eq!(field.accessibility_role(), AccessibilityRole::TextInput);
    }

    #[test]
    fn accessibility_label_is_field_label() {
        let field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(21), "greeting", "");

        assert_eq!(field.accessibility_label(), Some("greeting".to_string()));
    }

    #[test]
    fn clone_box_preserves_state() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(22), "greeting", "");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("三郎".to_string()));

        let cloned: Box<dyn Widget<FormState>> = field.clone_box();

        assert_eq!(cloned.id(), WidgetId::from_raw(22));
        let commands = cloned.paint(
            (0.0, 0.0),
            Measurement {
                width: 10.0,
                height: 10.0,
            },
        );
        assert_eq!(
            commands,
            vec![PaintCommand::Rect, PaintCommand::Text("三郎".to_string())]
        );
    }

    #[test]
    fn widget_trait_object_is_object_safe_and_boxable() {
        let widget: Box<dyn Widget<FormState>> = Box::new(TextFieldWidget::new_static(
            WidgetId::from_raw(23),
            "greeting",
            "ab",
        ));

        assert_eq!(widget.accessibility_role(), AccessibilityRole::TextInput);
        assert!(!widget.interaction_state().disabled);
    }

    #[test]
    fn debug_output_does_not_expose_buffer_content() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(24), "greeting", "秘密の値");
        focus(&mut field);

        let output = format!("{field:?}");

        assert!(output.contains("TextFieldWidget"));
        assert!(!output.contains("秘密の値"));
    }

    // TASK-8.4-S14: paint_styled のフォールバック契約。
    #[test]
    fn paint_styled_falls_back_to_paint_when_resolver_lacks_tokens() {
        let field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(200), "greeting", "hello");
        let empty_resolver = crate::token::TokenResolver::new(
            crate::token::TokenSet::new(),
            crate::token::TokenSet::new(),
        );
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        assert_eq!(
            field.paint_styled((0.0, 0.0), size, &empty_resolver),
            field.paint((0.0, 0.0), size)
        );
    }

    // TASK-8.5-S8（#189）: IME 統合配線のテスト。

    #[test]
    fn preedit_starts_composing_and_updates_in_place() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(300), "greeting", "ab");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);

        // Idle → Composing。anchor はこの遷移の瞬間のカーソル位置（1）で固定される。
        let event = field.apply_ime(ImeEvent::Preedit {
            text: "か".to_string(),
            cursor: Some((0, 1)),
        });
        assert_eq!(event, None);
        assert_eq!(field.composition.as_ref().unwrap().anchor, 1);
        assert_eq!(field.composition.as_ref().unwrap().text, "か");
        assert_eq!(field.buffer(), "ab", "確定バッファには混入しない");

        // Composing 中の更新は text/cursor のみで anchor は不変。
        field.apply_ime(ImeEvent::Preedit {
            text: "かな".to_string(),
            cursor: Some((0, 2)),
        });
        assert_eq!(field.composition.as_ref().unwrap().anchor, 1);
        assert_eq!(field.composition.as_ref().unwrap().text, "かな");

        // 空 Preedit でクリア(Idle へ戻る)。
        field.apply_ime(ImeEvent::Preedit {
            text: String::new(),
            cursor: None,
        });
        assert!(field.composition.is_none());
    }

    #[test]
    fn commit_inserts_at_anchor_and_advances_cursor_then_blur_commits() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(301), "greeting", "こんにちは");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);
        field.handle_input(TextFieldInput::MoveCursorRight);

        field.apply_ime(ImeEvent::Preedit {
            text: "かな".to_string(),
            cursor: Some((0, 2)),
        });

        let event = field.apply_ime(ImeEvent::Commit("かな".to_string()));
        assert_eq!(event, None, "Commit 自体は Event を返さず dirty のみ立てる");
        assert_eq!(field.buffer(), "こんかなにちは");
        assert_eq!(field.cursor(), 4);
        assert!(field.composition.is_none());

        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "こんかなにちは".to_string(),
            })
        );
    }

    #[test]
    fn commit_of_control_characters_only_is_no_op_and_clears_composition() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(302), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });

        let event = field.apply_ime(ImeEvent::Commit("\n\t".to_string()));

        assert_eq!(event, None);
        assert_eq!(field.buffer(), "ab", "制御文字のみの確定は挿入しない");
        assert!(field.composition.is_none());

        // dirty は立っていないため blur でも発火しない。
        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(event, None);
    }

    #[test]
    fn preedit_and_commit_are_discarded_while_unfocused() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(303), "greeting", "ab");

        let event = field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        assert_eq!(event, None);
        assert!(field.composition.is_none());

        let event = field.apply_ime(ImeEvent::Commit("y".to_string()));
        assert_eq!(event, None);
        assert_eq!(field.buffer(), "ab");
        assert!(!field.dirty);
    }

    #[test]
    fn disabled_event_clears_composition_regardless_of_focus_and_is_idempotent() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(304), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        assert!(field.composition.is_some());

        let event = field.apply_ime(ImeEvent::Disabled);
        assert_eq!(event, None);
        assert!(field.composition.is_none());
        assert_eq!(field.buffer(), "ab", "buffer は巻き戻さない");

        // 冪等（非フォーカス中でも安全に重ねて適用できる）。
        field.apply_input(InteractionInput::FocusLost);
        let event = field.apply_ime(ImeEvent::Disabled);
        assert_eq!(event, None);
    }

    #[test]
    fn disabled_after_commit_does_not_reapply_or_revert_buffer() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(305), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        field.apply_ime(ImeEvent::Commit("x".to_string()));
        assert_eq!(field.buffer(), "abx");

        let event = field.apply_ime(ImeEvent::Disabled);
        assert_eq!(event, None);
        assert_eq!(
            field.buffer(),
            "abx",
            "既に適用済みの Commit を再適用・巻き戻ししない"
        );
    }

    #[test]
    fn focus_lost_discards_composition_before_blur_commit_and_requests_disable() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(306), "greeting", "ab");
        focus(&mut field);
        field.take_ime_requests();
        field.handle_input(TextFieldInput::InsertText("x".to_string()));
        field.apply_ime(ImeEvent::Preedit {
            text: "y".to_string(),
            cursor: None,
        });
        assert!(field.composition.is_some());

        let event = field.apply_input(InteractionInput::FocusLost);

        assert!(field.composition.is_none(), "preedit は確定せず破棄する");
        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "abx".to_string(),
            }),
            "preedit を含まない blur-commit のみ発火する"
        );
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetEnabled(false)]
        );
    }

    #[test]
    fn focus_gained_requests_purpose_then_enable_and_requests_drain() {
        // S23 / #204: FocusGained は SetPurpose(現在の purpose) → SetEnabled(true) の
        // 順で要求する（既定 purpose は Normal のため SetEnabled(true)）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(307), "greeting", "ab");

        field.apply_input(InteractionInput::FocusGained);

        assert_eq!(
            field.take_ime_requests(),
            vec![
                ImeRequest::SetPurpose(ImePurpose::Normal),
                ImeRequest::SetEnabled(true),
            ]
        );
        // drain 後は空になる。
        assert_eq!(field.take_ime_requests(), Vec::new());
    }

    #[test]
    fn set_disabled_while_focused_discards_composition_and_requests_disable() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(308), "greeting", "ab");
        focus(&mut field);
        field.take_ime_requests();
        field.apply_ime(ImeEvent::Preedit {
            text: "y".to_string(),
            cursor: None,
        });

        let event = field.set_disabled(true);

        assert_eq!(event, None);
        assert!(field.composition.is_none());
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetEnabled(false)]
        );
    }

    #[test]
    fn composing_ignores_edit_input_and_clears_before_commit_or_cancel() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(309), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });

        // Composing 中の編集入力は no-op。
        field.handle_input(TextFieldInput::InsertText("z".to_string()));
        field.handle_input(TextFieldInput::DeleteBackward);
        field.handle_input(TextFieldInput::MoveCursorLeft);
        assert_eq!(field.buffer(), "ab");
        assert!(field.composition.is_some());

        // Cancel はまず composition をクリアしてから既存処理を実行する。
        field.handle_input(TextFieldInput::Cancel);
        assert!(field.composition.is_none());
        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn composing_commit_input_clears_composition_before_existing_commit_logic() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(310), "greeting", "");
        focus(&mut field);
        field.handle_input(TextFieldInput::InsertText("太郎".to_string()));
        field.apply_ime(ImeEvent::Preedit {
            text: "さん".to_string(),
            cursor: None,
        });

        let event = field.handle_input(TextFieldInput::Commit);

        assert!(field.composition.is_none());
        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "太郎".to_string(),
            }),
            "TextFieldInput::Commit は既存の確定バッファのみを対象とする"
        );
    }

    #[test]
    fn sync_from_state_does_not_change_buffer_while_composing_even_if_not_dirty() {
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(311), "greeting", "", read_name);
        focus(&mut field);
        // dirty は立てず Composing のみ開始する（Preedit 自体は dirty を立てない）。
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        assert!(!field.dirty);

        let state = FormState {
            name: "外部更新".to_string(),
        };
        field.sync_from_state(&state);

        assert_eq!(field.buffer(), "", "Composing 中は buffer を変更しない");
        assert_eq!(
            field.committed_value(),
            "外部更新",
            "committed_value の更新は継続する"
        );
    }

    #[test]
    fn paint_emits_composition_text_segments_while_composing_and_plain_text_when_idle() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(312), "greeting", "こんにちは");
        let size = Measurement {
            width: 100.0,
            height: 20.0,
        };

        // Idle 時は従来どおり PaintCommand::Text。
        assert_eq!(
            field.paint((0.0, 0.0), size),
            vec![
                PaintCommand::Rect,
                PaintCommand::Text("こんにちは".to_string())
            ]
        );

        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);
        field.handle_input(TextFieldInput::MoveCursorRight);
        field.apply_ime(ImeEvent::Preedit {
            text: "かな".to_string(),
            cursor: Some((0, 2)),
        });

        assert_eq!(
            field.paint((0.0, 0.0), size),
            vec![
                PaintCommand::Rect,
                PaintCommand::CompositionText {
                    before: "こん".to_string(),
                    preedit: "かな".to_string(),
                    after: "にちは".to_string(),
                    preedit_cursor: Some((0, 2)),
                }
            ]
        );
    }

    #[test]
    fn debug_output_does_not_expose_composition_content() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(313), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "秘密の未確定文字列".to_string(),
            cursor: None,
        });

        let output = format!("{field:?}");

        assert!(!output.contains("秘密の未確定文字列"));
    }

    // TASK-8.5-S23（#204）: IME オン/オフ・入力モード状態の公開 API のテスト。

    #[test]
    fn ime_enabled_tracks_enabled_and_disabled_events() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(400), "greeting", "");
        assert!(!field.ime_enabled(), "初期値は false");

        focus(&mut field);
        field.apply_ime(ImeEvent::Enabled);
        assert!(field.ime_enabled());

        field.apply_ime(ImeEvent::Disabled);
        assert!(!field.ime_enabled());

        // Windows 型の高頻度な Disabled → Enabled 再サイクルでも最終値が正しく追随する
        // （`ImeEvent::Enabled`/`Disabled` の doc 参照）。
        field.apply_ime(ImeEvent::Enabled);
        field.apply_ime(ImeEvent::Disabled);
        field.apply_ime(ImeEvent::Enabled);
        assert!(field.ime_enabled());
    }

    #[test]
    fn set_ime_purpose_queues_set_purpose_and_updates_getter() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(401), "greeting", "");
        assert_eq!(field.ime_purpose(), ImePurpose::Normal);

        field.set_ime_purpose(ImePurpose::Terminal);

        assert_eq!(field.ime_purpose(), ImePurpose::Terminal);
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetPurpose(ImePurpose::Terminal)]
        );
    }

    #[test]
    fn set_ime_purpose_with_same_value_does_not_queue_request() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(402), "greeting", "");

        field.set_ime_purpose(ImePurpose::Normal);

        assert_eq!(field.take_ime_requests(), Vec::new());
    }

    #[test]
    fn focus_gained_with_password_purpose_does_not_disable_ime() {
        // Bugbot #457 指摘 1 対応: SetEnabled は Android/iOS でソフトキーボードの
        // 表示/非表示に直結するため、Password フィールドでも SetEnabled(false) を
        // 発行しない（発行するとフォーカスした瞬間にキーボードが隠れて入力できなく
        // なる。モジュール doc「Password 時のポリシー」参照）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(403), "greeting", "");
        field.set_ime_purpose(ImePurpose::Password);
        field.take_ime_requests();

        field.apply_input(InteractionInput::FocusGained);

        assert_eq!(
            field.take_ime_requests(),
            vec![
                ImeRequest::SetPurpose(ImePurpose::Password),
                ImeRequest::SetEnabled(true),
            ]
        );
    }

    #[test]
    fn switching_to_password_while_focused_discards_composition_without_disabling_ime() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(404), "greeting", "ab");
        focus(&mut field);
        field.take_ime_requests();
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        assert!(field.is_composing());

        field.set_ime_purpose(ImePurpose::Password);

        assert!(
            !field.is_composing(),
            "Password 切替時に未確定 preedit を残さない"
        );
        assert_eq!(field.buffer(), "ab", "確定挿入は行わない");
        // Bugbot #457 指摘 1 対応: SetEnabled(false) はもう発行しない。
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetPurpose(ImePurpose::Password)]
        );
    }

    #[test]
    fn switching_back_from_password_while_focused_requests_no_extra_ime_toggle() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(405), "greeting", "ab");
        focus(&mut field);
        field.set_ime_purpose(ImePurpose::Password);
        field.take_ime_requests();

        field.set_ime_purpose(ImePurpose::Normal);

        // SetEnabled(false) を送っていないため、対称的な再有効化要求も不要
        // （Bugbot #457 指摘 1 対応）。
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetPurpose(ImePurpose::Normal)]
        );
    }

    #[test]
    fn late_commit_after_switching_to_password_is_discarded() {
        // Bugbot #457 指摘 2: フォーカス中に Password へ切り替えて preedit を破棄した
        // 直後、プラットフォームから遅延到着した Commit（Windows の
        // WM_IME_ENDCOMPOSITION 等）はパスワードバッファへ挿入されてはならない。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(409), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        field.set_ime_purpose(ImePurpose::Password);
        assert!(!field.is_composing());

        let event = field.apply_ime(ImeEvent::Commit("x".to_string()));

        assert_eq!(event, None);
        assert_eq!(
            field.buffer(),
            "ab",
            "破棄したはずの preedit が遅延 Commit で混入していない"
        );
    }

    #[test]
    fn commit_after_guard_consumed_still_inserts_normally() {
        // late_commit ガードはワンショットであり、以降の正当な Commit まで
        // 飲み込んでは（Bugbot 指摘 1 の再発を防ぐため）ならない。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(410), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        field.set_ime_purpose(ImePurpose::Password);
        field.apply_ime(ImeEvent::Commit("x".to_string()));
        assert_eq!(field.buffer(), "ab", "1 件目の遅延 Commit は破棄済み");

        let event = field.apply_ime(ImeEvent::Commit("y".to_string()));

        // `apply_ime` の `Commit` 腕自体は `Event::TextFieldChanged` を返さず、
        // buffer への確定挿入のみ行う契約（発火は別途 `TextFieldInput::Commit` /
        // blur 経由の `commit_if_changed` が担う）。
        assert_eq!(event, None);
        assert_eq!(
            field.buffer(),
            "aby",
            "ガード解除後の Commit は通常どおり挿入する"
        );
    }

    #[test]
    fn set_ime_purpose_while_unfocused_only_updates_purpose() {
        // フォーカス外では purpose の更新のみ行い、実際の有効/無効要求は次回
        // FocusGained（apply_input）が担う。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(406), "greeting", "");

        field.set_ime_purpose(ImePurpose::Password);

        assert_eq!(field.ime_purpose(), ImePurpose::Password);
        assert_eq!(
            field.take_ime_requests(),
            vec![ImeRequest::SetPurpose(ImePurpose::Password)]
        );
    }

    #[test]
    fn is_composing_reflects_preedit_state() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(407), "greeting", "");
        focus(&mut field);
        assert!(!field.is_composing());

        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        assert!(field.is_composing());

        field.apply_ime(ImeEvent::Commit("x".to_string()));
        assert!(
            !field.is_composing(),
            "Commit 後は composition がクリアされる"
        );

        field.apply_ime(ImeEvent::Preedit {
            text: "y".to_string(),
            cursor: None,
        });
        assert!(field.is_composing());
        field.apply_ime(ImeEvent::Disabled);
        assert!(
            !field.is_composing(),
            "Disabled 後は composition がクリアされる"
        );
    }

    #[test]
    fn debug_output_includes_ime_enabled_and_purpose_as_non_sensitive_fields() {
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(408), "greeting", "");
        focus(&mut field);
        field.set_ime_purpose(ImePurpose::Password);
        field.apply_ime(ImeEvent::Enabled);

        let output = format!("{field:?}");

        assert!(output.contains("ime_enabled: true"));
        assert!(output.contains("Password"));
    }

    // 以下、TASK-8.5-S27（#208）が追加する状態モデル・コンポジションの棚卸しテスト。
    // `docs/architecture/text-input-state-model.md` §4.3 遷移表・§5 不変条件を軸に、
    // S4〜S8・S23 が既に配線したテストの欠落セルを補う（プロダクションコードは無変更）。

    #[test]
    fn idle_direct_commit_inserts_at_cursor_without_preceding_preedit() {
        // §4.3 表 6 行目: Idle 中に Preedit を経由せず直接届く Commit も、
        // §4.5 と同じ確定挿入処理を行う（anchor 相当は現在の cursor をそのまま使う）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(500), "greeting", "ab");
        focus(&mut field);
        field.handle_input(TextFieldInput::MoveCursorToStart);
        field.handle_input(TextFieldInput::MoveCursorRight);
        assert!(
            !field.is_composing(),
            "Preedit を経由していないため Idle のまま"
        );

        let event = field.apply_ime(ImeEvent::Commit("XY".to_string()));

        assert_eq!(event, None, "Commit 自体は Event を返さず dirty のみ立てる");
        assert_eq!(field.buffer(), "aXYb");
        assert_eq!(field.cursor(), 3);
        assert!(field.composition.is_none(), "Idle のまま変化しない");

        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(
            event,
            Some(Event::TextFieldChanged {
                label: "greeting".to_string(),
                value: "aXYb".to_string(),
            })
        );
    }

    #[test]
    fn commit_with_mixed_control_and_valid_characters_inserts_only_valid_chars() {
        // §4.5 手順 1: 制御文字混在の Commit は部分除去のみ行う（制御文字のみの
        // ケースは commit_of_control_characters_only_is_no_op_and_clears_composition
        // が既にカバーしており、本テストは「一部が有効文字」の欠落セルを補う）。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(501), "greeting", "");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "か".to_string(),
            cursor: None,
        });

        let event = field.apply_ime(ImeEvent::Commit("か\nな".to_string()));

        assert_eq!(event, None);
        assert_eq!(
            field.buffer(),
            "かな",
            "制御文字のみ除去し有効文字は挿入する"
        );
        assert!(field.composition.is_none());
        assert!(field.dirty, "有効文字が挿入されたため dirty が立つ");
    }

    #[test]
    fn commit_with_empty_string_clears_composition_without_dirty() {
        // §4.5 手順 2: 除去後を待たず、Commit("") 自体が最初から空の場合も
        // 同じ契約（dirty を立てず composition のみクリア）に従う。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(502), "greeting", "ab");
        focus(&mut field);
        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });

        let event = field.apply_ime(ImeEvent::Commit(String::new()));

        assert_eq!(event, None);
        assert_eq!(field.buffer(), "ab", "空文字列の確定は挿入しない");
        assert!(field.composition.is_none());
        assert!(!field.dirty);

        // dirty が立っていないため blur でも TextFieldChanged は発火しない
        // （§4.5 手順 2 の doc が明記する「stale blur-commit 上書き防止」の直接検証）。
        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(event, None);
    }

    #[test]
    fn preedit_alone_does_not_mark_dirty_or_fire_change_event() {
        // 不変条件 1: composition は確定バッファ・Event::TextFieldChanged に混入しない。
        // Preedit の開始 → 更新 → 空 Preedit によるクリアのみでは、Commit を
        // 一度も経由しないため dirty は立たず、FocusLost でも変化イベントを出さない。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(503), "greeting", "ab");
        focus(&mut field);

        field.apply_ime(ImeEvent::Preedit {
            text: "か".to_string(),
            cursor: Some((0, 1)),
        });
        assert!(!field.dirty);
        field.apply_ime(ImeEvent::Preedit {
            text: "かな".to_string(),
            cursor: Some((0, 2)),
        });
        assert!(!field.dirty);
        field.apply_ime(ImeEvent::Preedit {
            text: String::new(),
            cursor: None,
        });
        assert!(!field.dirty);
        assert!(field.composition.is_none());
        assert_eq!(field.buffer(), "ab", "確定バッファへは一切混入しない");

        let event = field.apply_input(InteractionInput::FocusLost);
        assert_eq!(
            event, None,
            "Commit を経由していないため blur-commit は発火しない"
        );
    }

    #[test]
    fn empty_preedit_while_idle_is_a_no_op() {
        // winit は Commit 直前に必ず空 Preedit を送る契約（§4.3 表・
        // ime-architecture.md winit マッピング注記）への防御。Idle 中に空 Preedit が
        // 届いても panic せず、composition は None のまま変化しない。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(504), "greeting", "ab");
        focus(&mut field);
        assert!(field.composition.is_none());

        let event = field.apply_ime(ImeEvent::Preedit {
            text: String::new(),
            cursor: None,
        });

        assert_eq!(event, None);
        assert!(field.composition.is_none());
        assert_eq!(field.buffer(), "ab");
    }

    #[test]
    fn preedit_cursor_out_of_range_is_retained_and_paint_does_not_panic() {
        // Preedit.cursor は本モデルが直接クランプする契約ではなく、表示合成
        // （CompositionDisplay::synthesize）側がクランプを担う（§4.4・
        // composition_display.rs の cursor_out_of_range_clamps_and_normalizes_reversed_range
        // 参照）。ここでは範囲外カーソルを保持したまま paint まで panic しないことを
        // 状態モデル側の安全性として確認する。
        let mut field: TextFieldWidget<FormState> =
            TextFieldWidget::new_static(WidgetId::from_raw(505), "greeting", "ab");
        focus(&mut field);

        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: Some((100, 200)),
        });
        assert_eq!(field.composition.as_ref().unwrap().cursor, Some((100, 200)));

        // paint / measure が panic せずに完走することを確認する（クランプは描画側の責務）。
        let measurement = Measurement {
            width: 100.0,
            height: 20.0,
        };
        let commands = field.paint((0.0, 0.0), measurement);
        assert_eq!(commands.len(), 2);
        let _ = field.measure(measurement);
    }

    #[test]
    fn sync_from_state_resumes_tracking_after_composition_ends() {
        // §4.9 の裏面: composition.is_some() の間だけ sync_from_state が buffer を
        // 据え置く。Composing が終わり Idle へ戻った後は、次の sync_from_state 呼び出しで
        // 再び外部状態への追随を再開する（据え置きが恒久化しないことの確認）。
        let mut field =
            TextFieldWidget::new_dynamic(WidgetId::from_raw(506), "name", "", read_name);
        focus(&mut field);
        let state = FormState {
            name: "initial".to_string(),
        };
        field.sync_from_state(&state);
        assert_eq!(field.buffer(), "initial");

        field.apply_ime(ImeEvent::Preedit {
            text: "x".to_string(),
            cursor: None,
        });
        let updated = FormState {
            name: "external-update".to_string(),
        };
        field.sync_from_state(&updated);
        assert_eq!(
            field.buffer(),
            "initial",
            "Composing 中は sync_from_state が buffer を据え置く"
        );

        // 空 Preedit で Idle へ戻る（§4.3 表）。
        field.apply_ime(ImeEvent::Preedit {
            text: String::new(),
            cursor: None,
        });
        assert!(field.composition.is_none());

        field.sync_from_state(&updated);
        assert_eq!(
            field.buffer(),
            "external-update",
            "Idle へ戻った後は sync_from_state が再び外部更新へ追随する"
        );
    }
}
