# Changelog — fandhe-core

書式は [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) に、バージョニングは
[Semantic Versioning](https://semver.org/lang/ja/)（0.x 系: minor で破壊的変更を許容）に従う。

## [Unreleased]

画面遷移スタック（`ScreenStack`）の戻る操作を `Event` 経由で発火する導線を追加（Issue #726）。

### 追加

- `state::Event::BackRequested`: OS の戻る操作（macOS/Linux/Windows の Esc・Android の
  バックキー等）をプラットフォームレイヤが変換して渡すイベント。`ui::request_back` へ委譲し、
  画面の `.on_back()` ハンドラ（典型的には `ui::ScreenStack::pop`）を発火する
  （`fandhe-egui-proxy::dispatch_back_if_requested` が実配線）

### 破壊的変更（BREAKING CHANGE）

- 公開 enum `state::Event` に `BackRequested` バリアントを追加。`Event` を
  網羅 match（`match event { ... }` を `_ => {}` 無しで書いている）呼び出し側は
  コンパイルが壊れる

### 移行手順

1. `Event` を網羅 match している箇所へ `BackRequested` 分岐を追加するか、
   `_ => { /* 既存の分岐 */ }` のワイルドカードで受ける
2. 戻る操作を扱わない呼び出し側は、既存の match アームに手を加えず
   `Event::BackRequested => {}` のような no-op 分岐を追加すれば移行完了

IME セッション識別子によるフォーカス遷移直後の遅延イベント誤帰属対策
（Issue #587、PR #680）。設計文書: `docs/architecture/ime-architecture.md` §4.8。

### 破壊的変更（BREAKING CHANGE）

- `ImeRequest` に `BeginSession(ImeSessionId)` バリアントを追加。`ImeRequest` を
  網羅 match（`match request { ... }` を `_ => {}` 無しで書いている）呼び出し側は
  コンパイルが壊れる（例: `fandhe-winit-ime::sensitive_input::SensitiveInputTracker::observe`）
- `ImeFocusCoordinator::dispatch_ime` に必須引数 `session: Option<ImeSessionId>`
  を追加: `dispatch_ime<S>(&mut self, fields: &mut [&mut TextFieldWidget<S>], event: ImeEvent)`
  から `dispatch_ime<S>(&mut self, fields: &mut [&mut TextFieldWidget<S>], session: Option<ImeSessionId>, event: ImeEvent)`
  へ変更（`crates/fandhe-core/src/ui/ime_focus.rs`）

### 移行手順

1. `ImeRequest` を網羅 match している箇所へ `BeginSession(_)` 分岐を追加する。
   `BeginSession` は native（winit 等）へ転送する要求ではないため、既存の
   native 転送処理へは渡さず読み捨てるか、`ImeSessionTracker::observe_request`
   （後述）へ渡す:

   ```rust
   match request {
       ImeRequest::SetEnabled(enabled) => { /* 既存処理 */ }
       ImeRequest::SetCursorArea { .. } => { /* 既存処理 */ }
       ImeRequest::SetPurpose(purpose) => { /* 既存処理 */ }
       // 追加: Password 抑止状態（purpose/enabled）を変更しない要求のため no-op でよい
       ImeRequest::BeginSession(_) => {}
   }
   ```

2. `dispatch_ime` の呼び出し元は、`fandhe-winit-ime::ImeSessionTracker` を保持し、
   「要求観測 → stamp → dispatch」の順で配線する
   （実配線は `crates/fandhe-winit-ime/examples/ime_text_field_sample.rs` 参照）:

   ```rust
   use fandhe_core::{ImeFocusCoordinator, ImeSessionId}; // ImeSessionId は fandhe_core から再エクスポート
   use fandhe_winit_ime::{apply_ime_request, ime_event_from_winit, ImeSessionTracker};

   // フォーカス遷移で生成された ImeRequest 群を native へ適用する前に、
   // BeginSession/SetEnabled(false) を tracker へ観測させる。
   for request in transition.requests {
       tracker.observe_request(&request);
       apply_ime_request(window, &request);
   }

   // native から届いた ImeEvent（winit の Ime::* → ime_event_from_winit）は、
   // まず tracker.stamp で帰属セッションを算出してから dispatch_ime へ渡す。
   let event = ime_event_from_winit(ime);
   let session = tracker.stamp(&event);
   coordinator.dispatch_ime(&mut fields, session, event);
   ```

## [0.2.0] - 2026-08-12

TV フォーカスナビゲーション対応（TASK-17.1 / TASK-17.2、REQ-17）。
設計文書: `docs/architecture/tv-focus-navigation-architecture.md`
（3.1 節・4.3 節・4.4 節・5 節）。

### 破壊的変更（BREAKING CHANGE）

- `Screen::button()` のシグネチャを `button(label, on_click)` から
  `button(id: WidgetId, label, on_click)` へ変更（id が先頭引数）
- `Node::Button` / `Node::IndexedButton` に `id: WidgetId` フィールドを追加
- `Node::List` / `Node::VirtualList` に `axis: Axis` フィールドを追加
  （構造体リテラルで構築している場合のみ影響。ビルダー経由は非破壊）

### 移行手順

1. `.button(label, on_click)` 呼び出しへ画面内で一意な id を追加する:
   `.button(WidgetId::from_raw(1), label, on_click)`。
   動的リスト内の項目は位置 index ではなく項目固有の永続 ID から導出すること
   （挿入・削除で index がシフトするとフォーカス再解決の意味が崩れる。設計文書 3.1 節）
2. `Node::Button { .. }` / `Node::IndexedButton { .. }` リテラルへ `id:` を追加する
3. `Node::List { .. }` / `Node::VirtualList { .. }` リテラルへ `axis:` を追加する
   （従来挙動は `Axis::default()` = `Vertical`）

### 追加

- `Axis` enum（`Vertical` / `Horizontal` / `Grid { columns }`）
- `Screen::list_with_axis()` / `Screen::virtual_list_with_axis()`（既存
  `.list()` / `.virtual_list()` は `Axis::default()` で非破壊維持）
- `FocusRing` / `FocusEntry` / `FocusDirection` / `WindowHints`（TV フォーカスの
  中央機構: 自己更新型 `rebuild`・三段探索・Axis 宣言ベースの軸解決）
- `activate_focus_at()`（id 検証付きの決定操作。`click_button_at` と path 降下を共有）

## [0.1.0]

初期実装（TASK-1.1 以降の宣言的 UI DSL・状態管理・ウィジェット群）。
