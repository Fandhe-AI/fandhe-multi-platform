# Changelog — fandhe-core

書式は [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) に、バージョニングは
[Semantic Versioning](https://semver.org/lang/ja/)（0.x 系: minor で破壊的変更を許容）に従う。

## [Unreleased]

`.label()`・`.list_view()`・`.scroll_view()` を削除し、`.dialog()` と `.image()` / `.image_bound()` を描かれる型へ変える（Issue #1427）。

どれも egui で何も描かれず、同じ役割の描かれる書き方が既にあった。「置いても何も出ない」書き方を残さず、
役割ごとに書き方を 1 つへ寄せる。

### 破壊的変更（BREAKING CHANGE）

- `Screen::label` / `Screen::list_view` / `Screen::scroll_view` を削除
- `Screen::dialog` の引数を `(id, title, body, open)` から `DialogWidget<S>` 1 つへ変更（描かれない `Dialog<S>` ではなく
  描かれる `DialogWidget<S>` を置く。`Screen::stack` と同じ形）
- `Screen::image` / `Screen::image_bound` が作る型を `Image<S>` から `ImageWidget<S>` へ変更（呼び出しの書き方は同じ。
  `Node::Widget` の中身を `Image<S>` へ downcast していた場合は `ImageWidget<S>` へ）。`Image<S>` の型自体は残る

### 追加

- `Label<S>` が `Widget::as_any` を実装。`fandhe-render-egui` が `Label` を文字として描く。
  `Stack` / `Grid` / `Card` / `Tooltip` の子が `Label` のときも描かれる（#1430 の「子 `Label` は描かれない」を解消）
- `ImageWidget<S>` が `Widget::accessibility_label`（alt）を実装
- `.image()` は、識別子と一致する画素が `fandhe_runtime::image_store::put` で置かれていないとき（ソースが空でない場合）、
  alt を文字で描く（AX ノードは作らず、読み上げは合成の Image ノード 1 つだけ）。識別子はパスや URL を
  読み込むものではなく、置けるのは同時に 1 枚
- `screens::containers`（gallery の `containers`）に子 `Label`（固定・状態束縛）を足した

### 挙動の変更

- egui: 子 `Label` を描く。`Label` の AX は、合成ノードから egui の label ノードに変わった
  （macOS・Android・Windows。ノードは 1 つのまま）。`ImageWidget` の AX は alt をラベルとして持つ。
  ただしソースが空のとき（写真選択・書類スキャン・scanner_devices_import の選ぶ前の表示など、直接置いた
  `ImageWidget` も含む）は従来どおり名前を出さない。直接置いた `ImageWidget` も、ソースが空でなく画素が無いときは
  alt が名前になる（以前は名前なし）
- egui: `LabelWidget`・`Image`・`ListView`・`ScrollView`・`Dialog` の型を `.with_node()` で直接置いても描かれない
  （ビルダーを無くしたので、通常の画面の組み方では置かない）
- web-dom: `.image()` / `.image_bound()` のソースが `blob:` のとき `img` になる（それ以前は `Image<S>` だったため
  常に空プレースホルダ）。それ以外は、ソースが空でなければ `role="img"` + alt の `aria-label` の `div`、
  空なら属性なしの空 `div`（直接置いた `ImageWidget` も同じ。以前は `blob:` 以外は属性なしの `div`）。`Label` は web-dom で出ない
  （従来どおり。固定文字の `Label` は `aria-label` だけが出る。コンテナの子も同じ）
- swiftui-bridge: 変化なし（`Node::Widget` は従来どおり型を問わず共通の変換）

### 移行手順

1. `.label(id, get)` を `.dynamic_text(get)` へ（コンテナの子にするなら `Label::bound(id, get)` / `Label::fixed(id, text)`）
2. `.list_view(id, len, item)` を `.list()`（`expand` が `Node::Text` 等を返す）、大量なら `.virtual_list()` へ
3. `.scroll_view(id, content)` を削除し、`content` を直接置く（`render_screen` のルートが縦スクロールする）
4. `.dialog(id, title, body, open)` を `.dialog(DialogWidget::new_dynamic(..).with_content(..))` へ
   （`.dialog(..)` の引数が `DialogWidget` に変わった。`screens::confirm_dialog` が参照実装）
5. `.image(id, "assets/x.png", alt)` でパスの画像が出ると期待していた場合は、`fandhe_runtime::image_store::put` で
   同じ識別子の画素を置くか、画素を持っているなら `.rgba_image()` を使う

`Stack`・`Grid`・`Card`・`Tooltip` の子を egui で描画する（Issue #1430）。

### 追加

- `Stack` / `Grid` / `Card` / `Tooltip` が `Widget::children`（子を `Node::Widget` に包んで宣言順に返す。
  `Card` は無効化中は空）と `Widget::as_any` を実装。子の操作が `WidgetValueChanged` の path で状態へ届く
- `Stack::direction` / `spacing`、`Grid::columns` / `spacing`、各型の `child_nodes`（描画層が読む）
- `Tooltip` の `accessibility_label` が本文を返す。AX ツリーに出るのは macOS・Android・Windows のみ（読み上げは
  実機で未確認）。iOS・Web・Linux のタッチには本文へ届く経路が無い（既知の天井）
- 確認用画面 `screens::containers`（gallery の `containers`）

### 挙動の変更

- これらのコンテナの子が `Node::expand_children` / `resolve_node_at` / `Screen::visit_*` から辿れる
  （それ以前は子なしとして扱われた）
- web-dom: 4 型の子が実 DOM に出る（操作できる）。`Tooltip` は `role="group" aria-label=本文` になる。
  無効化した `Card` は子が消える（`DialogWidget` と同じ割り切り）。`Stack` の方向・`Grid` の列は反映しない
- swiftui-bridge: スナップショットが空 list の入れ子になる（スキーマ版は変わらない）

`menu`・`tabs` を egui で描画し、選択を状態へ返せるようにする（Issue #1429）。

### 破壊的変更（BREAKING CHANGE）

- `Screen::menu` / `Menu::bound` に書き戻し先 `set_open: WriteFn<S, bool>` と
  `set_selected: WriteFn<S, usize>` を追加（引数の並びは `open, set_open, selected, set_selected`）
- `Screen::tabs` / `Tabs::bound` に書き戻し先 `set_selected: WriteFn<S, usize>` を追加

### 追加

- `Menu` / `Tabs` が `Widget::apply_value` を実装。`WidgetValue::Bool` は開閉、`WidgetValue::Index` は
  選択を書く（範囲外の番号は無視。`Menu` は選択に続けて `set_open(false)` も呼ぶ）
- `Menu` / `Tabs` に `Widget::as_any` を実装（egui 描画の downcast 用）
- 確認用画面 `screens::menu_tabs`（gallery の `menu-tabs`）

### 移行手順

1. `.menu(id, label, items, open, selected)` を `.menu(id, label, items, open, set_open, selected, set_selected)` へ
2. `.tabs(id, labels, selected)` を `.tabs(id, labels, selected, set_selected)` へ

`ScreenStack` の構築関数が状態を読み取り専用で受け取れるようにする（Issue #1289）。

### 破壊的変更（BREAKING CHANGE）

- `ScreenStack::new` / `push` が受け取る型を `fn() -> Screen<S>` から `fn(&S) -> Screen<S>` に変更
- `ScreenStack::current` を `current(&self, state: &S)` に変更
- `screens::list_detail::{list_screen, detail_screen}` が `&ListDetailState` を受け取る。詳細画面の
  トグルボタンの文言を `LABEL_ADD_FAVORITE` / `LABEL_REMOVE_FAVORITE` に変更（状態で切り替わる）

### 移行手順

1. 画面を作る関数の引数に `_state: &S` を足す（状態を読まない画面は未使用のままでよい）
2. `stack.current()` を `stack.current(state)` に置き換える（`state.nav.current(state)`）
3. 状態で文言が変わるボタンへ `ButtonClicked` を送る側は、dispatch のたびに画面を組み直し、
   その時点の文言を使う

一覧の行内に複数の操作を横に並べられるようにする（Issue #1288）。

### 追加

- `Node::Row { children, tap }`: 子を左から右へ並べる行。`tap` が指すボタンの当たり判定を行全体へ
  広げる（egui。ボタン以外の所＝文字・余白のタップが入口を押す）。`Node::row_tap_target()` が指す子の位置とラベルを返す
- `Node::CellButton { label, row, col, on_click, id }`: 行と列を値で埋め込んだボタン。
  `on_click` は `WriteFn<S, (usize, usize)>`（関数ポインタ限定。REQ-13）
- `click_button` / `click_button_at` / `activate_focus_at` が `CellButton` を発火する。
  `click_button_at` の path 解決が `Row` の子へ降りる。`FocusRing` は `Row` を横向きの入れ物として扱う
- `fandhe-render-egui`: `Node::List` の `axis`（`Vertical` / `Horizontal` / `Grid`）を描画へ配線。横並び（`Row`・`Horizontal`）は幅を超えると折り返す
- `fandhe-web-dom`: `Row` は `div.fandhe-row`（flex）。`ul` には軸が縦以外のときだけ style を付ける

### 破壊的変更（BREAKING CHANGE）

- 公開 enum `Node` に `Row` と `CellButton` バリアントを追加。`Node` を網羅 match している
  呼び出し側はコンパイルが壊れる
- `Node::List` に `Horizontal` / `Grid` を指定した画面は、egui での並びが縦から変わる
  （これまで描画は軸を無視していた）。縦の `List` も `ui.vertical` で包むため egui の自動 id が変わる

### 移行手順

1. `Node` の網羅 match に `Node::Row { .. }` / `Node::CellButton { .. }` 分岐を追加するか、`_` で受ける
2. 意図せず軸を指定していた `List` は `Axis::Vertical`（既定）へ戻す

実画素の画像ノード `Node::RgbaImage` を追加し、カメラ撮影のサムネイルを実画素にする（Issue #5）。

### 追加

- `ui::RgbaPixels` / `RGBA_MAX_SIDE`: 検証済み RGBA8 画素列（`new` が長さ不一致・寸法 0・
  一辺 2048 超を `None` にする。`Debug` は画素を出さない）
- `Node::RgbaImage { pixels, alt }` と `Screen::rgba_image`。`render_headless` は
  `[画像 W×H: alt]` を出す
- `screens::camera_capture::{THUMB_MAX_SIDE, thumbnail_size}`

### 破壊的変更（BREAKING CHANGE）

- 公開 enum `Node` に `RgbaImage` バリアントを追加。`Node` を網羅 match している呼び出し側は
  コンパイルが壊れる
- `screens::camera_capture::Thumbnail` の `rows` を `pixels: RgbaPixels` に置き換え、
  `THUMB_COLS` / `THUMB_ROWS` を削除（輝度モザイクの廃止）

### 移行手順

1. `Node` の網羅 match に `Node::RgbaImage { .. }` 分岐を追加するか、`_` で受ける
2. `Thumbnail::rows` / `THUMB_COLS` / `THUMB_ROWS` の参照を `Thumbnail::pixels` に置き換える

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
