# Web DOM 出力方式アーキテクチャ設計

## 1. 目的・位置づけ

本書は TASK-4.4-S1（Issue #79、親: TASK-4.4 / Issue #78、`docs/spec/05-tasks.md` L161〜166、
推定工数 2 日のうち本 sub-issue は約 4h）の成果物であり、REQ-4（`docs/spec/04-requirements.md`
L66〜78）の受け入れ基準「アクセシビリティ・SEO が重要な画面について、DOM 出力方式を選択できる
導線を提供する」（L76）の**設計部分**を満たすために作成する。

- **本 issue の成果物**: 本書 1 本のみ（コード変更なし）。DOM 出力レンダラの実装（S2）・
  選択導線の実装（S3）・a11y/SEO 確認テスト（S4）は行わない。
- **関連文書**: REQ-9（アクセシビリティ対応）Web 分の受け入れ基準（`04-requirements.md`
  L133）に対する制約・代替方針の正式な整理は
  `docs/architecture/web-accessibility-constraints.md`（TASK-9.4、Issue #217）を参照。
  同書は本書と S4（#82）の実測を根拠に REQ-9 の受け入れ基準・
  M3 完了基準との整合を確認する位置づけであり、本書の設計内容自体はここに集約したまま
  重複記載しない。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成する（`swiftui-bridge-architecture.md` /
  `crate-feature-matrix.md` / `responsibility-boundary.md` / `state-access-api-tradeoff.md` /
  `unsafe-audit.md` と同じ置き場・体裁）。spec 自体の変更はこのリポジトリでは行わない。
- **PoC との関係**: PoC-5（`docs/spec/03-poc/web-wasm/README.md`）が本設計の出発点である。
  独自レンダラ（WASM＋WebGL2/canvas）方式は性能 3 指標（配信サイズ brotli 圧縮後 約 932KB・
  TTI 約 332ms・フレームタイム p95 8.6〜9.3ms、いずれも同 README L80〜83 の実測表）を満たす
  一方、**Chrome の Accessibility Tree には `Canvas`（無ラベル）と IME 用の隠し `textbox` のみが
  公開され、見出し・リスト・ボタン等の意味情報は 0 件**（同 README L85 のアクセシビリティ行・
  L105）。
  原因は `egui`/`eframe` が AccessKit の Web バックエンドを持たないためであり（同 README L25）、
  代替として用意されている `web_screen_reader` feature（`SpeechSynthesis` API 経由の音声読み上げ）は
  OS 標準スクリーンリーダ（VoiceOver・NVDA・TalkBack 等）と連携しない（同 README L25・L97）。
  PoC-5 の結論は「Web は DOM 出力を採る、または独自レンダラと DOM 出力のハイブリッドとする」
  ことを支持する（同 README L142）。本書はこの結論を受け、DOM 出力方式のアーキテクチャを設計する。
- **現状の実装基盤**: `crates/fandhe-core` は `Screen<S>` / `Node<S>`（`Text` / `TextField` /
  `Button` / `List` の 4 バリアント）・`state::{StateStore, Event}` によるイベント→状態更新の
  配線・`Screen::visit_expanded` による `List` 込みの深さ優先走査基盤・`render_headless` による
  テキスト表現マッピングを提供する（依存クレートゼロ・`#![forbid(unsafe_code)]`）。
  `crates/fandhe-egui-proxy` は実描画検証専用のプロキシ（本実装レンダラではない）であり、
  `src/web.rs` が `<canvas id="fandhe-proxy-canvas">` へ `eframe::WebRunner`（WebGL2/`glow`
  バックエンド）をアタッチしている。DOM 出力レンダラはこれとは別の新設クレートとして設計する
  （§2 参照）。
- **並行 Issue との関係**: TASK-4.1（Safari canvas 対策、#75）・TASK-4.3（WebGL2 固定、#77）の
  完了には依存しない。ただし両タスクの方針（WebGL2 固定・`canvas` 要素へ
  の CSS サイズ明示）は「独自レンダラ側の既定挙動」として本書の前提に含める。
- **S2〜S4 との対応**（本書のどの節を各 sub-issue が実装するか）:

  | sub-issue | Issue | 対応する節 |
  |-----------|-------|-----------|
  | S1: DOM 出力方式のアーキテクチャ設計 | #79（本書） | 本書全体（設計） |
  | S2: DOM 出力レンダラ最小実装 | #80 | 2.1（クレート配置）、2.2（Node → HTML マッピング表）、2.3（イベント配線）、2.4（再描画戦略）、2.6（セキュリティ規約） |
  | S3: 画面単位の出力方式選択導線実装 | #81 | 2.5（選択導線のインターフェース） |
  | S4: アクセシビリティ・SEO 要件の確認テスト | #82（完了） | 2.2（ARIA・セマンティクス割当）、3（トレードオフ）の検証観点 |

## 2. 設計判断（結論）

### 2.1 クレート配置・依存方向

- DOM 出力レンダラは新設クレート **`fandhe-dom-renderer`** とする。プラットフォーム最適化レイヤ
  側の配置とし、依存方向は「`fandhe-dom-renderer` → `fandhe-core`」の一方向（coding-rust.md
  「共通コアからプラットフォーム固有クレートへ依存しない」に準拠。`fandhe-egui-proxy` /
  `fandhe-swiftui-bridge` と同型の関係）。
- `wasm32-unknown-unknown` ターゲット限定のクレートとし、`#![forbid(unsafe_code)]` を付ける
  （`fandhe-core` と同じ安全性方針。DOM 操作は `web-sys` の安全な API のみで完結し、`unsafe` を
  必要としない設計とする、§2.6 参照）。
- 依存クレートは workspace 一元管理を利用する。`wasm-bindgen`（`=0.2.126` 固定）・
  `wasm-bindgen-futures`・`web-sys`・`console_error_panic_hook` はルート `Cargo.toml` の
  `[workspace.dependencies]` に既存のため追加不要。`web-sys` の feature（`Element` /
  `HtmlElement` / `Node`（DOM の `Node`。`fandhe_core::Node<S>` とは別物であり、コード中では
  `web_sys::Node` と完全修飾して区別する）/ `Text` 等）は S2 実装時に必要最小限を追加する
  （coding-rust.md「feature フラグは加法的」・security.md「A06 脆弱な依存」の最小 feature 方針）。
- `fandhe-egui-proxy` は実描画検証専用プロキシであり拡張しない（本実装レンダラの追加先は
  新設クレートのみとし、検証用コードと本実装を混在させない）。

### 2.2 Node → HTML マッピング表

`fandhe_core::ui::Node<S>` の各バリアントを以下の HTML 要素へマッピングする。走査は
`Screen::visit_expanded`（既存の深さ優先展開）をそのまま流用し、DOM 出力レンダラは
「ノード列 → HTML 要素」の変換のみを追加する（`render_headless` が「ノード列 → テキスト表現」を
担うのと対称の関係）。

| `Node<S>` バリアント | HTML 要素 | ARIA・セマンティクス | 備考 |
|---|---|---|---|
| `Screen` タイトル | `<h1>` | — | 画面ルート要素の先頭子として 1 つのみ配置し、文書構造上の見出しレベルを固定する |
| `Text(String)` | `<p>` | — | 状態に依存しない静的テキスト。`textContent` として設定する（§2.6） |
| `DynamicText { get }` | `<p>` | — | `Text` と同じ。`get(state)` を都度評価する（#581） |
| `StyledText { role, get }` | `<p>`（`role: Error` は `role="alert" class="fandhe-text-error"` を付与） | `role="alert"` は暗黙に `aria-live="assertive"` を持ち、検証エラー文を支援技術へ即時通知する | 色は class 経由でホスト側 CSS に委ね DSL には書かせない（#811 方針）。`§2.3` のとおり `mount.rs` は全再構築のため、入力のたびに再読み上げが起き得る（#814） |
| `TextField { label, get, set }` | `<label>` ＋ `<input type="text">` | `<label for="{id}">` と `<input id="{id}">` を `id`/`for` で関連付ける | `id` は画面内一意な生成規則（例: ラベル文字列のスラッグ化＋連番）とし、S2 実装時に確定する |
| `Button { label, on_click }` | `<button>` | ラベル文字列を `textContent` として設定（既定でボタンとして読み上げ可能） | クリックイベントの配線は §2.3 |
| `List { expand }` | `<ul>` | 各子ノードは `<li>` でラップしてから、子ノード自身のマッピング規則（本表）を `<li>` の子として適用する | `visit_expanded` の深さ優先展開に対応し、ネストした `List` は `<li><ul>...</ul></li>` の入れ子になる |

S4（#82、完了）はこの表に基づき、ブラウザ標準の Accessibility Tree（Chrome DevTools Protocol の
`Accessibility.getFullAXTree`）に見出し・リスト・ボタン等の意味情報が実際に公開されることを
確認した（PoC-5 の独自レンダラ計測（0 件）との対比が検証の骨子）。実測では `<label>` が `<input>` を
内包する暗黙の関連付け（現行実装、本表が想定する `id`/`for` 明示とは異なる）でも
`textbox` の accessible name が正しく得られており、この乖離は
`web-accessibility-constraints.md` に観測事項として記録し、本表・現行実装の修正は行っていない。

### 2.3 イベント配線

DOM イベント → 状態更新は一方向のデータフローとする。

```
DOM イベント（input/click）
  → wasm_bindgen::closure::Closure ハンドラ
  → fandhe_core::state::Event（ButtonClicked / TextFieldChanged）
  → StateStore::dispatch
  → 状態 S 更新
  → 再描画（§2.4）
```

- 既存の `fandhe_core::state::{Event, StateStore}`（TASK-1.1-S5 で実装済み）をそのまま利用する。
  DOM 出力レンダラは `web_sys::Element` の `click` / `input` イベントリスナーから
  `Event::ButtonClicked { label }` / `Event::TextFieldChanged { label, value }` を組み立てて
  `StateStore::dispatch` へ渡すだけであり、状態更新ロジック自体を再実装しない（`ui::event` が
  走査ロジックの単一ソースであるという既存の設計方針を踏襲する）。
- `Closure`（`wasm_bindgen::closure::Closure`）の寿命管理は、DOM ノードを保持する構造体
  （例: 画面 1 つに対応する `DomScreen` 相当の型、具体名称は S2 実装時に確定）にハンドラを
  フィールドとして所有させ、画面が破棄されるまで `Closure` を保持し続ける方針とする。
  `Closure::forget`（意図的なメモリリーク API）は使わない。イベントリスナーの登録は
  `add_event_listener_with_callback`、解除は `remove_event_listener_with_callback` を
  画面破棄時（再構築時、§2.4）に対で呼ぶ。

### 2.4 再描画戦略

最小実装（S2）は「状態更新ごとに画面ルート要素配下を全再構築」とする。具体的には、
`StateStore::dispatch` 呼び出し後、画面ルート要素（`<div>` 等）の子要素をすべて削除し、
`Screen::visit_expanded` を再度呼び出して §2.2 のマッピングに従い DOM を再構築する
（`render_headless` が呼び出しのたびに `visit_expanded` を呼び直す都度評価の設計思想を踏襲）。

差分更新（変更のあった `Node` に対応する DOM 要素のみを更新する仮想 DOM 相当の機構）は
本設計のスコープ外とし、将来課題とする（§4）。全再構築方式は要素数が少ない画面では実用上
問題にならないが、300 項目規模のリスト（REQ-5 が言及する性能目標）でのスクロール位置保持・
再描画コストは未検証であり、DOM 出力方式を選ぶ画面の性質（アクセシビリティ・SEO 優先、
高頻度更新の少ない画面）と整合させる前提を置く。

### 2.5 選択導線のインターフェース（S3 で実装確定）

画面単位で出力方式を選べる API として、`Screen<S>` を保持する側（アプリ・プロトタイプ層）が
画面登録時に指定する列挙型を新設する案を採用し、S3（Issue #81）で実装した。

```rust
/// 画面の出力方式。`fandhe-egui-proxy`（独自レンダラ）と
/// `fandhe-dom-renderer`（DOM 出力）のいずれで描画するかを画面単位に指定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RenderMode {
    /// 独自レンダラ（WASM＋WebGL2/canvas）。性能優先の画面向け。既定値。
    #[default]
    Canvas,
    /// DOM 出力。アクセシビリティ・SEO 優先の画面向け。
    Dom,
}
```

- **配置先の確定**: `fandhe-core::ui`（`crates/fandhe-core/src/ui/render_mode.rs`）に確定した。
  依存ゼロの純データ enum であり、どちらのレンダラクレートにも依存させないという本節の制約を
  満たしつつ、アプリ層（`fandhe-prototype`）・両レンダラの全員が既存の依存方向
  （最適化レイヤ → コアの一方向、coding-rust.md）のまま参照できるため。調停層（アプリ側）に
  置く案は、レンダラ側から参照できず型の二重定義を招くため不採用とした。
- **指定 API**: `Screen<S>` にビルダー `Screen::with_render_mode(self, RenderMode) -> Self` と
  アクセサ `Screen::render_mode(&self) -> RenderMode`（既定 `Canvas`）を追加した。
  `fandhe-prototype` では、`TextField` フォームを含み `<label for>`/`<input id>` 関連付けの
  効果が最も現れる設定画面を `RenderMode::Dom` 指定の例とし、一覧・詳細は既定（`Canvas`）の
  ままとした（S4 #82 の検証対象）。
- **canvas 画面と DOM 画面の共存方式**: 同一ページ内に両方のルート要素（`<canvas
  id="fandhe-proxy-canvas">` と DOM 出力用のルート `<div>`）を用意し、`RenderMode` に応じて
  表示中の画面のルート要素のみ `display` を切り替える（非表示側は DOM から取り除くのではなく
  CSS で隠す）。
- **ナビゲーション時の切替規約**: 画面遷移で `RenderMode` が変わる場合、遷移前の画面のレンダラ
  （`Canvas` なら `eframe::WebRunner` の再描画停止、`Dom` なら §2.3 のイベントリスナー解除）を
  明示的に停止してから、遷移後のレンダラを起動する。両レンダラを同時に起動したままにしない
  （二重イベント配線・状態不整合を防ぐため）。
- **調停ロジックの実装**: 上記 3 規約（共存方式・排他起動・停止→起動の順序）のうち
  レンダラ非依存な部分を `RendererLifecycle` トレイト（`activate`/`deactivate`）と
  `ModeSwitcher`（現在モードを保持し `apply(next)` で「同一モードなら no-op / 旧モードの
  `deactivate` → 新モードの `activate` の順で必ず実行 / いかなる遷移列でも active は常に
  高々 1 つ」を保証）として切り出し、モックレンダラによる単体テストで不変条件を固定化した。
  **配置規約**: `RendererLifecycle` / `ModeSwitcher` はレンダラ非依存であるため
  `fandhe-core::ui`（`crates/fandhe-core/src/ui/render_mode.rs`）に置く。wasm32 結線部
  （DOM 側 activate/deactivate の実配線・canvas 側 `eframe::WebRunner` 停止・ルート要素の
  `display` 切替）は `fandhe-dom-renderer` 側で `RendererLifecycle` を実装する形で
  追加する（未完了事項として §4 参照）。

### 2.6 セキュリティの扱い

DOM 構築は `Document::create_element` と `Node::set_text_content`（`web_sys` の安全な API）の
組み合わせのみで行い、**`inner_html` / `insert_adjacent_html` への文字列展開を禁止する**。
`Node::Text` の内容・`TextField` の `get(state)` が返す値・`List` の展開結果はいずれも状態 `S`
由来の文字列だが、`set_text_content` はエスケープ処理込みでテキストノードとして設定するため、
状態由来の文字列がマークアップとして解釈されることはない（`render.rs` が「テキスト値を
コマンド・クエリ・マークアップへ展開しない」と定める既存の A03 インジェクション対策方針を、
DOM 出力へそのまま拡張する）。本方針は `security-auditor` のレビュー観点とする。

## 3. トレードオフ・代替案

- **`egui`/`eframe` の `web_screen_reader`（SpeechSynthesis）**: 独自レンダラのまま
  アクセシビリティを補う代替として検討したが不採用とした。理由は、OS 標準スクリーンリーダ
  （VoiceOver・NVDA・TalkBack 等）と連携せず、ブラウザ標準の Accessibility Tree にも要素が
  現れない（PoC-5 README L25・L97）ため、REQ-4 が要求する「アクセシビリティ・SEO が重要な
  画面」の要件を満たさないと判断したため。
- **AccessKit Web バックエンドの実装・upstream への貢献**: 独自レンダラのまま AccessKit 経由で
  ブラウザ標準の Accessibility Tree に要素を公開する案。PoC-5 時点（2024 年）で AccessKit は
  Web バックエンドを「今後の目標」としたまま実装が存在せず（同 README L25）、本フレームワーク
  側での対応可否・工数が見積れないため不採用とした。将来 AccessKit が Web バックエンドを
  提供した場合の再評価は対象外とせず、`.claude/rules/out-of-scope-tracking.md` の枠組みで
  継続監視の要否をユーザーと確認する候補とする。
- **DOM 出力方式の性能特性**: 独自レンダラの性能 3 指標（配信サイズ・TTI・フレームタイム）は
  PoC-5 で実測済みだが、DOM 出力方式については実装比較用の別実装を作る工数を確保できず、
  机上調査に留まった（同 README L25）。したがって本書で選んだ DOM 出力方式についても、
  同等の定量比較は本 issue のスコープ外であり、REQ-4 の受け入れ基準にも「実装による定量比較は
  追加検証とする」と明記されている（`docs/spec/04-requirements.md` L76）。

## 4. 未解決事項・将来課題

- **DOM 出力 vs 独自レンダラの定量性能比較**: REQ-4 で「追加検証」と明記済み。実施の要否は
  ユーザーと確認し、必要であれば `.claude/rules/out-of-scope-tracking.md` のフローで Issue 化
  する。
- **差分更新（仮想 DOM 相当の機構）**: §2.4 で述べたとおり将来課題。300 項目規模のリスト
  （REQ-5 参照）での実用性は未検証。
- **wasm32 結線部の実配線**: DOM 側 activate/deactivate（マウント/イベントリスナー解除 API
  呼び出し）・canvas 側 `eframe::WebRunner` 停止・ルート要素の `display` 切替は
  `fandhe-dom-renderer` 側で `RendererLifecycle` を実装する形で追加する。実ブラウザでの
  表示切替の目視確認もこの結線後に行う。
- **スタイル適用**: デザイントークン（TASK-8.4、`docs/spec/05-tasks.md` L274）との将来的な接続は
  本書のスコープ外。TASK-8.4 の着手状況は本書では確認していない。
- **SSR / SSG・OTA 更新・差分更新**: いずれも本タスク群（TASK-4.4 系列）のスコープ外。
  特に OTA 更新（Web 限定）は署名検証が必須（security.md「A08 整合性の不備」）である旨を
  混同防止のため明記する。
- **アクセシビリティ・SEO の改善候補**: アクセシビリティ・SEO 要件の確認テストは
  S4（#82、§1 対応表参照）で実施済み。以下の改善候補はユーザー承認前のため未起票:
  `<label>` の `for`/`id` 明示化要否・`<input type="text">` の明示付与・ランドマークロール等の
  追加アクセシビリティ属性・SSR/SSG 経路の本実装。
