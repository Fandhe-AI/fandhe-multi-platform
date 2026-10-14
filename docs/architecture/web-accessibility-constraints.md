# Web アクセシビリティ制約と DOM 出力代替方針

## 1. 目的・位置づけ

本書は TASK-9.4（Issue #217、`docs/spec/05-tasks.md` L318〜323、推定工数 0.5 日、前提タスク
TASK-4.4）の成果物であり、REQ-9（`docs/spec/04-requirements.md` L125〜135）の受け入れ基準
4 項目目（L133）「Web（WASM）ではブラウザ標準のアクセシビリティツリーに要素が公開されないと
いう制約（PoC-5 で確認済み）を前提に、DOM 出力方式（REQ-4）での代替を明記する」を満たすために
作成する、**制約・代替方針の正式なドキュメント**である。

- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ
  側（`docs/architecture/`）の新規ドキュメントとして作成する（`dom-output-architecture.md` /
  `swiftui-bridge-architecture.md` と同じ置き場・体裁）。spec 自体の変更はこのリポジトリでは
  行わない。
- **前提タスクの完了状況**: TASK-4.4 の 4 つの sub-issue（S1〜S4）はすべて完了済みである。
  - S1: `docs/architecture/dom-output-architecture.md`（DOM 出力方式の設計）
  - S2: `crates/fandhe-web-dom`（DOM 出力レンダラ最小実装）
  - S3: `crates/fandhe-core/src/ui/render_mode.rs`（`RenderMode::{Canvas, Dom}` /
    `ModeSwitcher` / `RendererLifecycle` による画面単位の選択導線）
  - S4（#82）: Chrome DevTools Protocol
    `Accessibility.getFullAXTree` による実測
  本書はこれら既存の設計・実装・検証ドキュメントを、REQ-9 Web 分の受け入れ基準に対応する
  「制約と代替の方針」として 1 本にまとめ直したものであり、新たな設計判断・実装は行わない。

## 2. 構造的制約

### 2.1 AccessKit に Web バックエンドが存在しない

独自レンダラ方式のネイティブプラットフォーム（macOS・iOS・Android）は `accesskit` を通じて
OS 標準のアクセシビリティ API（VoiceOver・TalkBack 等）へ UI ツリーを公開できる
（`docs/spec/04-requirements.md` L127「詳細」、macOS は PoC-1 で実証済み）。しかし
Web（WASM）では `accesskit` に Web バックエンドが存在しないため、この経路は成立しない
（同 L127・L276「AccessKit の Web バックエンド不在」）。

PoC-5（`docs/spec/03-poc/web-wasm/README.md`）が独自レンダラ（wasm＋WebGL2/canvas）方式で
実機検証した結果、Chrome の Accessibility Tree には `Canvas`（無ラベル）と IME 用の隠し
`textbox` のみが公開され、見出し・リスト・ボタン等の**意味情報は 0 件**であることが実証されて
いる（同 README「成功基準との照合」表・アクセシビリティ検証行、判定は「未達成」）。原因は
`egui`/`eframe` が AccessKit の Web バックエンドを持たないことに起因する
（`dom-output-architecture.md` §1）。

### 2.2 代替案（`web_screen_reader` feature）の不採用

`egui`/`eframe` には代替として `web_screen_reader` feature（既定で有効。`SpeechSynthesis`
API 経由でエンジン内蔵の音声読み上げを行う）が用意されているが、これは OS 標準の
スクリーンリーダー（VoiceOver・NVDA・TalkBack 等）と連携しない独自の読み上げ機構であり、
基準が意図する「標準スクリーンリーダーでの読み上げ」の代替とは言えないため、本フレームワーク
では不採用と判断する（PoC-5 README・`dom-output-architecture.md` §1 の不採用判断を踏襲）。

### 2.3 将来の再評価トリガー

この制約は `wgpu`/`libloading` の watchOS/visionOS 制約（REQ-3、`06-roadmap.md`）と同様に
upstream（AccessKit・`egui`/`eframe`）の対応状況により将来変化しうる。具体的には
AccessKit が Web バックエンドを実装する、または `egui`/`eframe` が標準スクリーンリーダー連携
可能な仕組みを新設するといった変化があれば、独自レンダラ方式でも Web のアクセシビリティ対応が
成立する可能性がある。

ただし本書はこの監視を新たな定常運用として新設するものではない（
`docs/operations/upstream-watch-wgpu-libloading.md` のような専用の監視運用ドキュメントは
本タスクのスコープ外とする、§5 参照）。upstream の対応状況に変化を検知した場合は、
REQ-9 の Web 分の網羅範囲見直しを spec リポジトリ（Fandhe-AI/fandhe-multi-platform-spec、非公開）側の
検討事項としてユーザーへ報告する（`out-of-scope-tracking.md` 準拠）。

## 3. DOM 出力方式（REQ-4）による代替方針

### 3.1 ハイブリッド構成の役割分担

PoC-5 の結論（「Web は DOM 出力を採る、または独自レンダラと DOM 出力のハイブリッドとする」）を
受け、本フレームワークは画面単位で出力方式を選択できるハイブリッド構成を採用している。

| 出力方式 | `RenderMode` | 用途 | 実装 |
|---|---|---|---|
| 独自レンダラ（WASM＋WebGL2/canvas） | `Canvas`（既定値） | 性能優先の画面 | 実描画検証は `fandhe-egui-proxy`（検証専用プロキシ、`dom-output-architecture.md` §2.1）。本実装レンダラは未実装 |
| DOM 出力 | `Dom` | アクセシビリティ・SEO が重要な画面 | `fandhe-web-dom` |

画面単位の選択導線は `crates/fandhe-core/src/ui/render_mode.rs` の `RenderMode` /
`RendererLifecycle` / `ModeSwitcher` が提供する（TASK-4.4-S3）。`ModeSwitcher` は
両レンダラの排他起動・停止→起動の順序を保証する薄いディスパッチャであり、具体的な
DOM 構築・再描画ロジックは `fandhe-web-dom` 側（`RendererLifecycle` 実装）が担う。

### 3.2 代替の実証（S4 実測）

DOM 出力方式が実際にブラウザ標準の Accessibility Tree へ意味情報を公開することは、
TASK-4.4-S4（#82）で Chrome DevTools Protocol の
`Accessibility.getFullAXTree` により実測済みである。

| 観点 | PoC-5（独自レンダラ） | S4 実測（`fandhe-web-dom`: DOM 出力） |
|---|---|---|
| Accessibility Tree への意味情報公開 | **0 件**（`Canvas` と隠し `textbox` のみ） | **5 種すべてが公開**（heading・textbox・button×2・list・listitem×2） |
| 見出し構造 | 無し | `heading`（name="サンプル画面"）1 件 |
| フォーム要素の name | 隠し `textbox` のみで可視 UI との対応が取れない | `textbox`（name="メモ"）。可視ラベルと一致 |
| ボタン | 無し（`Canvas` に統合） | `button` 2 件（accessible name 付き） |
| リスト構造 | 無し | `list` 1 件 + `listitem` 2 件 |

この実測により、「Web（WASM）ではブラウザ標準のアクセシビリティツリーに要素が公開されない
という制約を前提に、DOM 出力方式での代替を明記する」という受け入れ基準（L133）を、
設計（S1）・実装（S2・S3）・検証（S4）の全段階で裏付けている。

**観測事項（`<label>` の関連付け）**: 現行実装は `<label>` が `<input>` を内包する暗黙の
関連付けであり、`dom-output-architecture.md` §2.2 の表が想定する `id`/`for` 明示とは異なる。
それでも `textbox` の accessible name（「メモ」）は正しく得られたため、本表・現行実装の修正は
行っていない。`for`/`id` 明示化の要否は `dom-output-architecture.md` §4 の未起票の改善候補として扱う。

### 3.3 実装への参照

- `crates/fandhe-web-dom`: `Node<S>` → HTML 要素マッピング（`dom-output-architecture.md`
  §2.2）・ARIA セマンティクス割当・イベント配線（§2.3）を担う DOM 出力レンダラ本体
- `crates/fandhe-core::ui::{RenderMode, RendererLifecycle, ModeSwitcher}`
  （`render_mode.rs`）: 画面単位の出力方式選択導線
- 詳細設計は `docs/architecture/dom-output-architecture.md`（TASK-4.4-S1）を正とする。
  本書は制約と代替方針の要約・整合確認に専念し、Node→HTML マッピング表やイベント配線の
  詳細は重複記載しない。

### 3.4 代替方式の限界

- **全再構築方式**: S2 実装は状態更新のたびに画面ルート要素配下を全再構築する方式であり、
  差分更新（変更のあった `Node` に対応する DOM 要素のみを更新する仮想 DOM 相当の機構）は
  スコープ外・将来課題である（`dom-output-architecture.md` §2.4）。300 項目規模のリストなど
  高頻度更新画面でのスクロール位置保持・再描画コストは未検証である。
- **独自レンダラ画面のアクセシビリティは Web では引き続き提供されない**: `RenderMode::Canvas`
  を選んだ画面は、§2 の構造的制約により Web 上でブラウザ標準スクリーンリーダーと連携しない。
  アクセシビリティ・SEO が重要な画面では `RenderMode::Dom` を選ぶことで担保するのが本
  フレームワークの公式方針であり、画面設計時の方式選択（§3.1 の表）がこの担保の唯一の手段と
  なる。独自レンダラ側の将来対応（`web_screen_reader` 相当の高度化等）によって代替されるもの
  ではない。

## 4. REQ-9 受け入れ基準・M3 完了基準との整合確認

### 4.1 REQ-9 受け入れ基準 4 項目との対応

| 受け入れ基準（`04-requirements.md` L130〜133） | 状態 | 対応 |
|---|---|---|
| macOS で `accesskit` 経由の AX ツリー公開・VoiceOver 相当の読み上げ 1 要素以上 | PoC-1 で実証済み（8 要素）。M3 本実装（TASK-9.1、Issue #213）は実装中 | TASK-9.1 |
| iOS 実機での `accesskit` iOS アダプタによる VoiceOver 読み上げ確認 | 未検証 | M4 の実機検証タスク（TASK-9.2、Issue #240）で追跡 |
| Android で `winit` Activity 方式確定・`accesskit_android` 整合・TalkBack 読み上げ確認 | 未検証 | M4 の実機検証タスク（TASK-9.3、Issue #241）で追跡 |
| Web（WASM）の制約を前提に DOM 出力方式（REQ-4）での代替を明記 | **本書（TASK-9.4）で消化** | §2・§3 |

### 4.2 M3 完了基準との整合

`docs/spec/06-roadmap.md` L94〜113 の M3 タスク一覧（REQ-9: macOS アクセシビリティ・Web 制約
文書化、TASK-9.1・TASK-9.4 の 2 件）に TASK-9.4 が含まれており、本書の作成によりこの項目を
消化する。M3 完了基準（同 L101〜113）には TASK-9.4 固有の完了基準項目は存在せず、REQ-9 関連の
完了基準は「macOS で VoiceOver 相当の読み上げが 1 要素以上成立している（TASK-9.1）」のみである
ため、本書の作成は M3 完了基準と矛盾しない。

## 5. スコープ外（本書に含めないもの）

- AccessKit Web バックエンドの実装・調査 PoC
- `fandhe-web-dom` の機能追加（差分更新等、`dom-output-architecture.md` §2.4 の将来課題）
- TASK-9.1〜9.3（macOS 本実装・iOS/Android 実機検証、別途 Issue #240/#241 等で追跡）
- upstream（AccessKit・`egui`/`eframe`）動向の定常監視運用の新設。§2.3 のとおり将来の
  再評価が必要になった場合は `out-of-scope-tracking.md` のフローに従いユーザー承認後に
  Issue 化を検討する
