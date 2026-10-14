# 基盤クレート機能マトリクス（本実装版）

## 1. 目的・位置づけ

本書は TASK-8.1（`docs/spec/05-tasks.md` L253〜258）の成果物であり、REQ-8（`docs/spec/04-requirements.md`
L114〜127）の受け入れ基準 1「基盤クレートのカバー範囲・欠落範囲が機能マトリクスとして文書化されている」を満たすために、
PoC-7（`docs/spec/03-poc/crate-ecosystem/README.md`、実施日 2026-07-08）の機能マトリクスを本実装フェーズ向けに
更新したものです。

- **参照元**: PoC-7 の機能マトリクス（`docs/spec/03-poc/crate-ecosystem/README.md` L27〜36）。本書は PoC-7 の
  スナップショットを起点とし、2026-07 時点で再調査した変化点を追記する形で更新しています。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側（
  `docs/architecture/`）の新規ドキュメントとして作成しています。spec 自体の変更はこのリポジトリでは行いません。
- **後続タスクからの参照**:
  - TASK-8.2（新規開発工数見積もりの精緻化・実測照合、`05-tasks.md` L260〜266、M5）は本書の欠落領域の整理を
    照合ベースラインとして使用します。**工数の実測値照合そのものは本書のスコープ外**であり、TASK-8.2 側で
    別途実施します。
  - TASK-8.6（watchOS/visionOS 向け `winit`/`accesskit` 制約の REQ-3 反映、`05-tasks.md` L288〜295、M3）は
    本書の「5. watchOS/visionOS 制約セクション」を入力として、REQ-3 の実装方針メモを別途作成します。
    **REQ-3 実装方針の確定そのものは本書のスコープ外**です。

## 2. 基盤クレート機能マトリクス（更新版）

以下は PoC-7 記載の 6 クレート（`wgpu`・`vello`・`taffy`・`cosmic-text`・`winit`・`accesskit`）について、
2026-07 時点で一次情報（GitHub・crates.io・docs.rs・公式ブログ）を再調査し、PoC-7 時点からの変化点を
「変化点」列に整理したものです。一次情報で裏付けが取れなかった項目は PoC-7 記載値をそのまま転記し、
「要再検証」を付しています（誤った最新情報の断定を避けるため、安全側に倒しています）。

| クレート | 役割 | カバーする機能（PoC-7 時点） | 主な欠落・限界（PoC-7 時点） | プラットフォーム対応（PoC-7 時点） | 変化点（2026-07 再調査） | ライセンス |
|---------|------|--------------------------|--------------------------|------------------------------|------------------------|-----------|
| `wgpu` | GPU 描画抽象化（Vulkan/Metal/DX12/WebGPU/WebGL2） | 全プラットフォーム共通のピクセル描画パイプライン | 2D 図形・パス描画そのものは持たない。watchOS/visionOS 実機での動作実績は限定的 | iOS・Android・Web（WASM）・デスクトップで実績あり。watchOS/visionOS は未検証 | GitHub リリースページ上で v29.0.1（2026-03-26 付、Metal バックエンドのバッファ制限修正・`max_inter_stage_shader_variables` の制限比較ロジック修正を含む）を確認した。一部の docs.rs 系ページでは v30.0.0 の記載もあり、正確な最新安定版番号は要再検証。watchOS/visionOS 対応の実績拡大を示す一次情報は本調査では確認できず、PoC-7 時点の判定（未検証）を維持する | MIT/Apache-2.0 |
| `vello` | 2D GPU コンピュートレンダラ（`wgpu` 上に構築） | パスベースの高性能 2D 描画 | 2025 年 12 月時点でアルファ状態。ぼかし・フィルタ効果や conflation artifact が未完成 | wgpu 依存のためほぼ同じ対応範囲だが実績はデスクトップ・Web 中心 | Linebender の月次進捗報告（2025 年 12 月時点のブログ）により、Vello CPU 実装は「広く実用可能」な水準に達しつつあるが、Vello（GPU コンピュート版）・Vello Hybrid を含め全体としては 2026-07 時点でも安定版（1.0）未達のアルファ状態が継続していることを確認した。API 安定性保証は依然としてなく、レンダラ間で一貫した API への再設計を意図している段階 | MIT/Apache-2.0 |
| `taffy` | レイアウトエンジン | CSS Block・Flexbox・CSS Grid の各アルゴリズムを実装 | Float 以外の高度な CSS レイアウトは範囲外 | プラットフォーム非依存 | CSS Grid 実装はほぼ機能完成し、named grid lines/areas・`direction`（RTL レイアウト）対応が Block/Flexbox/Grid の各モードに追加されたことを GitHub 上で確認した。ただし安定版リリース前の追加テスト・検証が必要な段階が継続しており、2026 年に入ってからも Grid 関連の issue（例: 2026-03-01 起票分）が継続的に処理されている。PoC-7 時点の「範囲外」判定（多段カラム等）に変更はない | MIT |
| `cosmic-text` | テキストシェーピング・レイアウト・描画・編集 | HarfRust によるシェーピング、双方向テキスト、合字、絵文字、フォールバック、ラスタライズ、テキスト編集 | IME（変換確定・候補ウィンドウ）統合は明記なし。iOS/Android/Web 上の実績記述は薄い | Linux・macOS・Windows は full feature。iOS/Android/Web は要検証 | `cosmic-text` を採用する COSMIC デスクトップ環境側（COSMIC Edit）で 2026 年 5 月リリース分（v1.0.12 相当）にて「テキストボックスへの最小限の IME サポート」が入ったことを確認したが、これは上位アプリケーション（COSMIC Edit）側の統合であり、`cosmic-text` クレート自体の公式 IME API 追加を示す一次情報は本調査では確認できなかった。次期メジャー（COSMIC Epoch 2.0）で IME・fcitx 対応の強化が予告されている。クレート本体の IME 統合状況は PoC-7 時点の判定（自前実装が必要な可能性が高い）を維持し、要再検証とする | MIT/Apache-2.0 |
| `winit` | ウィンドウ生成・入力イベント | クロスプラットフォームのウィンドウ/イベントループ抽象化。UIKit 経由で iOS・tvOS・watchOS・visionOS への対応が進行中 | watchOS/visionOS 対応は「進行中」。IME 対応も Issue #1497 として長期 Tracking issue が残っている | iOS・Android・Web（WASM）は実用段階。watchOS/visionOS は開発中 | Issue #1497（IME/composition support の Tracking issue）は 2026-07 時点でも open のままであることを GitHub 上で確認した。UIKit バックエンド（iOS/tvOS/watchOS/visionOS を含む）が本 issue のスコープに含まれる旨の記載を確認したが、watchOS/visionOS 個別の解決状況を示す一次情報は本調査では確認できず、PoC-7 時点の判定（開発中・IME 未解決）を維持する | MIT/Apache-2.0 |
| `accesskit` | アクセシビリティ橋渡し | プラットフォームのアクセシビリティ API へアクセシビリティツリーを公開する共通抽象を提供 | watchOS/visionOS 向けアダプタは未確認。Web（WASM）向けの専用アダプタはなし | Windows・macOS・Linux・Android・iOS に公式アダプタあり。watchOS/visionOS・Web は要追加実装 | 本調査では `accesskit` に関する 2026-07 時点の新規一次情報（リリースノート・watchOS/visionOS アダプタ追加等）を確認できなかった。PoC-7 記載値（2026 年 5 月時点の更新を確認、iOS/Android/Windows/macOS/Linux に公式アダプタあり、watchOS/visionOS・Web は要追加実装）をそのまま転記し、要再検証とする | MIT/Apache-2.0 |

**判定**: 基盤 4 クレート＋関連 2 クレートについて、PoC-7 のカバー範囲・欠落範囲の判定を 2026-07 時点で
再確認した。一次情報で明確な進展が確認できたのは `taffy`（CSS Grid 実装の進捗）・`vello`（Vello CPU の実用性向上）
の 2 点であり、`wgpu`・`winit`・`accesskit`・`cosmic-text`（クレート本体の IME 対応）については PoC-7 時点からの
大きな判定変更を裏付ける一次情報は確認できなかった（要再検証のまま維持）。欠落領域の整理（「ウィジェットセット」
「デザイントークン」「IME 統合」「アクセシビリティ橋渡しの拡張」「ストア提出パイプライン」の 5 領域）は
PoC-7 の判定を維持する。

## 3. M2 段階の実装形態との対応

- `docs/spec/06-roadmap.md`（M2、L30〜58）が定める通り、M2 時点では macOS 向けの実描画は基盤クレート直接統合
  ではなく `egui`/`eframe` プロキシ経由（TASK-1.2）で行う段階構成であり、本書のマトリクスが対象とする
  `wgpu`/`vello`/`taffy`/`cosmic-text`/`winit`/`accesskit` への直接統合は M3（3 プラットフォーム統合・
  基盤クレート新規開発、TASK-8.3〜8.5 等）以降に予定されています。
- 本リポジトリの現状（`crates/fandhe-core`、TASK-1.1 完了分）は `Screen<S>`/`Node<S>` のコア型定義に留まり、
  本書が扱う基盤クレートへの依存はまだ導入されていません。本書は導入前の事前整理として位置づけます。

## 4. 欠落 5 領域 → 実装タスク対応表

PoC-7（`docs/spec/03-poc/crate-ecosystem/README.md` L40〜48）が特定した欠落 5 領域と、担当タスクの対応は
以下の通りです。**工数の実測照合・精緻化は TASK-8.2 のスコープであり、本書では PoC-7 の概算工数をそのまま
参照値として記載するに留めます**（新たな実測値は算出しません）。

| 欠落領域 | 担当タスク | PoC-7 概算工数（人日、実測未照合） | 備考 |
|---------|-----------|------------------------------|------|
| ウィジェットセット | TASK-8.3（`05-tasks.md` L267〜273） | 40〜55 | クリティカルパス上のタスク（`TASK-1.1`→`TASK-8.3` で 33 人日、`05-tasks.md` L618・L622） |
| デザイントークン／スタイルシステム | TASK-8.4（`05-tasks.md` L274〜280） | 8〜12 | TASK-8.3・TASK-8.5 と相互依存なく並行実施可能（`06-roadmap.md` 冒頭補足） |
| IME 統合 | TASK-8.5（`05-tasks.md` L281〜287） | 10〜15 | `winit` Issue #1497 が本調査時点でも open のままである点を踏まえ、上記 2 と同様に個別実装が前提 |
| アクセシビリティ橋渡しの拡張 | TASK-9.1〜9.3（`05-tasks.md` L297〜317） | 8〜12 | macOS（TASK-9.1）・iOS 実機検証（TASK-9.2、環境整備後）・Android 検証（TASK-9.3、環境整備後）に分割されている |
| ストア提出パイプライン | TASK-11.1〜12.3（`05-tasks.md` L343〜393） | 10〜13 | 開発署名まで（TASK-11.1〜11.4、M3）と実配布署名以降（TASK-12.1〜12.4、M5）に分割されている |
| 統合・糊付け・テスト | 各タスクに按分（独立タスクなし） | 15〜20 | PoC-7 は「過小評価されやすい」工数として主要機能合計の 20〜25% を計上している旨を明記（`03-poc/crate-ecosystem/README.md` L47、L95） |

TASK-8.2 はこの対応表を照合ベースラインとして、上記タスク群の実測工数と PoC-7 概算工数（91〜127 人日）との
差分を記録します（`06-roadmap.md` M5 完了基準、L165〜170）。

## 5. watchOS/visionOS 制約セクション

TASK-8.6（`05-tasks.md` L288〜295、前提タスク: TASK-3.1・TASK-8.1）が REQ-3 へ反映する際の入力として、
`winit`/`accesskit` の watchOS/visionOS 対応状況を独立節としてまとめます。**REQ-3 の実装方針そのものの確定は
TASK-8.6 のスコープであり、本書では現状整理のみを行います。**

- **`winit`**: UIKit バックエンド経由で iOS・tvOS・watchOS・visionOS への対応が進行中であり、2026-07 時点でも
  安定版としての完了を示す一次情報は確認できていません。IME 対応の Tracking issue（#1497）も open のままです。
- **`accesskit`**: watchOS/visionOS 向けの公式アダプタは PoC-7 時点で未確認であり、本調査でも新規の一次情報は
  確認できませんでした（要再検証）。
- **示唆**: 上記 2 クレートとも watchOS/visionOS 対応が発展途上であるという PoC-7 の判定は、本調査時点でも
  覆す一次情報が見つからず維持されます。TASK-8.6 では、この制約を踏まえて REQ-3（`docs/spec/04-requirements.md`
  該当節）の網羅範囲・優先度への反映方針を別途検討する必要があります。
- **反映先（TASK-8.6 成果物）**: 上記制約を踏まえた REQ-3 の網羅範囲・優先度への反映方針は
  `docs/architecture/req3-implementation-policy.md`（Issue #212）で確定しています。

## 6. REQ-8 受け入れ基準・M2 完了基準との整合確認

| 基準 | 内容 | 本書での対応 |
|------|------|-------------|
| REQ-8 受け入れ基準 1 | 基盤クレートのカバー範囲・欠落範囲（5 領域）が機能マトリクスとして文書化されている | 本書の「2. 基盤クレート機能マトリクス（更新版）」で対応（**本書のスコープ**） |
| REQ-8 受け入れ基準 2 | 欠落範囲の新規開発工数見積もりを、Phase 5 のタスク分解時に実測値と照合し精緻化する | TASK-8.2 のスコープ（本書は「4. 欠落 5 領域 → 実装タスク対応表」で照合ベースラインのみ提供） |
| REQ-8 受け入れ基準 3 | watchOS/visionOS 向けの `winit`/`accesskit` 対応が発展途上である制約を REQ-3 へ反映する | TASK-8.6 のスコープ（本書は「5. watchOS/visionOS 制約セクション」で入力のみ提供） |
| `06-roadmap.md` M2 完了基準 | REQ-8: 基盤クレート機能マトリクス更新（1 件・1 日、Must、TASK-8.1） | 本書の作成をもって TASK-8.1 の成果物とする |

## 7. 出典

- [wgpu/CHANGELOG.md - GitHub](https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md)
- [wgpu - crates.io](https://crates.io/crates/wgpu)
- [wgpu 30.0.0 - Docs.rs](https://docs.rs/crate/wgpu/latest)
- [GitHub - linebender/vello](https://github.com/linebender/vello)
- [Linebender in December 2025 - Linebender](https://linebender.org/blog/tmil-24/)
- [Linebender in July 2025 - Linebender](https://linebender.org/blog/tmil-19/)
- [GitHub - DioxusLabs/taffy](https://github.com/DioxusLabs/taffy)
- [taffy/README.md - GitHub](https://github.com/DioxusLabs/taffy/blob/main/README.md)
- [Support CSS Grid - Issue #204 - DioxusLabs/taffy](https://github.com/DioxusLabs/taffy/issues/204)
- [Releases - pop-os/cosmic-text - GitHub](https://github.com/pop-os/cosmic-text/releases)
- [COSMIC 1.0.12 Desktop Brings Improvements to Files, Terminal, and More - 9to5Linux](https://9to5linux.com/cosmic-1-0-12-desktop-brings-improvements-to-files-terminal-and-more)
- [Tracking issue for IME / composition support - winit Issue #1497 - GitHub](https://github.com/rust-windowing/winit/issues/1497)
- [winit - crates.io](https://crates.io/crates/winit)
- [accesskit - crates.io](https://crates.io/crates/accesskit)（本調査では新規更新情報は確認できず、PoC-7 記載を維持）
- 本書の元となった PoC-7 の出典一覧は `docs/spec/03-poc/crate-ecosystem/README.md` の「出典」節（L111〜143）を参照
- RustSec advisory（`https://rustsec.org`）: `wgpu`/`vello`/`taffy`/`cosmic-text`/`winit`/`accesskit` を対象に
  検索したが、本調査時点で該当する勧告は確認できなかった（要再検証。確認できなかったことは脆弱性が存在しない
  ことの証明ではない）
