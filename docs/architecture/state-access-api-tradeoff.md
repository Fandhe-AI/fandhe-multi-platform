# 状態アクセス API トレードオフ設計判断

## 1. 目的・位置づけ

本書は TASK-13.4（`docs/spec/05-tasks.md` L424〜429、推定工数 0.5 日）の成果物であり、REQ-13
（`docs/spec/04-requirements.md` L171〜181、Must）の受け入れ基準 4「状態アクセス API の『関数ポインタ
限定（安全性優先）』と『クロージャ許容（利便性優先）』のトレードオフを設計判断として文書化する」
（L179）を満たすために作成しています。

- **前提タスク**: TASK-13.1（#28）で `ReadFn<S, T>` / `WriteFn<S, T>` を
  `crates/fandhe-core/src/state/` に定義しています。
- **spec 側の決定との関係**: 「関数ポインタ限定を採用する」という設計判断そのものは、spec の
  データ要件（`04-requirements.md` L268「関数ポインタ（`fn(&S) -> T` / `fn(&mut S, T)`）による状態
  アクセスとし、クロージャによる環境キャプチャを型システムで拒否する（PoC-6 の実装を踏襲、REQ-13
  関連）」）と TASK-13.1 の実装で確定しています。本書はその**根拠・トレードオフ比較・再考条件**を
  独立した設計判断記録として残すものであり、判断自体を覆すものではありません。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています（`crate-feature-matrix.md` /
  `responsibility-boundary.md` と同じ置き場・体裁）。spec 自体の変更はこのリポジトリでは行いません。
- **ロードマップ上の位置づけ**: `docs/spec/06-roadmap.md`（M2、L30〜58）は TASK-13.4 を REQ-13
  配下の 4 件のタスクの 1 つとして掲げています（L49「TASK-13.4（0.5日・人間・設計判断）」）。M2 の
  完了基準（L51〜55）は TASK-1.2・TASK-2.1・TASK-7.1・TASK-13.2・TASK-13.3 を明示的に列挙しており
  TASK-13.4 単独の基準行はありませんが、REQ-13 受け入れ基準 4 の充足をもって本タスクの成果物とします。
- **担当区分の扱い**: spec 上の担当は「人間（設計判断）」（`05-tasks.md` L429）です。関数ポインタ限定
  の採用自体は spec・実装で確定しているため、本書はその根拠・トレードオフの文書化を Claude Code が
  ドラフトしたものであり、採用可否そのものの最終確認は PR 上でユーザーへ依頼します。

## 2. 設計判断（結論）

状態アクセス API は**関数ポインタ限定**（`ReadFn<S, T> = fn(&S) -> T` / `WriteFn<S, T> = fn(&mut S, T)`）
を採用し、環境をキャプチャするクロージャを型システムで拒否します（PoC-6 の実装を踏襲、
`04-requirements.md` L268 で spec 決定済み、`crates/fandhe-core/src/state/read.rs` /
`write.rs` で実装）。

## 3. 選択肢の比較（トレードオフ本体）

### 案 A: 関数ポインタ限定（採用）

安全性優先の設計です。

- **環境キャプチャの遮断**: `fn` ポインタ型に限定することで、環境をキャプチャするクロージャは
  代入できません（キャプチャ付きクロージャは `fn` へ coerce 不能なため、コンパイル時に拒否されます。
  `crates/fandhe-core/src/state/read.rs` のドキュメンテーションコメントに同旨の記載あり）。これにより
  状態 `S` への暗黙の外部参照経路をコンパイル段階で遮断します。
- **AI 生成コードへの安全網（NFR-9）の方向性**: REQ-13 受け入れ基準 3 が求める「意図的に注入した
  型/参照エラー系の誤り（10 ケース以上）を Rust コンパイラがビルド段階で 90% 以上検知・拒否する」の
  実測は PoC-6 で 12/12 = 100%（`04-requirements.md` L179）です。本実装では、環境キャプチャという
  誤りの部分集合について `crates/fandhe-core/tests/compile_fail/`（9 ケース、後述）が compile-fail
  として拒否されることを固定化していますが、NFR-9 の検知率そのもの（10 ケース以上・90% 以上）の
  測定・確定は TASK-13.3（#37）のスコープです（「5. 再考条件」参照）。
- **API のシンプルさ**: `fn` ポインタは `Copy` であり、モノモーフィック（型消去なし）です。これにより
  `ReadAccessor<S, T>` / `WriteAccessor<S, T>`（`crates/fandhe-core/src/state/read.rs` /
  `write.rs`）は `derive` マクロが自動付与する `S: Debug` / `S: Clone` という不要な境界を課さずに
  手動で `Debug` / `Clone` / `Copy` を実装でき、`Node<S>`（`crates/fandhe-core/src/ui/node.rs`）側の
  設計とも一貫します。
- **利便性の一部維持**: 環境をキャプチャしないクロージャ（非キャプチャクロージャ）は `fn` へ
  coerce 可能なため、`ReadAccessor::new(|state| state.count)` のようなクロージャ記法での呼び出しは
  引き続き可能です（`crates/fandhe-core/src/state/read.rs` /
  `write.rs` の `new_accepts_non_capturing_closure_coerced_to_fn_pointer` テストで確認済み）。案 A は
  「クロージャ記法を全面禁止する」のではなく、「環境キャプチャという副作用の経路のみを遮断する」
  設計である点に注意が必要です。

### 案 B: クロージャ許容（`F: Fn(&S) -> T` ジェネリクス または `Box<dyn Fn(&S) -> T>`）

利便性優先の設計です。

- **得られる利便性**: 環境キャプチャ・部分適用が可能になり、呼び出し側でローカル変数を束縛した
  ハンドラを直接渡せるようになります。
- **失われる安全網**: 状態 `S` への暗黙の外部参照経路が型システム上で開いてしまい、AI が生成しがちな
  「画面ローカル変数のキャプチャ」誤り（例: 別画面の状態や一時変数を誤って参照するコード）を
  コンパイル段階で検知できなくなります。これは REQ-13 が要求する安全網（NFR-9、受け入れ基準 3）の
  前提を損ないます。
- **API 複雑化のコスト**: ジェネリクス化（`F: Fn(&S) -> T`）は `Screen<S>` / `Node<S>` の型シグネチャに
  型パラメータ `F` を追加で持ち込み、`Node<S>` の列挙体としての単純さ（現状は関数ポインタ 1 つの
  フィールドのみ）を崩します。`dyn Fn` 化した場合は動的ディスパッチのオーバーヘッドに加え、
  `Copy` を失い（ヒープ確保・クローンコストが生じる）、`ReadAccessor` / `WriteAccessor` の
  `Debug` / `Clone` / `Copy` 実装方針（案 A の根拠）とも整合しなくなります。

### 中間案（参考・不採用）: ZST（キャプチャなし）クロージャのみを許容する型制約

理論上は「環境をキャプチャしない（ゼロサイズ型の）クロージャのみ許容する」型制約も考えられますが、
現時点では採用していません。案 A（`fn` ポインタ限定）において非キャプチャクロージャは既に `fn` へ
coerce できるため、この中間案が案 A に対して追加で提供する利便性はほぼありません。一方で
専用のトレイト境界・マーカー型の導入は API の複雑化のみをもたらすため、現時点では見送ります。

## 4. 実装上の根拠（現状のコードベースとの対応）

- **型定義・アクセサ**: `crates/fandhe-core/src/state/read.rs`（`ReadFn<S, T>` / `ReadAccessor<S, T>`）、
  `crates/fandhe-core/src/state/write.rs`（`WriteFn<S, T>` / `WriteAccessor<S, T>`）。両モジュールの
  入口である `crates/fandhe-core/src/state/mod.rs` が状態アクセス層全体の位置づけを説明しています。
- **compile-fail 網羅検証（環境キャプチャ拒否）**: `crates/fandhe-core/tests/compile_fail/` 配下に
  9 件の環境キャプチャ拒否ケース（`read_accessor_borrow_capture.rs` / `read_accessor_boxed_fn.rs` /
  `read_accessor_move_capture.rs` / `read_fn_type_annotation_capture.rs` / `screen_button_capture.rs` /
  `screen_list_capture.rs` / `screen_text_field_capture.rs` / `text_field_get_capture.rs` /
  `text_field_set_capture.rs`）を trybuild ベースで固定化しています（TASK-13.1-S3・#31）。補助の
  `pass/` ディレクトリ（`named_function.rs` / `non_capturing_closure.rs`）は「正しい使い方はコンパイル
  が通る」ことを確認する対照ケースです。この 9 件は環境キャプチャ拒否という設計判断の効果を示す
  代表例であり、REQ-13 受け入れ基準 3（NFR-9）が求める「10 ケース以上・90% 以上」の網羅測定は
  TASK-13.3（#37）が別途担当します（書き込み側 `WriteAccessor` 向けのケース追補は
  `state/mod.rs` に記載）。
- **PoC-6 整合テスト**: `crates/fandhe-core/tests/poc6_alignment.rs`（TASK-13.1-S4・#32）が、
  `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs` の PoC-6 実装（`fn(&S) -> T` / `fn(&mut S, T)` の
  関数ポインタによる状態アクセス）と本実装の API 面・挙動の整合を確認しています。
- **AI 向けメタ情報**: `docs/ai/SCHEMA.md`（TASK-13.2）が、関数ポインタ限定という制約自体を
  AI エージェント向けの型制約セクションとして明文化しています。

## 5. 再考条件（設計見直しトリガー）

- **安全網検知率が基準を下回った場合**: TASK-13.3（#37）が測定する「意図的に
  注入した型/参照エラー系の誤り（10 ケース以上）の検知率」が 90% を下回った場合、
  `docs/spec/06-roadmap.md` の M2 リスク欄（L57）が定める通り、DSL の型制約強化を優先して検討します。
  本書は TASK-13.3 の確定値を先取りするものではありません。
- **利便性起因の課題が M3 以降で顕在化した場合**: ウィジェット実装が広がる M3 以降（TASK-8.3 系）で
  「関数ポインタ限定によりどうしても表現できない利用パターン」が具体的に見つかった場合は、案 B・
  中間案を含めて再検討します。
- **spec 変更に相当する再判断**: 上記いずれかの結果として関数ポインタ限定という決定自体を覆す
  必要が生じた場合、それは `04-requirements.md` L268 のデータ要件の変更に相当するため、本リポジトリ
  では判断せず、spec リポジトリ（Fandhe-AI/fandhe-multi-platform-spec、非公開）側の検討事項としてユーザーへ
  報告します（`.claude/rules/out-of-scope-tracking.md` 準拠）。

## 6. REQ-13 受け入れ基準・M2 完了基準との整合確認

| 基準 | 内容 | 本書での対応 |
|------|------|-------------|
| REQ-13 受け入れ基準 4（`04-requirements.md` L179） | 状態アクセス API の「関数ポインタ限定」と「クロージャ許容」のトレードオフを設計判断として文書化する | 本書の「2. 設計判断（結論）」「3. 選択肢の比較」で対応（**本書のスコープ**） |
| REQ-13 受け入れ基準 1〜3（`04-requirements.md` L176〜178） | コンパイル成功率・メタ情報なし条件との差分・安全網検知率 | TASK-13.2（#34〜36）・TASK-13.3（#37）のスコープ（本書のスコープ外） |
| `06-roadmap.md` M2（L49） | TASK-13.4（0.5 日・人間・設計判断） | 本書の作成をもって成果物とする |
| `06-roadmap.md` M2 完了基準（L51〜55） | TASK-13.3 の安全網検知率 90% 以上確認等を列挙。TASK-13.4 単独の基準行はなし | 本書は TASK-13.3 の確定値を先取りしない |

## 7. 出典

- `docs/spec/04-requirements.md` L171〜181（REQ-13）、L268（データ要件・状態アクセス）
- `docs/spec/05-tasks.md` L424〜429（TASK-13.4）
- `docs/spec/06-roadmap.md` L30〜58（M2）
- `docs/spec/03-poc/ai-optimization/dsl/src/lib.rs`（PoC-6 実装）
- `crates/fandhe-core/src/state/mod.rs` / `read.rs` / `write.rs`（本実装、TASK-13.1）
- `crates/fandhe-core/tests/compile_fail/`（環境キャプチャ拒否の網羅検証、TASK-13.1-S3・#31）
- `crates/fandhe-core/tests/poc6_alignment.rs`（PoC-6 整合確認、TASK-13.1-S4・#32）
- `docs/ai/SCHEMA.md`（AI 向けメタ情報、TASK-13.2）
- `docs/architecture/responsibility-boundary.md`（本書と同一の置き場・体裁）
