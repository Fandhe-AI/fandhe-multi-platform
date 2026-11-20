# Changelog — fandhe-core

書式は [Keep a Changelog](https://keepachangelog.com/ja/1.1.0/) に、バージョニングは
[Semantic Versioning](https://semver.org/lang/ja/)（0.x 系: minor で破壊的変更を許容）に従う。

## [Unreleased]

一覧の行ごとに別の画像を出せるようにし、一定時間後に状態を変える書き方を足す（Issue #1291）。

画素の保管所（`fandhe_runtime::image_store`）が 1 枠だったため、一覧の行ごとに別の画像を出せなかった。
また effect は毎フレーム呼ばれる状態なしの関数で、時間で戻る表示はスレッドと sleep を利用者が毎回書く必要があった。

### 破壊的変更（BREAKING CHANGE）

- `fandhe_runtime::image_store::put` は、同じ識別子だけを置き換え、別の識別子の画像を残す（旧版は 1 枠で、後から置いた画像が先の画像を消していた）。
  `clear()` は従来どおり全部を捨てる。写真選択・書類スキャン・スキャナーが世代付きの識別子で前の画像を解放していたのは
  `remove_prefix`（`fandhe.photo:`・`fandhe.scan:`・`fandhe.scanner:`）へ変えた
- 機能 crate のドライバが `image_store` に置く識別子を `photo:<世代>`・`scan:<世代>`・`scanner:<世代>` から
  `fandhe.photo:<世代>`・`fandhe.scan:<世代>`・`fandhe.scanner:<世代>` へ変更（`fandhe.` で始まる識別子とタイマーのキーはフレームワークの予約。
  利用者の識別子との衝突を避ける）。状態が持つ識別子はドライバが作る値なので、利用者側の変更は不要
- `image_store::set_on_clear(fn())` を `set_on_remove(fn(&str))` へ変更（捨てた識別子ごとに呼ぶ。レンダラが GPU テクスチャを同時に捨てる用）

### 追加

- `image_store::begin_frame()`（`ProxyApp` がフレーム冒頭で呼ぶ）: このフレームですでに描かれた画像は、フレーム末尾の effect などの
  `put` で追い出さない（追い出す順は、描かれていない画像、足りなければ前のフレームだけで描かれた画像。新しく置いた画像は後者に入れ、
  ページ送りで置いたばかりの画像どうしが追い出し合わない）。守らない範囲: `begin_frame` の後、行がまだ描かれていない間に届く
  復号スレッドの `put` では、見えている行が一時的に alt になりうる。今のフレームで描かれた
  画像だけで予算いっぱいのときは、何も追い出さずに今置く画像のほうを捨てる。呼ばなければ純粋な LRU
- `image_store::put` が `PutResult`（`Stored` / `RejectedOverBudget`。`#[must_use]`）を返す。`RejectedOverBudget` は画像が保管所に入って
  いない（`contains` が偽）ことを表すので、effect は失敗として扱い読み直さない（`image_store` の doc の例）。写真選択・書類スキャン・
  スキャナーのドライバは、置けなかったとき成功にせず失敗として確定する
- `image_store`: 識別子ごとに複数枚を保持。`remove`・`remove_prefix`・`contains`（順位を上げない確認）・`BUDGET_BYTES`（合計 64 MiB。
  超えたら最後に描かれたのが古い画像から捨てる。`begin_frame` が呼ばれていなければ今置いた 1 枚は残すため、実際の上限は 64 MiB + 直前の 1 枚。
  呼ばれていれば、今置く画像が置けないことがあり（`put` の直後でも `contains` が偽）、上限は描かれた画像の合計と予算の大きいほう）。`.list()` は全行を描くので、
  全行の合計を収めるか `.virtual_list()` を使う
- `CompletionSender::after(key, delay, impl FnOnce(&mut S) + Send + 'static) -> Result<Armed, TimerRefused>`・`cancel_timer(key)`。
  `Ok(Armed::New)` は積んだ、`Ok(Armed::AlreadyPending)` は同じキーが残っていた（正常）、`Err(Full | TooFar | Closed)` は積まれていない
  （同時の予約が 1024 件に達した・期限が時計の範囲を超えた・アプリが破棄済み）。戻り値は `#[must_use]` で、`Err` を捨てると警告になる。
  同じキーの予約が残る間の呼び出しは何もしないため、effect で毎フレーム「状態がこうなら予約する」と書ける。
  期限は単調時計（`web-time`）で測り、`ProxyApp` がフレーム先頭で適用する。表の持ち主は `ProxyApp` だけで、sender は弱く参照する
  （予約のクロージャが sender の複製を抱えても循環しない。アプリの破棄で予約・捕まえた資源が解放される）
- `CompletionSender::with_timers(&Arc<Timers>)`、`fandhe_runtime::timer::{Timers, drain_timers}`: 予約表を描画ループの持ち主と共有して流すための口
  （`ProxyApp` は配線済みで、利用者は通常使わない）
- web-dom は `image_store` に画素がある `ImageWidget` を `img`（PNG の data URL）で出す。`(識別子, 通し番号)` で URL を覚え
  （合計バイト数に上限。足すたびに保管所に無い識別子を捨て、超えたら古いものから 1 件ずつ捨てる）、
  画素数が 512 x 512 を超える画像は data URL にせず alt の `div` にする
- gallery の `row-images` 画面
- 画像形式の対応範囲と形式を足す手順（`docs/ai/SCHEMA.md` パターン 7-4）、時間で状態を変える書き方（同 9-2）

### 移行手順

1. 1 枚だけ置いて「前の画像は置き換えで消える」ことに頼っていた effect は、不要になった画像を `remove` / `remove_prefix` で捨てる
   （捨てなくても合計 64 MiB を超えれば古い順に捨てられる）
2. `set_on_clear` を使っていたレンダラ実装は `set_on_remove` へ（引数が識別子になる）
3. `CompletionSender::new` は従来のまま使えるが、`with_timers` で表につないでいない sender の `after` は `Err(Closed)` で断る。
   予約を流すのは `with_timers(&Arc<Timers>)` で渡した表を `drain_timers` するループの持ち主だけ（`ProxyApp` は配線済み）
4. `image_store::put` の戻り値（`PutResult`、`#[must_use]`）を扱う。`let _ =` で捨てると、置けなかった識別子を毎フレーム復号し直す
   ことになりうる（`image_store` の doc の例のように、置けなかったら失敗として読み直さない）

`.icon()` / `.icon_bound()` を egui・web-dom で描かれるようにし、名前を文字列から列挙型 `IconName` へ変える（Issue #1437）。

名前の語彙が決まっておらず、図柄を引く仕組みも無かったため egui で何も描かれなかった。閉じた語彙にして、
未知の名前を型で作れなくした。図柄は egui の既定フォントの Unicode 記号に写す（新しい依存・資産なし）。

### 破壊的変更（BREAKING CHANGE）

- `Screen::icon` の `name` を `impl Into<String>` から `IconName` へ、`Screen::icon_bound` の `get` を `ReadFn<S, String>` から
  `ReadFn<S, IconName>` へ変更
- `Icon::fixed` / `Icon::bound` / `IconSource` も同様。`Icon::name_text() -> &str` を `Icon::name() -> Option<IconName>` へ置換
  （`bound` の初回 sync 前は `None`）

### 追加

- `IconName`（`fandhe_core::IconName`。`ALL`・`glyph()`。`#[non_exhaustive]`）
- `Icon` が `Widget::accessibility_label`（ラベル。空なら `None`）と `Widget::as_any` を実装
- `fandhe-render-egui` が `Icon` を記号で描く。記号専用の font family `fandhe-icon`（emoji-icon-font・Noto Emoji。CJK を入れない。
  `font::apply_fonts` が CJK の有無にかかわらず登録（Web でフォント取得に失敗しても登録）し、未登録の context では `Proportional`）で描くので、CJK が一部の字を取らない
- web-dom は `span.fandhe-icon`（本文は記号 + U+FE0E、`font-variant-emoji:text`。ラベルあり: `role="img"` + `aria-label`、空: `aria-hidden="true"`）。
  Emoji_Presentation=Yes の字はブラウザによりカラーのまま残りうる
- gallery の `icons` 画面

### 挙動の変更

- egui: ラベルが空の `Icon` は AX ノードを作らない（装飾用）。それ以外は role Image・名前はラベル
- swiftui-bridge: 変化なし（`Node::Widget` は従来どおり共通の変換）
- `IconWidget` / `IconWidgetSource` は文字列のままで、`.with_node()` で置いても描かれない（`Icon` を使う）

### 移行手順

1. `.icon(id, "settings", label)` を `.icon(id, IconName::Settings, label)` へ。名前の対応は `IconName` の各変種（`Settings`・`Search`・`Home` 等）。
   語彙に無い名前を使っていた場合は、近い変種へ置き換えるか issue で語彙の追加を依頼する
2. `.icon_bound(id, get, label)` の `get` を `fn(&S) -> IconName` に変える（状態に名前の文字列を持っていたなら `IconName` を持つ）
3. `icon.name_text()` を `icon.name()` へ（`Option<IconName>`。文字は `name.glyph()`）
4. 装飾用のアイコン（読み上げ不要）は第 3 引数を `""` にする

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
