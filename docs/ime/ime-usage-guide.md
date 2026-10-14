# IME 統合 使用ガイド

本文書は `fandhe-core` の IME 統合境界型・`fandhe-winit-ime` の winit 変換レイヤ・
`fandhe-web-dom` の Web composition 変換を利用するアプリ開発者・プラットフォーム
統合レイヤ実装者向けの使用ガイドです（親: #181 TASK-8.5「IME 統合実装」、REQ-8 / M3）。

## 1. 目的・対象読者・位置づけ

TASK-8.5 の成果物は以下のとおり複数クレート・文書に分散しています。本文書はこれらを
俯瞰し、「どこに何があり、実際にどう配線するか」をまとめる橋渡し役です
（様式は `docs/tokens/design-token-guide.md`、TASK-8.4-S24 を踏襲）。

| 成果物 | 種別 | 場所 | 対応 sub-issue |
|--------|------|------|----------------|
| コア境界型 `ImeEvent` / `ImeRequest` / `ImePurpose` | 設計 + 実装 | `crates/fandhe-core/src/ui/ime.rs` | S1（設計、#182）/ S7（実装、#188） |
| テキストフィールド本体（`apply_ime` / `handle_input` / `take_ime_requests` / `set_ime_purpose` / `ime_enabled` / `is_composing`） | 実装 | `crates/fandhe-core/src/ui/text_field.rs` | S8（#189）/ S23（#204） |
| カーソル・選択範囲（`CharOffset` / `SelectionRange`） | 実装 | `crates/fandhe-core/src/ui/text_selection.rs` | S4（#185）/ S26（#207） |
| コンポジション表示合成 `CompositionDisplay` | 実装 | `crates/fandhe-core/src/ui/composition_display.rs` | S5（#186） |
| 候補ウィンドウ矩形計算 `ime_cursor_area` | 実装 | `crates/fandhe-core/src/ui/ime_cursor_area.rs` | S6（#187） |
| 複数フィールドのフォーカス調停 `ImeFocusCoordinator` | 実装 | `crates/fandhe-core/src/ui/ime_focus.rs` | S24（#205） |
| winit 変換レイヤ（`ime_event_from_winit` / `apply_ime_request` / `text_field_input_from_key`） | 実装 | `crates/fandhe-winit-ime/src/lib.rs` | S7（#188）/ S16（#197）/ S19（#200） |
| Web composition 変換（`composition_phase_from_event_type` / `ime_event_from_composition`） | 実装 | `crates/fandhe-web-dom/src/ime.rs` | S21（#202） |
| 設計文書 | 設計 | `docs/architecture/ime-architecture.md`（S1・#182）・`text-input-state-model.md`（S2・#183） | — |
| 使用ガイド + サンプル（本文書、S30） | 手書きガイド | `docs/ime/ime-usage-guide.md`（本文書）+ `crates/fandhe-winit-ime/examples/ime_text_field_sample.rs` | S30（#211） |

対象読者:

- **アプリ開発者**: `TextFieldWidget<S>` を配線してテキスト入力 UI を組み立てる開発者
- **プラットフォーム統合レイヤ実装者**: winit 以外のバックエンド（AppKit 直接・Win32 直接等）
  へ本ガイドの契約を移植する開発者

IME 統合の実装は `crates/fandhe-core/src/ui/`（コア、`#![forbid(unsafe_code)]`）と
`crates/fandhe-winit-ime` / `crates/fandhe-web-dom`（プラットフォーム層）に分かれており、
二層アーキテクチャ（REQ-2）上、コアは特定プラットフォームの IME API に一切依存しません。

## 2. 全体アーキテクチャ総括

### 2.1 責務分界

`docs/architecture/ime-architecture.md` §4.3 の定義どおり、コアとプラットフォーム層は
以下のとおり責務が分かれます。

| 責務 | 担当 |
|------|------|
| コンポジション状態（preedit 文字列・カーソル範囲）の保持 | コア（`fandhe-core`） |
| コンポジション状態のウィジェット描画（`CompositionDisplay`） | コア |
| 変換候補ウィンドウの位置計算（`ime_cursor_area`） | コア |
| フォーカス・IME 状態遷移の調停（`ImeFocusCoordinator`） | コア |
| ネイティブ IME イベント（`Ime::*` / `compositionevent` / `UITextInput` / `InputConnection`）→ `ImeEvent` 変換 | プラットフォーム層（`fandhe-winit-ime` / `fandhe-web-dom`） |
| `ImeRequest` → ネイティブ API（`Window::set_ime_*` / DOM 属性等）変換 | プラットフォーム層 |
| 確定文字列の `S` へのブリッジ（`Event::TextFieldChanged`） | コア |

コアはプラットフォーム型（`winit::event::Ime` 等）に一切依存しません（REQ-2 の依存方向:
最適化レイヤ → コアの一方向）。プラットフォーム層はコアの `ImeEvent` / `ImeRequest` の
みを介してコアと通信します。

### 2.2 モジュール構成（対応 sub-issue付き）

`crates/fandhe-core/src/ui/` 配下:

| モジュール | 責務 | 対応 sub-issue |
|-----------|------|-----------------|
| `ime` | 境界型 `ImeEvent` / `ImeRequest` / `ImePurpose` | S1（#182）設計、S7（#188）実装 |
| `text_field` | `TextFieldWidget<S>`（IME 配線・フォーカス・入力処理） | S8（#189）、`set_ime_purpose` は S23（#204） |
| `text_selection` | `CharOffset` / `SelectionRange`、UTF-8/UTF-16 変換ヘルパー | S4（#185）、grapheme 境界は S26（#207） |
| `composition_display` | `CompositionDisplay`（preedit の表示合成） | S5（#186） |
| `ime_cursor_area` | 候補ウィンドウ矩形計算 `ime_cursor_area` | S6（#187） |
| `ime_focus` | `ImeFocusCoordinator`（複数フィールド調停） | S24（#205） |

プラットフォーム層:

| クレート | 責務 | 対応 sub-issue |
|---------|------|-----------------|
| `fandhe-winit-ime` | winit `Ime::*` ⇔ `ImeEvent`/`ImeRequest` 変換、`ImeWindow` トレイト | S7（#188）、iOS/Android のキー入力経路は S16（#197）/S19（#200） |
| `fandhe-web-dom` | DOM `compositionevent` ⇔ `ImeEvent` 変換 | S21（#202） |

### 2.3 spec 整合性

対応要件は REQ-8（IME 統合）、マイルストーンは M3。詳細な spec 対応は
`docs/architecture/ime-architecture.md` §2 を参照してください。

## 3. 使用ガイド（アプリ開発者向け）

本節のコード例は `crates/fandhe-winit-ime/examples/ime_text_field_sample.rs`
（TASK-8.5-S30 で新設した一次情報）からの抜粋です。**サンプルが一次情報、本ガイドは
抜粋**であり、両者が乖離した場合はサンプル側（実際にコンパイル・lint 済み）を正としてください。

### 3.1 `TextFieldWidget<S>` の生成と基本入力

```rust
use fandhe_core::ui::{TextFieldWidget, WidgetId};

// 静的初期値のみのフィールド（アプリ状態 S から値を読み取らない）。
let field = TextFieldWidget::<()>::new_static(WidgetId::from_raw(1), "field_a", "");
```

状態 `S` から動的に確定値を読み取る場合は `new_dynamic`（`ReadFn<S, String>` を渡す）を
使います。フォーカス・ポインタ等の一時的インタラクションは `apply_input`
（`InteractionInput::FocusGained` / `FocusLost` 等）、非 IME 経路の編集操作は
`handle_input`（`TextFieldInput::InsertText` / `DeleteBackward` 等）で行います。

### 3.2 デスクトップ配線: winit `Ime::*` → コア

単一フィールドのみの場合の最小経路です（複数フィールドがある場合は §3.6 の
`ImeFocusCoordinator` 経由を使ってください。単一フィールドでも将来のフィールド追加を
見越して coordinator 経由に統一するのが安全です）。

```text
WindowEvent::Ime(ime)
  → fandhe_winit_ime::ime_event_from_winit(ime)     // winit → ImeEvent 変換
  → field.apply_ime(event)                          // コア: コンポジション状態を更新
  → field.take_ime_requests()                        // 蓄積された ImeRequest を取り出す
  → fandhe_winit_ime::apply_ime_request(&window, &req) // ImeRequest → winit API 委譲
```

`apply_ime` はコンポジションの内部状態を更新し、`Event::TextFieldChanged`
（確定文字列を含みうる、ログへ出力しないこと）を返すことがあります。
`ImeRequest` はこの呼び出しの副作用として `pending_ime_requests` に蓄積されるため、
`take_ime_requests()` で明示的に drain する必要があります。

### 3.3 複数フィールドのフォーカス遷移: `ImeFocusCoordinator`

2 個以上のテキストフィールドを扱う場合は `ImeFocusCoordinator` を使い、
`TextFieldWidget` を直接操作しないでください。フォーカス遷移時に
「旧フィールドの `SetEnabled(false)` → 新フィールドの `SetEnabled(true)`」の順序
（`ime-architecture.md` §4.7）と、「同時に 2 フィールドが IME 有効にならない」不変条件を
コア側が保証します。

```rust
use fandhe_core::ui::ImeFocusCoordinator;

let mut coordinator = ImeFocusCoordinator::new();
let mut fields = [&mut field_a, &mut field_b];

// フォーカスを field_b へ移す（field_a の blur-commit と SetEnabled(false)、
// field_b の SetEnabled(true) がこの順に transition.requests へ積まれる）。
let transition = coordinator.focus(&mut fields, field_b_id);
for event in transition.events {
    // Event::TextFieldChanged 等。StateStore::dispatch へ回す（ログへは出さない）。
}
for request in transition.requests {
    apply_ime_request(&window, &request);
}
```

winit の `WindowEvent::Ime` を受けた場合は `TextFieldWidget::apply_ime` を直接呼ばず、
`ImeFocusCoordinator::dispatch_ime(&mut fields, event)` を使ってください
（フォーカス中フィールドへのルーティングと、フォーカス遷移直後の遅延 `Commit` を
破棄するガードを兼ねます。`ime_focus.rs` モジュール doc「フォーカス遷移直後の遅延
`Commit` に対するガード」参照）。`dispatch_ime` は `Vec<Event>` のみを返すため、
`ImeRequest` は引き続き各フィールドの `take_ime_requests()` から drain します。

### 3.4 候補ウィンドウ位置: `ime_cursor_area`

`ImeRequest::SetCursorArea` はコアが自動発行しません。テキストフィールドのレイアウト
矩形（`Point` / `Size`、論理ピクセル）とコンポジション情報から、呼び出し側
（アプリ層・プラットフォーム層）が明示的に計算して送る必要があります。

```rust
use fandhe_core::ui::{ImeCursorAreaParams, ImeRequest, ime_cursor_area};

let rect = ime_cursor_area(&ImeCursorAreaParams {
    field_origin: layout_origin,       // レイアウト結果の左上原点（論理ピクセル）
    field_size: layout_size,           // レイアウト結果のサイズ（論理ピクセル）
    buffer: field.buffer(),
    anchor: field.cursor(),
    preedit: &preedit_text,            // 下記「preedit 内容の橋渡し」参照
    preedit_cursor,                    // 同上
});
let request = ImeRequest::SetCursorArea {
    origin: (rect.origin.x, rect.origin.y),
    size: (rect.size.width, rect.size.height),
};
apply_ime_request(&window, &request);
```

**preedit 内容の橋渡し**: `TextFieldWidget` はコンポジション内部状態（`Composition` の
preedit テキスト・アンカー）を `is_composing()`（真偽値のみ）以外に公開しません
（`ime_cursor_area.rs` モジュール doc 参照）。そのため呼び出し側は、
`ImeEvent::Preedit { text, cursor }` を受け取った時点でその内容を自前で一時保持し、
`ime_cursor_area` の `preedit` / `preedit_cursor` 引数へ渡す必要があります。サンプル
（`ime_text_field_sample.rs` の `FieldSlot`）を参照してください。

### 3.5 入力目的ヒント: `set_ime_purpose` / `ImePurpose`

```rust
use fandhe_core::ui::ImePurpose;

field.set_ime_purpose(ImePurpose::Password);
for request in field.take_ime_requests() {
    apply_ime_request(&window, &request);
}
```

`Password` へ切替時、コアは進行中のコンポジションを（確定せずに）破棄し、遅延到着する
`Commit` を 1 件無視するガードを立てます。**`Password` へ切替時は
`ImeRequest::SetPurpose(Password)` のみを発行し、`SetEnabled(false)` は併送しません。
iOS/Android では `SetEnabled` がソフトウェアキーボードの表示/非表示そのものに直結し、
パスワードフィールドへフォーカスした瞬間にキーボードごと隠れて入力できなくなるため
です**（`text_field.rs::set_ime_purpose` の doc「Password 時のポリシー」が最新の
実装契約。`ime.rs` の `ImeRequest::SetPurpose` バリアント doc も参照。Issue #586）。

`SetPurpose` がヒントとして実際に効くかはプラットフォーム依存です。詳細は §4 を参照してください。

**学習・予測変換抑止の代替担保（Issue #586）**: `SetPurpose` が no-op なプラットフォーム
向けに、`fandhe-winit-ime` が `apply_ime_request` とは独立した経路を提供します
（コア契約には影響しません。両方を並列に呼んでください）。

```rust
use fandhe_winit_ime::sensitive_input::SensitiveInputTracker;

let mut tracker = SensitiveInputTracker::new();
for request in field.take_ime_requests() {
    if let Some(active) = tracker.observe(&request) {
        let succeeded = fandhe_winit_ime::sensitive_input::apply_sensitive_input(active);
        // 戻り値を捨てず record_apply_result へ反映する契約（apply_sensitive_input
        // の doc 参照）。捨てると macOS で EnableSecureEventInput / DisableSecureEventInput
        // 失敗時に tracker.active が実態とずれたまま残り、再試行が発行されず、
        // 保護されていない入力を保護済みと誤認する、または無効化に失敗した
        // SecureEventInput ロックが他アプリの IME 入力も含め残存する。
        tracker.record_apply_result(active, succeeded);
    }
    apply_ime_request(&window, &request);
}
```

- **macOS**: `tracker.observe` が有効化された回に Carbon の `EnableSecureEventInput`
  （`NSSecureTextField` と同じ機構）を呼び、パスワードフィールドへフォーカスがある間
  IME 自体を無効化します（`NSSecureTextField` と同挙動である点に注意）。
- **Android**: `TextFieldWidget::set_ime_purpose` が呼ばれるたびに
  `fandhe_winit_ime::android_editor_info::apply_ime_purpose_android(&android_app, purpose)`
  を呼び、`AndroidApp::set_ime_editor_info` へ `TYPE_TEXT_VARIATION_PASSWORD` +
  `IME_FLAG_NO_PERSONALIZED_LEARNING` を反映してください（`SensitiveInputTracker` とは
  別経路で、`SetPurpose` 発行のたびに直接呼ぶ設計です）。
- **iOS / Linux X11 / Windows / Web**: 代替担保はありません
  （`docs/architecture/ime-known-constraints.md` §3.3 参照）。

### 3.6 状態参照 API

- `field.ime_enabled() -> bool`: プラットフォームからの `Enabled`/`Disabled` 応答の観測値
  （要求を発行したことそのものではない）
- `field.is_composing() -> bool`: コンポジション進行中かどうか（内容は非公開）
- `field.ime_purpose() -> ImePurpose`: 現在の入力目的ヒント

### 3.7 モバイル（iOS / Android）の commit 型経路

iOS（`fandhe_winit_ime`、S16 / #197）・Android（S19 / #200）は winit バックエンド上では
`Ime::*` が一切発火せず、`WindowEvent::KeyboardInput` 経由で確定済みテキストが
1 文字（または結合済み文字列）ずつ届きます。この経路には
`text_field_input_from_key(state, logical_key, text)` を使い、戻り値の
`TextFieldInput` を `handle_input` へ渡してください。

```rust
use fandhe_winit_ime::text_field_input_from_key;

if let Some(input) = text_field_input_from_key(key_event.state, &key_event.logical_key, key_event.text.as_deref()) {
    field.handle_input(input);
}
```

**`ime_event_from_winit` とは排他に使うこと**（呼び出し側契約。同一 `KeyboardInput` を
両関数へ二重に流さない）。ただし `handle_input` はコンポジション進行中
（`is_composing() == true`）は `Commit`/`Cancel` 以外の入力を無視する既存ガードを
持つため、デスクトップのようにコンポジションと直接キー入力が混在する環境
（S9 申し送りの ASCII 直接入力等）でも、両方の経路を同一フィールドへ配線しても
二重挿入は起きません（`text_field.rs::handle_input` 実装参照。ただし iOS/Android では
`Ime::*` 自体が発火しないため実運用上この混在は起きません）。

### 3.8 Web: composition イベント連携

`fandhe-web-dom`（S21 / #202）は DOM の `compositionstart` / `compositionupdate` /
`compositionend` イベント種別文字列を `composition_phase_from_event_type` で
`CompositionPhase` へ変換し、`ime_event_from_composition(phase, data)` で `ImeEvent`
へ変換します。変換後は §3.2〜§3.3 と同じ経路（`apply_ime` または
`ImeFocusCoordinator::dispatch_ime`）に合流します。

## 4. プラットフォーム差異早見表

以下は要約です。詳細は `docs/architecture/ime-known-constraints.md` を参照してください。
発火タイミング・no-op 挙動は S9・S11・S13・S16・S19 の各検証（各 issue）で確認した内容です。

| 項目 | macOS | Windows | Linux (X11) | Linux (Wayland) | iOS | Android |
|------|-------|---------|--------------|------------------|-----|---------|
| `Enabled` 発火タイミング | 最初の `Preedit` で遅延発火 | `WM_IME_STARTCOMPOSITION` で遅延発火 | `SetEnabled(true)` へ即時発火 | 同左（X11 と同型） | 発火しない（`SetEnabled` は表示/非表示） | 同左（iOS と同型） |
| `SetPurpose` の実効性 | no-op | no-op | no-op | 有効（`ContentHint`/`ContentPurpose`） | no-op | no-op |
| `SetEnabled` の意味論 | IME 有効/無効 | IME 有効/無効 | IME 有効/無効 | IME 有効/無効 | ソフトウェアキーボード表示/非表示 | 同左 |
| `SetCursorArea` の実効性 | 有効 | 有効 | 有効 | 有効 | no-op（候補は候補バー表示） | no-op（同左） |
| 確定文字列の主経路 | `Ime::Commit` | `Ime::Commit` | `Ime::Commit` | `Ime::Commit` | `KeyboardInput`（§3.7） | `KeyboardInput`（§3.7） |
| 学習・予測変換抑止の代替担保（#586） | 実装済み（SecureEventInput） | 設計のみ（別 issue） | 対応不能 | `SetPurpose` で担保済み | 対応不能 | 実装済み（`EditorInfo`） |

出典: S9・S11・S13・S16・S19 の検証（各 issue）。

## 5. サンプルの案内

`crates/fandhe-winit-ime/examples/ime_text_field_sample.rs`（TASK-8.5-S30 で新設）は、
2 個の `TextFieldWidget` + `ImeFocusCoordinator` + winit 変換レイヤの実配線例です。
`macos_ime_probe.rs`（S10、変換確認用の最小プローブ）とは異なり、フォーカス調停・
候補ウィンドウ位置計算まで含みます。

```bash
cargo check -p fandhe-winit-ime --examples   # コンパイル確認（CI で実行、CI runner）
cargo run -p fandhe-winit-ime --example ime_text_field_sample   # 対話的実行（手元環境）
```

ウィンドウ内をクリックしてフォーカスし、日本語 IME 等で入力を確定してください。
Tab キーで 2 フィールド間のフォーカスを切り替えられます。**対話的な動作確認
（日本語 IME での preedit → 確定）はディスプレイ・IME を要するため CI では実行できません。
実機での動作確認は人間担当とします**（既存 S10 等の検証記録と同じ運用）。

## 6. セキュリティ考慮（`.claude/rules/security.md` 準拠）

- **preedit / commit 文字列の非露出**: `ImeEvent` は `Debug` を手書き実装し、
  `Preedit` / `Commit` の内容を出力せず文字数のみ記録します
  （`ime.rs` モジュール doc・`text_field.rs` の `Debug` 実装も同様のマスク方針）。
  アプリ側のログ・エラーメッセージも、`ImeEvent` を `{:?}` へそのまま委譲する限りこの
  契約を継承できますが、`Composition` を独自に橋渡しする場合（§3.4）は自前のログ出力で
  内容を露出しないよう注意してください。
- **A03 インジェクション**: 本ガイド・サンプルの範囲では、型付きイベント
  （`ImeEvent` / `TextFieldInput`）のみを扱い、外部入力の生バイト列・キーコードを
  直接解釈しません（既存の型レベル防御をそのまま利用）。
- **検証入力**: 動作確認にはダミー文字列のみを使い、個人情報・実パスワードを
  入力しないでください（`macos_ime_probe.rs` と同じ方針）。

## 参考: 関連ファイル一覧

- `docs/architecture/ime-architecture.md` — 設計文書（コア⇔プラットフォーム契約の一次情報）
- `docs/architecture/text-input-state-model.md` — コンポジション状態モデル設計
- `crates/fandhe-winit-ime/examples/macos_ime_probe.rs` — S10 実機検証ハーネス（変換確認用プローブ）
- `crates/fandhe-winit-ime/examples/ime_text_field_sample.rs` — 本ガイドの一次情報（S30 新設）
