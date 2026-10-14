# Android `InputConnection` 連携設計（TASK-8.5-S18）

## 1. 目的・位置づけ

本文書は #181（TASK-8.5「IME 統合実装」、REQ-8、M3）の sub-issue（30 件中 18 件目）である
#199（TASK-8.5-S18「Android: InputConnection 連携設計」）の成果物であり、
**設計文書 1 本のみを成果物とし、コード変更は行わない**（S1・#182 → `ime-architecture.md`、
S15・#196 → `ime-ios-uikit-text-input.md` と同じパターンを踏襲する）。

`ime-architecture.md` §4.5「Android（`InputConnection`）マッピング」は Android マッピングの
概要のみを定義し、以下 3 点を本文書へ明示的に委譲している（同文書 §9・§4.5 参照）:

1. `setComposingText(CharSequence, int)` の `newCursorPosition`（挿入後の**相対位置**）→
   `ImeEvent::Preedit.cursor`（preedit 内の絶対 char 範囲）への変換ロジック
2. `finishComposingText()`（確定を伴わないコンポジション終了）を `ImeEvent::Commit` 相当と
   `Disabled` 相当のどちらとして扱うかの確定
3. winit の Android バックエンド実装詳細（android-activity 経由）との整合確認

本 issue はこの 3 点を確定し、S19（#200、実装）が着手可能な粒度まで設計を詰める。
動作検証（実機での日本語入力確認）は S20（#201）のスコープであり、本文書には含まない。

**#196（S15、iOS 側設計）との相互参照**: iOS 側の詳細設計は
`ime-ios-uikit-text-input.md`（S15・#196）を参照する。Android 側との共通契約は
`ime-architecture.md` を正とする（§9 参照）。

## 2. spec 対応

- **REQ-8**（`docs/spec/04-requirements.md`）: TASK-8.5「IME 統合」の一部。CJK かな漢字変換・
  拼音変換等のコンポジション入力を Android で成立させるための詳細設計。
- **TASK-8.5**（`docs/spec/05-tasks.md`）: 成果物「IME 統合モジュール」への寄与。前提タスクは
  TASK-1.1（完了済み）・TASK-1.5（Android 実行基盤、後述 §3.1）。
- **REQ-2**: 依存方向は最適化レイヤ → コアの一方向。本設計は `fandhe-core` に
  `InputConnection` / `EditorInfo` / JNI 型を一切露出しない（`ime-architecture.md` §4.1 の
  `ImeEvent` / `ImeRequest` 契約をそのまま利用し、新しいコア型は追加しない）。
- **REQ-10**（CJK 既定レンダリング）: 本設計の核心は「Android でかな漢字変換・拼音変換の
  コンポジション表示を成立させる経路が存在するか」の確定であり、§4.1 の調査結果が
  REQ-10 の Android 側の実現可能性そのものに関わる。
- **REQ-13 / NFR-9**: `S` へのブリッジは既存 `ImeEvent::Commit` → `TextFieldInput::InsertText`
  経路のみを使い、`fn(&S) -> T` / `fn(&mut S, T)` 契約を維持する（新規経路を追加しない）。
- **REQ-9**（accesskit）: `crates/fandhe-egui-proxy/Cargo.toml` の既存コメント（TASK-9.3・#241）が
  記録するとおり、`accesskit_android` は GameActivity 限定である。§4.1 の GameActivity 移行判断は
  この既存の未解決事項（#241）とも接続点を持つ（§5 参照）。

## 3. 現状分析

### 3.1 Android 実行アーキテクチャの現状（`fandhe-egui-proxy`・TASK-1.5-S2）

- `crates/fandhe-egui-proxy/src/android.rs`・`Cargo.toml`（TASK-1.5-S2）は
  **android-activity の NativeActivity flavor**（`android_activity/native-activity` feature 経由、
  winit の `android-native-activity` feature）を採用している。理由は「OS 同梱のグルーコードで
  起動でき、Java/Gradle 側の追加実装が不要」（同 Cargo.toml コメント）。
- `ime-architecture.md` §3.3 のとおり `fandhe-egui-proxy` は egui 自体の IME 処理
  （`egui::Event::Ime` 相当）に委ねる検証用プロキシであり、本設計（本番の独自レンダラ方式
  プラットフォーム統合レイヤ）の対象読者ではない。ただし **Android の起動方式
  （NativeActivity か GameActivity か）はプロセス全体の構成**であり、S19 が本設計に基づいて
  独自レンダラ方式の Android 実装を追加する際、`fandhe-egui-proxy` とは別プロセス
  （別アプリ）として扱う限り NativeActivity/GameActivity の選択に相互制約はない。ただし
  将来的に両者を同一 Gradle プロジェクト・同一 `Activity` へ統合する場合は起動方式を揃える
  必要があり、その要否は S19 のスコープ外（本文書 §8 に明記）。
- `Cargo.toml`（ルート）は `winit = { version = "0.30", default-features = false }`（workspace
  一元管理）。既存 S9〜S14 の Windows/Linux/macOS 検証はいずれも winit 0.30.13 のソースを
  参照しており、本文書もこれに合わせて **winit 0.30.13** を調査対象バージョンとする
  （`Cargo.lock` は Issue #637 で追跡対象へ変更されているが、`Cargo.toml` のレンジ内で
  最新パッチである 0.30.13 を採用する。S19 実装時に `cargo tree` で実際の解決バージョンを
  確認する旨を申し送る、§9）。

### 3.2 winit 0.30.13 Android バックエンドのソースレベル調査

`src/platform_impl/android/mod.rs`（[GitHub: rust-windowing/winit v0.30.13](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/android/mod.rs)）を読解した結果:

| 契約 | 調査結果 | 判定 |
|------|---------|------|
| `Ime::Enabled`/`Ime::Preedit`/`Ime::Commit`/`Ime::Disabled` の発火 | **一切実装されていない**。`WindowEvent::Ime(_)` を送出するコードパスが存在しない | winit は Android で IME コンポジションを一切扱わない |
| `set_ime_allowed(bool)` | 実装あり。`true` → `self.app.show_soft_input(true)`、`false` → `self.app.hide_soft_input(true)` へ委譲するのみ | ソフトキーボードの表示/非表示のみ機能する |
| `set_ime_cursor_area(Position, Size)` | `pub fn set_ime_cursor_area(&self, _position: Position, _size: Size) {}` — 引数を捨てる no-op | 候補ウィンドウ位置通知は Android では機能しない |
| `set_ime_purpose(ImePurpose)` | `pub fn set_ime_purpose(&self, _purpose: ImePurpose) {}` — no-op | パスワード等のヒントは winit 経由では Android に伝わらない |
| `android_activity::InputEvent` の処理 | 入力イベントループ（`handle_input_event`）は `InputEvent::MotionEvent` と `InputEvent::KeyEvent` の 2 種のみを処理し、他は `warn!("Unknown android_activity input event {event:?}")` で握りつぶす | **`InputEvent::TextEvent`（GameActivity の GameTextInput が発火するコンポジション更新イベント）が未処理**。GameActivity flavor を使っても winit 側がこれを拾わない限り `ImeEvent` へ変換する経路がない |
| `raw-window-handle` 経由の JNI/`AndroidApp` 到達性 | `rwh_05`/`rwh_06` はディスプレイ・ウィンドウハンドルのみを提供し、JNI `JNIEnv`/`AndroidApp` そのものは公開しない。`AndroidApp` は winit 内部で `self.app` として保持されるのみ | プラットフォーム層が `InputConnection` 相当の機能を得るには winit の外側（`android-activity` の `AndroidApp` を直接操作する経路）が必要 |

**結論（委譲事項 3 の確定）**: winit の Android バックエンドは **IME コンポジションを一切
扱わない**。macOS/Windows/Linux（S9/S11/S13）のように「winit が `Ime::*` へ変換してくれる」
前提は Android では成立しない。`fandhe-winit-ime`（既存クレート、winit の `Ime::*` を
`ImeEvent` へ変換する設計）を Android にそのまま適用することはできず、**winit を経由しない
別経路**（android-activity の `AndroidApp` を直接操作する新クレート、§4.5）が必須になる。

### 3.3 android-activity クレートの調査（NativeActivity vs GameActivity）

[rust-mobile/android-activity README](https://github.com/rust-mobile/android-activity)
（android-activity 0.6 系、winit 0.30.13 が要求するバージョンと互換）を調査した結果:

| 観点 | NativeActivity | GameActivity |
|------|----------------|--------------|
| IME/ソフトキーボード対応 | **組み込みの入力メソッド対応なし**。「一部のソフトキーボードは基本 ASCII 入力のための物理キーイベントを配信することがあり、プロトタイピング用途の基本テキスト入力は可能」だが「本番アプリの用途には不十分」（README 原文） | `GameTextInput` ライブラリ経由で組み込みの入力メソッド対応を持つ。`ImeOptions`・アクション設定の更新、IME span 更新（コンポジション状態）の受信が可能 |
| `AndroidApp::text_input_state()` / `set_text_input_state(TextInputState)` | API 自体は `AndroidApp` に存在するが、NativeActivity 経由では IME が `TextInputState` を更新する経路（`GameTextInput` の Java 側 `InputConnection` 実装）が存在しないため、実質的にコンポジション状態が更新されない | `GameTextInput` が Java 側で実装する `InputConnection` が `TextInputState`（`text` / `selection: TextSpan` / `compose_region: Option<TextSpan>`）を更新し、Rust 側から取得・設定できる |
| `AndroidApp::show_soft_input` / `hide_soft_input` | 動作する（ソフトキーボードの表示/非表示自体は NativeActivity でも可能） | 動作する |
| Gradle / AAR 依存 | 不要（OS 同梱のグルーコードのみ） | **必要**（`GameActivity` AAR・`GameTextInput` ライブラリを Gradle プロジェクトへ組み込む必要がある） |
| `InputEvent::TextEvent`（コンポジション更新の通知） | 発火しない | GameTextInput 経由で発火する（ただし winit はこれを処理しない、§3.2） |

`TextInputState` / `TextSpan` の型定義（[android-activity `src/input.rs`](https://github.com/rust-mobile/android-activity/blob/main/android-activity/src/input.rs)）:

```rust
pub struct TextInputState {
    pub text: String,
    pub selection: TextSpan,
    pub compose_region: Option<TextSpan>,
}
pub struct TextSpan {
    pub start: usize, // inclusive
    pub end: usize,   // exclusive（start > end も許容、空範囲は start == end）
}
```

**結論**: 現行 `TASK-1.5-S2` の NativeActivity 方式では **CJK かな漢字変換等のコンポジション
入力が原理的に成立しない**（README が明言する既知の制約）。本設計が要求する「コア側が
preedit を描画する」契約（`ime-architecture.md` §4.3）を Android で実現するには
**GameActivity flavor への移行が前提条件**になる。この判断の詳細と影響範囲は §4.1 で確定する。

## 4. 設計判断（核心）

### 4.1 実現方式の選定（GameActivity + GameTextInput 採用、winit 素通しは不採用）

比較した 3 案:

| 案 | 概要 | 判定 |
|----|------|------|
| (a) **GameActivity + `GameTextInput`**（`AndroidApp::text_input_state()` の差分監視 → `ImeEvent` 変換） | android-activity の GameActivity flavor を採用し、`TextInputState`（`text`/`selection`/`compose_region`）のポーリング/コールバックで差分検出し、独自変換ロジックで `ImeEvent` へ変換する。プラットフォーム層は winit の `Ime::*` に依存せず、`android-activity` の API を直接叩く | **採用**（詳細下記） |
| (b) NativeActivity + 独自 JNI で `InputConnection` 実装ビューを注入 | NativeActivity のネイティブウィンドウの上に、独自に JNI 経由で `View`（`InputConnection` を実装する `EditText` 代替）を生成・フォーカス管理する | **不採用**。NativeActivity は Java 側のアクティビティレイアウトを持たないため、JNI で任意の `View` を注入し `InputMethodManager` へフォーカスさせる実装は `GameActivity` が既に提供する機能を車輪の再発明することになり、AAR 依存を避けられる利点も実質的にない（`GameActivity` 自体は AAR 依存だが Google 公式・保守された実装であるのに対し、本案は自前 JNI 実装の保守負債になる） |
| (c) winit `Ime::*` 素通し | §3.2 の調査どおり winit は Android で `Ime::*` を一切発火しないため、この案自体が成立しない | **不採用（実現不能）** |

**採用: (a) GameActivity + `GameTextInput`**。

**NativeActivity から GameActivity への移行が前提条件になる点の影響範囲**（TASK-1.5 成果物への
影響、明記のみ・変更は行わない）:

- `crates/fandhe-egui-proxy`（TASK-1.5-S2）は本設計の対象読者ではない（§3.1）ため、
  egui-proxy 自体を GameActivity へ移行する必要はない。ただし S19 が実装する**独自レンダラ
  方式の Android 統合クレート**（後述 §4.5「クレート構成案」）は GameActivity flavor 前提で
  設計する必要がある。
- GameActivity 採用には Gradle プロジェクト（`AndroidManifest.xml` の `<meta-data
  android:name="android.app.lib_name">`・`GameActivity`/`GameTextInput` AAR 依存）が必要になり、
  `scripts/android/`（TASK-18.3・TASK-1.5-S1）の NDK/Gradle セットアップスクリプトへの
  影響が生じる可能性がある。この Gradle 構成変更・依存追加は **S19（#200）のスコープ**とし、
  本 issue では変更しない（§8）。
- `accesskit_android` は GameActivity 限定（`fandhe-egui-proxy/Cargo.toml` の既存コメント、
  既存 issue #241・TASK-9.3 参照）であるため、本設計が要求する GameActivity 移行は
  副次的に #241 の前提条件を満たす方向に働く。ただし #241 自体の解決（accesskit 有効化の
  可否判断）は本 issue のスコープ外であり、S19 または #241 側で改めて判断する（§9）。

### 4.2 `InputConnection`（GameTextInput 経由）→ コアのマッピング詳細

android-activity は Java 側の `InputConnection` 実装を `GameTextInput` ライブラリに委ね、
Rust 側には `TextInputState`（差分監視対象）と `show_soft_input`/`hide_soft_input`/
`set_ime_editor_info` という要求 API のみを公開する。すなわち **Rust 側は個々の
`InputConnection` メソッド呼び出し（`setComposingText`・`commitText` 等）を直接は観測できず、
`TextInputState` の値そのものの変化を観測してイベントを合成する**（winit のようにメソッド
呼び出し粒度のイベントには変換されない、という android-activity の抽象化レベルの違いに注意）。

| Android 側の意味論 | android-activity 経由での観測方法 | `ImeEvent` / `ImeRequest` |
|---------------------|-----------------------------------|----------------------------|
| `setComposingText(CharSequence, int)` / `setComposingRegion(int, int)` の呼び出し | `TextInputState.compose_region` が `Some(span)` に変化、かつ `text` が更新される | `ImeEvent::Preedit { text: text[span], cursor }`（`compose_region` 範囲を preedit 文字列として切り出し、`cursor` は §4.3 の変換ロジックで算出） |
| `commitText(CharSequence, int)` / `finishComposingText()` の呼び出し | `TextInputState.compose_region` が `Some` から `None` へ変化する。`text` はこの遷移と共に変化する場合（`commitText` が preedit を別の確定文字列へ置換する場合）としない場合（`finishComposingText` は仕様上 `text` を変更しない。`commitText` が直前の preedit と同一の文字列を確定する場合も同様）がある。**`text` の変化有無だけでは両メソッドを区別できない**（android-activity は `TextInputState` の値そのものしか公開せず、どちらの呼び出しで変化したかを示す情報を持たない）ため、本設計は区別せず同一ロジックで処理する（§4.4） | `ImeEvent::Commit(committed_text)`（新旧 `text` の長さ差分から確定文字列を算出する統一抽出ロジック、§4.4 で確定。`commitText`/`finishComposingText` のどちらであっても同一ロジックで正しい値が得られる） |
| `deleteSurroundingText(int, int)` の呼び出し | `TextInputState.text` からカーソル前後の文字列が削除される（`compose_region` の有無に関わらず発生しうる） | 本設計の中核契約には含めない（§8 スコープ外）。既存 `TextFieldInput`（`crates/fandhe-core/src/ui/text_field.rs`）は `DeleteBackward`/`DeleteForward`（いずれも 1 char 単位）のみを持ち、`deleteSurroundingText` が要求しうる複数文字・任意長の削除には対応しない。複数文字削除の配線方法（`DeleteBackward` の複数回発行に分解するか、`TextFieldInput` へ新バリアントを追加するか）は S19 の実装判断に委ねる |
| `InputMethodManager.showSoftInput` / `hideSoftInputFromWindow` / `restartInput` | コア → android-activity | `ImeRequest::SetEnabled(true/false)` → `AndroidApp::show_soft_input(true)` / `hide_soft_input(true)`（§3.2 で確認済みの winit と同じ委譲形、ただし winit を経由せず android-activity を直接呼ぶ） |
| `CursorAnchorInfo`（候補ウィンドウ位置） | コア → android-activity | `ImeRequest::SetCursorArea` は winit と異なり android-activity に対応 API が存在しない（§3.2・§9 未解決事項）。`AndroidApp::set_ime_editor_info` 経由の代替可否を S19 で確認する必要がある旨を申し送る |
| `EditorInfo.inputType` の `TYPE_TEXT_VARIATION_PASSWORD` | コア → android-activity | `ImeRequest::SetPurpose(Password)` → `AndroidApp::set_ime_editor_info(input_type, ..)` の `input_type` へ `TYPE_TEXT_VARIATION_PASSWORD` 相当のビットを設定する（winit `set_ime_purpose` が no-op である代替経路、§7） |

### 4.3 `newCursorPosition`（相対位置）→ 絶対 char 範囲の変換ロジック（委譲事項 1 の確定）

**前提（Android 公式仕様、[`InputConnection.setComposingText`](https://developer.android.com/reference/android/view/inputmethod/InputConnection#setComposingText(java.lang.CharSequence,%20int))）**:
`newCursorPosition` は挿入するコンポジション文字列 `text` に対する相対値であり、

- `newCursorPosition > 0` の場合: カーソル位置は「コンポジション文字列の末尾から
  `newCursorPosition - 1` 文字分だけ後ろ」（`newCursorPosition == 1` は「挿入文字列の直後」）。
  絶対位置（コンポジション開始位置を 0 とする composing-region 内オフセット）に換算すると
  `absolute = text.chars().count() + newCursorPosition - 1`。
- `newCursorPosition <= 0` の場合: カーソル位置は「コンポジション文字列の先頭から
  `newCursorPosition` 文字分だけ前」（`newCursorPosition == 0` は「挿入文字列の直前」）。
  絶対位置に換算すると `absolute = newCursorPosition`（`newCursorPosition` は 0 以下なので
  文字列先頭より前を指しうる）。
- `commitText(CharSequence, int)` も同一の相対値意味論を持つ（Android API は両メソッドで
  同じ計算式を採用しており、本設計の変換ロジックはどちらにも適用できる）。

**しかし android-activity 経由では `newCursorPosition` という生パラメータそのものが渡って
こない**（§4.2 のとおり、観測できるのは `TextInputState.selection`/`compose_region` という
「計算済みの絶対値」のみ）。したがって本設計が定義する変換関数は、Android の
`newCursorPosition` パラメータを直接受け取るものではなく、**`GameTextInput` が既に上記の
計算式を適用した後の `TextInputState.selection`（`TextSpan { start, end }`）を、
`ImeEvent::Preedit.cursor`（`text`=preedit 文字列基準、char 単位、§4.6）へ再正規化する関数**
として設計する。

**`TextSpan` のオフセット単位は UTF-16 code unit ではなく、`text`（modified UTF-8 のバイト列）に
対するバイトオフセットである**。
[GameTextInputState（Android Developers API リファレンス）](https://developer.android.com/reference/games/game-text-input/struct/game-text-input-state)
は `text_UTF8` フィールドを「modified UTF-8 string」、`text_length` を「Length in **bytes** of
text_UTF8」と明記しており、`selection`/`composingRegion`（[`GameTextInputSpan`](https://developer.android.com/reference/games/game-text-input/struct/game-text-input-span)）の
`start`/`end` はこの `text_UTF8` バイト列を指すインデックスである。`android-activity` の
Rust 側 `TextInputState.text: String` は JNI 層でこの modified UTF-8 から変換された Rust
`String`（標準 UTF-8）であり、`TextSpan { start, end }` はその `String` に対する**バイト
オフセット**として扱う必要がある（iOS `NSRange` や Java `CharSequence` が UTF-16 code unit
基準であるのとは異なり、Android の `CharSequence` 経由の意味論がそのまま android-activity の
公開 API に引き継がれるわけではない点に注意。GameTextInput は Java 側で `InputConnection` の
UTF-16 ベースの呼び出しを一旦 modified UTF-8 へ変換してから Rust 側へ渡している）。
なお modified UTF-8 は補助面文字（絵文字等）を CESU-8 形式のサロゲートペア相当（3 バイト ×
2 = 6 バイト）で符号化する点が標準 UTF-8（4 バイト）と異なるが、`android-activity` が
Rust `String` へ変換する際に標準 UTF-8 へ正規化される前提であり（`String` は常に妥当な
UTF-8 を保持する Rust の型契約上、modified UTF-8 のまま保持することはできない）、本設計は
以降すべて「変換後の Rust `String` に対する標準 UTF-8 バイトオフセット」を前提とする
（この変換自体の正確性は S19 実装時に実機確認する未解決事項として §9 に記録する）。

`TextSpan` は「`start`/`end` が逆転しうる」契約（[`android-activity` `src/input.rs`](https://github.com/rust-mobile/android-activity/blob/main/android-activity/src/input.rs)
のコメント、§3.3 引用）であり、これは `selection` だけでなく `compose_region` にも同じ型が
使われる以上、両方に対して正規化（`min`/`max` で前後関係を揃える）を明示的に行う必要がある
（`fandhe-core` の `SelectionRange` が `start()`/`end()` で自動的に `anchor`/`active` の
大小を揃えるのとは異なり、`TextSpan` はフィールドへ直接アクセスするだけでは正規化されない
ため、この変換関数の内部で明示的に行う）。

```text
入力: TextInputState { text, selection, compose_region: Some(region) }
  - region: TextSpan（text 全体基準、バイトオフセット・標準 UTF-8）— preedit 文字列の範囲。
    start > end の逆転を許容する（§3.3 引用）
  - selection: TextSpan（text 全体基準、バイトオフセット・標準 UTF-8）— カーソル/選択範囲。
    同様に逆転を許容する

手順（すべて `text` 全体を基準にした 1 つの座標系で計算し、preedit 文字列を切り出してから
     再度オフセット計算をやり直す二重変換を避ける — マルチバイト文字が region 境界を
     またぐ場合に誤差が生じるのを防ぐため）:
  1. region 正規化: region_start = min(region.start, region.end)
                    region_end   = max(region.start, region.end)   （バイトオフセット、text 全体基準）
  2. selection 正規化: sel_start = min(selection.start, selection.end)
                       sel_end   = max(selection.start, selection.end)   （同上）
  3. バイトオフセット → char offset 変換（`fandhe-core::ui::text_selection::char_offset_from_byte`、
     S4・#185。範囲外・非文字境界は panic せず直近の文字境界へフォールバックする既存契約を
     そのまま利用）を `text` 全体に対して適用する:
       region_start_char = char_offset_from_byte(text, region_start)
       region_end_char   = char_offset_from_byte(text, region_end)
       sel_start_char    = char_offset_from_byte(text, sel_start)
       sel_end_char      = char_offset_from_byte(text, sel_end)
  4. selection を compose_region の範囲へクランプ（region 外を指す場合の安全側の丸め、
     coding-rust.md の「panic を API 契約にしない」を満たす。`usize` の減算下限割れを
     避けるため、クランプを先に行ってから region_start_char を引く）:
       clamped_start_char = sel_start_char.clamp(region_start_char, region_end_char)
       clamped_end_char   = sel_end_char.clamp(region_start_char, region_end_char)
  5. compose_region 先頭を 0 とした相対 char オフセットへ変換する（両端とも
     `region_start_char` を基準に引く。**片方だけ `region_end_char` を基準にしない**こと
     — 両端を同じ基準点で引かないと範囲が反転・縮退する）:
       cursor_start = clamped_start_char - region_start_char
       cursor_end   = clamped_end_char   - region_start_char
  6. `ImeEvent::Preedit.cursor = Some((cursor_start, cursor_end))` を得る
     （`cursor_start <= cursor_end` は手順 1〜5 の正規化・クランプにより常に成立する）
```

**具体例による検算**: `text = "あいうえお"`（全 5 文字、UTF-8 で 1 文字 3 バイトのため
全体で 15 バイト）、`compose_region = TextSpan { start: 0, end: 15 }`（バイトオフセット）、
`selection = TextSpan { start: 6, end: 6 }`（「あい」の直後、2 文字 × 3 バイト = 6 バイトの
位置。キャレットのみ、範囲選択なし）とすると、`region_start_char = 0`・`region_end_char = 5`・
`sel_start_char = sel_end_char = 2`・クランプ後も `2`・`cursor_start = cursor_end = 2 - 0 = 2`。
すなわち preedit 文字列「あいうえお」の 2 文字目
（「う」の直前）にキャレットがある、という直感どおりの結果になる。

**この設計により、Android の `newCursorPosition` 相対値変換という Android 固有の計算
（§4.3 冒頭の式）は `GameTextInput`（Java 側実装）の内部に既に吸収されており、
`fandhe-core` ⇔ プラットフォーム層の契約側では「`TextInputState.selection` を
`compose_region` 相対の絶対範囲へ再正規化するだけ」というプラットフォーム非依存に近い
単純な処理に帰着する**。ただし、この帰着が成立する前提（`GameTextInput` が
`newCursorPosition` の計算を正しく `selection` へ反映することを常に保証するか）は
一次情報（android-activity/GameTextInput ソース）でのメソッド単位の追跡までは行っておらず、
`TextInputState` という公開契約のドキュメントに基づく設計判断である旨を明記する
（S19 実装時に実機ログで実際の `selection` 値を確認するチェック項目として申し送る、§9）。

### 4.4 `commitText()` / `finishComposingText()` の共通処理・確定文字列抽出ロジック（委譲事項 2 の確定）

**観測不能性の確認（§4.2 表の補足）**: android-activity は `TextInputState` の値そのものしか
公開せず、`compose_region` が `Some` から `None` へ変化した際にそれが `commitText()` の
呼び出しによるものか `finishComposingText()` の呼び出しによるものかを示す情報を持たない。
`text` が変化したかどうかも区別の手がかりにはならない ——
`finishComposingText()` は仕様上 `text` を変更しない一方、`commitText()` も「直前の preedit と
同一の文字列を確定する」場合（例: 変換候補を確定せず素通しする、既存の未確定文字列をそのまま
確定する）には `text` が変化しない。逆に `commitText()` が preedit を別の文字列へ置き換える
場合（例:「かんじ」→「漢字」の変換確定）は `text` が変化する。**したがって「`text` の変化
有無」を `commitText()`/`finishComposingText()` の判別に用いることはできず、本設計は
両者を区別せず同一の抽出ロジックで処理する**（§4.2 の表もこの前提に統一済み）。

**Android 公式仕様**:

- [`InputConnection.finishComposingText`](https://developer.android.com/reference/android/view/inputmethod/InputConnection#finishComposingText())：
  「現在アクティブなコンポジションを終了させる。テキスト自体はそのまま残し、コンポジションに
  付随する特殊なスタイル・状態を除去するのみ。カーソル位置は変化しない」。すなわち
  `finishComposingText()` は**「テキストを消す」動作ではなく「confirm はするがテキストの
  変更は行わない」動作**である。
- [`InputConnection.commitText`](https://developer.android.com/reference/android/view/inputmethod/InputConnection#commitText(java.lang.CharSequence,%20int))：
  コンポジション中の文字列（存在する場合）を `text` 引数で置き換えて確定する。置き換え後の
  文字列は直前の preedit と同一の場合も、異なる場合（変換候補の選択等）もある。

`ime-architecture.md` §4.5 が提示した 2 択（`Commit` 相当 / `Disabled` 相当）について、
本設計は次の理由で両メソッドとも **`Commit` 相当として扱う**ことを確定する:

- **`Disabled` 相当が不適切な理由**: `ime-architecture.md` §4.7 の契約は「`Disabled` は
  コンポジション状態（preedit 表示）をクリアするのみで、`Commit` 済み文字列の再適用・
  巻き戻しをしてはならない」というクリーンアップ専用の意味論である。しかし
  `commitText()`/`finishComposingText()` はいずれもテキストを**確定として残す**動作であり、
  「破棄してクリアする」という `Disabled` の意味論とは逆である。`Disabled` として扱うと、
  コンポジション中の文字列がコア側の `buffer`（`ime-architecture.md` §4.3）へ一切反映されない
  まま消えてしまい、ユーザーが変換確定したはずの文字列が失われる（データロスであり、
  OWASP 的にも「意図しない入力破棄」は堅牢性の欠陥）。
- **`Commit` 相当が適切な理由**: 「テキストは確定として残る」という両メソッド共通の仕様を、
  既存コア契約の「確定した文字列のみが `buffer` へ挿入される」（`ime-architecture.md` §4.3）
  という意味論に最も忠実に写像できる。**新たに `Disabled` イベントを追加発火する必要はない**
  （直後のコンポジション状態クリアは `Disabled` 相当ではなく、単純に「preedit 表示のクリア」
  という `Commit` の副作用、`ime-architecture.md` §4.7「`Commit` はコンポジション状態の
  クリアとは独立して即時に確定文字列を反映する契約」に委ねる。`commitText()`/
  `finishComposingText()` の後にユーザーが引き続き入力を続けるか、フォーカスを外すかは
  別のシグナル（`TextInputState.compose_region` が再度 `Some` になるか、`FocusLost` が
  来るか）で判定されるため、これらの呼び出し自体は「IME 無効化」を意味しない）。
- **既存 `TextFieldWidget::apply_ime` との整合**: `ime-architecture.md` §4.7 により
  `Disabled` 受信時のクリーンアップは冪等（二重適用禁止）に設計されているため、本判断
  （`Commit` 相当として扱い `Disabled` を発火しない）は既存の状態モデルに対して**追加の
  変更を要求しない**（プラットフォーム層の変換ロジックのみに閉じる判断であり、S3/S8 が
  実装したコア側の状態機械には影響しない）。

**確定文字列の抽出ロジック（長さ差分ベース、両メソッド共通）**: `finishComposingText()` 検出時
（すなわち `text` が変化しない場合）に限り「直前の `compose_region` 内文字列をそのまま使う」
という単純な抽出は、`commitText()` が preedit を別の文字列へ置き換える場合（例:「かんじ」→
「漢字」）には**誤り**になる ——
その場合 `compose_region` が指していた旧 `text` 上の範囲は既に確定前の文字列（「かんじ」）を
指しており、そのまま使うと確定後の実際の文字列（「漢字」）ではなく確定前の preedit 文字列を
`ImeEvent::Commit` として発火してしまう。したがって本設計は `commitText()`/
`finishComposingText()` のどちらであっても正しく動作する、次の**新旧 `text` の長さ差分に
基づく統一抽出ロジック**を確定する:

```text
入力: 直前の TextInputState { text: old_text, compose_region: Some(region) }
      直後の TextInputState { text: new_text, compose_region: None }
  - region はバイトオフセット・標準 UTF-8（§4.3 参照）で正規化済み（region_start_byte/region_end_byte）

手順:
  1. region_len_byte    = region_end_byte - region_start_byte
  2. committed_len_byte = new_text.len() + region_len_byte - old_text.len()
     （`commitText()` が region_len_byte バイトの preedit を committed_len_byte バイトの
     確定文字列に置き換えたと仮定した場合の確定文字列の長さ。`finishComposingText()` は
     new_text.len() == old_text.len() のため committed_len_byte == region_len_byte に一致し、
     結果として「compose_region 内文字列をそのまま使う」場合と同じ値になる。実装では
     `new_text.len() - old_text.len()` を先に評価すると確定文字列が preedit より短い
     通常の CJK ケース（例:「かんじ」9 バイト→「漢字」6 バイトのような縮小）で `usize`
     アンダーフローするため、上記の順序（`new_text.len() + region_len_byte` を先に加算
     してから `old_text.len()` を減算する）または符号付き整数での計算にする。縮小自体は
     エラーではなく通常のケースであり、`committed_len_byte` が `new_text` の範囲外に
     なる場合のみ空文字列へフォールバックする）
  3. committed_text = new_text[region_start_byte .. region_start_byte + committed_len_byte]
     （`region_start_byte` / `region_start_byte + committed_len_byte` が `new_text` の
     char 境界上にない場合は §4.3 と同じ安全側のクランプ方針に従う。ただし
     `char_offset_from_byte` は char 単位オフセットを返す関数であり、スライス境界
     （byte index）として直接使うと多バイト文字（CJK 等）を含む文字列で切り出しが
     ずれる、または境界不一致で panic する。安全にクランプするには
     `byte_offset_at(new_text, char_offset_from_byte(new_text, byte_index))` のように
     `char_offset_from_byte` の戻り値を `byte_offset_at` へ通して byte index へ
     戻してから使う。`committed_len_byte` が負値・`new_text` の範囲外になる場合は
     空文字列へフォールバックし panic しない）
```

この式は `commitText()`（確定文字列が preedit と異なる・同じ場合の両方）・
`finishComposingText()`（`text` 不変）のいずれの場合でも `new_text` 上の実際に確定された
文字列を正しく切り出す（`text` が変化したかどうかを判定する必要がない）。

**確定**: `commitText()`/`finishComposingText()`（区別しない）→
`ImeEvent::Commit(committed_text)`（上記の長さ差分ベースの抽出ロジックで算出）。
`Disabled` は発火しない。

### 4.5 バイトオフセット → char offset 正規化・クレート構成案

- **バイトオフセット → char offset 正規化**: §4.3 のとおり、`TextInputState` の
  `selection`・`compose_region`（`TextSpan`）は Android の `CharSequence`（UTF-16 code unit 基準）
  ではなく、`text`（`android-activity` が modified UTF-8 から変換した Rust `String`）に対する
  **標準 UTF-8 バイトオフセット**である。したがって本設計が実際に呼び出すのは
  `crates/fandhe-core/src/ui/text_selection.rs` の
  `try_char_offset_from_byte` / `char_offset_from_byte`（クランプ版）・
  `try_selection_from_byte_range`（byte offset 系、S4・#185）であり、iOS 側が使う
  `try_char_offset_from_utf16` / `char_offset_from_utf16` / `try_selection_from_utf16_range`
  （UTF-16 code unit 系）ではない。`ime-architecture.md` §4.6 の既定方針（非文字境界は文字境界へ
  クランプ、`Result` を返さず安全側へフォールバック）は byte offset 系の関数にも同様に適用
  される。**新規関数の追加は不要**（S19 は既存の byte offset 系関数を呼ぶだけでよい）。
- **クレート構成案**: 既存 `fandhe-winit-ime`（winit の `Ime::*` を変換するクレート）は
  §3.2 の結論により Android では利用できない。S19 は **`fandhe-android-ime`
  （新クレート、`fandhe-` 接頭辞、coding-rust.md 準拠）を新設**し、
  `android-activity` の `AndroidApp`（GameActivity flavor）を直接操作する設計を推奨する
  （`fandhe-winit-ime` へ Android 分岐を追加する案は、依存する下位クレートが winit では
  なく android-activity 直接になるため、責務が異なり同一クレートに同居させる利点がない）。
- **`unsafe`/JNI 境界の置き場所**: `android-activity` クレート自体が JNI 呼び出しを
  `unsafe` なしで安全にラップして公開する（`AndroidApp` の公開 API は safe）ため、
  **`fandhe-android-ime` 自体は `unsafe` を必要としない見込み**である。ただし
  `EditorInfo.inputType` の生ビットフラグ操作など、android-activity が safe API を
  提供しない領域が S19 実装時に見つかった場合は、coding-rust.md（最小範囲・
  `// SAFETY:` コメント・safe API ラップ）に従い `fandhe-android-ime` 内の最小モジュールへ
  閉じ込める。`fandhe-core` は `#![forbid(unsafe_code)]` を維持する（本設計はコア型を
  一切変更しないため、この制約に影響しない）。

### 4.6 コア → Android の `ImeRequest` 変換（§4.2 表の補足）

- `SetEnabled(true/false)` → `AndroidApp::show_soft_input(true)` / `hide_soft_input(true)`
  （§3.2・§4.2 で確認済み。winit と同じ委譲形だが android-activity を直接呼ぶ）。
- `SetCursorArea { origin, size }` → **対応する android-activity API が存在しない**
  （winit の `set_ime_cursor_area` は no-op であり、android-activity 自体にも
  `CursorAnchorInfo` 相当の直接 API が確認できていない）。候補ウィンドウ位置の通知は
  Android では実質的に「システム標準の位置（画面下部固定）に委ねる」フォールバックと
  なる可能性が高く、この点は未解決事項として §9 に記録し、S19 実装時に
  `GameTextInput`/`InputMethodManager` 側の代替 API（`updateCursorAnchorInfo` を
  JNI 経由で直接呼ぶ独自実装等）の要否を判断する。
- `SetPurpose(Password)` → `AndroidApp::set_ime_editor_info(input_type, action, options)` の
  `input_type` へ `EditorInfo.TYPE_TEXT_VARIATION_PASSWORD`（`android.text.InputType` の
  定数値）相当のビットを設定する。winit `set_ime_purpose` が Android で no-op であるため
  （§3.2）、`ImeRequest::SetPurpose` を実際に反映できる**唯一の経路**がこの
  `set_ime_editor_info` 呼び出しになる。

### 4.7 ソフトウェアキーボードのライフサイクル

- フォーカス遷移との結合は `ime-architecture.md` §4.7 の契約（`FocusGained` →
  `SetEnabled(true)`、`FocusLost` → コンポジションクリーンアップ後 `SetEnabled(false)`）を
  そのまま継承する。Android 固有の追加考慮点は無い（`show_soft_input`/`hide_soft_input`
  は明示呼び出しであり、macOS/Windows/Linux のような暗黙の発火タイミング差異
  （§4.4 デスクトップ調査、S9/S11/S13）は生じない）。
- **`WindowInsets`（ソフトキーボード表示時のレイアウト圧迫）は本設計の中核契約外**とする
  （§8 スコープ外）。`ime-architecture.md` はレイアウト再計算の責務分界を定義しておらず、
  Android のみの追加要件になるため、必要になった時点で別 issue として起票する
  （out-of-scope-tracking.md 準拠、ユーザー承認後）。

## 5. S19・S20 への接続点

- **S19（#200、実装）**: §4.1 の GameActivity 移行判断・§4.5 の `fandhe-android-ime` クレート
  新設・§4.2〜§4.4 のマッピング/変換ロジックをそのまま実装対象として引き継ぐ。Gradle/AAR
  依存の追加は S19 のスコープ（本 issue では変更しない）。
- **S20（#201、動作検証）**: 実機（Android 実行環境、#15 の整備完了後）での日本語入力
  （かな漢字変換）確認、§4.3 の `selection`/`compose_region` 実値の実機ログ確認
  （§4.3 末尾の申し送り）、§4.6 のカーソル位置フォールバック挙動の目視確認を含む。
- **#241（TASK-9.3、accesskit_android の GameActivity 限定問題）**: 本設計の GameActivity
  移行判断が前提条件を満たす方向に働く旨を #241 側へ情報共有する候補（ユーザー承認後、
  §9 参照）。

## 6. トレードオフ・代替案（不採用理由つき）

- **winit の Android IME 対応強化を待つ案（不採用）**: `winit` は Issue #1497
  （IME/composition support の Tracking issue、`ime-architecture.md` §3.4 で 2026-07 時点
  open 確認済み）を抱えるが、Android の `InputEvent::TextEvent` 未処理（§3.2）は
  この Tracking issue とは別の欠落であり、winit 側の対応時期が不明な状態でスケジュール
  （TASK-8.5 推定 15 日、M3 所属）を待つことはできない。よって winit を経由しない
  直接統合（§4.1 案 (a)）を採用する。
- **NativeActivity のまま「基本 ASCII 入力」に限定する案（不採用）**: README が明言する
  「物理キーイベント経由の基本 ASCII 入力」で妥協する案も検討したが、REQ-10（CJK 既定
  レンダリング）の前提を満たせず、日本語・中国語・韓国語のコンポジション入力自体が
  不可能になるため不採用。
- **独自 JNI で `InputConnection` を自前実装する案（不採用、§4.1 案 (b) 再掲）**: 保守コストが
  高く、Google 公式の `GameActivity`/`GameTextInput` が同等機能を提供する以上、車輪の
  再発明になるため不採用。

## 7. セキュリティ考慮（OWASP Top 10 観点）

`.claude/rules/security.md` 準拠。

- **A03 インジェクション**: `TextInputState.text`/`compose_region` 経由で届く文字列は外部
  入力として扱い、`ImeEvent` への変換時に生の JNI/`CharSequence` 型・キーコードを含めない
  （§4.1 の型レベル防御を継承、`ime-architecture.md` §7 と同一方針）。制御文字除去は既存
  `TextFieldInput::InsertText` の意味論を継承する。バイトオフセットの境界外・非文字境界は
  panic せずクランプする（§4.3・§4.5、`Result` を返さず安全側の丸めで統一。
  coding-rust.md の「panic を API 契約にしない」を満たす）。
- **機微情報の露出防止**: `TextInputState` の `Debug`/ログ出力は文字数のみ出力する契約とし
  （既存 `TextFieldWidget` のマスク方針を継承）、`fandhe-android-ime`（S19）が
  `TextInputState` を直接 `Debug` 出力しない・文字列内容をログへ書かない設計を要求する。
  `ImePurpose::Password` 時は `EditorInfo.inputType` の `TYPE_TEXT_VARIATION_PASSWORD`
  相当（§4.6）で IME の学習・予測変換を抑止する。ただし winit の `set_ime_purpose` が
  Android で no-op である問題（`ime-architecture.md` §9）とは別に、本設計の
  `set_ime_editor_info` 経路がこの抑止を実際に保証するかは GameTextInput の実装依存であり、
  S19 実装時に実機確認する必要がある旨を記録する（§9）。
- **A05 設定ミス**: 本 issue は CI・ビルド設定を変更しない。GameActivity 採用に伴う
  Gradle/`AndroidManifest.xml` 変更は S19 のスコープとし、デバッグフラグ・過剰権限の
  混入がないことはその際に検査する。
- **A06 脆弱な依存**: 本 issue は依存追加を行わない。S19 で `GameActivity`/`GameTextInput`
  AAR・`android-activity` の GameActivity feature を追加する際は `cargo audit` 相当の確認を
  行う旨を明記する。
- **メモリ安全性**: §4.5 のとおり `fandhe-android-ime` は android-activity の safe API のみで
  構成できる見込みであり、`unsafe` が必要になった場合は最小範囲・`// SAFETY:` コメント・
  safe API ラップを必須とする（coding-rust.md 準拠）。`fandhe-core` の
  `#![forbid(unsafe_code)]` は本設計で変更しない。security-auditor は `unsafe` の新規追加を
  必ずレビュー対象とするが、本文書はコード変更を伴わないため対象なし。
- **シークレット**: 混入なし（create-commit スキルの検査を通す）。

## 8. スコープ外（明記）

- **音声入力・手書き入力**（`ime-architecture.md` §8 で全プラットフォーム共通のスコープ外
  として明記済み。Android も同様）。
- **`getCursorCapsMode` 等の大文字化ヒント**（`ime-architecture.md` §8 と同様、Android
  でも中核契約に含めない）。
- **`WindowInsets` によるレイアウト再計算**（§4.7）。
- **GameActivity 移行に伴う Gradle/AAR 構成変更そのもの**（S19 のスコープ）。
- **`fandhe-egui-proxy`（TASK-1.5-S2）の NativeActivity → GameActivity 移行**: 本設計の
  対象読者ではなく（§3.1）、変更を要求しない。
- **実機での動作検証**（S20・#201 のスコープ、本文書は設計のみ）。
- **#241（accesskit_android の GameActivity 限定問題）の解決そのもの**: 情報共有の候補
  として §5・§9 に記録するのみで、本 issue では対応しない。

## 9. 未解決事項・将来課題

- **modified UTF-8 → 標準 UTF-8（Rust `String`）変換の境界での正確性が未確認**（§4.3）:
  `android-activity` の JNI 層が `GameTextInputState.text_UTF8`（modified UTF-8、補助面文字は
  CESU-8 形式のサロゲートペア表現）を Rust `String`（標準 UTF-8）へ変換する実装、および
  その際に `TextSpan` のバイトオフセットを再計算するか元の値をそのまま使うかは
  `android-activity` のソースレベル追跡までは行っていない。補助面文字（絵文字等）を含む
  コンポジション入力がある場合、バイトオフセットの計算がずれる可能性があり、S19/S20 で
  実機確認が必要な項目として申し送る。
- **`ImeRequest::SetCursorArea` の Android 対応 API が未確認**（§4.6）: winit・
  android-activity のいずれにも `CursorAnchorInfo` 相当の直接 API が確認できておらず、
  候補ウィンドウ位置通知が Android で実現できるかは S19 実装時に
  `InputMethodManager.updateCursorAnchorInfo`（JNI 経由の独自呼び出し）の要否を含めて
  確認する必要がある。
- **`TextInputState.selection` が `newCursorPosition` の計算を正しく反映する保証の未確認**
  （§4.3）: `GameTextInput`（Java 側実装）のソースレベル追跡までは行っておらず、
  公開された `TextInputState` 契約のドキュメントに基づく設計判断である。S19/S20 で実機
  ログにより実際の値を確認するチェック項目として申し送る。
- **`ImePurpose::Password` の学習・予測変換抑止**（§7）: 実装は Issue #586 で完了済み
  （`fandhe-winit-ime::android_editor_info::apply_ime_purpose_android`）。`set_ime_editor_info`
  で実際に Gboard 等の学習辞書へ反映されるかは GameTextInput の実装依存のため未確認のまま
  であり、実機確認は Issue #588 へ
  人間担当として申し送り済み（`TextFieldWidget` を Android 実行アプリへ配線する
  前提が別途必要）。
- **GameActivity 移行が `scripts/android/`（TASK-18.3・TASK-1.5-S1）の NDK/Gradle
  セットアップスクリプトへ与える影響**（§4.1）: 具体的な変更内容は S19 のスコープとし、
  本文書では「影響が生じる可能性がある」ことの記録のみに留める。
- **#196（S15、iOS 設計）との相互参照**: #196 が本文書より先にマージされた場合、
  §1 に記した保留を解除し `ime-ios-uikit-text-input.md` への相互参照を追加する
  （フォローアップ、ユーザー承認後に別コミットで対応）。
- **#241（TASK-9.3、accesskit_android の GameActivity 限定問題）との接続**: 本設計の
  GameActivity 移行判断を #241 側へ情報共有するかはユーザー承認事項とし、本 issue では
  Issue コメント等の追記を行わない（out-of-scope-tracking.md 準拠）。
- **watchOS/visionOS の IME 制約**（`ime-architecture.md` §9 と同様、TASK-8.6 のスコープ）。
- **winit Issue #1497 の進展監視**（`ime-architecture.md` §9 と同様）。Android の
  `InputEvent::TextEvent` 未処理（§3.2）が今後の winit リリースで解消された場合、本文書
  §3.2・§4.1 の前提を再評価する必要がある。

## 10. 出典

- [`winit` v0.30.13 `src/platform_impl/android/mod.rs`（GitHub）](https://github.com/rust-windowing/winit/blob/v0.30.13/src/platform_impl/android/mod.rs) —
  `Ime::*` 未実装、`set_ime_allowed`/`set_ime_cursor_area`/`set_ime_purpose` の実装内容、
  `InputEvent::MotionEvent`/`KeyEvent` のみ処理し `TextEvent` は未処理（§3.2 出典）。
- [`android-activity`（rust-mobile、GitHub README）](https://github.com/rust-mobile/android-activity) —
  NativeActivity の IME 制約・GameActivity + `GameTextInput` の対応内容（§3.3 出典）。
- [`android-activity` `android-activity/src/input.rs`（GitHub）](https://github.com/rust-mobile/android-activity/blob/main/android-activity/src/input.rs) —
  `TextInputState`/`TextSpan` の型定義（§3.3 出典）。
- [`android-activity`（docs.rs、latest）](https://docs.rs/android-activity/latest/android_activity/struct.AndroidApp.html) —
  `AndroidApp::show_soft_input`/`hide_soft_input`/`text_input_state`/
  `set_text_input_state`/`set_ime_editor_info` の API（§3.3・§4.2・§4.6 出典）。
- [Android Developers: `InputConnection`（`setComposingText`/`finishComposingText`/
  `commitText`/`setComposingRegion`/`deleteSurroundingText`）](https://developer.android.com/reference/android/view/inputmethod/InputConnection) —
  `newCursorPosition` の相対位置意味論（§4.3 出典）、`finishComposingText()` の
  「テキストは残す・状態のみ解除」の意味論（§4.4 出典）。
- Android Developers: `InputMethodManager`（`showSoftInput`/`hideSoftInputFromWindow`/
  `restartInput`）、`EditorInfo`（`inputType`・`TYPE_TEXT_VARIATION_PASSWORD`）、
  `CursorAnchorInfo` — §4.2・§4.6 の一般知識に基づく設計判断（個別ページの逐語引用は
  行っていない。S19 実装時に実装対象 API ごとに一次情報を再確認する）。
- `docs/architecture/ime-architecture.md`（S1・#182 成果物、本文書が §4.5「Android
  マッピング」・§9 の委譲事項を受けて詳細化する親文書）。
- `crates/fandhe-egui-proxy/Cargo.toml`（TASK-1.5-S2、NativeActivity 採用の既存判断・
  accesskit_android の GameActivity 限定コメント、#241 参照）。
- `crates/fandhe-core/src/ui/ime.rs` / `crates/fandhe-winit-ime/src/lib.rs`（既存
  `ImeEvent`/`ImeRequest` 契約・winit 向け変換実装、本設計がそのまま踏襲する対象）。
- `crates/fandhe-core/src/ui/text_selection.rs`（S4・#185 実装済み、`char_offset_from_byte`
  等のバイトオフセット → char offset 変換関数。§4.3・§4.5 が実際に呼び出す関数として引用）。
- [`GameTextInputState`（Android Developers API リファレンス）](https://developer.android.com/reference/games/game-text-input/struct/game-text-input-state) —
  `text_UTF8` が modified UTF-8、`text_length` がバイト長であることの出典（§4.3 の出典）。
- [`GameTextInputSpan`（Android Developers API リファレンス）](https://developer.android.com/reference/games/game-text-input/struct/game-text-input-span) —
  `start`/`end` フィールドの定義（§4.3 の出典）。
