# Web canvas の CSS サイズ明示規約（TASK-4.1）

## 1. 目的・位置づけ

本書は TASK-4.1（`docs/spec/05-tasks.md`）・REQ-4（`docs/spec/04-requirements.md` L66〜75）・
`06-roadmap.md` M3 の Conditional Go 条件 4（Issue #75）に対応する成果物です。
Web（wasm32-unknown-unknown）ターゲットで `egui`/`eframe` を用いる際、canvas 要素への
CSS サイズ明示が Safari 対策として必須であることを、リポジトリの実装規約として明文化します。

- `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています。
- 前提: PoC-5（`docs/spec/03-poc/web-wasm/README.md` 発見事項1）、TASK-1.2（PR #306。
  `crates/fandhe-egui-proxy` の Web エントリ）。

## 2. 背景（PoC-5 発見事項・バグの機序）

PoC-5 の Safari 26.5 実描画検証で、`canvas` 要素に CSS 上の明示サイズ指定が無い場合に
以下の無限フィードバックループが発生し、描画不能になることを確認しました。

1. `eframe`（`egui-winit` 経由）は canvas の表示サイズ変化を `ResizeObserver` で監視し、
   `devicePixelRatio` を乗じた値を `canvas.width`/`canvas.height`（描画バッファの
   backing store サイズ）へ書き戻す。
2. canvas に CSS サイズ指定が無いと、ブラウザは canvas 自身の `width`/`height` **属性値**を
   レイアウトサイズとして扱う（CSS 内在サイズの仕様上の既定動作）。
3. `ResizeObserver` が `canvas.width`/`height` の書き換えをそれ自体のレイアウト変化として
   再度検知し、`devicePixelRatio`（Safari では 2 の環境が多い）を再度乗じてしまう。
4. 2〜3 が繰り返され、`canvas.width`/`height` が指数的に増大し、最終的に
   `2^26`（67,108,864）に達してブラウザの canvas サイズ上限に抵触し、描画不能になる。

これは egui 本体の既知 Issue（`emilk/egui` #4699「Web canvas resize loop」）と同種の事象です。
この現象は Chrome では再現せず Safari で再現します（REQ-4 受け入れ基準 3 項目目に対応）。
PoC-5 は対策として
`canvas` 要素に `style="width:...px;height:...px;"` を明示することを確認済みです
（`docs/spec/03-poc/web-wasm/README.md` 発見事項1）。

## 3. 規約本文

**Web ターゲットに埋め込む `canvas` 要素は、必ず `style` 属性（インライン CSS）で
`width`/`height` を明示する。**

- CSS サイズ明示があると、ブラウザは canvas の内在サイズ（`width`/`height` 属性値）ではなく
  CSS サイズをレイアウトの基準にするため、`ResizeObserver` が「canvas 属性値の書き換え」を
  独立したレイアウト変化として再検知しなくなり、フィードバックループが断ち切られる。
- CSS のピクセル値は canvas の `width`/`height` **属性値**（初期の論理サイズ）と一致させる。
  実際の描画バッファサイズ（backing store）は `devicePixelRatio` を乗じた値を `eframe` 側が
  設定するため、CSS 側は論理サイズのみを指定すればよい。
- **インライン `style` 属性に限定する理由**: `HTMLElement.style`（`CSSStyleDeclaration`）は
  要素のインラインスタイルのみを反映する API であり、`<style>` ブロックや外部スタイルシートの
  ルールは反映しない。4 章の Rust 側ガードはこの `style` プロパティ経由でしか判定できないため、
  `<style>` ブロック・外部スタイルシートでの指定では規約遵守を機械的に確認できず、ガードが
  「未設定」と誤判定してインライン値で上書きしてしまう（レスポンシブに `width: 100%` 等を
  スタイルシートで指定する将来のホストページを、意図せず固定 px に固定してしまう懸念もある）。
  そのため本規約は明示的にインライン `style` 属性のみを対象とする。

### 二重化（HTML 側 + Rust ブートストラップ側）

規約は 2 箇所で強制する。

1. **ホストページ側（HTML）**: `crates/fandhe-egui-proxy/web/index.html` の
   `#fandhe-proxy-canvas` へ `style="width:...px;height:...px"` を明示する（本来の対策
   そのもの、PoC-5 で確認済みの形式と一致）。
2. **Rust ブートストラップ側（構造的ガード）**: `crates/fandhe-egui-proxy/src/web.rs` の
   `ensure_canvas_css_size` 関数。`WebRunner.start()` を呼ぶ前に必ず通し、インラインスタイルの
   `width`/`height` が未設定であれば canvas の `width()`/`height()` 数値属性から
   `"{n}px"` を組み立てて補完し、`console.warn` で規約違反を警告する。

二重化する理由: ホストページ（HTML）は本フレームワーク利用者が用意するファイルであり、
CLAUDE.md の日本語運用・レビュー体制の外で規約が守られない可能性がある。Rust 側に
フォールバックのガードを持たせることで、規約違反のホストページでも症状（描画不能）を
未然に防ぐ。ガードはあくまでフォールバックであり、根本対策はホストページ側のインライン
`style` 明示。

### セキュリティ上の注意（A03 インジェクション対策）

`ensure_canvas_css_size` が style へ設定する値は、canvas 自身の数値属性（`u32`）から
`format!("{n}px")` で組み立てたものに限る。外部入力・自由テキストを style や DOM へ
展開しない。

## 4. 適用範囲

- 本規約は `crates/fandhe-egui-proxy`（TASK-1.2 の実描画検証プロキシ）の Web エントリに
  現在適用されている。
- 今後の Web 実装（TASK-4.4 の DOM 出力導線、TASK-8.x のウィジェット層の Web 埋め込み等）で
  新規に canvas を DOM へ追加する場合も、本規約（インライン style 明示 + 構造的ガードの
  二重化）に従うこと。
- `.claude/agents/implement/web-builder.md` の実装規約節が本書を参照する。

## 5. 検証方法

- `scripts/web/verify-wasm-proxy.sh` は、描画後の DOM を `chromium --dump-dom` で取得し、
  canvas の `width`/`height` 属性が閾値（8192px）以下であることを検査する回帰ガードを持つ
  （TASK-4.1）。閾値超過はリサイズ無限ループの症状（`2^26` への膨張）を検出するためのもので、
  通常の CSS ピクセルサイズ（480/360 に devicePixelRatio を乗じた程度）を大きく上回る水準に
  設定している。
- Firefox は `scripts/web/verify-wasm-proxy-firefox.sh`（TASK-4.2）で実描画を確認する。Safari は実機が
  無いため人間担当の手順書のみを用意している。
