# IME 統合アーキテクチャ設計（共通コア⇔プラットフォーム層の契約）

## 1. 目的・位置づけ

本文書は共通コア（`fandhe-core`）⇔ プラットフォーム統合レイヤの IME 契約
（イベント・要求・責務分界・プラットフォーム別マッピング・文字インデックス正規化方針）を
定義する（#181、TASK-8.5、REQ-8）。各 sub-issue（#182〜#211）と本文書の節の対応は
下表のとおり。

### #182〜#211（S1〜S30）の分担表

`gh api repos/Fandhe-AI/fandhe-multi-platform/issues/181/sub_issues` で取得した実際の
30 件の子 issue（#182〜#211）に基づく。

| sub-issue | 内容 | 本文書との関係 |
|-----------|------|----------------|
| #182（本文書・S1） | IME 統合アーキテクチャ設計（共通コア⇔プラットフォーム層の契約） | 本文書がその成果物 |
| #183（S2） | テキスト入力状態モデル（コンポジション・確定文字列）設計 | 本文書 §4 の `ImeEvent` / コンポジション状態契約を受け、状態モデルの内部詳細（バッファ構造・カーソル表現）を設計する |
| #184（S3） | テキスト入力状態モデル実装 | S2 の設計を実装する |
| #185（S4） | カーソル・選択範囲管理実装 | 本文書 §4.6 の文字インデックス正規化方針を実装へ落とす |
| #186（S5） | コンポジション（未確定文字列）表示実装 | 本文書 §4.3「コア側の責務」を実装する |
| #187（S6） | 変換候補ウィンドウ位置計算実装 | 本文書 §4.2「コア → プラットフォームの要求契約」のカーソル矩形通知を実装する |
| #188（S7） | winit IME イベント（`Ime::*`）ハンドリング実装 | 本文書 §4.4「winit マッピング」を実装する |
| #189（S8） | テキストフィールドウィジェットとの統合配線 | 本文書 §4.7「フォーカス遷移との結合」を `TextFieldWidget<S>` へ配線する |
| #190（S9） | macOS: NSTextInputClient 連携確認・調整 | 本文書 §4.4「winit マッピング」の macOS バックエンド分岐を検証する |
| #191（S10） | macOS: 日本語入力動作検証 | 同上の動作検証 |
| #192（S11） | Windows: IMM32/TSF 連携確認・調整 | 本文書 §4.4 の Windows バックエンド分岐を検証する |
| #193（S12） | Windows: 日本語入力動作検証 | 同上の動作検証 |
| #194（S13） | Linux: ibus/fcitx 連携確認・調整 | 本文書 §4.4 の Linux バックエンド分岐を検証する |
| #195（S14） | Linux: 日本語入力動作検証 | 同上の動作検証 |
| #196（S15） | iOS: UIKit テキスト入力連携設計 | 本文書 §4.5「iOS（UIKit）マッピング」を詳細設計する |
| #197（S16） | iOS: ソフトウェアキーボード・IME 統合実装 | S15 の設計を実装する |
| #198（S17） | iOS: 日本語入力動作検証 | 動作検証 |
| #199（S18） | Android: InputConnection 連携設計 | 本文書 §4.5「Android（InputConnection）マッピング」を詳細設計する |
| #200（S19） | Android: ソフトウェアキーボード・IME 統合実装 | S18 の設計を実装する |
| #201（S20） | Android: 日本語入力動作検証 | 動作検証 |
| #202（S21） | Web: composition イベント連携実装 | 本文書 §4.5「Web（compositionevent）マッピング」を実装する（既存 `mount.rs` との整合含む） |
| #203（S22） | Web: 日本語入力動作検証 | 動作検証 |
| #204（S23） | IME オン/オフ・入力モード状態の API 公開 | 本文書 §4.2 の入力目的ヒント（`ImePurpose` 相当）契約を公開 API 化する |
| #205（S24） | 複数テキストフィールド間のフォーカス・IME 状態遷移処理 | 本文書 §4.7「フォーカス遷移との結合」を実装する |
| #206（S25） | CJK 以外（韓国語・中国語）入力の確認 | 本文書の契約が CJK 全般で成立することを検証する |
| #207（S26） | 絵文字・サロゲートペア処理の確認・修正 | 本文書 §4.6「文字インデックス正規化方針」の境界条件を検証する |
| #208（S27） | ユニットテスト（状態モデル・コンポジション） | S2〜S6 の実装をテストする |
| #209（S28） | 結合テスト（テキストフィールド＋IME シナリオ） | S7〜S24 の統合をテストする |
| #210（S29） | 既知制約・プラットフォーム差異の文書化 | 既知制約・プラットフォーム差異を扱う（成果物: `docs/architecture/ime-known-constraints.md`） |
| #211（S30） | 使用ガイド・サンプル作成 | 本文書の契約を使ったアプリ側の実装例を作成する |

## 2. spec 対応

- **REQ-8**（`docs/spec/04-requirements.md` L114-124）: `wgpu`・`taffy`・`cosmic-text`・
  `winit`・`accesskit` を基盤として採用し、その上に「IME 統合」を含む欠落 5 領域を新規開発する。
  受け入れ基準 3「watchOS/visionOS 向けの `winit`/`accesskit` 対応が発展途上である制約を
  REQ-3 へ反映する」は TASK-8.6（別 issue）のスコープであり、本文書は §9 で参照のみ行う。
- **TASK-8.5**（`docs/spec/05-tasks.md` L281-287）: 成果物「IME 統合モジュール」、推定工数 15 日、
  前提タスク TASK-1.1（完了済み）、担当「Claude Code（分野: コア Rust）」。
- **REQ-2**（同 L42 以降）: 共通コア（`fandhe-core`）⇔ プラットフォーム最適化レイヤの二層
  アーキテクチャ。依存方向は最適化レイヤ → コアの一方向。本設計の契約は `fandhe-core` に
  winit・UIKit・`InputConnection` 等のプラットフォーム型を一切露出させない（§4.1）。
- **REQ-10**（同 L137〜）: CJK を含む多言語グリフの既定レンダリング。IME 統合は CJK 変換入力
  （かな漢字変換・拼音変換・２ボル式等）を成立させるための前提機能であり、本設計の
  コンポジション契約（§4.3）がこれを担う。
- **REQ-13 / NFR-9**（同 L171 以降・L256・L268）: 状態アクセスは `fn(&S) -> T` /
  `fn(&mut S, T)` の関数ポインタに限定し、クロージャによる環境キャプチャを拒否する。本設計は
  コンポジション状態を「一時的 UI 状態」として `S` に含めない二層モデル（`text_field.rs` の
  既存判断）を継承し、確定文字列のみが `ReadFn`/`WriteFn` 経由で `S` へブリッジされる。
- **REQ-9**（同 L125-135）: `accesskit` を通じたアクセシビリティ橋渡し。本設計は
  コンポジション中のテキストフィールドが `AccessibilityRole::TextInput`
  （既存 `text_field.rs` / `text_input.rs`）のまま扱われることを前提とし、コンポジション状態自体を
  アクセシビリティツリーへ追加公開しない（根拠・将来の再検討条件は
  `docs/architecture/ime-known-constraints.md` §4.1 を参照）。

## 3. 現状分析

### 3.1 `TextFieldWidget<S>` / `TextInput<S>`（既存実装）

- `crates/fandhe-core/src/ui/text_field.rs`（TASK-8.3-S14、Issue #109）はモジュール doc
  「スコープ外事項」で **「IME 変換合成（composition）・テキスト選択範囲・grapheme cluster
  単位のカーソル処理」を明示的にスコープ外**としている。現状は `TextFieldInput::InsertText`
  経由の char 単位・確定文字列のみの編集モデルであり、未確定文字列（preedit）を保持する
  フィールドを持たない。
- カーソルは **char 単位**で管理され（`byte_index_at` で byte index へ変換）、`InsertText` は
  `char::is_control` で制御文字を除去してから挿入する（OWASP A03 対策、モジュール doc 参照）。
  本設計のコンポジション契約もこの char 単位方針を継承する（§4.6）。
- `Debug` 実装は `buffer_len` / `committed_value_len` のみを出力し内容を出さない
  （機微情報の露出防止）。本設計のコンポジション状態（未確定文字列）も同じ方針を要求する
  （§4.3・§7）。
- `apply_input(InteractionInput::FocusLost)` は blur-commit（dirty かつ値が異なる場合のみ
  `Event::TextFieldChanged` を発火）を行う。本設計はフォーカス喪失時にコンポジションが
  残っている場合の扱いを追加で定義する（§4.7）。
- `crates/fandhe-core/src/ui/text_input.rs`（TASK-8.3-S13、Issue #108）は姉妹型 `TextInput<S>`
  であり、`ReadFn`/`WriteFn` によるビルダー API を提供するが、S14 時点でイベント配線は
  `TextFieldWidget<S>` 側にのみ存在する（2 型統合は既存のスコープ外事項のまま）。
- `crates/fandhe-core/src/ui/widget.rs` の `InteractionInput`（`PointerEntered` /
  `PointerExited` / `PointerPressed` / `PointerReleased` / `FocusGained` / `FocusLost`）と
  `InteractionState`（`hovered` / `pressed` / `disabled` / `focused`）がフォーカスモデルの
  基盤である。IME 有効化は `focused` と連動する契約とする（§4.2・§4.7）。

### 3.2 `fandhe-web-dom` の Web 限定 composition 対応（既存実装、参考実装）

- `crates/fandhe-web-dom/src/mount.rs` の `attach_input` は、`input` イベントの
  `InputEvent::is_composing()` が `true` の間は state 反映・全再構築を**スキップ**し、
  `compositionend` イベントで確実に commit する設計を採る。
  理由: 変換中に要素を破棄・再生成すると IME の composition セッションがブラウザ内部状態ごと
  失われる。また `isComposing: false` の追従 `input` イベントは Chromium/Safari で必ず発火する
  保証がないため、仕様上必ず発火する `compositionend` を commit の正とする（PR #341 の
  レビュー指摘、TASK-4.4-S2）。
- 本実装は「DOM の `<input>` 要素へブラウザの IME を素通しする」方式であり、未確定文字列
  （preedit）自体をアプリ側の状態やコア層へ渡してはいない。本設計が新設する `ImeEvent`
  契約（§4.1）は、この既存実装が扱っていない「preedit をコア側の状態として表示する」
  ケース（独自レンダラ方式の macOS/iOS/Android/デスクトップ Linux/Windows）を主眼とし、
  Web は §4.5 でマッピングを追加定義する。

### 3.3 `fandhe-egui-proxy`（参考実装、独自レンダラではない）

- `crates/fandhe-egui-proxy` は eframe 0.33 / winit 0.30 経由で `fandhe-core` の宣言的 UI を
  検証するプロキシであり、egui 自体が持つ IME 処理（`egui::Event::Ime` 相当）に委ねている。
  本設計の契約（§4）はこのプロキシを対象読者に含めない（egui が独自に IME を処理するため）。
  ただし winit 0.30 系のバージョン整合（`docs/architecture/swiftui-bridge-architecture.md`
  §3.4 と同じ MSRV 制約）は S7（#188）の実装時に確認する。

### 3.4 `crate-feature-matrix.md` の調査結果（PoC-7・2026-07 再調査）

- **`winit` Issue #1497**（IME/composition support の Tracking issue）は 2026-07 時点でも
  **open のまま**であることを確認済み（`docs/architecture/crate-feature-matrix.md` L35・L64・
  L108）。すなわち winit は個別の `Ime` イベント・`set_ime_*` メソッド群を既に提供しているが、
  「IME 統合の包括的な完成」を示すものではなく、個別 API の組み合わせで自前実装する前提が
  変わっていない。
- **`cosmic-text`** はクレート本体の公式 IME API 追加を示す一次情報が確認できておらず
  （COSMIC Edit 側の統合は確認したが、クレート自体の API ではない）、テキストレイアウト・
  カーソル位置計算（変換候補ウィンドウ位置の前提、S6）は自前で `cosmic-text` の測定 API を
  組み合わせる必要がある（同 L34）。
- 上記 2 点を踏まえ、本設計は **「winit の現行 `Ime::*` API で成立する範囲」を基準**にする
  （§9 で winit #1497 の進展監視を継続する）。

## 4. 設計判断（層間契約の核心）

### 4.1 契約の設計原則: プラットフォーム型を `fandhe-core` に露出しない

`fandhe-core` は `#![forbid(unsafe_code)]` のクレートであり、`winit::event::Ime` や
`web_sys::CompositionEvent`、iOS `UITextInput`、Android `InputConnection` 等の
プラットフォーム型に一切依存しない（REQ-2 の依存方向: 最適化レイヤ → コアの一方向）。
`widget-architecture.md` が taffy を `ui::layout` 内部に閉じた判断と同じ方針を採り、
本設計は **プラットフォーム非依存の `ImeEvent` / `ImeRequest` 列挙**をコア⇔プラットフォーム層
の境界インターフェースとして新設する（Rust 実装は S2〜S8 のスコープ、以下はシグネチャ案）。

```rust
/// プラットフォーム層（S7〜S22）からコア（S2〜S8）へ届く、
/// IME コンポジションの状態遷移を表す非依存イベント。
///
/// 外部由来の生バイト列・キーコードを直接扱わず、意味づけ済みの列挙のみで構成する
/// （`TextFieldInput` / `InteractionInput` と同じ OWASP A03 対策の型レベル防御、
/// `.claude/rules/security.md` 準拠）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeEvent {
    /// IME が有効化された（フォーカス獲得直後、`ImeRequest::SetEnabled(true)` への応答）。
    Enabled,
    /// コンポジション（未確定文字列）が更新された。
    Preedit {
        /// 現在の未確定文字列全体（空文字列はコンポジションのクリアを表す）。
        text: String,
        /// `text`（未確定文字列）内でのカーソル範囲（`CharOffset` 単位、§4.6 参照）。
        /// **`text_field.rs` の `TextFieldWidget::cursor`（確定済み `buffer` 内の位置）とは
        /// 別の座標系である点に注意** — 本フィールドは常に `text` の先頭を 0 とした
        /// 相対オフセットであり、`buffer` へのマッピング（挿入位置の決定）は S5 が
        /// コンポジション開始時のカーソル位置を基準に行う。
        /// `None` はカーソルを表示しないことを表す（winit `Preedit.1 == None` に対応）。
        cursor: Option<(CharOffset, CharOffset)>,
    },
    /// コンポジションが確定した（挿入すべき文字列を伴う）。
    Commit(String),
    /// IME が無効化された（フォーカス喪失、または `ImeRequest::SetEnabled(false)` への応答）。
    /// 次の `Enabled` まで `Preedit` / `Commit` は発生しない契約とする。
    Disabled,
}

/// バイト/UTF-16 表現に依存しない char 単位オフセット（§4.6 参照）。
/// 既存 `text_field.rs` の `cursor: usize`（char 単位）契約と型を揃える。
pub type CharOffset = usize;
```

```rust
/// コア（S5〜S8・S23）からプラットフォーム層（S9〜S22）への要求。
/// 双方向の要求チャネル（コールバック / イベントキュー等の実装形態は S7〜S22 に委ねる）。
#[derive(Debug, Clone, PartialEq)]
pub enum ImeRequest {
    /// IME 有効/無効を切り替える（フォーカスされたテキストフィールドの有無に連動、§4.7）。
    SetEnabled(bool),
    /// 変換候補ウィンドウの位置計算に使うカーソル矩形を通知する（論理ピクセル座標、S6 の前提）。
    /// プラットフォーム層が物理ピクセルへの変換・座標系変換を担う（コアはウィンドウの
    /// スケールファクタを知らない、REQ-2 の境界）。
    SetCursorArea { origin: (f32, f32), size: (f32, f32) },
    /// 入力目的のヒント（S23 の前提）。パスワード等では IME 無効化・学習抑止を要求できる。
    SetPurpose(ImePurpose),
    /// 新しい IME セッションの開始宣言（S24 の残存課題対応、Issue #587・§4.8）。
    BeginSession(ImeSessionId),
}

/// IME セッション識別子（Issue #587・§4.8）。`ImeFocusCoordinator::focus` が新フィールドを
/// IME 有効化するたびに払い出す不透明な連番。プラットフォーム層は以降の `ImeEvent` へ
/// この識別子を帰属させ `dispatch_ime` へ渡す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImeSessionId(u64);

/// 入力目的ヒント（winit `ImePurpose` 相当、S23 で公開 API 化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImePurpose {
    /// 既定（通常のテキスト入力）。
    Normal,
    /// パスワード等の機微情報（IME の学習・予測変換を抑止するヒント）。
    Password,
    /// 単一行の終端に Enter 相当が来るヒント（Terminal 等、必要になれば追加）。
    Terminal,
}
```

### 4.2 コア → プラットフォームの要求契約（§4.1 `ImeRequest` の意味論）

- **IME 有効/無効の切替**: `SetEnabled` はコア側の `InteractionState.focused` が
  `TextFieldWidget<S>`（IME 対応型、S8 で配線）へ遷移するたびに発火する。フォーカスが無い
  ウィンドウ・非テキスト系ウィジェットでは常に `SetEnabled(false)` を維持する契約とする
  （winit の `set_ime_allowed(false)` が既定である設計思想と整合、§3.4 出典）。
- **候補ウィンドウ位置**: `SetCursorArea` はテキストフィールドのカーソル位置（論理ピクセル、
  ウィジェットの `measure`/`paint` が使う座標系）を通知する。プラットフォーム層は
  winit の場合 `Window::set_ime_cursor_area(position, size)`
  （`PhysicalPosition<f64>` / `PhysicalSize<u32>`、要スケールファクタ変換）へ委譲する（S6・S9〜S14）。
- **入力目的ヒント**: `SetPurpose` はコアの公開 API（S23、例: `TextFieldWidget::set_purpose`）
  経由でアプリ側が設定し、プラットフォーム層は winit `Window::set_ime_purpose(ImePurpose)` 等へ
  変換する。`Password` 指定時はプラットフォーム層が IME の学習・予測変換を抑止する
  （§7 セキュリティ考慮）。

### 4.3 責務分界: コア = 状態保持・表示、プラットフォーム = ネイティブ IME API 相互変換

| 責務 | 担当 | 該当 sub-issue |
|------|------|----------------|
| コンポジション状態（preedit 文字列・カーソル範囲）の保持 | コア | S2〜S4 |
| コンポジション状態のウィジェット描画（下線・背景色等の視覚表現） | コア | S5 |
| 変換候補ウィンドウの位置計算（カーソル座標 → 論理ピクセル矩形） | コア | S6 |
| ネイティブ IME イベント（`Ime::*`・`compositionevent`・`UITextInput`・`InputConnection`）の受信・`ImeEvent` への変換 | プラットフォーム | S7・S15・S18・S21 |
| `ImeRequest` のネイティブ API（`set_ime_*`・DOM 属性・`UIResponder`・`InputMethodManager`）への変換 | プラットフォーム | S7・S9〜S14・S16・S19 |
| 確定文字列の `S` へのブリッジ（`Event::TextFieldChanged` 経由） | コア | S8（既存 `commit_if_changed` の延長） |
| フォーカス・IME 状態遷移の調停（複数フィールド間） | コア | S24 |

責務分界は既存 `widget-architecture.md` §3「二層モデル」（一時的 UI 状態は `S` に含めない）を
継承する。コンポジション状態（preedit）は `buffer` と同じく「編集中の一時的 UI 状態」であり、
**確定（`ImeEvent::Commit`）した文字列のみ**が `TextFieldInput::InsertText` 相当の経路で
`buffer` へ挿入され、その後は既存の `commit_if_changed` / `Event::TextFieldChanged` 契約に乗る
（新しい `Event` バリアントを追加しない）。

### 4.4 winit マッピング（デスクトップ: macOS・Windows・Linux、S7・S9〜S14）

| winit API | 方向 | `ImeEvent` / `ImeRequest` |
|-----------|------|---------------------------|
| `WindowEvent::Ime(Ime::Enabled)` | winit → コア | `ImeEvent::Enabled` |
| `WindowEvent::Ime(Ime::Preedit(text, cursor))` | winit → コア | `ImeEvent::Preedit { text, cursor: cursor.map(...) }`（byte offset → char offset 変換、§4.6） |
| `WindowEvent::Ime(Ime::Commit(text))` | winit → コア | `ImeEvent::Commit(text)` |
| `WindowEvent::Ime(Ime::Disabled)` | winit → コア | `ImeEvent::Disabled` |
| `Window::set_ime_allowed(bool)` | コア → winit | `ImeRequest::SetEnabled` |
| `Window::set_ime_cursor_area(PhysicalPosition, PhysicalSize)` | コア → winit | `ImeRequest::SetCursorArea`（論理→物理ピクセル変換をプラットフォーム層が担う） |
| `Window::set_ime_purpose(ImePurpose)` | コア → winit | `ImeRequest::SetPurpose` |

出典（一次情報、2026-07 時点）:
- [`winit::event::Ime`（docs.rs）](https://docs.rs/winit/latest/winit/event/enum.Ime.html) —
  `Enabled` / `Preedit(String, Option<(usize, usize)>)`（byte-wise インデックス、winit は
  `Commit` 直前に必ず空 `Preedit` を送る） / `Commit(String)` / `Disabled` の 4 バリアント。
- [`winit::window::Window`（docs.rs）](https://docs.rs/winit/latest/winit/window/struct.Window.html) —
  `set_ime_allowed(&self, allowed: bool)` / `set_ime_cursor_area(&self, position: PhysicalPosition<f64>, size: PhysicalSize<u32>)` / `set_ime_purpose(&self, purpose: ImePurpose)`。
- [`winit` Issue #1497（Tracking issue for IME / composition support）](https://github.com/rust-windowing/winit/issues/1497) —
  2026-07 時点でも open（`crate-feature-matrix.md` L108 の既存調査を継承）。

macOS（S9〜S10）は winit 内部で `NSTextInputClient` へ委譲されるため、`fandhe-core` から見た
契約は上記のみで完結する（S9 は winit 経由で来る `Ime::*` イベントが実機で期待どおり発火するかの
確認作業）。Windows（S11〜S12）は IMM32/TSF、Linux（S13〜S14）は ibus/fcitx への委譲も同様に
winit 内部で完結し、`fandhe-core` 契約には影響しない。

**macOS バックエンドの実挙動（#190）**: winit 0.30.13 の macOS バックエンド実装
（`src/platform_impl/macos/{view.rs,window_delegate.rs}`）を読解し、上記マッピング表が
ソースレベルで成立することを確認した。ただし `Window::set_ime_purpose` が macOS
（Wayland 以外全般）で no-op である点、`Ime::Enabled` が `SetEnabled(true)` の即時応答ではなく
最初のコンポジション開始時に遅延発火する点の 2 点は設計文書の暗黙の前提と差異があり、
`fandhe-winit-ime` / `fandhe-core::ui::ime` の doc コメントへ注記済み。

**Windows バックエンドの実挙動（#192）**: winit 0.30.13 の Windows バックエンド実装
（`src/platform_impl/windows/{event_loop.rs, ime.rs, minimal_ime.rs, window.rs,
window_state.rs}`）を読解し、上記マッピング表がソースレベルで成立することを確認した。
`Window::set_ime_purpose` が Windows でも no-op である点（S9 の macOS 所見と同型）、
`Ime::Enabled` が `WM_IME_STARTCOMPOSITION` 時に遅延発火する点（macOS と同型）に加え、
**`Ime::Disabled` がコンポジションを確定するたびに `WM_IME_ENDCOMPOSITION` で発火し、
macOS（入力ソース切替時のみ）より高頻度に `Disabled`→`Enabled` の再サイクルが起こりうる点**
が設計文書の暗黙の前提と差異があり、`fandhe-winit-ime` / `fandhe-core::ui::ime` の
doc コメントへ注記済み。また winit は Windows で TSF ネイティブではなく IMM32 API
（`windows_sys::Win32::UI::Input::Ime`）のみを直接使用し、TSF ベースの IME は OS 側の
IMM32 互換レイヤ経由で連携する位置づけであることも確認した。

**Windows の再サイクル契約（#193）**: winit 0.30.13 の Windows バックエンド
実装（`src/platform_impl/windows/{ime.rs,event_loop.rs}`）を読解し、上記マッピング表が
IMM32（`WM_IME_STARTCOMPOSITION` / `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION`）経由で
ソースレベルで成立することを確認した。macOS との最大の差異は **`Ime::Disabled` の発火頻度**
であり、macOS がフォーカス喪失・入力ソース切替時のみ発火するのに対し、Windows は
**1 回の変換完了（確定・キャンセルいずれも）ごとに必ず `Ime::Disabled` を発火する**
（`event_loop.rs` L1603-1608）。`TextFieldWidget::apply_ime` の `Disabled` 処理は
composition のクリアのみで冪等なため（`text_field.rs` §4.6）、この高頻度な
Enabled/Disabled サイクルを既存の状態モデルが変更なく許容できることを、winit が実際に
発火するイベント列を `ime_event_from_winit()` → `TextFieldWidget::apply_ime()` へ流す
結合テスト（`crates/fandhe-winit-ime/tests/windows_japanese_input.rs`、6 シナリオ）で確認した。実機
（Microsoft IME）での実操作検証は Issue #588 の対象（#15 Windows 実行環境整備が前提）。

**Linux バックエンドの実挙動（#194）**: winit 0.30.13 の Linux バックエンド実装
（X11: `src/platform_impl/linux/x11/{ime/*, window.rs, event_processor.rs}`、
Wayland: `src/platform_impl/linux/wayland/{seat/text_input/mod.rs, window/mod.rs,
window/state.rs}`）を読解し、上記マッピング表が X11・Wayland 双方でソースレベルで
成立することを確認した。**`Ime::Enabled`/`Ime::Disabled` が macOS・Windows とは逆方向に、
`ImeRequest::SetEnabled` 呼び出し（X11）・フォーカス変化やその呼び出し（Wayland）へ
同期的・即時に発火する点**（コンポジション開始/終了までの遅延がない）が設計文書の
暗黙の前提と差異があり、`fandhe-winit-ime` / `fandhe-core::ui::ime` の doc コメントへ
注記済み。また `Window::set_ime_purpose` は X11 では no-op（macOS・Windows と同型）だが
**Wayland のみ唯一 `ContentHint`/`ContentPurpose` へ実際にマップされる**という Linux
内部での非対称性も確認した。ibus/fcitx は X11 では XIM ブリッジ
（`XMODIFIERS=@im=ibus|fcitx` 環境変数）、Wayland ではコンポジタの
`zwp_text_input_v3` 実装を介した間接連携であり、winit・`fandhe-winit-ime` のいずれも
ibus/fcitx を直接意識しない。

**Linux バックエンドの実挙動（#195）**: winit 0.30.13 の Linux バックエンド
実装（X11: `src/platform_impl/linux/x11/ime/{mod.rs,context.rs,callbacks.rs}`・
`event_processor.rs`、Wayland: `src/platform_impl/linux/wayland/seat/text_input/mod.rs`・
`window/mod.rs`）を読解し、上記マッピング表が ibus/fcitx が XIM（X11）・
`zwp_text_input_v3`（Wayland）を介して連携する経路でソースレベルで成立することを確認した。
macOS・Windows との最大の差異は **`Ime::Enabled`/`Disabled` の発火タイミング**であり、
macOS/Windows が「コンポジション開始/終了まで遅延発火」なのに対し、Linux（X11・Wayland
共通）は **`SetEnabled` 要求・フォーカス変化に同期して即時発火**する（第三のパターン）。
加えて `set_ime_purpose`（`SetPurpose` 要求）は X11 では macOS・Windows と同様に no-op だが、
**Wayland のみ**デスクトップ 3 プラットフォーム中で唯一 `ContentHint`/`ContentPurpose` へ
実際にマップされる非対称性がある（Password の学習抑止は `ime-known-constraints.md`
§3.3 を参照）。X11 の preedit カーソルは
常に `start == end` の単一点である点も Windows/Wayland との差異として記録した。
`TextFieldWidget::apply_ime` の既存状態モデルはこれらの差異を変更なく許容できることを、
winit が実際に発火するイベント列を `ime_event_from_winit()` →
`TextFieldWidget::apply_ime()` へ流す結合テスト
（`crates/fandhe-winit-ime/tests/linux_japanese_input.rs`、7 シナリオ）で確認した。実機（ibus-mozc /
fcitx5-mozc）での実操作検証は Issue #588 の対象。

**韓国語・中国語入力（#206）**:
S9〜S14 が確定させた「winit の `Ime::Enabled`/`Preedit`/`Commit`/`Disabled` は IME 方式に
依存しないプレーンな契約であり、韓国語（2 ボル式・漢字変換）・中国語（拼音・注音）専用の
winit 側分岐は存在しない」ことを前提に、これら言語固有のイベント**パターン**が既存の
`TextFieldWidget` 状態モデルで問題なく処理できることをシーケンス再現結合テスト
（`crates/fandhe-winit-ime/tests/korean_chinese_input.rs`、10 シナリオ）で確認した。
韓国語 2 ボル式は 1 打鍵ごとに音節が完成しうるため、Windows の「変換確定ごとに
`Disabled`→`Enabled` が再サイクルする」（S12 所見）頻度が日本語よりさらに高くなる点、
中国語 拼音は preedit が ASCII 文字列（"nihao"）である一方 `Commit` は複数文字の CJK
文字列（"你好"）になり文字種・長さが乖離する点、中国語 注音は声調記号込みのマルチバイト
preedit になる点が言語固有の特徴として確認できたが、いずれも `ime_event_from_winit()` /
`TextFieldWidget::apply_ime()` の既存契約を変更する必要はなかった。実機（macOS 韓国語 IME・
Microsoft Pinyin・ibus-hangul / fcitx5-rime 等）での実操作検証は Issue #588 の対象。
Web（`compositionevent`）経路での韓国語・中国語の実操作検証は、実機検証として Issue #588 の対象に含める。

### 4.5 プラットフォーム別マッピング（モバイル・Web）

#### iOS（UIKit、S15〜S17）

`UITextInput` プロトコルの marked text（コンポジション）API を `ImeEvent` へ変換する
（[Apple Developer Documentation: UITextInput](https://developer.apple.com/documentation/uikit/uitextinput)）。

| UIKit API | 方向 | `ImeEvent` / `ImeRequest` |
|-----------|------|---------------------------|
| `setMarkedText(_:selectedRange:)` 呼び出し | UIKit → コア | `ImeEvent::Preedit { text, cursor }`（`selectedRange` は UTF-16 `NSRange`、§4.6 変換必須） |
| `unmarkText()` 呼び出し + 直後の確定挿入 | UIKit → コア | `ImeEvent::Commit(text)` |
| `markedTextRange` の有無 | UIKit → コア（状態確認） | `Enabled`/`Disabled` はキーボード表示・フォーカス在席と連動（UIKit にはコンポジション専用の enable/disable API がなく、`becomeFirstResponder`/`resignFirstResponder` に連動させる） |
| カーソル矩形通知 | コア → UIKit | `UITextInput` の `caretRect(for:)` / `selectionRects(for:)` を実装してシステムへ返す形（winit の `set_ime_cursor_area` のような明示 API ではなく、プロトコルメソッドの実装として提供する点が差異） |

詳細設計（`UITextInputTokenizer` との整合、ソフトウェアキーボードのライフサイクル）は S15
（#196、詳細設計済み。下記追記参照）のスコープとする。本文書は UIKit 側が「marked text」
という異なる語彙を使うことのみを契約として明記する。

**winit iOS バックエンドの実挙動（#197）**: winit 0.30.13 iOS バックエンド
（`platform_impl/ios/{view.rs,window.rs}`）を確認した結果、`WinitView` は `UITextInput`
プロトコル自体を実装しておらず（空の `unsafe impl UITextInputTraits for WinitView {}`）、
`UIKeyInput`（`hasText`/`insertText:`/`deleteBackward`）のみを実装する。したがって
**`WindowEvent::Ime(Ime::*)` は winit 0.30 系の iOS バックエンドでは一切発火しない**
（上表は S15 が対象とする将来的な `UITextInput` 完全実装後の理想形であり、winit
現行実装はこの理想形に未到達）。`insertText:` は確定済み文字列を 1 文字ずつ
`KeyboardInput`（`Key::Character`）として送出し、`deleteBackward` は
`Key::Named(NamedKey::Backspace)` として送出する。この経路は `fandhe-winit-ime` の
`text_field_input_from_key` で `TextFieldInput::InsertText`/`DeleteBackward`
（既存の編集入力契約）へ変換し、`ImeRequest::SetEnabled` はキーボードの表示/非表示
（`becomeFirstResponder`/`resignFirstResponder`）として作用する。カーソル矩形通知
（`set_ime_cursor_area`）と `set_ime_purpose` は winit iOS バックエンドでは no-op
（`warn!` ログのみ）であることも確認済み。
インライン preedit（marked text）の実現には winit 側の `UITextInput` 対応または独自
ビュー実装が必要であり、本 issue のスコープ外（§9 参照）。

**commit 型入力経路の契約（#198）**: 上記が確定する commit 型入力経路（`insertText:`/
`deleteBackward` → `TextFieldInput::InsertText`/`DeleteBackward`）について、変換候補バー
確定文字列が 1 回の `insertText:` 呼び出しにまとまって届く場合の原子的挿入・その直後の
`deleteBackward` の文字単位削除・iOS には preedit が存在しないためフォーカス喪失時に
破棄される未確定入力が存在しないことをシーケンス再現結合テストで確認した。

**iOS 詳細設計（#196）**: 現行 commit 型方式（`UIKeyInput` 素通し）を iOS の当面の
正式方式とし、3 点——`ImeRequest::SetCursorArea` の内部変換設計（pull 型プロトコルメソッドへの
アダプタ変換）・`UITextInputTokenizer` との整合（既定実装 `UITextInputStringTokenizer` の
採用）・ソフトウェアキーボードのライフサイクル（`Enabled`/`Disabled` イベントは UIKit から
発火しない契約の明記）——を確定する。将来の `UITextInput` フル対応（インライン preedit）は
独自 `UITextInput` 準拠ビュー実装によってのみ実現可能であり（winit 側の対応待ちは
スケジュールと両立しないため不採用）、実装着手はユーザー承認を要する別 issue の判断とする
設計のみを確定した。詳細は `docs/architecture/ime-ios-uikit-text-input.md` を参照。

#### Android（`InputConnection`、S18〜S20）

`InputConnection` の composing text API を `ImeEvent` へ変換する
（[Android Developers: InputConnection](https://developer.android.com/reference/android/view/inputmethod/InputConnection)）。

| Android API | 方向 | `ImeEvent` / `ImeRequest` |
|--------------|------|---------------------------|
| `setComposingText(CharSequence, int)` 呼び出し | InputConnection → コア | `ImeEvent::Preedit { text, cursor }`（`newCursorPosition` は挿入後相対位置であり、winit の絶対範囲とは意味が異なる点に注意、S18 で変換ロジックを設計） |
| `commitText(CharSequence, int)` 呼び出し | InputConnection → コア | `ImeEvent::Commit(text)` |
| `finishComposingText()` 呼び出し | InputConnection → コア | コンポジションの確定を伴わない終了。`ImeEvent::Commit(現在の preedit text)` として扱うか `Disabled` として扱うかは S18 で確定する（未解決事項、§9） |
| `InputMethodManager.showSoftInput` / `restartInput` | コア → Android | `ImeRequest::SetEnabled` |
| `getCursorCapsMode` 等のクエリ応答 | コア → Android（間接） | 本設計のスコープ外（テキスト大文字化ヒントは IME 統合の中核契約に含めない） |

`InputConnection` は Android の `View`（`GameActivity`/`NativeActivity` 経由）が実装する
インターフェースであり、winit 側の Android バックエンド実装詳細との整合は S18（#199）の
詳細設計スコープとする。

**winit Android バックエンドの実挙動（#200）**: winit 0.30.13 Android バックエンド
（`platform_impl/android/mod.rs`）と android-activity 0.6.1（NativeActivity 方式）を確認した
結果、`handle_input_event` は `InputEvent::KeyEvent` のみを `WindowEvent::KeyboardInput` へ
変換し、それ以外（`InputEvent::TextEvent` を含む）は `warn!` ログのみで無視する。したがって
**`WindowEvent::Ime(Ime::*)` は winit 0.30 系の Android バックエンドでは一切発火しない**
（上表は S18 が対象とする将来的な `InputConnection` 完全実装後の理想形であり、winit 現行実装
はこの理想形に未到達。iOS・S16 と同型の状況）。`InputEvent::KeyEvent` は
`character_map_and_combine_key`（JNI `KeyCharacterMap` 照会 + dead-key 結合）を経て 1 キーずつ
`KeyboardInput`（`Pressed` 時のみ `logical_key.to_text()` が `text` に載る）として送出され、
`KEYCODE_DEL` は `NamedKey::Backspace` へ正規化される。この経路は `fandhe-winit-ime` の
`text_field_input_from_key`（S16 で追加済み、iOS と同一の変換規則）でそのまま
`TextFieldInput::InsertText`/`DeleteBackward` へ変換し、`ImeRequest::SetEnabled` は
`AndroidApp::show_soft_input`/`hide_soft_input` に委譲されてソフトウェアキーボードの
表示/非表示として作用する。`set_ime_cursor_area`/`set_ime_purpose` は winit Android
バックエンドでは no-op（空実装）であることも確認済み。また `InputEvent::TextEvent`
（`InputConnection`/GameTextInput 由来の composing text）は android-activity 0.6.1 では
GameActivity バックエンド限定でのみ発火し、本リポジトリが採用する NativeActivity 方式では
到達経路が存在しない。インライン
preedit（`setComposingText` の反映）の実現には GameActivity 方式への切替
（既存 issue #241・TASK-9.3 のスコープ）または独自の `InputConnection` 実装が必要であり、
本 issue のスコープ外（§9 参照）。

**Android 詳細設計（#199）**: winit 0.30.13 の Android バックエンド実装
（`src/platform_impl/android/mod.rs`）は **winit は Android で `Ime::*` イベントを
一切発火せず、`android-activity` の `InputEvent::TextEvent`（GameTextInput 経由の
コンポジション更新）も未処理である**（macOS/Windows/Linux のように
winit が `Ime::*` へ変換してくれる前提が Android では成立しない）。また既存
`fandhe-egui-proxy`（TASK-1.5-S2）が採用する **NativeActivity flavor は組み込みの IME 対応を
持たず、CJK コンポジション入力が原理的に成立しない**ため、本設計は
**GameActivity + `GameTextInput` 方式への移行を前提**とし、winit を経由せず
`android-activity` の `AndroidApp`（`TextInputState`）を直接操作する新クレート
（`fandhe-android-ime` 案）を、GameActivity 方式へ移行した後の実装対象として設計する
（現行実装は NativeActivity 方式の commit 型経路（`fandhe-winit-ime`）であり、
GameActivity への切替は既存 issue #241・TASK-9.3 のスコープ）。`newCursorPosition`
（相対位置）→ 絶対 char 範囲の変換ロジック、
`finishComposingText()` の意味論（`ImeEvent::Commit` 相当として確定、下記参照）を含む詳細は
`docs/architecture/ime-android-input-connection.md` を参照。

**Android commit 型入力の契約（#201）**: 上記が確定する commit 型入力経路
（`KeyEvent` → `TextFieldInput::InsertText`/`DeleteBackward`）について、複数文字が 1 回の
`KeyEvent` にまとまって届いた場合でも分割されず原子的に挿入される契約・その直後の
`KEYCODE_DEL` の文字単位削除・Android には preedit が存在しないため複数文字を原子挿入した
直後にフォーカスを失っても入力が破棄されないことをシーケンス再現結合テストで確認した
（現行 winit 実装の `character_map_and_combine_key` は常に結合済み 1 文字を返すため、
複数文字が 1 回で届く経路自体は現行実装では発生せず、将来の実装変更・GameActivity 移行時
の回帰ガードとして維持する）。設計（GameActivity + `GameTextInput` 方式、`compose_region`
差分監視）と現行実装（NativeActivity 方式を維持したままの commit 型経路）は乖離している。

#### Web（`compositionevent`、S21〜S22）

W3C UI Events の composition イベント
（[W3C UI Events: Composition Events](https://w3c.github.io/uievents/#events-compositionevents)）を
`ImeEvent` へ変換する。

| DOM イベント | 方向 | `ImeEvent` / `ImeRequest` |
|--------------|------|---------------------------|
| `compositionstart` | DOM → コア | `ImeEvent::Enabled` |
| `compositionupdate`（`CompositionEvent.data`） | DOM → コア | `ImeEvent::Preedit { text: data, cursor: None }`（DOM の compositionevent はカーソル範囲を公開しないため `cursor` は常に `None`、§9 未解決事項） |
| `compositionend`（`CompositionEvent.data`） | DOM → コア | `ImeEvent::Commit(data)` |
| （IME 有効/無効を明示的に切り替える DOM API はない） | コア → DOM | `ImeRequest::SetEnabled` は no-op（ブラウザの IME 制御はネイティブ `<input>`/`<textarea>` 要素の存在に暗黙依存する契約とする） |

**既存 `fandhe-web-dom/src/mount.rs` との整合**: 既存実装は「DOM の `<input>` 要素へブラウザの
IME を素通しし、`compositionend` で commit する」方式（§3.2）であり、preedit をコア側の状態と
して表示していない。本設計の `ImeEvent::Preedit` は独自レンダラ方式のプラットフォーム
（macOS/iOS/Android/デスクトップ Linux/Windows）で「コア側が preedit を描画する」ために必要な
契約であり、Web（DOM 出力方式）では `compositionupdate` を購読しても実際に描画へ反映するかは
S21（#202）の設計判断とする（DOM が既にネイティブ IME の視覚的表現を提供するため、二重表示に
ならない配線が必要）。既存の「`isComposing` 中は state 反映をスキップし `compositionend` で
commit する」設計判断（TASK-4.4-S2）は変更しない。

**Web 実装（#202）**: 上表の変換を `fandhe-web-dom/src/ime.rs`
（[`composition_phase_from_event_type`]・[`ime_event_from_composition`]）へ純粋な変換関数と
して実装した。`web-sys::CompositionEvent` は `wasm32` 専用型のため、変換関数自体はフェーズ
列挙（`CompositionPhase`） + `&str` という target 非依存の入力で設計してあり、native
（`cargo test`）でユニットテストできる（`fandhe-winit-ime` と同型の設計判断）。
`fandhe-web-dom/src/mount.rs` の `attach_input` を、`compositionstart`/`compositionupdate`/
`compositionend` の 3 イベント全てが `ime_event_from_composition` を単一の変換経路として
経由する形に配線した。ただし二重表示回避の設計判断（上段落）に従い、`Enabled`
（`compositionstart`）・`Preedit`（`compositionupdate`）はコア描画へ反映せず no-op のまま
とし、`Commit`（`compositionend`）到達時のみ既存の commit 経路を呼ぶ。この commit 経路が
実際に反映する値は `ImeEvent::Commit` が運ぶ合成断片（`CompositionEvent.data`）ではなく、
従来どおり `input.value()`（`Node::TextField::set` が全文置換型の `WriteFn(S, String)` で
あるため）である点に注意する。`ImeRequest::SetEnabled` に対応する DOM API 実装（Web での
no-op 化）は `ImeRequest` を扱わず、`mount.rs` では no-op が暗黙に成立している。シーケンス
再現テストは `fandhe-web-dom/tests/web_composition_ime.rs`。実ブラウザでの実操作検証は人間担当（Issue #588）。
`ImePurpose` の Web 対応（S23・#204）・複数フィールド間フォーカス遷移調停（S24・#205）は
引き続きスコープ外。

**headless Chromium での実 DOM 検証（#203）**: 上記が確定する `attach_input` composition 配線
（`ime_event_from_composition` を単一経路とする 3 イベント配線・`Enabled`/`Preedit` の
no-op・`compositionend` での `input.value()` 全文コミット）は headless Chromium 上の
実 DOM を通して動作する。CDP の `Input.imeSetComposition`（実験的
API）は headless 環境での挙動保証が一次情報から確認できないため、`Runtime.evaluate` に
よる合成 `CompositionEvent`/`InputEvent` の `dispatchEvent` 方式を採用し、標準入力
（「か」→「かん」→「漢字」の commit）・変換
キャンセル（`compositionend.data` が空文字列）・`compositionend` 後の後続
`input`（`isComposing:false`）による二重挿入がないことの 3 シナリオすべてで
再構築後の `<input>.value` が期待値と一致することを確認した。値の一致だけでは
テストスクリプト自身が書き込んだ値を読み返しているに過ぎず配線の不活性を検出
できないため、`compositionend`（シナリオ 3 は追加で末尾 `input`）発火直前に
`<input>` へ付与した `dataset` マーカーが、`mount.rs` の全再構築（要素差し替え）で
実際に消えていることも合わせて確認し、`compositionend` リスナーを意図的に
無効化した状態でマーカー消失の assertion のみが fail に反転することで検証した
（合成イベントは実 IME 実装を経由しないため「実 IME ソフトが実際にこのイベント列を
発火させること」自体は未検証、限界は検証記録に明記）。また S21 §5 チェックリストが
扱っていない日本語入力観点
（文節ごとの連続確定・preedit 途中の縮退・composition 中の `FocusLost` 到達・サロゲート
ペア・全角英数・`compositionstart` を伴わない防御的順序異常）をシーケンス再現結合テストで
追加固定化した。実ブラウザ（Firefox/Safari）・実 IME ソフト・モバイル・スクリーンリーダー
での実操作確認は自動化できず、引き続き人間担当チェックリストへ残置している。追加テストは
`fandhe-web-dom/tests/web_japanese_input.rs`、検証スクリプトは
`scripts/web/verify-web-ime.sh`/`verify-web-ime.mjs` を参照。

### 4.6 文字インデックス正規化方針（S4・S26 の前提）

プラットフォームによってカーソル・範囲のインデックス基準が異なる:

| プラットフォーム | インデックス基準 | 出典 |
|-----------------|-----------------|------|
| winit `Ime::Preedit` | byte offset（UTF-8） | [`winit::event::Ime`](https://docs.rs/winit/latest/winit/event/enum.Ime.html) |
| Web `compositionupdate` | インデックス非公開（`data` 文字列のみ、カーソル位置は取得不可） | [W3C UI Events](https://w3c.github.io/uievents/#events-compositionevents) |
| iOS `UITextInput` (`NSRange`) | UTF-16 code unit offset | [Apple Developer Documentation: UITextInput](https://developer.apple.com/documentation/uikit/uitextinput) |
| Android `InputConnection` | UTF-16 code unit offset（`CharSequence` 基準。ただし `setComposingText` の `newCursorPosition` は絶対位置ではなく挿入後の相対位置であり、絶対範囲への変換ロジックは S18 が設計する、§4.5 参照） | [Android Developers: InputConnection](https://developer.android.com/reference/android/view/inputmethod/InputConnection) |

**方針**: `fandhe-core` の契約（`ImeEvent::Preedit.cursor`）は既存 `text_field.rs` の
`cursor: usize` と同じ **char 単位（`chars().count()` 基準）** に統一する
（`byte_index_at` と同型の変換関数を要求する契約とする）。プラットフォーム層が
byte offset（winit）・UTF-16 offset（iOS/Android）から char offset への変換を担う。

- 変換は `str::char_indices()` を使い、**panic を API 契約にしない**（範囲外・不正境界は
  末尾へクランプする、`byte_index_at` と同じ設計判断、coding-rust.md 準拠）。
- サロゲートペア（絵文字等）を含む UTF-16 offset → char offset 変換では、サロゲートペアの
  中間点を指すオフセットが渡された場合に文字境界へクランプする契約とし、`Result` を返す
  変換関数（S4 が実装）として設計する。境界条件の網羅的な確認は S26（#207）で行う。
- Web の `compositionupdate` はカーソル位置を提供しないため、`ImeEvent::Preedit.cursor` は
  常に `None` とし、コア側の描画（S5）は「preedit 文字列の末尾にキャレットを置く」既定動作に
  フォールバックする契約とする（§9 未解決事項として明記）。

**編集操作は grapheme cluster 単位で作用する**: char 単位オフセット契約
（`ImeEvent::Preedit.cursor` / `CharOffset` / プラットフォーム変換関数群の座標系）は
変更しない（S4 実装のまま）。一方、`text_field.rs::handle_input` の編集操作
（`DeleteBackward` / `DeleteForward` / `MoveCursorLeft` / `MoveCursorRight`）は
grapheme cluster（UAX #29、`unicode-segmentation` クレートへ判定を委譲）単位で作用し、
カーソルは cluster 内部に立たない不変条件を持つ。char 単位のまま実装すると、肌色修飾子つき
絵文字（👍🏽）・ZWJ 連結絵文字（👨‍👩‍👧‍👦）・Regional Indicator の国旗（🇯🇵）・結合文字
（e + U+0301 等）を分断するためである。`fandhe-core` の `ui::text_selection` が
`prev_grapheme_boundary` / `next_grapheme_boundary` / `is_grapheme_boundary` を提供する
（自前実装は正しさのリスクが高く不採用）。プラットフォーム境界の座標系
（byte offset・UTF-16 offset・char offset の変換）には影響しない。

**未対応事項**:

- Regional Indicator の隣接挿入によるカーソル位置起因の国旗の意図しない合成（挿入位置の
  検証、テキスト計測実装に依存）。
- grapheme cluster の実測幅（cosmic-text 導入が必要。現状は等幅近似
  `CHAR_WIDTH` を char 数ベースで適用しているため、複数 char から成る 1 クラスタでも
  クラスタ内の char 数分の幅を占有してしまう）。
- preedit 内カーソル範囲（プラットフォーム提供値）が grapheme cluster の内部を指す場合の
  表示上のスナップ（`ime_cursor_area.rs` / `composition_display.rs` は char 境界の範囲で
  panic しないことのみを保証し、grapheme 境界へのスナップは行わない）。

### 4.7 フォーカス遷移との結合（S24 の前提）

既存 `InteractionInput::FocusGained` / `FocusLost` と IME 状態遷移を以下のように結合する:

- `FocusGained` が `InteractionState.focused` を `true` にした直後、コアは
  `ImeRequest::SetEnabled(true)` をプラットフォーム層へ送る契約とする（IME 対応ウィジェット
  のみ。`AccessibilityRole::TextInput` を持つウィジェットに限定）。
- `FocusLost` が `focused` を `false` にする際、**コンポジション中（preedit が空でない）
  であれば先にコンポジションを破棄・確定するかを決定してから** `ImeRequest::SetEnabled(false)`
  を送る。既存 `commit_if_changed`（blur-commit）と同様に、未確定文字列を破棄せず確定として
  扱うか、破棄するかはプラットフォームの標準挙動（winit は `Ime::Disabled` 前に
  `Ime::Commit`/空 `Preedit` を送る契約、§4.4 出典）に委ね、コア側は「`ImeEvent::Disabled` を
  受け取った時点でコンポジション状態（preedit 表示）を必ずクリアする」というクリーンアップ
  契約のみを持つ（プラットフォームごとの確定/破棄の意味論差はプラットフォーム層が吸収し、
  コアへ漏らさない）。**`Disabled` 到達前に `Commit` が届いていた場合、その文字列は
  `Disabled` 受信時点で既に `buffer` へ適用済みである**（`ImeEvent::Commit` はコンポジション
  状態のクリアとは独立して即時に確定文字列を反映する契約、§4.3）。したがって
  `Disabled` ハンドラは preedit 表示状態のリセットのみを行い、`Commit` 済みの文字列を
  再適用・破棄してはならない（二重適用・意図しない巻き戻しの防止、S8 の実装契約）。
- 複数テキストフィールド間のフォーカス移動（フィールド A → フィールド B）では、
  A の `FocusLost` → A の `ImeRequest::SetEnabled(false)` → B の `FocusGained` →
  B の `ImeRequest::SetEnabled(true)` の順序を保証する（同時に 2 つのフィールドが
  IME 有効になる状態を作らない、S24 の実装契約）。
- `set_disabled(true)`（既存 `TextFieldWidget::set_disabled`）がフォーカスを奪う経路も
  同じ `FocusLost` 相当の IME 無効化契約に従う（既存の blur-commit 呼び出し順序を変更しない）。

### 4.8 IME セッション識別子（S24 の残存課題への対応、Issue #587）

`ImeFocusCoordinator`（§4.7・S24）は、フォーカス遷移直後の旧フィールド由来の遅延イベントの
誤配置に対し、IME セッション識別子による帰属照合（Issue #587）と `armed` ゲートの二段構えで
対処する。`armed` ゲート単体では旧セッションの「空 `Preedit` → `Commit`」がペアで
遅延到着するとゲートが開くため、識別子照合を第一段とする。

**生成責務**: `ImeFocusCoordinator`（コア）が SSOT。`focus()` が新フィールドを IME 有効化
するたびに `ImeSessionId`（不透明な連番、`ui::ime::ImeSessionId`）を払い出し、新フィールドの
`SetPurpose`/`SetEnabled(true)` より前に `ImeRequest::BeginSession(id)` を発行する。
生成をコア側に置くのは、coordinator が「現在どのフィールドのセッションか」の SSOT であり
自分が払い出した識別子を照合するだけで済むため（層間でカウンタ意味論を合わせる必要がない）。
winit アダプタ（`fandhe-winit-ime`）はネイティブのセッショントークンを持たない（下表参照）ため
プラットフォーム発行にしても情報量は増えず、Web は DOM 要素単位で composition が閉じ、
iOS/Android は `Ime::*` 自体が発火しないため、識別子を必要とするのは winit デスクトップ
経路のみである。

**伝搬経路**: `BeginSession` → プラットフォーム層（`fandhe-winit-ime::session::ImeSessionTracker`
等）が以降の `Ime::*`/`ImeEvent` へ帰属セッションを付ける → `ImeFocusCoordinator::dispatch_ime`
（`fields`・`session: Option<ImeSessionId>`・`event` を受け取る形へ拡張済み）が
`self.session()` と一致しないイベント（`Enabled`/`Preedit`/`Commit`、`None` を含む）を
`Disabled` 以外すべて破棄する。`Disabled` はセッション不問で全フィールドへ配る（§4.6 の
冪等クリーンアップ契約を維持）。

**プラットフォーム別の付け替え規則**（winit 0.30.13、`Cargo.lock` 固定のソース照合に基づく。
`BeginSession` を受けても即座には切り替えず、旧セッションの終了信号（`Disabled`）または新
セッションの開始信号（`Enabled`）を観測してから切り替える）:

| プラットフォーム | 付け替え契機 | 根拠 |
|---|---|---|
| macOS | blur 未要求（同一フィールド内の入力ソース切替）: `Disabled` 到達、または非空 `Preedit`。blur 要求済み（フィールド間フォーカス遷移）: `Enabled`/`Disabled` 到達まで昇格しない | `set_ime_allowed(false)` は同期的に marked text を破棄し `ime_state` を `Disabled` にする。次の `setMarkedText:` は `ime_state == Disabled` を経由するため必ず `Enabled` を先に発行する。よって blur 要求済みの間は `Enabled`/`Disabled` を待てば十分（`Commit` は必ず非空 `Preedit` が先行する、§4.4 出典）。blur 未要求の入力ソース切替では `Disabled` 自体が届かないことがあるため（S9 の検証結果）非空 `Preedit` も昇格契機に含める |
| Windows | `Enabled`（`WM_IME_STARTCOMPOSITION`）到達（IMM32 同期配送の範囲。TSF 非同期配送は残存制約、下記参照） | `Enabled` は正当な `Commit` に必ず先行する。`Disabled` 到達前に旧セッションの「空 Preedit → Commit」が届く形（本 Issue の主対象）は IMM32 単独駆動なら正しく旧セッションへ帰属できるが、TSF 経由の通知タイミングを保証する一次情報がない（Issue #4） |
| Linux（X11） | `Disabled`→`Enabled` の同期発火ペア | XIC がフォーカス遷移で破棄・再生成され、新 XIC 生成時に `Enabled` が同期発火する（§4.4 出典） |
| Web（DOM） | 対象外 | `<input>` 要素ごとに composition が閉じるため、要素（イベント `target`）が帰属根拠になり識別子は不要 |
| iOS / Android | 対象外 | `Ime::*` 自体が発火しない（commit 型経路のみ、§4.5） |

**「Enabled ゲート」（旧フィールドが blur 要求済みの間は非空 `Preedit` を昇格契機にしない）**:
`fandhe-winit-ime::session::ImeSessionTracker` は旧フィールドがコア側から
`ImeRequest::SetEnabled(false)` を要求済みの間、`Enabled` の到着のみを新セッションの開始信号
とし、非空 `Preedit` では昇格しない。非空 `Preedit` を即座に昇格契機とすると、
`SetEnabled(false)` 発行と native 側の実際の無効化完了の間に native がキューイング済みの
旧フィールドの `Preedit`/`Commit` が `Enabled` を伴わずに届く場合（Windows 等）、それを
新セッションの開始信号と誤認し、後続の旧フィールドの確定文字列が新フィールドへ誤帰属する。
blur 要求が無い状態（フォーカス遷移を伴わない、同一フィールド内の入力ソース切替）での
非空 `Preedit` 昇格は維持する。

**`armed` ゲートを撤去しない理由**: 上記のルールでも判別できない残存経路が Wayland・
Windows にある。Wayland は `zwp_text_input_v3` の `done` ハンドラが `ime_allowed`/`serial`
を見ずに、旧フィールドの遅延イベントバッチを新フィールドの `Enabled` の「後」に送出しうる
（winit 側が serial を検証しない実装上の限界）。Windows は `set_ime_allowed` が `ime_state`
をリセットしないため、TSF 経由で旧フィールドの `Disabled`→`Enabled` 再サイクル
（`WM_IME_ENDCOMPOSITION → WM_IME_STARTCOMPOSITION → WM_IME_COMPOSITION`）が
`BeginSession` の後に届くと、`Enabled` ゲート単体では「旧フィールドの再サイクル」と
「新フィールドへの遷移」を区別できない。いずれも
`ImeSessionTracker` の Enabled ゲートでは判別できず、`armed` ゲートは撤去せず、この
残存経路に対する多層防御として維持する（残存限界・追跡 Issue #4 は
`ime-known-constraints.md` §3.6 に記録する）。

**デッドロック検証**: 正当な `Commit`/`Preedit` が誤って旧セッション判定されて落ちないこと
（macOS: `Commit` は非空 `Preedit` が必ず先行し `Disabled` も同期発火、Windows: IMM32
同期配送の範囲では `Commit` に `Enabled` が必ず先行する（TSF 非同期配送は残存制約、
Issue #4）、Linux: `Disabled`/`Enabled` が同期発火。いずれも正当な新セッションの
イベントは付け替え契機を必ず経由してから届く）。旧セッションがアイドル（未変換のまま
フォーカス移動）で `Disabled` が届かないケースも、新セッションの `Enabled` で正しく昇格できる
（`fandhe-winit-ime::session` モジュールのユニットテスト・`tests/focus_transition_session.rs`
で検証済み）。

## 5. S2〜S30 への接続点

§1 の分担表を参照。要約:

- **状態モデル系（S2〜S6）**: §4.1 の `ImeEvent`／§4.3 の責務分界を受けて、コンポジション状態の
  内部表現（バッファ構造・カーソル表現）・描画・候補ウィンドウ位置計算を設計・実装する。
- **プラットフォーム実装系（S7〜S22）**: §4.4・§4.5 の各マッピング表を実装対象として引き継ぐ。
- **横断・仕上げ系（S23〜S30）**: §4.2 の入力目的ヒント公開（S23）、§4.7 のフォーカス遷移
  調停（S24）、§4.6 の境界条件検証（S25・S26）、テスト（S27・S28）、文書化・ガイド（S29・S30）。

## 6. トレードオフ・代替案

- **egui 既存 IME サポートの流用案（不採用）**: `fandhe-egui-proxy` は egui 自体の IME 処理
  （`egui::Event::Ime` 相当）に委ねているが、これは検証用プロキシであり独自レンダラ方式の
  本番プラットフォーム（macOS/iOS/Android/デスクトップ）には適用できない。egui の IME 処理は
  egui 内部の `TextEdit` ウィジェットに閉じており、`fandhe-core` の `Widget<S>` 抽象と
  互換性がない（責務分界が異なる）。よって不採用とし、winit の `Ime::*` イベントを
  直接ハンドリングする方針（§4.4）を採る。
- **`cosmic-text` の公式 IME 対応待ち案（不採用）**: `crate-feature-matrix.md`（§3.4）の
  調査で `cosmic-text` クレート本体の公式 IME API 追加を示す一次情報がなく、COSMIC Epoch 2.0
  （将来リリース）での強化が予告されているのみである。待機はスケジュール（TASK-8.5 推定
  15 日、M3 所属）と両立しないため不採用とし、`cosmic-text` はテキストレイアウト・カーソル
  位置計算の下請けとしてのみ使用し、IME プロトコル自体は本設計（`ImeEvent`/`ImeRequest`）で
  自前実装する。
- **プラットフォーム型を `ImeEvent` に含める案（不採用）**: 実装の初速のため
  `winit::event::Ime` をそのまま `fandhe-core` の公開型にする案も検討したが、
  REQ-2 の依存方向（最適化レイヤ → コアの一方向）に反し、iOS/Android/Web でも winit 型への
  変換が必要になり利点がない。`widget-architecture.md` が taffy を内部に閉じた判断と一貫性を
  取るため不採用とした。

## 7. セキュリティ考慮（OWASP Top 10 観点）

`.claude/rules/security.md` 準拠。本文書で確定する契約自体に以下を組み込む。

- **A03 インジェクション**: プラットフォーム層から届く `Commit` / `Preedit` の文字列は
  外部入力として扱い、コア側契約で検証（制御文字除去は既存 `TextFieldInput::InsertText` の
  意味論を継承）してから編集バッファへ反映する。`ImeEvent` は生のキーコード・byte 列を
  含まず、意味づけ済み列挙のみで構成する（既存 `InteractionInput` / `TextFieldInput` と
  同じ型レベル防御、§4.1）。
- **機微情報の露出防止**: コンポジション状態（未確定文字列含む）の `Debug` 実装・ログ出力に
  内容を含めない契約とする（既存 `TextFieldWidget` の Debug マスク方針
  ―`buffer_len`/`committed_value_len` のみ出力― を `ImeEvent::Preedit`/`Commit` の
  文字列にも継承し、S2〜S3 が実装する状態型は preedit 文字列を含む `Debug` 実装で
  文字数のみを出力する契約とする）。パスワード等の入力目的ヒント（`ImePurpose::Password`、
  §4.1・§4.2・S23）では IME 無効化・学習抑止をプラットフォームへ要求できる契約にする
  （winit `set_ime_purpose` / iOS `UITextInputTraits.isSecureTextEntry` 相当 /
  Android `InputType.TYPE_TEXT_VARIATION_PASSWORD` 相当への変換は各プラットフォーム実装
  sub-issue のスコープ）。
- **メモリ安全性**: `ImeEvent` / `ImeRequest` は `fandhe-core`（`#![forbid(unsafe_code)]`）内の
  safe な型のみで定義する。FFI が必要になるのはプラットフォーム実装側（S9〜S22）であり、
  `unsafe` 境界の扱いは coding-rust.md（最小範囲・`// SAFETY:` コメント・安全な公開 API での
  ラップ）に従う旨を本契約の前提として明記する。
- **サロゲートペア・不正 UTF 境界（A03 / 堅牢性）**: §4.6 の文字インデックス正規化方針で、
  byte / UTF-16 offset の変換時に panic しない（`Result` 返却・境界クランプ）契約とする
  （S26 の前提。既存 `byte_index_at` の「境界外は末尾へフォールバックし panic しない」
  設計判断を継承）。
- 本 issue 自体はドキュメント追加のみで、シークレット混入・依存追加・CI 変更はない。

## 8. スコープ外（明記）

- **状態モデルの内部詳細設計**（コンポジションバッファの具体的な Rust 型・メモリレイアウト）
  は S2（#183）のスコープとする。本文書は契約（`ImeEvent`/`ImeRequest` のシグネチャ案・
  責務分界・マッピング表）のみを確定させる。
- **実装そのもの**は S3 以降（#184〜）のスコープ。本 issue（S1）はコード変更を一切含まない。
- **音声入力・手書き入力**（Android の音声入力 IME・iOS の手書き認識等）は IME 統合の対象外
  とする（キーボード型 IME に限定した契約）。
- **`getCursorCapsMode` 等の大文字化ヒント**（Android）: テキスト大文字化の自動化は本設計の
  中核契約（コンポジション・確定・フォーカス連動）に含めない。
- **TASK-8.6**（watchOS/visionOS 向け `winit`/`accesskit` 制約の REQ-3 反映）は別 issue の
  スコープであり、本文書は §9 で参照のみ行う。

## 9. 既知制約・プラットフォーム差異（要約）

既知制約・プラットフォーム差異の SSOT は `ime-known-constraints.md`。本節は本文書の契約に
直接関わる制約の要約と参照のみを置く。

- **`set_ime_purpose` の macOS・Windows・Linux X11（Wayland 以外全般）非対応**: `Window::set_ime_purpose`
  は winit 0.30 系で macOS・Windows・X11・iOS・Android・Web・Orbital では no-op。コアは
  `Password` でも `SetEnabled(true)` を要求し `SetPurpose` の意図表明のみを送る
  （`SetEnabled(false)` に紐づけると iOS/Android でソフトキーボードごと隠れるため不採用）。
  代替担保は `ime-known-constraints.md` §3.3 を参照。
- **Password 切替の遅延 Commit**: `ime_purpose_switch_guard` によるワンショットガード。
  `ime-known-constraints.md` §3.4 を参照。
- **Windows での `Disabled`→`Enabled` 高頻度再サイクル**: `ime-known-constraints.md` §3.5
  を参照。
- **Linux（X11・Wayland 共通）での `Enabled`/`Disabled` 発火タイミングが macOS/Windows と
  逆方向**: `ime-known-constraints.md` §3.7 を参照。
- **フォーカス遷移直後の旧フィールド由来の遅延 `Commit` 混入**: 識別子照合 + `armed` ゲート
  の二段構え（§4.8）で対処する。`Enabled` を起点にしないのは、`Enabled` が最初の
  `Preedit`/`Commit` より前に届くとは限らず、macOS では `Enabled` 自体が届かないことも
  あるためである（設計根拠として残す。`crates/fandhe-winit-ime/tests/macos_japanese_input.rs`
  の `enabled_event_may_arrive_late_or_be_skipped_before_first_preedit` 参照）。残存限界は
  `ime-known-constraints.md` §3.6・追跡 Issue #4 を参照。
- **`winit` Issue #1497 の進展監視**: `ime-known-constraints.md` §3.13 を参照。
- **watchOS/visionOS の IME 制約**: `ime-known-constraints.md` §3.12 を参照。
- **Web `compositionupdate` のカーソル位置非公開**: `ime-known-constraints.md` §3.9 を参照。
- **Android `finishComposingText()` の意味論**: `finishComposingText()` は Android 仕様上
  「テキストは残したままコンポジション状態のみ解除する」動作であるため、
  `ImeEvent::Commit(現在の preedit text)` 相当として扱い、`Disabled` は発火しない
  （`Disabled` 相当＝クリアして破棄とすると変換確定済み文字列が失われるため不採用）。
  詳細は `docs/architecture/ime-android-input-connection.md` §4.4、
  `ime-known-constraints.md` §3.10 を参照。
- **iOS `UITextInput` のカーソル矩形通知の実装形態差**: 詳細は
  `docs/architecture/ime-ios-uikit-text-input.md` §4.4 を参照。
- **iOS インライン preedit（marked text）は未対応**: `ime-known-constraints.md` §3.1、
  `ime-known-constraints.md` §4.2 を参照。
- **Android インライン preedit（`setComposingText` の反映）は未対応**: `ime-known-constraints.md`
  §3.2、`ime-known-constraints.md` §4.2 を参照。

## 10. 出典

- [`winit::event::Ime`（docs.rs、latest）](https://docs.rs/winit/latest/winit/event/enum.Ime.html)
- [`winit::window::Window`（docs.rs、latest。`set_ime_allowed`/`set_ime_cursor_area`/`set_ime_purpose`）](https://docs.rs/winit/latest/winit/window/struct.Window.html)
- [Tracking issue for IME / composition support — `winit` Issue #1497（GitHub）](https://github.com/rust-windowing/winit/issues/1497)
- [W3C UI Events — Composition Events（`compositionstart`/`compositionupdate`/`compositionend`）](https://w3c.github.io/uievents/#events-compositionevents)
- [Apple Developer Documentation: `UITextInput`（marked text: `setMarkedText`/`unmarkText`/`markedTextRange`）](https://developer.apple.com/documentation/uikit/uitextinput)
- [Android Developers: `InputConnection`（`setComposingText`/`commitText`/`finishComposingText`）](https://developer.android.com/reference/android/view/inputmethod/InputConnection)
- `docs/architecture/crate-feature-matrix.md`（PoC-7・2026-07 再調査、`cosmic-text`/`winit` の
  IME 対応状況）
- `docs/architecture/widget-architecture.md`（`Widget<S>` トレイト契約・二層モデル・
  `InteractionState`/`InteractionInput`）
- `crates/fandhe-core/src/ui/text_field.rs`（`TextFieldWidget<S>` の既存編集モデル・
  スコープ外事項）
- `crates/fandhe-web-dom/src/mount.rs`（`attach_input`、既存の Web 限定 composition 対応）
