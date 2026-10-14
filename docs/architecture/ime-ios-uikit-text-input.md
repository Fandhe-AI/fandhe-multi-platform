# iOS `UITextInput` テキスト入力連携設計（TASK-8.5-S15）

## 1. 目的・位置づけ

本文書は #181（TASK-8.5「IME 統合実装」、REQ-8、M3）の sub-issue である
#196（TASK-8.5-S15「iOS: UIKit テキスト入力連携設計」）の成果物であり、iOS の UIKit
テキスト入力連携を定める設計文書である（姉妹文書: S1・#182 → `ime-architecture.md`、
S18・#199 → `ime-android-input-connection.md`）。

`ime-architecture.md` §4.5「iOS（UIKit、S15〜S17）」は iOS マッピングの理想形の概要のみを
定義し、詳細設計（`UITextInputTokenizer` との整合・ソフトウェアキーボードのライフサイクル・
`ImeRequest::SetCursorArea` の UIKit 内部変換）を本文書へ明示的に委譲している
（同文書 §4.5 L375-376・§9 L770-773 参照）。

iOS の現行実装は commit 型入力（`text_field_input_from_key` →
`TextFieldInput::InsertText`/`DeleteBackward`、S16・#197 実装 / S17・#198 検証）である。
winit 0.30.13 の iOS バックエンドは `UITextInput` プロトコル（marked text API）を実装しておらず
（`UIKeyInput` のみ実装）、`WindowEvent::Ime(Ime::*)` を一切発火しない
（根拠: S16・#197 / S17・#198 の検証）。

本文書の役割は次の 3 点である:

1. 現行 commit 型方式（S16 実装）を iOS の正式方式として設計文書上で定める。
2. `ime-architecture.md` §4.5 が「将来的な `UITextInput` 完全実装後の理想形」とだけ記した
   **`UITextInput` フル対応（インライン preedit）の詳細設計**を確定させ、S15 へ委譲されていた
   3 つの設計判断（`SetCursorArea` の内部変換方式・`UITextInputTokenizer` との整合・
   ソフトウェアキーボードのライフサイクル）を確定する。
3. `ImePurpose` no-op の代替担保整理（インライン preedit 未対応の扱いを含む）を
   設計判断として整理する。

本文書は **「現行方式（commit 型）の定義」と「将来のフル対応（`UITextInput` 準拠
インライン preedit）の設計」の二部構成**を取る。

## 2. spec 対応

- **REQ-8**（`docs/spec/04-requirements.md`）: TASK-8.5「IME 統合」の一部。CJK かな漢字変換・
  拼音変換等のコンポジション入力を iOS で成立させるための詳細設計。
- **TASK-8.5**（`docs/spec/05-tasks.md`）: 成果物「IME 統合モジュール」への寄与。前提タスクは
  TASK-1.1。
- **REQ-2**: 依存方向は最適化レイヤ → コアの一方向。本設計は `fandhe-core` に
  `UITextInput` / `UIKeyInput` / `NSRange` 等の UIKit 型を一切露出しない
  （`ime-architecture.md` §4.1 の `ImeEvent` / `ImeRequest` 契約をそのまま利用し、新しい
  コア型は追加しない）。
- **REQ-10**（CJK 既定レンダリング）: 本設計は「iOS でかな漢字変換・拼音変換のコンポジション
  表示（インライン preedit）を成立させる経路が存在するか」を将来課題として整理し、当面は
  commit 型入力（変換候補バーでの確定文字列の逐次挿入）で REQ-10 の実質を担保する方針を確定する。
- **REQ-13 / NFR-9**: `S` へのブリッジは既存 `TextFieldInput::InsertText`/`DeleteBackward`
  経路のみを使い、`fn(&S) -> T` / `fn(&mut S, T)` 契約を維持する（新規経路を追加しない）。

## 3. 現状分析

### 3.1 iOS 実行アーキテクチャの現状（winit UIKit バックエンド）

- `Cargo.toml`（ルート）は `winit = { version = "0.30", default-features = false }`（workspace
  一元管理）。既存 S9〜S14・S16〜S17 の検証はいずれも winit 0.30.13 のソースを参照しており、
  本文書もこれに合わせて **winit 0.30.13** を調査対象バージョンとする。
- iOS 側の実行基盤は `platforms/apple`（Swift 側のブートストラップ、存在すれば）を通じて
  winit の `WinitView`（`UIView` サブクラス）へレンダリング・入力を委ねる構成であり、
  Android の NativeActivity/GameActivity のような起動方式の選択肢は存在しない
  （UIKit の `UIView`/`UIResponder` チェーンに一本化されている）。

### 3.2 winit 0.30.13 iOS バックエンドのソースレベル確定事実

`platform_impl/ios/{view.rs,window.rs}`（出典: S16・#197 の検証）を
本設計の前提として引用する:

| 契約 | 確認内容 | 判定 |
|------|---------|------|
| `Ime::Enabled`/`Ime::Preedit`/`Ime::Commit`/`Ime::Disabled` の発火 | `WinitView` は `UITextInput` を実装せず、`UIKeyInput`（`hasText`/`insertText:`/`deleteBackward`）のみ実装する（`unsafe impl UITextInputTraits for WinitView {}` は空実装） | winit は iOS で `Ime::*` を一切発火しない |
| `insertText:` の変換 | `handle_insert_text` は `NSString` を 1 文字ずつ `chars()` で分解し、各文字を `KeyboardInput`（Pressed: `text: Some(1 文字)` / Released: `text: None`）として送出する | 確定済み文字列の per-char 逐次配信のみ |
| `deleteBackward` の変換 | `handle_delete_backward` は `Key::Named(NamedKey::Backspace)` の Pressed/Released 対を送出する | 1 回の呼び出しにつき 1 文字（1 code point）削除 |
| `set_ime_allowed(bool)` | `true` → `view.becomeFirstResponder()`、`false` → `view.resignFirstResponder()` へ委譲 | ソフトウェアキーボードの表示/非表示のみ機能する |
| `set_ime_cursor_area(Position, Size)` | 引数を無視し `warn!` ログのみ（no-op） | 候補ウィンドウ位置通知は winit 経由では機能しない |
| `set_ime_purpose(ImePurpose)` | 引数を無視し `warn!` ログのみ（no-op） | パスワード等のヒントは winit 経由では iOS に伝わらない |
| `KeyEvent::platform_specific` | `pub(crate)`（winit 内部限定） | `fandhe-winit-ime` は `KeyEvent` を直接構築できない（プラットフォーム層は winit が公開する `WindowEvent::KeyboardInput` を受信する側に徹する） |

**結論（Android §3.2 と同型）**: winit の iOS バックエンドは **IME コンポジション（marked
text）を一切扱わない**。macOS/Windows/Linux（S9/S11/S13）のように「winit が `Ime::*` へ変換
してくれる」前提は iOS では成立しない。

### 3.3 UIKit テキスト入力 API 体系の段階構造

[Apple Developer Documentation: `UIKeyInput`](https://developer.apple.com/documentation/uikit/uikeyinput)・
[`UITextInputTraits`](https://developer.apple.com/documentation/uikit/uitextinputtraits)・
[`UITextInput`](https://developer.apple.com/documentation/uikit/uitextinput) を調査した結果、
3 つのプロトコルは次の段階構造を持つ:

| プロトコル | 提供する機能 | winit `WinitView` の対応 |
|-----------|-------------|--------------------------|
| `UIKeyInput`（`UITextInputTraits` に準拠） | `hasText`/`insertText:`/`deleteBackward` の 3 メソッドのみ。基本的な文字入力・削除のみを扱う最小限のプロトコル。**ソフトウェアキーボードは表示できるが、marked text（コンポジション中の未確定文字列）の概念自体を持たない** | **実装あり**（現行方式の土台） |
| `UITextInputTraits` | `keyboardType`/`isSecureTextEntry`/`autocapitalizationType` 等のキーボード外観・入力属性ヒントのみ。`UIKeyInput` 単体でも `UITextInput` の一部としても使われる | `unsafe impl UITextInputTraits for WinitView {}` は**空実装**（既定値のまま、ヒントを一切カスタマイズしていない） |
| `UITextInput`（`UITextInputTraits` を包含） | `setMarkedText(_:selectedRange:)`/`unmarkText()`/`markedTextRange`/`caretRect(for:)`/`selectionRects(for:)`/`firstRect(for:)`/`tokenizer`（`UITextInputTokenizer`）等、**フル機能のテキスト入力・コンポジション対応**。UIKit の `InputMethodManager` 相当（ソフトウェアキーボード側のコンポジション制御・候補ウィンドウ位置照会）はこのプロトコルの実装を前提に動作する | **未実装** |

この段階構造は Android の `InputConnection`（フル機能）に対する `UIKeyInput` の関係が
Android の `KeyEvent` 経路単体（`InputConnection` 抜きの最小限入力）に相当することを示す
——すなわち **iOS の現行方式は Android の NativeActivity 方式（コンポジション非対応の
最小構成）と構造的に同型の制約下にある**（両者とも「フル機能プロトコルへの委譲経路が
存在せず、キー単位の最小 API のみで確定済み文字列を受信する」）。

## 4. 設計判断（核心）

### 4.1 実現方式の選定: 当面は `UIKeyInput` 素通し（commit 型）を正式方式として採用

比較した 2 案:

| 案 | 概要 | 判定 |
|----|------|------|
| (a) **`UIKeyInput` 素通し（commit 型、S16 実装）を当面の正式方式として採用** | winit が既に実装している `UIKeyInput` 経路（`insertText:`/`deleteBackward` → per-char `KeyboardInput` → `TextFieldInput::InsertText`/`DeleteBackward`）をそのまま継続する。インライン preedit（marked text の可視化）は提供しない | **採用**（当面） |
| (b) 本フレームワーク独自の `UITextInput` 準拠ビュー実装 | winit の `WinitView`（または独自の `UIView` サブクラス）に `UITextInput` プロトコルを実装し、`setMarkedText`/`unmarkText`/`caretRect(for:)`等を自前実装してインライン preedit を実現する | **設計のみ確定・実装着手は別 issue 判断**（下記 4.1.1 参照。Android S18 の GameActivity 判断とは異なり、winit 側の対応を待つ選択肢が実質的にない代わりに、独自実装の工数・保守コストが単独の判断材料になる） |

**(c) winit 側の `UITextInput` 対応を待つ案は、Android の「winit 素通し」案（不採用の (c)）とは
性質が異なり本設計では「別案」として扱わない**: winit Issue #1497（Tracking issue）は
2026-07 時点で open のままであり対応時期が不明である点は Android と同じだが、Android は
「winit がそもそも `InputEvent::TextEvent` を処理しない」という実装未着手の欠落であるのに対し、
iOS は「`UITextInput` プロトコル自体を実装するかどうか」という winit 側の設計判断待ちである点で
性質が異なる。いずれにせよ**待機はスケジュール（TASK-8.5 推定 15 日、M3 所属）と両立しない**
ため、フル対応の実現手段としては (b)（独自ビュー実装）のみが本フレームワーク側で完結する
選択肢になる。

#### 4.1.1 (b) 独自 `UITextInput` 準拠ビュー実装の採用条件（設計のみ確定、実装着手は別 issue 判断）

- **採用条件**: 次のいずれかが満たされた場合に実装着手を検討する（`out-of-scope-tracking.md`
  準拠、ユーザー承認を経て別 issue として起票する）。
  1. REQ-10（CJK 既定レンダリング）の観点でインライン preedit の欠如がユーザー体験上の
     重大な欠陥と判断された場合（変換途中の未確定文字列が見えないこと自体は、最終的に
     確定される文字列の正しさを損なわない、`ime-known-constraints.md` §4.2 の評価を参照）。
  2. winit 側で `UITextInput` 対応の具体的な進展（PR マージ等）が確認され、独自実装の
     代わりに winit の実装を利用できる見込みが立った場合（この場合は (b) ではなく winit
     アップデートへの追随に方針転換する）。
- **不採用側に倒す理由（安全側判断）**: 独自 `UITextInput` ビューの実装は、`markedTextRange`・
  `UITextInputTokenizer`・`caretRect(for:)`/`selectionRects(for:)` 等 UIKit の内部整合性が
  厳格に要求されるプロトコル群を winit の外側（プラットフォーム統合レイヤ）で自前実装する
  ことを意味し、工数・保守コストが大きい。かつ winit の `WinitView` を差し替える、または
  ラップする実装形態の選定自体が別途の設計判断を要する（`WinitView` のサブクラス化・
  委譲・完全な独自 `UIView` 実装のいずれを取るか）。したがって本 issue は「設計判断（4.2〜4.6）
  の確定」までに留め、実装着手の可否・実装形態の選定は別 issue（ユーザー承認後）に委ねる。

### 4.2 `UITextInput` marked text API → `ImeEvent` マッピング詳細（将来のフル対応時の設計）

`ime-architecture.md` §4.5 の表を精緻化する。**この表は 4.1 で確定した「当面は commit 型」
方針の下では未使用（4.3 参照）だが、4.1.1 の採用条件が満たされ独自 `UITextInput` ビューを
実装する場合の変換契約として、S15 の委譲事項の一部（マッピング精緻化）を本節で確定する**:

| UIKit API | 方向 | `ImeEvent` / `ImeRequest` | 備考 |
|-----------|------|---------------------------|------|
| `setMarkedText(_ markedText: String?, selectedRange: NSRange)` 呼び出し | UIKit → コア | `ImeEvent::Preedit { text: markedText.unwrap_or_default(), cursor }`（`selectedRange` は `markedText` 内の UTF-16 相対範囲、§4.3 の変換ロジックで char offset へ正規化） | `markedText == nil` は「コンポジションのクリア」を表す（`ImeEvent::Preedit { text: String::new(), cursor: None }` へ写像。`ime-architecture.md` §4.1 の `ImeEvent::Preedit.text` 空文字列 = クリアの契約と整合） |
| `unmarkText()` 呼び出し | UIKit → コア | 単独では `ImeEvent` を発火しない（下記参照） | `unmarkText()` は「現在の marked text をそのまま確定テキストとして残しコンポジションを終了する」仕様（[Apple Developer Documentation: `unmarkText`](https://developer.apple.com/documentation/uikit/uitextinput/unmarktext())）であり、Android `finishComposingText()` と同義。呼び出し直後の `text` の状態変化（`insertText:` が続けて呼ばれる場合が多い）を観測して `ImeEvent::Commit` を合成する（Android §4.4 と同型の「単独メソッド呼び出しだけでは確定文字列を得られない」制約） |
| `unmarkText()` + 直後の確定（`insertText:` 経由、または marked text がそのまま残る） | UIKit → コア | `ImeEvent::Commit(committed_text)` | `committed_text` は `unmarkText()` 呼び出し直前の `markedTextRange` が指していた文字列（UIKit は `unmarkText()` 自体でテキストを変更しないため、直前の marked text がそのまま確定文字列になる。Android の `finishComposingText()` と同じ「テキストを変更しない」意味論であり、Android §4.4 の長さ差分ベース抽出ロジックは iOS では不要——`unmarkText()` はテキストを変更しないため、marked text 文字列をそのまま使えばよい） |
| `markedTextRange` の有無 | UIKit → コア（状態確認） | `Enabled`/`Disabled` はキーボード表示・フォーカス在席と連動 | UIKit にはコンポジション専用の enable/disable API がなく、`becomeFirstResponder`/`resignFirstResponder` に連動させる（4.6 参照。macOS/Windows/Linux のような明示的な `Ime::Enabled`/`Ime::Disabled` イベントは存在しない） |
| `replace(_ range: UITextRange, withText text: String)`（marked text 外での直接置換） | UIKit → コア | 本設計の中核契約には含めない（8 節スコープ外） | 自動修正・音声入力確定等の経路であり、キーボード型 IME の中核契約（`ime-architecture.md` §8）の対象外 |

**Android §4.2 との対比（重要な差異）**: Android の `GameTextInput` は「値そのものの差分監視」
（`TextInputState` のポーリング/コールバック）でイベントを合成するのに対し、iOS の
`UITextInput` は **プロトコルメソッド呼び出し単位のイベント**（`setMarkedText`/`unmarkText`
が個別に呼ばれる）である。したがって iOS 側の変換ロジックは「メソッド呼び出しをそのまま
`ImeEvent` へ 1:1 に近い形で変換する」（winit デスクトップバックエンドと同じ抽象化レベル）
設計になり、Android のような「新旧 `text` の長さ差分から確定文字列を逆算する」ロジックは
不要である。

### 4.3 `NSRange`（UTF-16 code unit）→ char offset 正規化ロジック

**前提（Apple 公式仕様）**: `setMarkedText(_:selectedRange:)` の `selectedRange`（`NSRange`）は
`markedText`（引数の `String`）内の相対範囲であり、`NSRange` は Objective-C `NSString`/Swift
`String`（UIKit 経由では実質 `NSString` 相当）の **UTF-16 code unit** を単位とする
（[Apple Developer Documentation: `NSRange`](https://developer.apple.com/documentation/foundation/nsrange)、
Android の `CharSequence` が UTF-16 code unit 基準であるのと同じ計算機科学的背景——いずれも
基盤 API が UTF-16 文字列表現を前提とする歴史的経緯に由来する）。

**変換ロジック**: `fandhe-core::ui::text_selection` の
`try_char_offset_from_utf16` / `char_offset_from_utf16`（クランプ版）・
`try_selection_from_utf16_range`（S4・#185、
[`crates/fandhe-core/src/ui/text_selection.rs` L238-392] 参照）
を**そのまま呼び出す**。新規関数は不要（`ime-android-input-connection.md`
§4.5 が Android 向けに byte offset 系関数を指定したのに対し、iOS 向けの UTF-16 系関数は既存であり、
`ime-architecture.md` §4.6 の表（L523）が iOS を UTF-16 code unit offset と整理している）。

```text
入力: markedText: String（setMarkedText の第 1 引数、コンポジション文字列そのもの）
      selectedRange: NSRange { location: usize, length: usize }（markedText 内の UTF-16 相対範囲）

手順:
  1. UTF-16 offset の算出:
       utf16_start = selectedRange.location
       utf16_end   = selectedRange.location + selectedRange.length
     （`NSRange` は `start`/`end` ではなく `location`/`length` で表現される点が Android
      `TextSpan { start, end }` との API 形状の差異。ただし「逆転（length が負相当）」は
      `NSRange` の型定義上 `length: NSUInteger`（符号なし）のため発生しない
      —— Android `TextSpan` のような正規化手順は不要）
  2. `try_char_offset_from_utf16(markedText, utf16_start)` /
     `try_char_offset_from_utf16(markedText, utf16_end)` を呼び出し、char offset を得る。
     戻り値が `Err`（サロゲートペア中間点・範囲外）の場合は `char_offset_from_utf16`
     （クランプ版）にフォールバックし、panic せず安全側の境界へ丸める
     （coding-rust.md「panic を API 契約にしない」準拠。既存関数がこの契約を既に
     満たしているため、本設計は呼び出し順序のみを定義する）。
  3. `ImeEvent::Preedit.cursor = Some((char_start, char_end))` を得る
     （`char_start <= char_end` は `NSRange.length` が非負であることから常に成立する）。
```

**Android §4.3 との対比**: Android は `TextSpan` の逆転許容・バイトオフセット基準・
`compose_region` 相対への再正規化という複数段の変換を要したのに対し、iOS の `NSRange` は
`markedText` 引数自体を基準とする相対範囲であり、`compose_region` に相当する外部状態への
依存や逆転処理が不要な分、変換ロジックは単純である（`try_selection_from_utf16_range` を
`markedText` 文字列に対して直接呼び出すだけで完結する）。

### 4.4 `ImeRequest::SetCursorArea` の内部変換設計（委譲事項の確定）

`ime-architecture.md` §9 L770-773 が本文書へ委譲する判断:
winit の `set_ime_cursor_area(position, size)` のような**明示 push 型 API**とは異なり、
`UITextInput` はカーソル矩形を **pull 型（システムからの問い合わせに応答する）** で提供する
プロトコルメソッド群（`caretRect(for position: UITextPosition) -> CGRect`・
`selectionRects(for range: UITextRange) -> [UITextSelectionRect]`・
`firstRect(for range: UITextRange) -> CGRect`、
[Apple Developer Documentation: `UITextInput`](https://developer.apple.com/documentation/uikit/uitextinput))
として実装する必要がある。

**確定する設計**:

- `fandhe-core` 側の契約（`ImeRequest::SetCursorArea { origin, size }`）自体は変更しない
  （winit・Android・iOS いずれのプラットフォームでも「コアがカーソル位置を通知する」という
  push 型の抽象を維持する。REQ-2 の依存方向を守り、iOS の pull 型という実装詳細を
  `fandhe-core` へ漏らさない）。
- プラットフォーム層（独自 `UITextInput` ビュー実装、4.1.1 の対象範囲）は
  `ImeRequest::SetCursorArea` を**都度 UIKit へ通知するのではなく、直近の値を内部状態として
  保持する**設計とする。`caretRect(for:)`/`selectionRects(for:)`/`firstRect(for:)` が
  システム（ソフトウェアキーボードの候補ウィンドウ配置ロジック）から呼ばれた際に、
  保持している最新の矩形値をそのまま返却する（pull 型プロトコルを push 型の内部状態から
  応答する典型的なアダプタパターン）。
- **`origin`/`size` の座標系変換**: `ImeRequest::SetCursorArea` の `origin`/`size` は論理
  ピクセル座標（`ime-architecture.md` §4.1 の doc コメント参照）であり、UIKit の
  `CGRect`（ポイント単位、UIKit の座標系は既定でポイント = 論理ピクセルに近い概念）への
  変換はスケールファクタの扱いが winit デスクトップ版（物理ピクセルへの変換が必要、§4.4）
  と異なり、**単純なポイント→ `CGRect` のラップのみで済む見込み**である（UIKit の
  `UIScreen.scale` は物理ピクセルとの変換に使うものであり、`CGRect` 自体は常にポイント単位
  で扱われるため。ただし本判断は一次情報からの設計上の推定であり、独自ビュー実装着手時に
  実機確認が必要な項目として §9 に記録する）。
- `UITextPosition`/`UITextRange` は UIKit が要求する不透明プロトコル型（具体的な実装クラスは
  実装者が定義する）であり、本設計は「コア側の char offset 範囲を独自の `UITextPosition`/
  `UITextRange` 実装でラップする」変換が必要である旨のみを設計判断として記録し、
  具体的な型定義は実装（4.1.1 の対象）に委ねる。

**現行 commit 型方式（4.1 採用案）での扱い**: `UITextInput` 未実装のため
`caretRect(for:)` 等は呼ばれず、`ImeRequest::SetCursorArea` は winit の
`set_ime_cursor_area` no-op（S16・#197 の検証）へ
委譲されたまま無視される。上記の pull 型変換設計は 4.1.1 の独自ビュー実装時にのみ適用される。

### 4.5 `UITextInputTokenizer` との整合

[Apple Developer Documentation: `UITextInputTokenizer`](https://developer.apple.com/documentation/uikit/uitextinputtokenizer)・
[`UITextInputStringTokenizer`](https://developer.apple.com/documentation/uikit/uitextinputstringtokenizer)
を調査した結果:

- `UITextInput.tokenizer` は「単語・文・段落・行の境界を判定するオブジェクト」を返すプロパティ
  であり、システムのテキスト選択 UI（マグニファイア・選択ハンドル・ダブルタップでの単語選択等）
  が利用する。UIKit は既定実装 `UITextInputStringTokenizer` を提供しており、多くのケースで
  カスタム実装は不要（`UITextInputStringTokenizer(textInput:)` をそのまま返せばよい）。
- **確定する設計**: 独自 `UITextInput` ビュー実装（4.1.1）では `UITextInputStringTokenizer`
  の**既定実装をそのまま採用**し、独自のトークナイザ実装は行わない。理由:
  1. `fandhe-core::ui::text_selection` の grapheme cluster 境界判定
     （`prev_grapheme_boundary`/`next_grapheme_boundary`、S26・#207）は UAX #29
     （unicode-segmentation クレート）準拠であり、UIKit の既定トークナイザが内部で使う
     境界判定ロジック（NSLinguisticTagger 相当、非公開実装）と厳密な整合を取る必要はない
     ——両者は「単語・文境界の近似判定」という UX 補助機能であり、`fandhe-core` の編集操作
     契約（grapheme cluster 単位、S26）と UIKit のシステム UI（マグニファイア等）
     の境界判定がわずかに異なっても、コンポジション・確定入力の正しさには影響しない。
  2. 独自トークナイザ実装は、UIKit 内部の非公開ロケール依存ロジックとの整合を保証する
     一次情報がなく、実装・保守コストに見合う便益が確認できない（車輪の再発明のリスク、
     Android S18 §4.1 案 (b) 不採用理由と同型の判断）。
- **将来の再検討条件**: システムのテキスト選択 UI（単語単位の選択・マグニファイア）で
  `fandhe-core` の grapheme cluster 境界と目に見える不整合が実機検証で確認された場合に
  カスタムトークナイザ実装を検討する（一次情報からの推定であり、実機確認が
  必要な項目として §9 に記録する）。

### 4.6 ソフトウェアキーボードのライフサイクル

- **フォーカス連動（現行方式・将来方式で共通）**: `ime-architecture.md` §4.7 の契約
  （`FocusGained` → `ImeRequest::SetEnabled(true)`、`FocusLost` → コンポジション
  クリーンアップ後 `SetEnabled(false)`）をそのまま継承する。iOS では `SetEnabled(true/false)`
  が `becomeFirstResponder()`/`resignFirstResponder()` へ委譲され、これがそのまま
  ソフトウェアキーボードの表示/非表示として作用する（S16・#197 の検証 §2 #4）。
- **`Enabled`/`Disabled` イベント（UIKit → コア方向）は存在しない**: macOS/Windows/Linux が
  コンポジション開始/終了に応じて `Ime::Enabled`/`Ime::Disabled` を発火するのに対し、UIKit には
  コンポジション専用の enable/disable 通知 API が存在しない。`markedTextRange` の有無
  （`nil` かどうか）を状態確認に使うことは可能だが、これは「イベントとして届く」のではなく
  「都度クエリする」性質であるため、本設計は独自 `UITextInput` ビュー実装時も
  `Enabled`/`Disabled` イベントを UIKit 側から発火しない設計とする
  （`ImeRequest::SetEnabled` はコア → プラットフォームの一方向のみで完結し、UIKit →
  コアの逆方向 `Enabled`/`Disabled` イベントは iOS では発生しない契約のまま維持する。
  `ime-architecture.md` §4.5 表・S16 検証記録と整合）。
- **キーボード表示アニメーションとの関係**: `becomeFirstResponder()`/`resignFirstResponder()`
  自体はソフトウェアキーボードのスライドイン/アウトアニメーションを引き起こすが、これは
  UIKit の既定動作でありプラットフォーム層が明示的に制御する対象ではない
  （`UIKeyboardWillShowNotification`/`UIKeyboardWillHideNotification` を購読して
  レイアウト再計算（セーフエリア調整等）を行う設計は本設計の中核契約外、8 節スコープ外）。
- **`WindowInsets`（ソフトキーボード表示時のレイアウト圧迫）は本設計の中核契約外**とする
  （Android §4.7 と同型の判断。`ime-architecture.md` はレイアウト再計算の責務分界を定義して
  おらず、iOS 固有の追加要件になるため、必要になった時点で別 issue として起票する
  （out-of-scope-tracking.md 準拠、ユーザー承認後）。

### 4.7 `ImeRequest::SetPurpose` → `UITextInputTraits` マッピングと現行 no-op 制約下の代替担保

- **マッピング設計**（4.1.1 の独自ビュー実装時に適用）: `ImePurpose::Password` →
  `UITextInputTraits.isSecureTextEntry = true`（[Apple Developer Documentation:
  `isSecureTextEntry`](https://developer.apple.com/documentation/uikit/uitextinputtraits/issecuretextentry)）。
  `ImePurpose::Normal`/`Terminal` は `keyboardType`（`UIKeyboardType`）のヒント切替に
  対応させる余地があるが、本設計の中核契約（パスワード時の学習・予測変換抑止）には
  `isSecureTextEntry` のみが直接関わる。
- **現行制約**: winit `set_ime_purpose` は iOS で no-op（`warn!` ログのみ）
  であり、`ImeRequest::SetPurpose` は現行実装では iOS に一切反映されない
  （`ime-known-constraints.md` §3.3・マトリクス L38 の全プラットフォーム横断制約の一部）。
- **代替担保方針との整合**: `text_field.rs`（S23・#204）は `ImePurpose::Password` でも
  `ImeRequest::SetEnabled` を他の purpose と同様に扱い（フォーカス中は常に `true` を要求し、
  無効化するのは `FocusLost`/`set_disabled(true)` 等の既存経路のみ）、
  `SetPurpose(Password)` の意図表明のみをプラットフォーム層へ送る。
  本設計はこれを継承する。`SetEnabled(false)` によるキーボード非表示化でパスワード入力自体を
  封じる代替案は、パスワードフィールドにフォーカスした瞬間にキーボードごと隠れて入力できなく
  なる欠陥があるため採用しない（S16・#197 の検証 §6）。
  したがって iOS では `Password` の学習・予測変換抑止は、独自ビュー実装時の
  `isSecureTextEntry = true` 設定（上記）が実現できて初めて有効になり、**現行 commit 型方式
  では代替担保が存在しない**（`ime-known-constraints.md` §3.3 の全プラットフォーム横断の
  未解決事項に iOS も含まれる。`SetEnabled` を乗っ取る代替案は上記の欠陥により採用しない）。

## 5. 現行実装・検証記録との対応

- **`text_field_input_from_key` の変換規則との突合**: 本設計 4.1（commit 型を正式方式と
  して採用）は S16 が実装した `text_field_input_from_key`（`insertText:`/`deleteBackward` →
  `TextFieldInput::InsertText`/`DeleteBackward`）の変換規則と一致する。
- **契約上の重要判断との整合**: `KeyboardInput` を `ImeEvent::Commit` へ偽装変換しない
  （`ime-architecture.md` §4.1 の「`Disabled` 中は `Preedit`/`Commit` が発生しない」契約を
  守るため）契約を共有する。本設計 4.2〜4.6 が定義する「将来の `UITextInput` フル対応時の
  `ImeEvent` マッピング」もこの契約に従う——独自 `UITextInput` ビューを実装した場合にのみ
  真の `ImeEvent::Preedit`/`Commit`/`Enabled`/`Disabled` が発生し、現行の commit 型方式
  （`KeyboardInput` 経由）ではこれらのイベントを発火させない。
- **S17 §3 ギャップ表との整合**: S17 が作成した「設計文書 §4.5 契約 ⇔ S16 実装」ギャップ表
  （項目 1〜8）と本設計 4.1〜4.7 の確定内容は矛盾しない。
  特に項目 5（カーソル矩形通知は成立しない）は本設計 4.4 が「現行方式では
  no-op のまま無視される」と明記した内容と一致する。
- **実機チェックリストとの関係**: 本設計はチェックリストに新規項目を追加しない（設計文書
  であり実機検証はスコープ外、8 節参照）。本設計 4.4・4.5 が「実機確認が必要な項目」として
  §9 へ記録する事項は、4.1.1（独自ビュー実装）着手時に新たに必要となる実機確認であり、
  S16/S17 の既存チェックリスト（commit 型方式の検証）とは独立した将来課題である。

## 6. トレードオフ・代替案（不採用理由つき）

- **`KeyboardInput` → `ImeEvent::Commit` への偽装変換案（不採用）**: `insertText:` 由来の
  `KeyboardInput` を `ImeEvent::Commit` へ変換すれば `ime-architecture.md` §4.3 の「確定した
  文字列のみが `buffer` へ挿入される」経路に統一的に乗せられるように見えるが、`ImeEvent` の
  契約は「`Disabled` 中は `Preedit`/`Commit` が発生しない」（§4.1）であり、iOS では
  `Enabled`/`Disabled` 自体が一切発火しないため、この契約を満たせない（`Enabled` を伴わない
  `Commit` は契約違反になる）。既存の `TextFieldInput::InsertText` 経路（`ImeEvent` を経由
  しない直接編集入力）の方が iOS の実態（コンポジションが存在しない）に忠実である。
- **iOS を Android の GameActivity 移行のような「別実行方式への切替」で解決する案（不成立、
  そもそも選択肢が存在しない）**: Android は NativeActivity/GameActivity という**起動方式の
  選択肢**が存在し、GameActivity + `GameTextInput` への移行でフル機能の `InputConnection`
  相当を得られた。iOS には `UIApplication`/`UIWindow`/`UIView` 階層以外の代替実行方式が
  存在せず、`UITextInput` 対応は「winit の実装を待つ」か「本フレームワーク独自に実装する」
  の 2 択に絞られる（4.1 参照）。
- **winit の `UITextInput` 対応強化を待つ案（不採用、4.1 参照）**:
  winit Issue #1497（open（2026-07 確認）、`ime-architecture.md` §3.4・§9 出典）の
  解消時期が不明であり、TASK-8.5 のスケジュールと両立しない。

## 7. セキュリティ考慮（OWASP Top 10 観点）

`.claude/rules/security.md` 準拠。

- **A03 インジェクション**: 本設計は、iOS 由来の入力文字列（`insertText:` 経由の現行方式、
  および将来の `setMarkedText`/`unmarkText` 経由の確定文字列）が既存契約の制御文字除去
  （`TextFieldInput::InsertText` 内蔵）へ必ず委譲される設計であることを明記する。
  `ImeEvent::Commit`/`ImeEvent::Preedit` 経由で届く文字列も同様に、コア側契約
  （`ime-architecture.md` §7）が検証してから編集バッファへ反映する契約を維持する。
  UIKit 層（独自 `UITextInput` ビュー実装時）で独自の文字列加工・サニタイズを行わない方針
  とし、検証はすべてコア側（`fandhe-core`）に一元化する（責務の重複・不整合を避ける）。
- **A02/A05（機微入力）**: `ImePurpose::Password` → `UITextInputTraits.isSecureTextEntry`
  マッピング設計（4.7）により、独自ビュー実装時にパスワード入力時の IME 学習・予測変換
  抑止を UIKit ネイティブの機構で担保する設計とする。現行 commit 型方式では代替担保が
  存在しない（4.7 参照。`SetEnabled` を乗っ取る代替案はキーボードが隠れて入力不能になる
  欠陥があるため採用しない）。この未解決状態は `ime-known-constraints.md` §3.3 の
  全プラットフォーム横断の未解決事項（Wayland 以外は `SetPurpose` がヒントに留まる）に
  iOS も含まれる。
- **A08 整合性**: 依存追加なし・コード変更なし。winit のバージョン前提（0.30.13）を本文書に
  固定的に明記し、将来の挙動変化時（`UITextInput` 対応が追加された場合等）の追随箇所
  （§4.2〜§4.7・`ime-architecture.md` §4.5・`ime-known-constraints.md` §3.1）を特定可能にする。
- **秘密情報混入防止**: 文書に認証情報を記載しない。
- **unsafe 境界**: 新規 unsafe/FFI なし（`#![forbid(unsafe_code)]` 不変）。将来の独自
  `UITextInput` 準拠ビュー設計（4.1.1、Swift 側実装が想定される）は FFI 境界そのものでは
  なく Swift 側のプロトコル実装であるため `unsafe` の直接対象ではないが、Swift ↔ Rust の
  橋渡し（winit の `WinitView` サブクラス化・イベント受け渡し）で `unsafe`/FFI 境界が
  生じる場合は coding-rust.md の `// SAFETY:` 要件を前提条件として実装時に明記する。

## 8. スコープ外（明記）

- **独自 `UITextInput` 準拠ビューの実装着手そのもの**（4.1.1）: 採用条件を満たすかの判断・
  起票はユーザー承認事項とし、本 issue では起票しない（out-of-scope-tracking.md 準拠）。
- **watchOS/visionOS の IME 対応**（TASK-8.6 のスコープ、`ime-architecture.md` §8 と同様）。
- **実機検証**: 実機検証は S16/S17 のチェックリスト（人間担当）で行い、本書は項目を
  追加しない。
- **`UIKeyboardWillShowNotification`/`WillHideNotification` 経由のレイアウト再計算**
  （`WindowInsets` 相当、4.6 参照）。
- **音声入力・手書き入力**（`ime-architecture.md` §8 で全プラットフォーム共通のスコープ外
  として明記済み）。
- **`replace(_:withText:)` 経由の自動修正・音声確定入力**（4.2 参照）。

## 9. 未解決事項・将来課題

- **独自 `UITextInput` ビュー実装の採用要否判断**（4.1.1）: REQ-10 の観点でインライン
  preedit 欠如が重大な UX 課題と判断された場合、または winit 側の `UITextInput` 対応の
  進展が確認された場合に再検討する。ユーザー承認を経て別 issue として起票する
  （out-of-scope-tracking.md 準拠、本 issue では起票しない）。
- **`SetCursorArea` のポイント→ `CGRect` 変換がスケールファクタ変換不要という判断の実機
  未確認**（4.4）: 一次情報からの設計上の推定であり、独自ビュー実装着手時に実機確認が
  必要。
- **`UITextInputStringTokenizer` 既定実装で十分という判断の実機未確認**（4.5）: システムの
  テキスト選択 UI（マグニファイア・単語単位選択）と `fandhe-core` の grapheme cluster
  境界判定との視認上の整合は実機確認が必要。
- **`ImePurpose::Password` の iOS 代替担保が現行方式では存在しない**（4.7・7 節）:
  `ime-known-constraints.md` §3.3 の全プラットフォーム横断の未解決事項に含まれる形で
  引き続き残る。新規 issue 起票の要否はユーザー承認を経て判断する
  （`ime-known-constraints.md` §5 の既存運用に従う）。
- **winit Issue #1497 の進展監視**（`ime-architecture.md` §9・`ime-known-constraints.md`
  §3.13 と同様）: 進展があれば本設計 4.1 の前提（フル対応は独自実装のみが選択肢）を
  再評価する必要がある。

## 10. 出典

- [Apple Developer Documentation: `UITextInput`](https://developer.apple.com/documentation/uikit/uitextinput) —
  `setMarkedText`/`unmarkText`/`markedTextRange`/`caretRect(for:)`/`selectionRects(for:)`/
  `firstRect(for:)`/`tokenizer` の API 定義（§4.2・§4.4・§4.5 出典）。
- [Apple Developer Documentation: `UITextInput.unmarkText()`](https://developer.apple.com/documentation/uikit/uitextinput/unmarktext()) —
  「marked text をそのまま確定文字列として残しコンポジションを終了する」意味論（§4.2 出典）。
- [Apple Developer Documentation: `UIKeyInput`](https://developer.apple.com/documentation/uikit/uikeyinput) —
  `hasText`/`insertText:`/`deleteBackward` の 3 メソッド定義（§3.3 出典）。
- [Apple Developer Documentation: `UITextInputTraits`](https://developer.apple.com/documentation/uikit/uitextinputtraits) —
  `keyboardType`/`isSecureTextEntry` 等の入力属性ヒント（§3.3・§4.7 出典）。
- [Apple Developer Documentation: `isSecureTextEntry`](https://developer.apple.com/documentation/uikit/uitextinputtraits/issecuretextentry) —
  パスワード等の機微入力時の学習・予測変換抑止ヒント（§4.7 出典）。
- [Apple Developer Documentation: `UITextInputTokenizer`](https://developer.apple.com/documentation/uikit/uitextinputtokenizer)・
  [`UITextInputStringTokenizer`](https://developer.apple.com/documentation/uikit/uitextinputstringtokenizer) —
  単語・文・段落境界判定プロトコルと既定実装（§4.5 出典）。
- [Apple Developer Documentation: `NSRange`](https://developer.apple.com/documentation/foundation/nsrange) —
  `location`/`length` による UTF-16 code unit 基準の範囲表現（§4.3 出典）。
- [winit Issue #1497（IME/composition support の Tracking issue）](https://github.com/rust-windowing/winit/issues/1497) —
  `ime-architecture.md` §3.4 で open（2026-07 確認）（§4.1・§6 出典）。
- S16・#197（winit 0.30.13 iOS バックエンドのソースレベル検証、本文書 §3.2・§5 の直接の根拠）、
  S17・#198（commit 型入力の動作検証・設計文書とのギャップ表、本文書 §5 の直接の根拠）。
- `docs/architecture/ime-architecture.md`（S1・#182 成果物、本文書が §4.5「iOS」・§9 の
  委譲事項を受けて詳細化する親文書）。
- `docs/architecture/ime-android-input-connection.md`（S18・#199 成果物、本文書が構成・
  比較対象として参照する姉妹文書。§3・§4.2・§4.3 で Android との構造的対比に使用）。
- `docs/architecture/ime-known-constraints.md`（S29・#210 成果物、§3.1・§3.3・§4.2 の
  恒久制約整理。本文書 §4.7・§9 が整合を確認する対象）。
- `crates/fandhe-core/src/ui/text_selection.rs`（S4・#185、
  `try_char_offset_from_utf16`/`char_offset_from_utf16`/`try_selection_from_utf16_range`。
  §4.3 が実際に呼び出す関数として引用）。
- `crates/fandhe-core/src/ui/ime.rs` / `crates/fandhe-winit-ime/src/lib.rs`（既存
  `ImeEvent`/`ImeRequest` 契約・winit 向け変換実装（`text_field_input_from_key`）、
  本設計 §5 が整合確認の対象とする実装）。
- `crates/fandhe-core/src/ui/text_field.rs`（S23・#204、`ImePurpose::Password` の
  代替担保方針の根拠。§4.7 が継承する設計判断の根拠）。
