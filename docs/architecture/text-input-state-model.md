# テキスト入力状態モデル（コンポジション・確定文字列）設計

## 1. 目的・位置づけ

本文書は #181（TASK-8.5「IME 統合実装」、REQ-8）の sub-issue である #183
（TASK-8.5-S2「テキスト入力状態モデル（コンポジション・確定文字列）設計」）の成果物であり、
`TextFieldWidget<S>` のコンポジション（未確定文字列）と確定文字列の二層状態モデル（内部表現・
状態遷移・不変条件）を定める。実装は `crates/fandhe-core/src/ui/text_field.rs`
（S3・#184 / S8・#189）。

### S1（#182・`ime-architecture.md`）との関係

用語・型は `ime-architecture.md`（S1・#182）と整合させる。引用はセクション番号ではなく
シンボル名（`ImeEvent::Preedit` 等）で行う（同文書のセクション番号変更の影響を受けないため）。

S1 が確定した契約（要約、詳細は `ime-architecture.md` 参照）:

- コア⇔プラットフォーム層の境界イベント `ImeEvent`（`Enabled` / `Preedit { text, cursor }` /
  `Commit(String)` / `Disabled`）と要求 `ImeRequest`（`SetEnabled` / `SetCursorArea` /
  `SetPurpose`）
- `CharOffset = usize`（char 単位オフセット。既存 `text_field.rs` の `cursor: usize` と型を揃える）
- 責務分界: **コアがコンポジション状態の保持・描画を担い、プラットフォーム層がネイティブ
  IME API との相互変換を担う**
- プラットフォーム別マッピング表（winit・UIKit・`InputConnection`・`compositionevent`）は
  `ime-architecture.md` が定義するものであり、本文書では再掲しない（重複管理を避ける。
  差分が生じた場合は `ime-architecture.md` を正とする）

本文書のスコープは、S1 が定義した `ImeEvent` を受け取った**コア側の状態モデルが内部で
何を保持し、どう遷移するか**（バッファ構造・カーソル表現・不変条件）に限定される。

## 2. spec 対応

- **REQ-8**（`docs/spec/04-requirements.md` L114-124）: IME 統合は欠落 5 領域の 1 つ。本文書は
  その状態モデル設計を担う。
- **REQ-2**: 共通コア（`fandhe-core`）⇔ プラットフォーム最適化レイヤの二層アーキテクチャ。
  本文書が設計する状態型はプラットフォーム型（`winit`/`web_sys`/`UIKit`/`InputConnection`）に
  一切依存しない `fandhe-core` 内の純粋な状態表現とする。
- **REQ-10**: CJK を含む多言語グリフの既定レンダリング。かな漢字変換等の CJK 変換入力を
  成立させる前提としてコンポジション状態が必要。
- **REQ-13 / NFR-9**: 状態アクセスは `fn(&S) -> T` / `fn(&mut S, T)` に限定。本文書の
  コンポジション状態は既存 `buffer` と同じ「`S` に含めない一時的 UI 状態」として設計し、
  確定文字列のみが `Event::TextFieldChanged` 経由で `S` へブリッジされる（S1 §4.3 を継承）。
- **REQ-9**: `accesskit` を通じたアクセシビリティ橋渡し。コンポジション中も
  `AccessibilityRole::TextInput`（既存）のまま扱う前提を継承し、コンポジション状態自体の
  アクセシビリティツリー公開は将来課題とする（S1 §9 を継承、本文書では扱わない）。

## 3. 前提（既存の編集モデル）

- `crates/fandhe-core/src/ui/text_field.rs`（`TextFieldWidget<S>`）:
  - `buffer: String`（編集中バッファ、一時的 UI 状態）・`cursor: usize`（**char 単位**、
    `buffer.chars().count()` を上限にクランプ）・`dirty: bool`（stale blur-commit 防止フラグ、
    `sync_from_state` がフォーカス中も `committed_value` を進めるため差分比較のみでは
    「外部更新」と「ユーザー編集」を区別できない問題への対策）・`committed_value: String`
  - `byte_index_at(buffer: &str, char_index: usize) -> usize`: char 単位 → byte 単位への
    変換関数（`str::char_indices()` 相当、境界外は末尾へクランプし panic しない）
  - `commit_if_changed(&mut self) -> Option<Event>`: `dirty` かつ `buffer != committed_value`
    の場合のみ `Event::TextFieldChanged` を発火する blur-commit の実体
  - `sync_from_state(&mut self, state: &S)`: `value_source: Option<ReadFn<S, String>>` から
    確定値を読み取り、常に `committed_value` を進める。`buffer` / `cursor` は
    `!self.interaction.focused || !self.dirty`（非フォーカス中、またはフォーカス中でも
    ユーザー未編集）の場合に追随させ、フォーカス中にユーザーが編集済み（`dirty`）の
    場合のみ据え置く（`dirty` はユーザー編集を表す唯一のフラグであり、コンポジション中か
    どうかを区別しない。**本文書の状態モデルが `composition` を導入する前提を壊さないよう、
    §4.9 で `composition.is_some()` の間も `buffer` / `cursor` を据え置く契約を追加する**）
  - `apply_input(&mut self, input: InteractionInput) -> Option<Event>`:
    `InteractionInput::FocusLost` で `commit_if_changed()` を呼ぶ
  - `TextFieldInput`（`InsertText` / `DeleteBackward` / `DeleteForward` /
    `MoveCursorLeft` / `MoveCursorRight` / `MoveCursorToStart` / `MoveCursorToEnd` /
    `Commit` / `Cancel`）: `InsertText` は `char::is_control` で制御文字を除去してから
    カーソル位置へ挿入する（OWASP A03 対策）
  - `Debug for TextFieldWidget<S>`: `buffer_len` / `committed_value_len`（`chars().count()`）
    のみを出力し、内容を出さない（機微情報の露出防止）
  - モジュール doc のスコープ外事項に「IME 変換合成（composition）・テキスト選択範囲・
    grapheme cluster 単位のカーソル処理」を明記している。本文書は composition を扱う。
    テキスト選択範囲・grapheme cluster は S4・S26 の対象。
- `crates/fandhe-core/src/ui/widget.rs`:
  `InteractionState`（`hovered` / `pressed` / `disabled` / `focused`）・
  `InteractionInput`（`PointerEntered` / `PointerExited` / `PointerPressed` /
  `PointerReleased` / `FocusGained` / `FocusLost`）
- `crates/fandhe-core/src/state/store.rs`: `Event::TextFieldChanged { label, value }`。
  widget 層は `&mut S` に触れず `StateStore::dispatch` へブリッジする単一ソース設計。
- winit 0.30（`Cargo.toml` workspace 依存、`fandhe-egui-proxy` 経由で使用）:
  S1 が確定した `WindowEvent::Ime(Ime::{Enabled, Preedit(String, Option<(usize, usize)>), Commit(String), Disabled})`
  が本モデルへの主要な入力ソース（winit は byte offset、コア契約は char offset、変換は
  プラットフォーム層が担う。§4.6 出典は `ime-architecture.md` 参照）。

## 4. 設計判断

### 4.1 二層構造の定義

- **確定文字列（committed text）**: 既存 `buffer` / `committed_value` の意味論をそのまま
  踏襲する。`Event::TextFieldChanged` で `S` へブリッジされる唯一の値であり続ける
  （新しい `Event` バリアントを追加しない、S1 §4.3 を継承）。
- **コンポジション（composition / 未確定文字列 / preedit）**: IME が所有する一時状態。
  `ImeEvent::Preedit` を受けて状態モデルが保持するが、**確定バッファ（`buffer`）には
  決して混入させず**、表示時にのみ合成する（§4.4 参照）。`ImeEvent::Commit` を受けて
  初めて `buffer` へ反映される。

### 4.2 型設計

```rust
/// IME コンポジション（未確定文字列）の状態。
///
/// [`super::text_field::TextFieldWidget`] が保持する一時的 UI 状態であり、
/// `S` には含めない（二層モデル、REQ-13）。プラットフォーム層から届く
/// `ImeEvent::Preedit`（`docs/architecture/ime-architecture.md` §4.1）を
/// 受けて更新される。
pub struct Composition {
    /// 現在の未確定文字列全体。
    text: String,
    /// `text` 内でのカーソル範囲（`text` の先頭を 0 とした相対 `CharOffset`、
    /// `ImeEvent::Preedit.cursor` と同じ座標系）。`None` はカーソルを表示しない
    /// ことを表す。
    cursor: Option<(CharOffset, CharOffset)>,
    /// コンポジション開始時点の確定バッファ内カーソル位置（char 単位、
    /// `TextFieldWidget::cursor` と同じ座標系）。
    ///
    /// `cursor` フィールド（`text` 内の相対範囲）と `anchor`（`buffer` 内の絶対位置）は
    /// **別の座標系**であり、混同しないこと（`ime-architecture.md` L166-168 の
    /// 「`Preedit.cursor` と `TextFieldWidget::cursor` は別座標系」という注記を受け、
    /// 本モデルは両者を橋渡しする第 3 の値として `anchor` を新設する）。
    /// 表示合成（§4.4）・確定挿入位置（§4.5）はいずれもこの `anchor` を起点とする。
    anchor: CharOffset,
}

impl std::fmt::Debug for Composition {
    /// 未確定文字列の内容を出力しない（文字数のみ）。既存
    /// `TextFieldWidget` の Debug マスク方針（`buffer_len` /
    /// `committed_value_len` のみ出力）を継承する（機微情報の露出防止、
    /// `.claude/rules/security.md`）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Composition")
            .field("text_len", &self.text.chars().count())
            .field("cursor", &self.cursor)
            .field("anchor", &self.anchor)
            .finish()
    }
}
```

```rust
/// `TextFieldWidget<S>` へ追加するフィールド（方針。実装は `text_field.rs` を参照）。
///
/// ```ignore
/// pub struct TextFieldWidget<S> {
///     // ...既存フィールド...
///     /// 進行中のコンポジション。`None` は非コンポジション中（Idle）を表す。
///     composition: Option<Composition>,
/// }
/// ```
```

`CharOffset` は S1（`ime-architecture.md` §4.1）が定義済みの `pub type CharOffset = usize`
をそのまま再利用する（本文書では再定義しない）。

### 4.3 状態遷移（ステートマシン）: Idle ⇄ Composing

状態は `composition: Option<Composition>` の有無で表現する（`None` = Idle、
`Some(_)` = Composing）。入力は `ImeEvent`（S1 で確定済み）と既存 `TextFieldInput` /
`InteractionInput` の 3 系統。

**フォーカスガード（本文書が定める前提条件、§4.5 手順 0・§5 不変条件 8 も参照）**:
以下の表の `ImeEvent::Preedit` / `ImeEvent::Commit` 行は、いずれも
**`TextFieldWidget::interaction.focused == true`（フォーカス保持中）の場合にのみ適用する**。
非フォーカス中に届いた `ImeEvent::Preedit` / `ImeEvent::Commit` は破棄し、
`buffer` / `cursor` / `dirty` / `composition` を一切変更しない（Idle のまま何もしない no-op）。
これは §4.6 の `InteractionInput::FocusLost` によるローカルクリアと対になる防御であり、
「フォーカスが既に外れたテキストフィールドへ、blur 後のプラットフォームレースで届いた
遅延 `Commit` が意図せず挿入される」ことを防ぐ（§4.5・§4.6・§9 参照）。**`ImeEvent::Disabled`
のみこのガードの対象外**とする（フォーカスの有無によらず常に `composition = None` の
クリーンアップを実行する、§4.6 の契約を優先する）。

| 現在状態 | 受信イベント | 遷移先 | 状態モデルの動作 |
|---------|-------------|--------|-----------------|
| Idle | `ImeEvent::Preedit { text, cursor }`（`text` が非空、フォーカス中） | Composing | `composition = Some(Composition { text, cursor, anchor: 現在の TextFieldWidget::cursor })` を新設（`anchor` はこの遷移の瞬間にのみ確定し、Composing 中は不変） |
| Composing | `ImeEvent::Preedit { text, cursor }`（`text` が非空、フォーカス中） | Composing | `composition.text` / `composition.cursor` のみ更新（`anchor` は変更しない） |
| Composing | `ImeEvent::Preedit { text, cursor }`（`text` が空、フォーカス中） | Idle | `composition = None`（コンポジションのクリア。空 preedit は「未確定文字列なし」を表す契約、`ime-architecture.md` の winit マッピング注記「winit は `Commit` 直前に必ず空 `Preedit` を送る」と整合） |
| Composing | `ImeEvent::Commit(s)`（フォーカス中） | Idle | §4.5 参照（確定挿入・`composition = None`） |
| Idle または Composing | `ImeEvent::Disabled` | Idle | §4.6 参照（クリーンアップ契約。フォーカスガードの対象外） |
| Idle | `ImeEvent::Commit(s)`（フォーカス中） | Idle | Idle 中の `Commit`（IME によっては `Preedit` を経由せず直接確定を送る実装がありうる）も §4.5 と同じ確定挿入処理を行う。`anchor` 相当の値はこの場合 `TextFieldWidget::cursor`（現在位置）をそのまま使う |
| Idle または Composing | `ImeEvent::Preedit` / `ImeEvent::Commit`（**非フォーカス中**） | 変化なし | フォーカスガードにより破棄する（上記参照、`buffer`/`cursor`/`dirty`/`composition` は不変） |

### 4.4 表示文字列の導出（純関数）

```text
display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]
```

（`buffer[..anchor]` はいずれも char 単位オフセットに基づき、実際の実装は
`byte_index_at` と同じ変換を経て byte スライスへ変換する。panic を API 契約にしない、
`.claude/rules/coding-rust.md` 準拠。）

`composition` が `None`（Idle）の場合は `display = buffer` そのものとなる。下線等の
装飾描画は S5（#186）のスコープであり、本文書は文字列合成のみを確定させる。

### 4.5 確定挿入（`ImeEvent::Commit(s)` の処理）

0. **フォーカスガード**: `TextFieldWidget::interaction.focused == false`（非フォーカス中）
   の場合、以降の手順 1〜6 を一切実行せず、`s` を破棄してこの `ImeEvent::Commit` の処理を
   終える（`buffer` / `cursor` / `dirty` / `composition` は変更しない）。§4.6 の
   `FocusLost` によるローカルクリアで `composition` は既に `None` になっているため、
   このガードは主に「`FocusLost` 後、`ImeEvent::Disabled` 到着前に届く遅延 `Commit`」を
   意図せず `buffer` へ再挿入することを防ぐ（§4.3 フォーカスガード・§9 参照）。
1. `s` に対し既存 `InsertText` と同一の制御文字除去（`char::is_control`、改行含む）を
   適用する（OWASP A03 対策の継承、§7 参照）。
2. **除去後の文字列が空の場合**、既存 `InsertText`（`sanitized.is_empty()` で早期
   リターンし `dirty` を変更しない）と同じ契約に揃え、手順 3〜5（挿入・カーソル前進・
   `dirty = true`）を実行せず、手順 6（`composition = None`）のみを適用してこの
   `ImeEvent::Commit` の処理を終える（空 preedit のキャンセルや制御文字のみの
   commit のような no-op 確定で `dirty` を立ててしまうと、以降の
   `sync_from_state`（§3・§4.9）が `buffer` を外部更新へ追随させなくなり、
   `dirty` 導入の目的だった stale blur-commit 上書き防止が骨抜きになる）。
3. 除去後の文字列を `composition` が `Some` なら `anchor`、`None` なら現在の
   `TextFieldWidget::cursor` の位置へ挿入する（既存 `InsertText` の挿入処理・
   `byte_index_at` を再利用）。
4. `cursor` を挿入した文字列の char 数だけ前進させる。
5. `dirty = true` を設定する（既存 `commit_if_changed` の判定条件に乗せる。新しい
   `Event` バリアントを追加せず、次の `commit_if_changed` 呼び出しで
   `Event::TextFieldChanged` が発火する既存経路をそのまま使う）。
6. `composition = None` にする（コンポジション状態のクリア）。

上記 1〜6 は `ImeEvent::Commit` を受信した時点で**即時に**適用される。これは
`ImeEvent::Disabled` のクリーンアップ契約（§4.6）とは独立した処理である。

### 4.6 `ImeEvent::Disabled` のクリーンアップ契約

`ime-architecture.md` §4.7 が既に定めた契約を、状態モデル側の具体的操作として引き継ぐ
（フォーカス喪失時の確定/破棄の意味論差はプラットフォーム層が吸収し、コアへ漏らさない
という S1 の設計を変更しない）。

- **`ImeEvent::Disabled` を受け取った時点で `composition = None` にする**（preedit 表示状態を
  必ずクリアする。これが状態モデル側の唯一の契約）。
- `Disabled` 到達前に `Commit` が届いていた場合、その文字列は §4.5 の手順で既に `buffer` へ
  適用済みである。**`Disabled` ハンドラは `composition` のクリアのみを行い、`buffer` /
  `cursor` / `dirty` を再適用・巻き戻ししてはならない**（二重適用・意図しない巻き戻しの
  防止）。
- **`InteractionInput::FocusLost` と `ImeEvent::Disabled` は別チャネルの入力である**
  （前者はウィジェットのフォーカス状態、後者はプラットフォーム層からの IME 状態通知）。
  両者の到着順序はプラットフォーム実装に依存するため、状態モデルは次の不変条件を保証する:
  - `apply_input(InteractionInput::FocusLost)` は（既存の `commit_if_changed` 呼び出しに
    加えて）**`composition` をローカルに `None` へクリアする**（コンポジションを確定せず
    破棄する。IME 側が後から `Commit`/`Disabled` を送ってきても、フォーカスが既に外れた
    テキストフィールドへは適用しない設計とする。プラットフォーム層は `FocusLost` の前後で
    `ImeRequest::SetEnabled(false)` を送出済みのため、通常この順序で問題は生じない）。
    この「適用しない」は §4.3 フォーカスガード・§4.5 手順 0 が実装する具体的な拒否条件
    （`interaction.focused == false` の間に届いた `ImeEvent::Preedit` / `ImeEvent::Commit` を
    破棄する）そのものであり、本箇所独自の別ロジックではない。
  - この `FocusLost` によるローカルクリアは、後続で `ImeEvent::Disabled` が届いた場合に
    対しても**冪等**である（`composition` は既に `None` のため、`Disabled` ハンドラの
    `composition = None` は no-op として安全に重ねて適用できる）。
  - `ImeEvent::Disabled` が届かないプラットフォーム（例: Web の `compositionevent` 経路は
    `ImeRequest::SetEnabled` が no-op であり `Disabled` 自体が発火しない、
    `ime-architecture.md` §4.5 参照）でも、`FocusLost` 側のローカルクリアにより
    「blur 後に preedit が表示され続ける」状態を作らない。
- `set_disabled(true)`（既存 `TextFieldWidget::set_disabled`）がフォーカスを奪う経路も
  `FocusLost` 相当のクリーンアップに従う（既存の blur-commit 呼び出し順序を変更しない）。

### 4.7 Composing 中の `TextFieldInput` の扱い

Composing 中（`composition.is_some()`）は `TextFieldInput` の編集入力
（`InsertText` / `DeleteBackward` / `DeleteForward` / カーソル移動系）を **no-op** とする
（IME がキー入力を占有するため、ネイティブ実装でも編集入力自体がコアへ届かないのが通常だが、
プラットフォーム層の実装差異に対する防御として状態モデル側でも明示的に無視する契約とする）。
`Commit`（Enter 相当の `TextFieldInput::Commit`、既存の blur-commit とは別の意味）・
`Cancel` は既存の `buffer` に対する処理（`commit_if_changed` / 確定値への巻き戻し）自体は
そのまま実行する（IME の確定/破棄とは独立したユーザー操作のため）が、**Composing 中に
これらを受信した場合は、既存処理を実行する前に `composition = None` を先に適用する**
（クリア → 既存処理の順序を守る。`anchor` は `composition` と同時に破棄されるため、
`Cancel` が `buffer` を巻き戻した後・`Commit` が `buffer` を書き換えた後の新しい
`buffer` を古い `anchor` が指し続けることがない）。これを怠ると、`Cancel` 後も
preedit オーバーレイが残り Escape でコンポジションを閉じられず、`anchor` が
巻き戻し後の `buffer` の意図しない位置を指したままになる。

### 4.8 選択範囲との相互作用（現時点では対象外）

`text_field.rs` のスコープ外事項に「テキスト選択範囲」が明記されている通り、選択範囲は
本文書のモデルには存在しない。コンポジション開始時の選択範囲置換は本文書の範囲外であり、
選択範囲管理（S4・#185）側の契約に委ねる（§8 スコープ外にも明記）。

### 4.9 `sync_from_state` と `composition` の相互作用

§3 で確認したとおり、既存 `sync_from_state` は `!self.interaction.focused || !self.dirty` の
場合に `buffer` / `cursor` を外部の確定値へ追随させる。`dirty` は「ユーザーが
`TextFieldInput` で編集したか」のみを表し、**コンポジション中かどうかを区別しない**ため、
このままでは Composing 中（`dirty == false` のまま preedit だけが進行している状態は
`ImeEvent::Preedit` 自体は `dirty` を立てないため十分にありうる）に外部 sync が `buffer` を
書き換え、固定された `anchor` が新しい `buffer` の意図しない位置を指す・表示合成
（§4.4）が破壊されるおそれがある。

本文書は次の契約を追加する: **`composition.is_some()`（Composing 中）の間、
`sync_from_state` は `dirty` の値によらず `buffer` / `cursor` を変更しない**
（`committed_value` の更新のみは通常どおり継続する）。実装上は、既存の
`!self.interaction.focused || !self.dirty` という条件に `&& self.composition.is_none()` を
追加する形になる（S3・S8 の実装契約、§6 参照）。sync はステートマシンの入力ではなく
（`ImeEvent` / `TextFieldInput` / `InteractionInput` の 3 系統のみが状態遷移の入力である、
§4.3 冒頭）、Composing を「編集中」と同様に扱うことで `anchor` の有効性を保証する。

## 5. 不変条件（まとめ）

1. コンポジション（`composition` フィールド）は確定バッファ・`Event::TextFieldChanged` に
   混入しない（`ImeEvent::Commit` を経由した場合のみ `buffer` へ反映される）。
2. カーソル・`anchor` は確定バッファ上の **char 単位**位置（byte index 不使用、
   マルチバイト文字列で panic しない）。grapheme cluster 対応は S26（#207）へ委ねる。
3. Composing 中は `TextFieldInput` の編集入力を no-op とする（§4.7）。
4. 表示文字列の導出は純関数 `display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]`
   （下線等の装飾描画は S5 のスコープ）。
5. `ImeEvent::Disabled` 受信時・`InteractionInput::FocusLost` 受信時・Composing 中の
   `TextFieldInput::Cancel` / `Commit` 受信時のいずれも `composition = None` にする
   （最後の 2 者は既存の `buffer` 処理より**先に**クリアする、§4.7）。これらは冪等に
   重ねて適用できる（§4.6）。
6. `ImeEvent::Commit` の確定挿入（§4.5）は `ImeEvent::Disabled` のクリーンアップ（§4.6）と
   独立した処理であり、`Disabled` ハンドラは既に適用済みの `Commit` を再適用・破棄しない。
7. `Composition` の `Debug` 実装は未確定文字列の内容を出力しない（文字数のみ、§4.2）。
8. `TextFieldWidget::interaction.focused == false` の間に届いた `ImeEvent::Preedit` /
   `ImeEvent::Commit` は破棄し、`buffer` / `cursor` / `dirty` / `composition` を変更しない
   （`ImeEvent::Disabled` はこのガードの対象外、§4.3・§4.5 手順 0・§4.6）。
9. `composition.is_some()`（Composing 中）の間、`sync_from_state` は `dirty` の値に
   よらず `buffer` / `cursor` を変更しない（`committed_value` の更新は継続する、§4.9）。

## 6. 実装との対応

- **`text_field.rs`（S3・#184 / S8・#189）**: 本文書 §4.2 の `Composition` 型・
  `TextFieldWidget::composition` フィールド・§4.3〜§4.7 の状態遷移・§4.9 の
  `sync_from_state` 変更（`composition.is_none()` 条件）・§5 不変条件 1〜9 を実装する。
- **S4（#185・カーソル・選択範囲管理）**: §4.8 の「コンポジション開始時に選択範囲を置換」は
  選択範囲管理側の契約に委ねる。
- **S5（#186・コンポジション表示）**: §4.4 の表示文字列導出を実際の描画（下線・背景色等）に
  落とす。
- **S6（#187・変換候補ウィンドウ位置）**: `Composition::anchor`・`cursor` から
  `ImeRequest::SetCursorArea` へ渡す論理ピクセル座標を計算する（`anchor` が確定バッファ内の
  基準点を提供する）。
- **S7（#188・winit `Ime::*` ハンドリング）**: winit の byte offset を `CharOffset` へ
  変換したうえで本文書の `ImeEvent::Preedit` 契約へ渡す（変換自体は S1 §4.6 のプラットフォーム
  層契約）。
- **S8（#189・テキストフィールド統合配線）**: `apply_input` に `ImeEvent` を受け付ける経路を
  追加し、本文書 §4.3〜§4.6 の状態遷移を配線する。§4.3 フォーカスガードにより、配線経路は
  `ImeEvent` 受信時に `interaction.focused` を必ず参照できる構成にする。

## 7. セキュリティ考慮事項（OWASP Top 10 観点）

- **A03 インジェクション**: コンポジション確定文字列（`ImeEvent::Commit(s)`）はプラットフォーム
  IME 由来の外部入力であるため、既存 `InsertText` と同一の制御文字除去（`char::is_control`、
  改行含む）を確定時に必ず適用する（§4.5 手順 1）。生のキーコード・バイト列を扱わず
  意味づけ済み列挙のみで API を構成する方針を維持する（S1 §4.1 を継承）。
- **機微情報の漏えい防止**（`.claude/rules/security.md`）: `Composition` の `Debug` 実装は
  未確定文字列の内容を出力しない（文字数のみ。既存 `TextFieldWidget` の Debug マスク方針を
  継承、§4.2）。パスワード等のセキュア入力フィールドでは `ImePurpose::Password`（S1 §4.1・
  S23（#204）で公開 API 化）により IME の学習・予測変換抑止をプラットフォームへ要求できる
  契約とする（本文書ではコンポジション自体の無効化までは規定せず、将来方針として記載する）。
- **A05 セキュリティ設定ミス**: 該当なし（コード・CI 設定変更なし）。
- **A06 脆弱な依存**: 依存クレート追加なし（winit は既存 workspace 依存の参照のみ）。
- **メモリ安全性**: 設計上 `unsafe` 不要（`#![forbid(unsafe_code)]` の `fandhe-core` 内で完結）。
  プラットフォーム FFI は S9 以降のスコープ。
- **panic 安全性**: char 単位インデックス（`anchor` / `cursor`）の徹底により、マルチバイト
  文字列（日本語・絵文字）で byte 境界 panic を起こさない（既存 `byte_index_at` の
  クランプ方針を継承）。サロゲートペア・grapheme の完全対応は S26（#207）。

## 8. スコープ外（本文書にも明記し、混入させない）

- 状態モデルの Rust 実装そのもの（S3 #184）。
- 選択範囲管理（S4 #185）・コンポジション開始時の選択範囲置換の確定（§4.8 参照）。
- コンポジション描画（S5 #186）・候補ウィンドウ位置計算（S6 #187）。
- winit 配線（S7 #188）・各プラットフォーム連携/検証（S9〜S22）。
- プラットフォーム別マッピング表の再定義（S1・`ime-architecture.md` で確定済み、本文書は
  参照のみ）。
- `TextInput<S>` と `TextFieldWidget<S>` の型統合（TASK-8.3 由来の既知スコープ外事項。
  本 issue で扱わない）。
- grapheme cluster 単位のカーソル処理（S26 #207）。

## 9. 未解決事項

- **選択範囲導入後のコンポジション開始時の置換動作**（§4.8）は S4（#185）で確定させる。
- **Android `finishComposingText()` の意味論**（`Commit` 相当か `Disabled` 相当か）は
  `ime-android-input-connection.md`（S18・#199）が定める。状態モデルは §4.5・§4.6 の
  いずれの経路でも処理できる。
- **フォーカスガード（§4.3・§4.5 手順 0）による正当な `Commit` の取りこぼし**: 非フォーカス中に
  届いた `ImeEvent::Commit` を一律破棄するため、プラットフォームの実装次第では
  `FocusLost` 直後に届く「正当に確定済みだが到着が遅れただけ」の `Commit` も失われる
  可能性がある（意図しない再挿入を防ぐため、§4.6 が `FocusLost` を破棄的に扱う設計を
  選択していることと整合させた判断）。Android `finishComposingText()`
  （直前の箇条書き参照）を含め、各プラットフォーム連携設計（S9〜S22）でこの取りこぼしが
  実用上問題になるかを確認する。

## 10. 出典

- `docs/architecture/ime-architecture.md`（S1・#182。`ImeEvent`/`ImeRequest`/`CharOffset` の
  定義・責務分界・プラットフォーム別マッピング表・文字インデックス正規化方針・
  §4.7 フォーカス遷移との結合の出典）
- `crates/fandhe-core/src/ui/text_field.rs`（`TextFieldWidget<S>` の既存編集モデル・
  `byte_index_at`・`commit_if_changed`・`sync_from_state`・`Debug` マスク方針）
- `crates/fandhe-core/src/ui/widget.rs`（`InteractionState`/`InteractionInput`）
- `crates/fandhe-core/src/state/store.rs`（`Event::TextFieldChanged`）
- `docs/architecture/widget-architecture.md`（`Widget<S>` トレイト契約・二層モデル）
