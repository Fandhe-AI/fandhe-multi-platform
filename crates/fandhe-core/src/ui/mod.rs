//! 宣言的 UI 記述の入口モジュール。
//!
//! `docs/spec/04-requirements.md` のデータ要件（`Screen<S>`/`Node<S>` から成る
//! 型付きツリー構造、状態アクセスは関数ポインタ限定）を実装したもの。
//! `docs/spec/03-poc/ai-optimization/dsl`（PoC-6）の型設計を土台とする。
//!
//! - [`node::Node`][]: 画面を構成する要素（`Text`/`TextField`/`Button`/`List`/
//!   `VirtualList`）の型定義（`VirtualList` は TASK-5.5-S2 / REQ-5、Issue #85）
//! - [`screen::Screen`][]: 1 画面の宣言的記述とツリー構築・走査 API（`.text()` /
//!   `.text_field()` / `.button()` / `.list()` / `.virtual_list()` ビルダーを含む、
//!   TASK-1.1-S2 / #18。`.virtual_list()` / `visit_windowed` は TASK-5.5-S2）
//! - [`render::render_headless`][]: ノード列をテキスト表現へ変換するヘッドレス描画
//!   マッピング（TASK-1.1-S2 / #18）
//! - [`event::click_button`][]: ボタン押下ヘルパー（TASK-1.1-S3 由来。本モジュールへ統合）
//! - [`sample::sample_screen`][]: ボタン・リスト・テキストを含む代表 1 画面サンプル
//!   （TASK-1.1-S6。`Screen`/`Node` DSL の使用例）
//! - [`screen_stack::ScreenStack`][] / [`screen::Screen::on_back`][] /
//!   [`event::request_back`][]: 複数 `Screen<S>` を重ねて戻る画面遷移スタック
//!   （Issue #726）。状態 `S` は単一共有のまま、画面ごとに `fn() -> Screen<S>` を
//!   `push`/`pop` する。OS の戻る操作は `.on_back()` で登録したハンドラを
//!   `state::Event::BackRequested` 経由で発火する
//! - [`render_mode::RenderMode`][] / [`render_mode::ModeSwitcher`][]: 画面単位の DOM/canvas
//!   出力方式選択導線（REQ-4 / TASK-4.4-S3、Issue #81）
//! - [`virtual_list::compute_visible_range`][] / [`virtual_list::VisibleRange`][]:
//!   可視範囲算出（TASK-5.5-S2 / REQ-5、Issue #85）。設計文書
//!   `docs/architecture/list-virtualization-architecture.md` §3.2〜3.3 の契約を実装し、
//!   `Node::VirtualList` / `Screen::visit_windowed` の性能経路の基盤となる。
//!   `fandhe-egui-proxy::render_screen` への実配線は `render_virtual_list`（Issue #487）
//!   が担う
//! - [`virtual_list::grid_item_range`][]: Grid 軸専用の可視行範囲 → 項目 index 範囲
//!   変換（Issue #557 で TV 側・非 TV 側の実装を集約）
//! - [`recycle::ListRecycler`][]: リスト項目再利用機構（TASK-5.5-S3 / REQ-5、Issue #86）。
//!   `Range<usize>` を引数で受け取る自己完結 API として実装している。`render_screen` への
//!   `ScrollArea::show_viewport` 配線（Issue #487）・フレーム間でノードを再利用する
//!   キャッシュの呼び出し配線（Issue #490）を経て、`render_screen<S>` が `S: 'static` を
//!   要求しないため egui 側の永続領域（`'static` 前提）へ `ListRecycler<S>` を直接
//!   保持できない制約（`render_virtual_list` の doc 参照）は、呼び出し側
//!   （`fandhe-egui-proxy` の `VirtualListRecyclers<S>`）がフレームをまたいで保持する
//!   設計で解消している（`crates/fandhe-egui-proxy/src/recyclers.rs` 参照）
//! - [`scroll::total_content_extent`][] / [`scroll::clamp_scroll_offset`][]: リストの
//!   スクロール幾何ヘルパー（TASK-5.5-S5 / REQ-5、Issue #88）。`fandhe-egui-proxy::
//!   render_screen` への実配線（Issue #487）は `render_virtual_list` が
//!   `ScrollArea::show_viewport` の viewport から両関数を呼ぶ形で担う
//! - [`widget::Widget`] / [`widget::InteractionState`] / [`widget::StateStyles`] 等:
//!   ウィジェット共通トレイト・インタラクション状態（hover/pressed/disabled/focus）・
//!   共通スタイリング基盤（TASK-8.3-S2 / REQ-8 / REQ-9、Issue #97）。
//!   `Node::Widget(Box<dyn widget::Widget<S>>)`（node.rs 参照）が保持する実体が
//!   実装する契約であり、`docs/architecture/widget-architecture.md`（TASK-8.3-S1、#96）
//!   の §4.3 シグネチャ契約を Rust コードとして定義する
//! - [`layout::LayoutTree`][]: レイアウトプロトコル（サイズ計測・配置）基盤
//!   （TASK-8.3-S4 / REQ-8、Issue #99）。taffy を内部実装として使うが公開 API には
//!   露出しない（モジュール doc 参照）
//! - [`catalog::WidgetCatalog`][]（`gallery_screen` / `gallery_state` メソッドを含む）:
//!   ウィジェットカタログ（サンプルギャラリー）土台（TASK-8.3-S6 / REQ-8、Issue #101）。
//!   各ウィジェット実装がエントリを登録する受け皿（登録・列挙 API）と、ヘッドレスな
//!   サンプルギャラリー画面を提供する。既存の `Screen<S>` / `Node<S>` /
//!   `render_headless` のみに依存する自己完結モジュールとして実装している
//!   （`Widget<S>` トレイトとは独立、`recycle` / `scroll` / `hit_test` と同じ設計）
//!
//! 状態管理・イベント配線全体（Issue #21）は引き続き未対応であり、`sample` モジュールは
//! 現行 API（`.text()` / `.text_field()` / `.button()` / `.list()` ビルダー +
//! `click_button`）のみで組み立てている。
//!
//! - [`geometry::Point`] / [`geometry::Size`] / [`geometry::Rect`]: ヒットテスト・
//!   ポインタディスパッチが共有する最小ジオメトリ型（TASK-8.3-S3 / REQ-8、Issue #98）。
//!   既存 main の API のみに依存する自己完結の型として定義している
//!   （`recycle` / `scroll` と同じ判断）
//! - [`hit_test::HitRegion`] / [`hit_test::hit_test`]: レイアウト確定後の矩形ツリーと
//!   座標から対象ウィジェットキーを解決する汎用ヒットテスト API（TASK-8.3-S3 / Issue #98）。
//!   矩形の算出方法（レイアウトプロトコル）自体には踏み込まない。
//! - [`label::LabelWidget`][]: ラベル・テキストウィジェットの状態・イベント処理
//!   （TASK-8.3-S11 / REQ-8、Issue #106）。`catalog` / `hit_test` / `harness` と同じ
//!   判断で `Widget<S>` を実装する自己完結の最小型として実装している（`label::Label`
//!   との型統合は行わない、後述）。
//! - [`text_input::TextInput`]: テキストフィールドウィジェット（TASK-8.3-S13 / REQ-8、
//!   Issue #108）。`Widget<S>` を実装する具体型として、単一行モノスペース近似の
//!   `measure` / `paint` を提供する。レガシー `Node::TextField`（`.text_field()`）は
//!   設計文書 §4.1 の温存方針により変更しない（`Screen::text_input` 経由で
//!   `Node::Widget` へ統合する新規ウィジェット）。入力イベント・`set` の実配線は
//!   `text_field::TextFieldWidget`（Issue #109）、`WidgetCatalog` 登録は Issue #110。
//! - [`text_field::TextFieldWidget`][] / [`text_field::TextFieldInput`][]: テキスト
//!   フィールドウィジェットの状態・イベント処理（TASK-8.3-S14 / REQ-8、Issue #109）。
//!   `label` と同じ判断で `Widget<S>` を実装する自己完結の最小型として実装している
//!   （`text_input::TextInput` とは名前・実装とも独立しており、両者の型統合は行わない）。
//!   編集バッファ・カーソルは char 単位で扱い、確定（`Commit`）は
//!   `state::Event::TextFieldChanged` へブリッジするのみで widget 層から `&mut S` へは
//!   触れない。
//! - [`pointer::PointerEvent`] / [`pointer::PointerDispatcher`]: press/release/capture の
//!   ポインタイベントディスパッチ（TASK-8.3-S3 / Issue #98）。ヒットテストで解決した
//!   ウィジェットキーをもとにクリック確定を判定し、`state::Event` へブリッジする
//!   （状態 `S` の実書き換えは既存の `StateStore::dispatch` に委譲し、走査ロジックを
//!   二重管理しない）。
//! - [`label::Label`] / [`label::LabelSource`]: `Widget<S>` トレイトを実装する
//!   最初期の具体ウィジェット型（TASK-8.3-S10 / REQ-8 / REQ-13、Issue #105）。
//!   既存の `Node::Text`（状態非依存の静的文字列）とは異なり、状態 `S` に束縛された
//!   テキスト表示（`LabelSource::Bound`、関数ポインタ限定）を `Node::Widget` 経由で
//!   提供する。`widget::Widget::sync_from_state` の未配線契約（`render_headless` 等の
//!   バックエンドは `&mut` 経路を持たず `paint()` へ委譲しない）は本モジュールでも
//!   維持する。インタラクション状態の実配線は `label::LabelWidget`（Issue #106）、
//!   カタログ登録・テスト拡充は Issue #107。
//! - [`harness::WidgetHarness`] / [`harness::LifecyclePhase`]: ウィジェットテスト
//!   ハーネス（TASK-8.3-S5 / REQ-8、Issue #100）。設計文書 §4.4 の 7 状態ライフサイクル
//!   （宣言 → mount → update → layout → paint → event → unmount）と `ListRecycler` 同様の
//!   再利用条件（状態バージョン不変ならスナップショットを再構築しない）をテストケース化
//!   するための駆動・観測 API を、各ウィジェット実装工程が共通利用できる形で提供する。
//!   `hit_test` / `pointer` と同じ判断で、既存 main の公開 API のみに依存する
//!   自己完結の実装としている。
//! - [`button::Button`]: ボタンウィジェット（TASK-8.3-S7 / REQ-8 / REQ-13、Issue #102）。
//!   `AccessibilityRole::Button` は定義済みのため `widget.rs` への変更は不要。
//!   `[Rect（背景）, Text(label)]` を描画し、既存の宣言層 `Node::Button`（TASK-1.1 由来）
//!   とは別物のウィジェット実装層の型。押下イベントの実配線（`ButtonInput`、
//!   `apply_input` / `handle_input`）は `TASK-8.3-S8 / Issue #103` が担う。押下確定は
//!   既存の `state::Event::ButtonClicked` へブリッジするのみで widget 層は `&mut S` に
//!   触れない（`toggle_switch::ToggleSwitch` と同じブリッジ設計）。`WidgetCatalog`
//!   登録・テスト拡充は Issue #104。
//! - [`list_view::ListView`] / [`list_view::ListItemSource`]: リストウィジェット
//!   （TASK-8.3-S16 / REQ-8 / REQ-13、Issue #111）。`label::Label`（S10）の前例を
//!   踏襲した `Widget<S>` 実装で、`Node::List` / `Node::VirtualList`（DSL 側の子ノード
//!   展開機構）とは別物のウィジェット実装層の型。テスト拡充・カタログ登録は Issue #113。
//! - [`list_view::ListViewInput`]: リストの状態・イベント処理（TASK-8.3-S17 / REQ-8、
//!   Issue #112）。`toggle_switch::ToggleSwitch`（S26 / #121）と同じ判断で別型を新設せず
//!   既存 `ListView<S>` へ直接メソッド（`item_at` / `press` / `release` / `apply_input` /
//!   `handle_input` / `set_disabled` / `visual_state`）を追加している。行確定は既存の
//!   `state::Event::ButtonClicked` へブリッジするのみで widget 層から `&mut S` へ
//!   直接触れない。`hit_test` は矩形内判定で実装している（`list_view` モジュール doc
//!   参照）。
//! - [`checkbox::Checkbox`]: チェックボックスウィジェット（TASK-8.3-S19 / REQ-8 /
//!   REQ-13、Issue #114）。`Label`（S10）・`TextInput`（S13）が確立したパターンを
//!   踏襲した `Widget<S>` 実装で、`[Rect, (checked 時のみ) Text(チェックマーク),
//!   Text(label)]` を描画する。状態・イベント処理は Issue #115、テスト拡充・
//!   カタログ登録は Issue #116。
//! - [`checkbox::CheckboxWidget`][] / [`checkbox::CheckboxInput`][]: チェックボックス
//!   ウィジェットの状態・イベント処理（TASK-8.3-S20 / REQ-8、Issue #115）。
//!   `text_field` と同じ判断で `Widget<S>` を実装する自己完結の最小型として実装している。
//!   チェック状態は制御型（`ReadFn<S, bool>` 経由の `sync_from_state` のみが表示値を
//!   更新し、編集バッファ・dirty 管理は持たない）で、トグル確定は既存の
//!   `state::Event::ButtonClicked` へブリッジする（専用イベントバリアントの新設は
//!   スコープ外、モジュール doc 参照）。`checkbox::Checkbox` と `checkbox::CheckboxWidget`
//!   は名前が異なり実装も独立しているため、統合は行わず両立させている
//!   （`label::Label` / `label::LabelWidget` と同じ判断、`AccessibilityRole::CheckBox`
//!   は S19 側の表記へ揃えた）。
//! - [`radio_button::RadioButton`]: ラジオボタンウィジェット（TASK-8.3-S22 / REQ-8 /
//!   REQ-13、Issue #117）。グループ内排他選択は専用コンテナ型を導入せず、同一グループの
//!   全 `RadioButton` が同じ状態フィールド（同じ `get` / `set`）を共有することで
//!   表現する。状態・イベント処理は `radio_group::RadioGroupWidget`（Issue #118）、
//!   テスト拡充・カタログ登録は Issue #119。
//! - [`radio_group::RadioGroupWidget`] / [`radio_group::RadioGroupInput`]: ラジオボタン
//!   グループの状態・イベント処理（TASK-8.3-S23 / REQ-8、Issue #118）。チェックボックスと
//!   異なりグループ内排他選択が本質のため、選択肢一覧を丸ごと保持するグループ単位の
//!   ウィジェットとして自己完結実装する（`radio_button::RadioButton`（S22、Issue #117）
//!   とはファイル名・型名を分離しており、両モジュールは名前・実装ともに独立したまま
//!   並存させる）。排他選択の SSOT はアプリ状態 `S`（`Option<usize>`）に置き、確定は
//!   既存の `state::Event::ButtonClicked` へブリッジするのみで widget 層は `&mut S` に
//!   触れない（`label::Label` / `label::LabelWidget` と同じ判断で統合は行わず両立
//!   させる）。テスト拡充・カタログ登録は Issue #119。
//! - [`toggle_switch::ToggleSwitch`]: トグルスイッチウィジェット（TASK-8.3-S25 / REQ-8 /
//!   REQ-13、Issue #120）。`Checkbox`（S19）が確立したパターンを踏襲した `Widget<S>`
//!   実装で、`[Rect（トラック）, Rect（サム）, （on 時のみ）Text(ON マーク), Text(label)]`
//!   を描画する。専用ロール `AccessibilityRole::Switch` を新設し、`CheckBox` とは
//!   区別する。
//! - [`toggle_switch::ToggleSwitchInput`][]: トグルスイッチの状態・イベント処理
//!   （TASK-8.3-S26 / REQ-8、Issue #121）。`checkbox::CheckboxWidget`（自己完結の
//!   別型）とは異なり、既存 `toggle_switch::ToggleSwitch` へ直接メソッド
//!   （`apply_input` / `handle_input` / `set_disabled` / `visual_state`）を追加している。
//!   トグル確定は既存の `state::Event::ButtonClicked` へブリッジするのみで widget 層
//!   から `&mut S` へ直接触れない。テスト拡充・カタログ登録は Issue #122。
//! - [`slider::Slider`]: スライダーウィジェット（TASK-8.3-S28/S29 / REQ-8 / REQ-13、
//!   Issue #123/#124）。`ToggleSwitch`（S25/S26）・`Checkbox`（S19）が確立したパターンを
//!   踏襲した `Widget<S>` 実装で、`[Rect（トラック）, Rect（フィル済みトラック）,
//!   Rect（サム）, Text(値の書式化文字列), Text(label)]` を描画する。`min`/`max`/`step`
//!   は構築時に正規化し（非有限値・逆転範囲のフォールバック）、数値健全性を担保する。
//!   ドラッグ（`press`/`drag`/`release`/`cancel_press`）・キーボード
//!   （[`slider::SliderInput`] 経由の `handle_input`）による値変更と `step` スナップを
//!   実装している。`WidgetCatalog` 登録は Issue #125。
//! - [`progress::ProgressBar`] / [`progress::Spinner`]: プログレス表示ウィジェット
//!   （TASK-8.3-S31 / REQ-8、Issue #126）。`Checkbox`（S19）/ `ToggleSwitch`（S25）が
//!   確立したパターンを踏襲した `Widget<S>` 実装で、`ProgressBar` は determinate
//!   （`ReadFn<S, f32>` 経由の進捗値・0.0..=1.0 へ正規化）、`Spinner` は indeterminate
//!   （状態バインディングなし）として役割を分ける。両者は専用ロール
//!   `AccessibilityRole::ProgressIndicator` を共用する（accesskit / SwiftUI いずれも
//!   両者を包含するロールのため）。
//! - [`progress::ProgressWidget`] / [`progress::ProgressMode`]: プログレス表示
//!   （バー・スピナー）の状態・イベント処理（TASK-8.3-S32 / REQ-8、Issue #127）。
//!   `label::LabelWidget` と同じ判断で `Widget<S>` を実装する自己完結の最小型として
//!   実装している。determinate モードは `ReadFn<S, f32>` 経由の `sync_from_state` のみが
//!   `cached_fraction` を更新する制御型（`[0.0, 1.0]` クランプ・非有限値の `0.0`
//!   フォールバック）、indeterminate モードはアニメーション位相 `phase` を
//!   `advance(elapsed_secs)` で前進させる（壁時計はプラットフォームレイヤが駆動）。
//!   非インタラクティブ要素（`hit_test` は常に `false`）であり、`ProgressBar` /
//!   `Spinner` と同じ `AccessibilityRole::ProgressIndicator` を使う。`ProgressBar` /
//!   `Spinner` との型統合は行わず両立させる（`checkbox::Checkbox` /
//!   `checkbox::CheckboxWidget` と同じ判断）。`catalog::WidgetCatalog` へ
//!   `progress_bar` / `spinner` の 2 エントリ（`catalog::CatalogCategory::Display`）を
//!   `Screen::progress_bar` / `Screen::spinner` 経由で登録し、結合テスト
//!   （`tests/progress_widget.rs`）を持つ（TASK-8.3-S33 / Issue #128）。`ProgressWidget`
//!   / `ProgressMode` は preview 経路からは呼び出せないため引き続き未接続
//!   （`catalog::WidgetCatalog::with_builtins` doc 参照）。
//! - [`image::Image`]: 画像ウィジェット（TASK-8.3-S34 / REQ-8 / REQ-9 / REQ-13、
//!   Issue #129）。`Label`（S10）・`Slider`（S28）が確立したパターンを踏襲した
//!   `Widget<S>` 実装で、`[Rect（画像プレースホルダ枠）, Text(alt)]` を描画する。
//!   画像ソース識別子（パス・URL）は不透明文字列として保持するのみで実デコード・I/O は
//!   行わない。固有サイズは構築時に正規化し（非有限値・非正値のフォールバック）、
//!   `measure` はアスペクト比維持の縮小のみ（content mode の Fit 相当）を行う。
//! - [`image_widget::ImageWidget`][]: 画像ウィジェットの状態・イベント処理
//!   （TASK-8.3-S35 / REQ-8、Issue #130）。`text_field::TextFieldWidget`（S14）と同じ
//!   判断で `Widget<S>` を実装する自己完結の型として実装している（型名は
//!   `image::Image` / `image::ImageSource` との衝突を避け `ImageWidget` /
//!   `ImageWidgetSource` とする。`image::Image` と `image_widget::ImageWidget` は
//!   名前が異なり実装も独立しているため、統合は行わず両立させている（`label::Label` /
//!   `label::LabelWidget` と同じ判断）。ロード状態機械
//!   [`image_widget::ImageLoadPhase`]（未開始 / ロード中 / 完了 / 失敗）を持ち、
//!   実デコード・I/O を担うプラットフォームレイヤからの通知
//!   （`notify_load_started` / `notify_load_succeeded` / `notify_load_failed`）でのみ
//!   遷移する。`WidgetCatalog` 登録・テスト網羅拡充・型統合は Issue #131。
//! - [`icon_widget::IconWidget`][] / [`icon_widget::IconWidgetSource`][]: アイコン
//!   ウィジェットの状態・イベント処理（TASK-8.3-S38 / REQ-8 / REQ-9 / REQ-13、Issue #133）。
//!   `image_widget::ImageWidget`（S35）と同じ判断で `Widget<S>` を実装する自己完結の
//!   型として実装している。型名を `IconWidget` / `IconWidgetSource`、ファイル名を
//!   `icon_widget.rs` とし、`icon::Icon<S>` / `icon::IconSource` との衝突を避ける
//!   （`image_widget::ImageWidget` と同じ命名判断）。アイコン識別子は不透明文字列として
//!   `sync_from_state` でキャッシュするのみで実グリフ解決は行わない。装飾的アイコン
//!   （`decorative`）は `paint` の `Text`（アクセシビリティラベル）を省略する（REQ-9）。
//!   既存の `AccessibilityRole::Image` を流用し、専用バリアントは追加しない
//!   （モジュール doc 参照）。`icon::Icon` との型統合は行わない。
//! - [`icon::Icon`]: アイコンウィジェット（TASK-8.3-S39 / REQ-8 / REQ-9 / REQ-13、
//!   Issue #134）。`Widget<S>` のみに依存する自己完結の型として実装している。API 面
//!   （`Icon<S>` / `IconSource<S>` / `fixed` / `bound` / `with_size`）の判断根拠は
//!   `icon.rs` モジュール doc 参照。`icon::Icon` と `icon_widget::IconWidget` は
//!   名前が異なり実装も独立しているため、統合は行わず両立させている（`image::Image` /
//!   `image_widget::ImageWidget` と同じ判断）。`AccessibilityRole::Image` を共用し、
//!   `WidgetCatalog` へ登録済み。
//! - [`scroll_view::ScrollView`] / [`scroll_view::ScrollAxis`]: スクロールビュー
//!   ウィジェット（TASK-8.3-S40 / REQ-8 / REQ-9、Issue #135）。本ウィジェットセット初の
//!   コンテナ型 `Widget<S>` 実装で、単一の子コンテンツ（`Box<dyn Widget<S>>`）を
//!   スクロール可能なビューポート内に配置する。`sync_from_state` は子へ委譲するのみで
//!   `S` へ直接触れない。オフセットの上限クランプは既存
//!   [`scroll::clamp_scroll_offset`] を再利用し、丸めロジックを二重管理しない。
//!   専用ロール `AccessibilityRole::ScrollView` を新設する。スクロール入力の実配線は
//!   Issue #136。`WidgetCatalog` 登録・結合テスト網羅拡充は TASK-8.3-S42（Issue #137）
//!   で行っている（`ScrollView<S>` コンテナ型 API を登録し、
//!   `scroll_view_widget::ScrollViewWidget`（下記）ではなく本型を登録する。`icon`
//!   エントリが `Icon` を登録し `IconWidget` を登録しない判断と同型）。
//! - [`scroll_view_widget::ScrollViewWidget`][] / [`scroll_view_widget::ScrollViewInput`][]:
//!   スクロールビューウィジェットの状態・イベント処理（TASK-8.3-S41 / REQ-8、Issue #136）。
//!   `image_widget::ImageWidget`（S35）/ `icon_widget::IconWidget`（S38）と同じ判断で
//!   `Widget<S>` を実装する自己完結の型として実装している（型名を `ScrollViewWidget`、
//!   ファイル名を `scroll_view_widget.rs` とし、`scroll_view::ScrollView` との衝突を
//!   避ける）。垂直スクロールオフセット・ビューポート/コンテンツ実寸・ドラッグ状態は
//!   アプリ状態 `S` ではなくウィジェットローカル状態として持ち、オフセットの安全域
//!   クランプは既存の [`scroll::clamp_scroll_offset`]（TASK-5.5-S5）を再利用して
//!   二重実装しない。ホイール・キーボード相当・ドラッグパンの意味づけ済み入力は
//!   `ScrollViewInput` で表現する（生キーコード・文字列は持たない、OWASP A03 対策）。
//!   `scroll_view::ScrollView` と `scroll_view_widget::ScrollViewWidget` は名前が異なり
//!   実装も独立しているため、統合は行わず両立させている（`image::Image` /
//!   `image_widget::ImageWidget` と同じ判断）。`WidgetCatalog` へは
//!   `scroll_view::ScrollView`（Issue #137）のみを登録する。
//! - [`stack_grid::Stack`] / [`stack_grid::Grid`]: スタック・グリッドレイアウト
//!   ウィジェット（TASK-8.3-S43 / REQ-8 / REQ-9、Issue #138）。本ウィジェットセット初の
//!   **複数子**コンテナ型 `Widget<S>` 実装で、`children`（`Vec<Box<dyn Widget<S>>>`）を
//!   `Stack` は主軸方向（[`layout::FlexDirection`]）へ、`Grid` は固定列数の行優先
//!   （row-major）で配置する。`sync_from_state` は全子へ委譲するのみで `S` へ直接
//!   触れない。専用ロール `AccessibilityRole::Group` を新設し両者で共用する。
//!   インタラクション状態・イベント処理の実配線は Issue #139、`WidgetCatalog` 登録は
//!   Issue #140。
//! - [`stack_grid_widget::StackWidget`] / [`stack_grid_widget::GridWidget`]: スタック・
//!   グリッドレイアウトの状態・イベント処理（TASK-8.3-S44 / REQ-8、Issue #139）。
//!   `scroll_view_widget::ScrollViewWidget`（S41）と同じ判断で `Widget<S>` を実装する
//!   自己完結の型として実装している（型名を `StackWidget` / `GridWidget`、ファイル名を
//!   `stack_grid_widget.rs` とし、`stack_grid::Stack` / `stack_grid::Grid` との衝突を
//!   避ける）。子リスト・軸・列数・ギャップ・フォーカス中の子インデックスはウィジェット
//!   ローカル状態として持ち、子ごとの配置矩形算出（`child_rect`/`child_at`）・
//!   フォーカス走査（disabled スキップ・端クランプ、ラップしない点で
//!   `radio_group::RadioGroupWidget` の roving focus とは異なる）を提供する。
//!   `stack_grid::Stack` / `stack_grid::Grid` と `stack_grid_widget::StackWidget` /
//!   `stack_grid_widget::GridWidget` は名前が異なり実装も独立しているため、統合は
//!   行わず両立させている（`label::Label` / `label::LabelWidget`、`scroll_view::
//!   ScrollView` / `scroll_view_widget::ScrollViewWidget` と同じ判断）。
//!   `AccessibilityRole::Group` を共用する。`WidgetCatalog` 登録は Issue #140。
//! - [`dialog::Dialog`]: ダイアログ・モーダルウィジェット（TASK-8.3-S46 / REQ-8、
//!   Issue #141）。制御型の状態バインディング（[`checkbox::Checkbox`] S19 /
//!   [`slider::Slider`] S28）とコンテナ型の単一子コンテンツ保持
//!   （[`scroll_view::ScrollView`] S40）を組み合わせた `Widget<S>` 実装で、開閉状態の
//!   SSOT はアプリ状態 `S`（`ReadFn<S, bool>` 経由）に置き、`sync_from_state` のみが
//!   表示キャッシュ `cached_open` を更新する。閉状態は「存在しないもの」として扱い
//!   （`measure` ゼロ寸法・`paint` 空・`hit_test` 常に `false`）、開状態は
//!   `[（modal 時のみ）Rect（スクリム）, Rect（パネル）, Text(title), Text(body)]` +
//!   `content.paint(..)` を描画する。専用ロール `AccessibilityRole::Dialog` を新設する。
//!   開閉イベント配線（背景クリック・Esc・フォーカストラップ）は Issue #142、
//!   `WidgetCatalog` 登録は Issue #143。
//! - [`dialog_widget::DialogWidget`][] / [`dialog_widget::DialogInput`][] /
//!   [`dialog_widget::DialogHitRegion`][]: ダイアログ・モーダルウィジェットの
//!   状態・イベント処理（TASK-8.3-S47 / REQ-8、Issue #142）。
//!   `scroll_view_widget::ScrollViewWidget`（S41）と同じ判断で `Widget<S>` を実装する
//!   自己完結の型として実装している（型名を `DialogWidget`、ファイル名を
//!   `dialog_widget.rs` とし、`dialog::Dialog` との衝突を避ける）。開閉イベント（Esc・
//!   閉じるボタン確定・背景（スクリム）クリック）は `Event::ButtonClicked` へ
//!   ブリッジし、専用イベントバリアントは新設しない（`CheckboxWidget` と同じ運用）。
//!   専用ロール `AccessibilityRole::Dialog` は `dialog::Dialog` の表記へ揃えた。
//!   `dialog::Dialog` との型統合は行わない（`WidgetCatalog` 登録は Issue #143）。
//! - [`menu::Menu`]: メニュー・ドロップダウンウィジェット（TASK-8.3-S49 / REQ-8、
//!   Issue #144）。制御型の状態バインディング（[`checkbox::Checkbox`] S19 /
//!   [`slider::Slider`] S28 / [`dialog::Dialog`] S46）と、選択肢一覧をグループ単位で
//!   保持するモデル（[`radio_group::RadioGroupWidget`] S23）を組み合わせた `Widget<S>`
//!   実装で、開閉状態・選択中項目 index の SSOT はいずれもアプリ状態 `S`
//!   （`ReadFn<S, bool>` / `ReadFn<S, Option<usize>>` 経由）に置き、`sync_from_state`
//!   のみが表示キャッシュ（`cached_open` / `cached_selected`）を更新する。閉状態は
//!   トリガー矩形のみ、開状態はトリガー + ポップアップ（項目数分の `Text`）を計測・
//!   描画・ヒットテストの対象とする。トリガー・ポップアップ矩形の計算は
//!   `trigger_size` / `popup_size` へ集約し、`measure` / `paint` / `hit_test` の
//!   3 箇所で共有する（`Dialog` が採る整合性契約と同型）。専用ロール
//!   `AccessibilityRole::ComboBox` を新設する。
//! - [`menu::MenuHitRegion`][] / [`menu::MenuInput`][]: メニュー・ドロップダウンの
//!   状態・イベント処理（TASK-8.3-S50 / REQ-8、Issue #145）。
//!   `toggle_switch::ToggleSwitch`（S26）/ `list_view::ListView`（S17）と同じ判断で
//!   既存 `menu::Menu<S>` へ直接メソッド・型を追加している（別型の新設は行わない）。
//!   開閉トグル・項目選択・外側クリック閉じは `dialog_widget::DialogWidget`（S47）の
//!   座標解決済み領域 [`menu::MenuHitRegion`] を明示引数として受け取る `apply_pointer`
//!   パターンを踏襲し、いずれも既存の `state::Event::ButtonClicked` へブリッジする
//!   のみで widget 層は `&mut S` に一切触れない。キーボード操作は
//!   `radio_group::RadioGroupWidget`（S23）と同型の roving focus（`focused_item`）を
//!   持つが、selection-follows-focus は**しない**（Enter/Space で確定する WAI-ARIA
//!   menu/listbox の慣例、`menu.rs` モジュール doc 参照）。`WidgetCatalog` 登録は
//!   Issue #146。
//! - [`tabs::Tabs`]: タブウィジェット（TASK-8.3-S52 / REQ-8、Issue #147）。制御型の
//!   状態バインディング（[`checkbox::Checkbox`] S19 / [`slider::Slider`] S28 /
//!   [`dialog::Dialog`] S46 / [`menu::Menu`] S49）と、コンテナ型の子コンテンツ保持
//!   （[`dialog::Dialog::with_content`]）を組み合わせた `Widget<S>` 実装で、
//!   選択中タブ index の SSOT はアプリ状態 `S`（`ReadFn<S, usize>` 経由）に置き、
//!   `sync_from_state` のみが表示キャッシュ（`cached_selected`）を更新する。
//!   タブバー + コンテンツ領域を計測・描画・ヒットテストの対象とし、タブ見出し・
//!   タブバー・コンテンツ領域の寸法計算は `tab_size` / `bar_size` /
//!   `content_area_size` へ集約して 3 箇所で共有する（`Dialog` が採る整合性契約と
//!   同型）。専用ロール `AccessibilityRole::TabList` を新設する。タブ切替イベント
//!   配線は Issue #148、`WidgetCatalog` 登録は Issue #149。
//! - [`tab_bar::TabBarWidget`] / [`tab_bar::TabBarInput`]: タブの状態・イベント処理
//!   （TASK-8.3-S53 / REQ-8、Issue #148）。`radio_group::RadioGroupWidget`（S23）と
//!   同じ判断で `Widget<S>` を実装する自己完結の型として実装している（型名を
//!   `TabBarWidget` / `TabBarInput`、ファイル名を `tab_bar.rs` とし、`tabs::Tabs` との
//!   衝突を避けている）。タブはラジオボタングループと同じ「排他選択」が本質のため、
//!   選択中インデックスの SSOT はアプリ状態 `S`（`Option<usize>`）に置き、確定は
//!   既存の `state::Event::ButtonClicked` へブリッジするのみで widget 層は `&mut S`
//!   に触れない。`tabs::Tabs` と `tab_bar::TabBarWidget` は名前が異なり実装も独立
//!   しているため、統合は行わず両立させている（`label::Label` / `label::LabelWidget`
//!   と同じ判断）。専用ロール `AccessibilityRole::TabList` は `tabs::Tabs` の表記へ
//!   揃えた（共用する）。`WidgetCatalog` 登録・テスト拡充は Issue #149。
//! - [`tooltip::Tooltip`] / [`tooltip::TooltipSource`]: ツールチップウィジェット
//!   （TASK-8.3-S55 / REQ-8、Issue #150）。テキストの状態束縛（[`label::Label`] S10 /
//!   [`image::Image`] S34）と、可視状態に応じた描画切替・コンテナ型のアンカー保持
//!   （[`dialog::Dialog`] S46）を組み合わせた `Widget<S>` 実装で、`measure` は
//!   アンカーの計測結果をそのまま返す（バブルはレイアウト空間を消費しないオーバー
//!   レイ）。バブルの表示・非表示は明示 API（`show` / `hide`）のみで切り替える最小
//!   可視モデルとし、hover 起因の表示・遅延タイマーは Issue #151 が担う。専用ロール
//!   `AccessibilityRole::Tooltip` を新設する。`WidgetCatalog` 登録は Issue #152。
//! - [`tooltip_widget::TooltipWidget`][] / [`tooltip_widget::TooltipVisibility`][] /
//!   [`tooltip_widget::TooltipTextSource`][]: ツールチップの状態・イベント処理
//!   （TASK-8.3-S56 / REQ-8、Issue #151）。`icon_widget::IconWidget`（S38）と同じ判断で
//!   `Widget<S>` を実装する自己完結の型として実装している（型名を `TooltipWidget`、
//!   ファイル名を `tooltip_widget.rs` とし、`tooltip::Tooltip<S>` との衝突を避ける）。
//!   可視性状態機械 `TooltipVisibility`（`Hidden` → `PendingShow` → `Visible`）を持ち、
//!   ホバー開始からの表示遅延をフレーム駆動の `advance(elapsed_secs)`
//!   （`progress::ProgressWidget::advance` と同じ契約）で前進させる。フォーカス獲得は
//!   遅延なしで即時表示する（WAI-ARIA 慣例、REQ-9）。ツールチップ可視性はアプリ状態
//!   `S` に属さない widget 内部状態のため `state::Event` へのブリッジは行わない
//!   （`tab_bar::TabBarWidget` がタブ選択確定を `Event` へブリッジしたのとは対照的な
//!   設計判断）。新設の `AccessibilityRole::Tooltip` は `tooltip::Tooltip` の表記へ
//!   揃える（共用する）。`tooltip::Tooltip` と `tooltip_widget::TooltipWidget` は
//!   名前が異なり実装も独立しているため、統合は行わず両立させている（`label::Label` /
//!   `label::LabelWidget` と同じ判断）。`WidgetCatalog` 登録・テスト網羅拡充・型統合は
//!   Issue #152 以降に委ねる。
//! - [`card::Card`] / [`card::CardInput`] / [`card::Separator`] /
//!   [`card::SeparatorOrientation`]: カード・セパレータウィジェット
//!   （TASK-8.3-S58/S59 / REQ-8、Issue #153/#154）。複数子コンテナ
//!   （[`stack_grid::Stack`] S43）と非インタラクティブ要素
//!   （[`progress::ProgressBar`] S31）のパターンを組み合わせた `Widget<S>` 実装で、
//!   `Card` は子を縦積みするグルーピングコンテナ（`padding` 分は `measure` の寸法
//!   計算のみに反映し描画へは反映しない）、`Separator` は水平・垂直の向きを持つ
//!   非インタラクティブな区切り線。`Card` は既存の `AccessibilityRole::Group`
//!   （S43）を共用し、`Separator` は専用ロール `AccessibilityRole::Separator` を
//!   新設する。カードのタップ可能化（opt-in、`with_tap_label`）・
//!   `CardInput::Activate` によるキーボード活性化を実装している（Issue #154）。
//!   `WidgetCatalog` へは `card`（`Container`）・`separator`（`Display`）の 2 エントリを
//!   登録している（Issue #155）。
//! - [`style_tokens::StyleTokenRefs`] / [`style_tokens::resolve_widget_style`] 等:
//!   基本ウィジェット（Action/Input/Display カテゴリ）へのトークン適用配線
//!   （TASK-8.4-S14 / REQ-8、Issue #170）。`ui::widget::StateStyles` を
//!   `T = StyleTokenRefs` でインスタンス化し、`crate::token::TokenResolver` 経由で
//!   解決した具体値を `PaintCommand::StyledRect` / `StyledText` へ橋渡しする
//!   （詳細は `style_tokens.rs` モジュール doc 参照）。
//! - [`text_selection::CharOffset`] / [`text_selection::SelectionRange`] /
//!   [`text_selection::OffsetError`] 等: カーソル・選択範囲管理（TASK-8.5-S4 / REQ-8、
//!   Issue #185）。`docs/architecture/ime-architecture.md`（S1・#182）§4.6「文字
//!   インデックス正規化方針」を実装し、winit の byte offset・iOS/Android の UTF-16
//!   offset を `fandhe-core` 契約の char 単位オフセットへ正規化する変換関数群と、
//!   選択範囲型 `SelectionRange` を提供する。状態モデル成果物（#183/#184）には
//!   依存しない自己完結モジュールとして実装している（本モジュール doc 参照）。
//! - [`composition_display::CompositionDisplay`][]: コンポジション（未確定文字列）表示合成
//!   （TASK-8.5-S5 / REQ-8、Issue #186）。`docs/architecture/text-input-state-model.md`
//!   （S2・#183）§4.4 の表示文字列導出（純関数）を
//!   [`widget::PaintCommand::CompositionText`] へ落とす。`text_selection`（S4・#185）と
//!   同じ自己完結パターンで `Composition` 型に依存せず実装しており、状態モデル
//!   （#184）確定後もこの純関数構成を意図的に維持する
//!   （`composition_display.rs` モジュール doc 参照）。`TextFieldWidget` への実配線は
//!   Issue #189 が担う。
//! - [`ime_cursor_area::ImeCursorAreaParams`] / [`ime_cursor_area::ime_cursor_area`]:
//!   変換候補ウィンドウの位置計算（TASK-8.5-S6 / REQ-8、Issue #187）。カーソル座標
//!   （`anchor` / `preedit` / `preedit_cursor`）から `ImeRequest::SetCursorArea` へ渡す
//!   論理ピクセル矩形を計算する（`ime-architecture.md` §4.2〜§4.3）。`Composition` 型
//!   成果物（#184）には依存しないプリミティブ引数の自己完結モジュールとして実装
//!   している（本モジュール doc 参照）。
//! - [`focus::FocusRing`] / [`focus::FocusEntry`] / [`focus::FocusDirection`] /
//!   [`focus::WindowHints`]: Android TV 等のリモコン操作向けフォーカス移動・
//!   再解決コア（TASK-17.2 / REQ-17、`docs/architecture/tv-focus-navigation-architecture.md`
//!   §3・§4.4・§5）。`Node::Button` / `Node::IndexedButton` のみをフォーカス可能と
//!   定義し（§3.1）、`Axis`（node.rs）に基づく軸解決アルゴリズムで 4 方向すべてを
//!   単一規則で解決する（§4.4）。`id`（`WidgetId`）比較による構造変化の自動検知・
//!   三段探索（観測キャッシュ照会 → 有界全域探索 → 先頭フォールバック）での復旧を
//!   自己更新型 API（`FocusRing::rebuild`）として提供する。`VirtualList` の描画窓
//!   （`VisibleRange`）は保持・変更しない（§4.6、プラットフォームレイヤの責務）。
//!   決定ボタン押下の実発火は [`event::activate_focus_at`] が別途担う（§5）。

mod button;
mod canvas;
mod card;
mod catalog;
mod checkbox;
mod composition_display;
mod dialog;
mod dialog_widget;
mod event;
mod focus;
mod geometry;
mod harness;
mod hit_test;
mod icon;
mod icon_widget;
mod image;
mod image_widget;
mod ime;
mod ime_cursor_area;
mod ime_focus;
mod label;
mod layout;
mod list_view;
mod menu;
mod node;
mod pointer;
mod progress;
mod radio_button;
mod radio_group;
mod recycle;
mod render;
mod render_mode;
mod sample;
mod screen;
mod screen_stack;
mod scroll;
mod scroll_view;
mod scroll_view_widget;
mod slider;
mod stack_grid;
mod stack_grid_widget;
mod style_tokens;
mod tab_bar;
mod tabs;
mod text_field;
mod text_input;
mod text_selection;
mod theme_demo;
mod toggle_switch;
mod tooltip;
mod tooltip_widget;
mod virtual_list;
mod widget;

pub use button::{Button, ButtonInput};
pub use canvas::CanvasWidget;
pub use card::{Card, CardInput, Separator, SeparatorOrientation};
pub use catalog::{CatalogCategory, CatalogEntry, CatalogError, GalleryState, WidgetCatalog};
pub use checkbox::{Checkbox, CheckboxInput, CheckboxWidget};
pub use composition_display::CompositionDisplay;
pub use dialog::Dialog;
pub use dialog_widget::{DialogHitRegion, DialogInput, DialogWidget};
pub use event::{
    activate_focus_at, click_button, click_button_at, request_back, set_widget_value_at,
};
pub use focus::{FocusDirection, FocusEntry, FocusRing, WindowHints};
pub use geometry::{Point, Rect, Size};
pub use harness::{LifecyclePhase, WidgetHarness};
pub use hit_test::{HitRegion, HitTarget, hit_test};
pub use icon::{Icon, IconSource};
pub use icon_widget::{IconWidget, IconWidgetSource};
pub use image::{Image, ImageSource};
pub use image_widget::{ImageLoadPhase, ImageWidget, ImageWidgetSource};
pub use ime::{ImeEvent, ImePurpose, ImeRequest, ImeSessionId};
pub use ime_cursor_area::{ImeCursorAreaParams, ime_cursor_area};
pub use ime_focus::{FocusTransition, ImeFocusCoordinator};
pub use label::{Label, LabelSource, LabelWidget};
pub use layout::{
    AvailableSpace, Dimension, FlexDirection, LayoutConstraints, LayoutError, LayoutNodeId,
    LayoutPoint, LayoutRect, LayoutSize, LayoutStyle, LayoutTree, Measure,
};
pub use list_view::{ListItemSource, ListView, ListViewInput};
pub use menu::{Menu, MenuHitRegion, MenuInput};
pub use node::{Axis, Node, RGBA_MAX_SIDE, RgbaPixels, TextRole};
pub use pointer::{PointerDispatcher, PointerEvent, PointerEventKind};
pub use progress::{ProgressBar, ProgressMode, ProgressWidget, Spinner};
pub use radio_button::RadioButton;
pub use radio_group::{RadioGroupInput, RadioGroupWidget};
pub use recycle::ListRecycler;
pub use render::render_headless;
pub use render_mode::{ModeSwitcher, RenderMode, RendererLifecycle};
pub use sample::{
    SampleState, add_item, expand_items, get_memo, increment_count, sample_screen, set_memo,
};
pub use screen::Screen;
pub use screen_stack::ScreenStack;
pub use scroll::{clamp_scroll_offset, total_content_extent};
pub use scroll_view::{ScrollAxis, ScrollView};
pub use scroll_view_widget::{ScrollViewInput, ScrollViewWidget};
pub use slider::{Slider, SliderInput};
pub use stack_grid::{Grid, Stack};
pub use stack_grid_widget::{GridInput, GridWidget, StackAxis, StackInput, StackWidget};
pub use style_tokens::{
    ResolvedWidgetStyle, StyleTokenRefs, action_style_refs, display_style_refs, input_style_refs,
    resolve_widget_style, style_paint_commands,
};
pub use tab_bar::{TabBarInput, TabBarWidget};
pub use tabs::{TabItem, Tabs};
pub use text_field::{TextFieldInput, TextFieldWidget};
pub use text_input::TextInput;
pub use text_selection::{
    CharOffset, OffsetError, SelectionRange, byte_offset_at, char_offset_from_byte,
    char_offset_from_utf16, is_grapheme_boundary, next_grapheme_boundary, prev_grapheme_boundary,
    try_char_offset_from_byte, try_char_offset_from_utf16, try_selection_from_byte_range,
    try_selection_from_utf16_range, utf16_offset_at,
};
pub use theme_demo::{
    demo_resolver, themed_catalog_lines, themed_sample_lines, themed_style_lines,
};
pub use toggle_switch::{ToggleSwitch, ToggleSwitchInput};
pub use tooltip::{Tooltip, TooltipSource};
pub use tooltip_widget::{TooltipTextSource, TooltipVisibility, TooltipWidget};
pub use virtual_list::{VisibleRange, compute_visible_range, grid_item_range};
pub use widget::{
    AccessibilityRole, InteractionInput, InteractionState, Measurement, PaintCommand, StateStyles,
    VisualState, Widget, WidgetId,
};
