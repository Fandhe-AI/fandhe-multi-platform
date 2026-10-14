# Web 描画バックエンド方針（WebGL2 固定）

## 1. 目的・位置づけ

本書は TASK-4.3（`docs/spec/06-roadmap.md` L80「TASK-4.3（0.5日・Claude Code・WebGL2 固定）」、
REQ-4（`docs/spec/04-requirements.md` L66〜75）の受け入れ基準「WebGPU は当面 WebGL2 固定（`glow`）とし、
WebGPU→WebGL2 の自動フォールバックは実装しない」（L75、条件4）の実装として作成しています。

- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています（`state-access-api-tradeoff.md` /
  `responsibility-boundary.md` と同じ置き場・体裁）。spec 自体の変更はこのリポジトリでは行いません。
- **対象クレート**: `crates/fandhe-egui-proxy`（TASK-1.2 の実描画検証用プロキシ。本実装レンダラでは
  ないが、wasm32 分岐のバックエンド選択方針は本実装レンダラへも踏襲する前提）。

## 2. 決定

Web（wasm32-unknown-unknown）ターゲットの描画バックエンドは **`glow`（WebGL2）に固定**します。

- WebGPU（`eframe`/`wgpu` の `"wgpu"` feature）は wasm32 分岐へ追加しません
  （`crates/fandhe-egui-proxy/Cargo.toml` の `cfg(target_arch = "wasm32")` 依存は `"glow"` のみ）。
- WebGPU→WebGL2 の自動フォールバックは実装しません。
- `eframe::WebOptions::default()` の既定値 `WebGlContextOption::BestFirst`
  （WebGL2 失敗時に WebGL1 へ暗黙フォールバックする）も採用せず、
  `WebGlContextOption::WebGl2` を明示的に指定して WebGL1 への暗黙フォールバックも無効化します
  （`crates/fandhe-egui-proxy/src/web.rs` の `WebOptions` 構築箇所）。

結果として、WebGL2 コンテキストの生成に失敗する環境（WebGL2 非対応ブラウザ等）では、
黙って劣化した描画（WebGL1）を出さず、`eframe::WebRunner::start()` がエラーを返し
`web.rs` の既存エラーハンドリング（`web_sys::console::error_1` への出力）で
明示的に起動失敗を通知します。

## 3. 根拠

- **PoC-5 の panic 実験**（`docs/spec/03-poc/web-wasm/README.md` 検証方法4）: `eframe` の `"wgpu"`
  feature（WebGPU）へ一時的に切り替えて検証したところ、`navigator.gpu` が存在しない
  WebGPU 非公開環境で自動フォールバックが機能せず、`unreachable` トラップによる
  `panic!` で起動そのものが失敗することを確認しました。
- **上流の既知報告**: `gfx-rs/wgpu` Issue #6166 でも同種の報告があり、`wgpu` 側の
  wasm32 における WebGPU→WebGL2 自動フォールバックが upstream で未成熟であることが
  裏付けられています。
- **`WebGlContextOption::BestFirst` の暗黙フォールバック**: `eframe` 0.33.3
  （`src/web/web_painter_glow.rs`）の実装を確認したところ、既定値 `BestFirst` は
  WebGL2 コンテキスト生成に失敗した場合 WebGL1 へ暗黙的にフォールバックする挙動を持ちます。
  WebGPU の自動フォールバックを避けても、この既定値のままでは「自動フォールバックを
  実装しない」という条件4の趣旨（劣化を黙って許容しない）に反するため、明示的に
  `WebGl2` を指定してこの経路も塞ぎます。

## 4. 影響

- WebGL2 非対応ブラウザ・環境では、描画が劣化した状態で継続するのではなく、
  起動時に明示的なエラー（console 出力）で失敗します。これはフェイルセーフ設計
  （`.claude/rules/security.md` の「サイレント障害を作らない」方針）に沿った挙動です。
- Chrome・Safari・Firefox の 3 ブラウザでの実描画確認は本タスクのスコープ外であり、
  **TASK-4.2**（`docs/spec/06-roadmap.md` L79「TASK-4.2（1日・共同・3ブラウザ確認）」）の
  担当範囲です。本タスクでは、既存の Chrome headless 検証で WebGL2 固定後も実描画が
  継続することのみを確認しています。
- Safari 対策（canvas CSS サイズ明示、TASK-4.1・#75）は別タスクであり、本書・本タスクの
  変更対象（`web.rs` の `WebOptions` 構築 1 箇所）とは競合しません。

## 5. REQ-4 受け入れ基準・M3 完了基準との整合確認

| 基準 | 出典 | 確認結果 |
|---|---|---|
| 「WebGPU は当面 WebGL2 固定（`glow`）とし、WebGPU→WebGL2 の自動フォールバックは実装しない」 | `04-requirements.md` L75（条件4） | **充足**。2 章の決定のとおり、WebGPU feature 自体を wasm32 分岐へ追加せず、`WebGlContextOption::WebGl2` 固定により WebGL1 への暗黙フォールバックも無効化した |
| 「WebGL2 固定実装（自動フォールバック非実装）が文書化され、Chrome・Safari・Firefox の 3 ブラウザで実描画を確認している」 | `06-roadmap.md` L106（M3 完了基準、条件4） | **本タスクでは前半（WebGL2 固定実装の文書化）のみ充足**。3 ブラウザでの実描画確認は TASK-4.2 のスコープであり、M3 完了基準全体の充足は TASK-4.2 完了後に判定される |

## 6. 再評価条件

以下のいずれかが確認された場合、本方針（WebGL2 固定・WebGPU 非採用）を再評価します。

- `wgpu`/`eframe` upstream で WebGPU→WebGL2 の自動フォールバックが安定動作するよう修正された
  （`gfx-rs/wgpu` Issue #6166 の解消状況を含む）。
- 対象ブラウザ（Chrome・Safari・Firefox）での WebGPU 公開率が実用上無視できる水準まで向上した。
- REQ-4 の要件自体が spec 側（`Fandhe-AI/fandhe-multi-platform-spec`、非公開）で見直された。

再評価は `.claude/rules/delegation.md` の外部仕様調査（`reference-researcher`）を通じて
upstream の対応状況を定期確認する運用（`docs/spec/06-roadmap.md` L258 のリスク管理項目）に含めます。
