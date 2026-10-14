# REQ-3 実装方針メモ: watchOS/visionOS 向け `winit`/`accesskit` 制約の反映

## 1. 目的・位置づけ

本書は TASK-8.6（`docs/spec/05-tasks.md` L288〜294、前提タスク: TASK-3.1・TASK-8.1、
推定工数 0.5 日、Issue #212）の成果物であり、REQ-8（`docs/spec/04-requirements.md` L114〜124）
受け入れ基準 3（L121）を満たすために作成しています。

- **入力**: `docs/architecture/crate-feature-matrix.md`「2. 基盤クレート機能マトリクス
  （更新版）」（`winit`/`accesskit` の行）・「5. watchOS/visionOS 制約セクション」
  （現状整理のみで方針確定を本書へ委ねている）、PoC-2
  （`docs/spec/03-poc/watchos-visionos/README.md`）、TASK-3.1 検証（Issue #56）、
  `docs/architecture/swiftui-bridge-architecture.md`（REQ-3 の FFI 境界設計）。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、REQ-3 本文
  （`04-requirements.md`）を本リポジトリから直接編集することはできません。本書は
  `crate-feature-matrix.md` / `swiftui-bridge-architecture.md` と同じ方式で、本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして「REQ-3 への反映」を実現します。spec 本文
  の変更が必要と判断される事項は、Issue 起票ではなく spec リポジトリ
  （Fandhe-AI/fandhe-multi-platform-spec、非公開）側の検討事項として「8. スコープ外・要ユーザー報告
  事項」にまとめます。
- **本 issue の成果物**: 本書 1 本のみ（コード変更なし）。`docs/architecture/crate-feature-matrix.md`
  §5 末尾には本書への相互参照を追記します（トレーサビリティ確保のみ、§5 の内容自体は変更しません）。

## 2. 制約の整理（ベースライン固定）

`crate-feature-matrix.md`「2. 基盤クレート機能マトリクス（更新版）」（2026-07 再調査）の記載を
そのまま引用し、本書のベースラインとして固定します。新規の一次情報再調査は行わず（工数
0.5 日に収めるため）、確認日を明記した引用として扱います。

| クレート | 制約内容 | 確認日・出典 |
|---------|---------|-------------|
| `winit` | UIKit バックエンド経由で iOS・tvOS・watchOS・visionOS への対応が「進行中」。2026-07 時点でも安定版としての完了を示す一次情報は確認できていない。IME 対応の Tracking issue（#1497）は open のまま | 2026-07（`crate-feature-matrix.md` §2・§5）。出典: [winit Issue #1497](https://github.com/rust-windowing/winit/issues/1497)、[winit - crates.io](https://crates.io/crates/winit) |
| `accesskit` | watchOS/visionOS 向けの公式アダプタは未確認（公式アダプタは Windows・macOS・Linux・Android・iOS のみ）。2026-07 再調査でも新規の一次情報は確認できず、PoC-7 記載値を維持（要再検証） | 2026-07（`crate-feature-matrix.md` §2・§5）。出典: [accesskit - crates.io](https://crates.io/crates/accesskit) |

**RustSec advisory 確認**: `crate-feature-matrix.md` §7 の出典節にて `winit`/`accesskit` を
含む対象クレートで RustSec advisory（https://rustsec.org）を検索したが、2026-07 時点で該当する
勧告は確認できなかった（要再検証。確認できなかったことは脆弱性が存在しないことの証明ではない、
`crate-feature-matrix.md` の先例と同じ但し書き）。

**要再検証の明示**: 上記 2 クレートとも、`winit`/`accesskit` 本体（依存先である `wgpu`/
`libloading` とは別のクレート）の watchOS/visionOS 対応進捗を裏付ける新規の一次情報は
2026-07 時点で確認できていません。誤った前提での実装判断を避けるため、以下「3. REQ-3 の
網羅範囲への反映」「4. 優先度への反映」はこの「未解決」という現状を前提に組み立てます。

## 3. REQ-3 の網羅範囲への反映（方針確定）

### 3.1 中核の判断: 独自レンダラ系スタックを網羅範囲から除外

watchOS/visionOS では、`wgpu`/`winit`/`accesskit` を含む独自レンダラ系スタックを **REQ-3 の
網羅範囲から除外**し、SwiftUI バインドのハイブリッド構成（Rust コア＝ロジック・状態管理、
SwiftUI＝描画・イベントループ・アクセシビリティ）を**必須方式として維持・確定**します。

根拠:

- `wgpu`/`libloading` は watchOS で Metal API 自体が非提供、visionOS でもビルド不能という
  「upstream 対応でも解消し得ない、または解消未確認」の制約がすでに REQ-3 本文で確定済み
  （`04-requirements.md` L55、`docs/operations/upstream-watch-wgpu-libloading.md`）。
- `winit`（ウィンドウ/イベントループ）・`accesskit`（アクセシビリティ橋渡し）は、`wgpu` の
  描画層とは独立したクレートですが、いずれも watchOS/visionOS 向けの成熟した対応を
  2026-07 時点で確認できていません（本書「2. 制約の整理」）。すなわち、仮に `wgpu` の描画層
  制約のみを別解決策（例: 将来的な Metal 提供）で回避できたとしても、ウィンドウ管理・
  アクセシビリティ公開の経路がなお欠けており、独自レンダラ系スタック全体としての
  watchOS/visionOS 対応は成立しません。
- したがって、`wgpu` 単体の制約解消を待つのではなく、`winit`/`accesskit` を含むスタック全体を
  対象外とする判断が、REQ-3 の既存確定事項（SwiftUI バインドハイブリッド構成の採用）と整合します。

### 3.2 責務分界の明文化（`winit`/`accesskit` の代替先）

`swiftui-bridge-architecture.md` の FFI 境界設計と接続する形で、以下の責務分界を明文化します。

| 独自レンダラ系での担当 | watchOS/visionOS での代替 | 理由 |
|----------------------|--------------------------|------|
| `winit`（ウィンドウ生成・イベントループ） | SwiftUI / WatchKit のネイティブ機構（`WindowGroup`・`RunLoop` 等） | `winit` の UIKit バックエンドは watchOS/visionOS 対応が進行中で未完了。SwiftUI バインド方式では Rust 側はロジック・状態管理のみを担い、イベントループそのものは SwiftUI/WatchKit 側に委譲する（`swiftui-bridge-architecture.md` 2 節の設計判断） |
| `accesskit`（アクセシビリティ橋渡し） | SwiftUI 標準のアクセシビリティ API（VoiceOver 等、`.accessibilityLabel` 等の修飾子） | `accesskit` の watchOS/visionOS 向け公式アダプタは未確認。SwiftUI ネイティブ UI 要素はアクセシビリティ API を標準で備えるため、`accesskit` を経由せずとも REQ-9（アクセシビリティ対応）相当の到達点をハイブリッド構成内で満たせる |

この分界は `swiftui-bridge-architecture.md`「3.1 ブリッジ層アーキテクチャ」が定める
「Rust コア ⇔ SwiftUI」の FFI 境界（`fandhe-swiftui-bridge` クレート、JSON スナップショット
経由の描画情報受け渡し）と矛盾しません。Rust 側はウィンドウ・入力・アクセシビリティ API を
一切呼び出さず、SwiftUI 側がそれらすべてを担う設計とすでに整合しています。

### 3.3 TASK-8.3〜8.5 成果物の watchOS/visionOS への適用範囲

| 成果物 | watchOS/visionOS での扱い |
|--------|---------------------------|
| TASK-8.3（ウィジェットセット） | Rust コア側のロジック・状態定義（`Node<S>`/`Screen<S>` 相当のモデル）は再利用可。実際の描画・レイアウトは SwiftUI 側のネイティブビュー（`Text`・`Button`・`List` 等）で行い、`winit`/`accesskit` を含む描画バックエンドは経由しない |
| TASK-8.4（デザイントークン） | トークン定義（色・タイポグラフィ・スペーシング等の値そのもの）は共通コア側で再利用可能。SwiftUI 側での適用（`Color`・`Font` 等へのマッピング）は別途 Swift 側の変換層が必要（`swiftui-bridge-architecture.md` のスコープ外、S5 の Swift 側ラッパ実装時に検討） |
| TASK-8.5（IME 統合） | `winit`/`accesskit` に依存する IME 統合実装は watchOS/visionOS では適用対象外。テキスト入力は SwiftUI 標準のテキストフィールド・キーボード API（WatchKit の文字入力手段を含む）に委譲する |

## 4. 優先度への反映

- **REQ-3 の優先度**: `04-requirements.md` L64 の Must（第 2 段階の中核要件）は**維持**します。
  本書が定める網羅範囲の除外（3.1）は SwiftUI バインド方式そのものを前提とした整理であり、
  REQ-3 の優先度・受け入れ基準を引き下げるものではありません。
- **watchOS・visionOS のどちらを優先するか**（`06-roadmap.md` M3 リスク表が示す再評価トリガー
  「悪化が確認された場合」への対応）: 現時点では両 OS とも
  「SwiftUI バインドハイブリッド構成に限定」という同一の制約段階にあり、`winit`/`accesskit`
  固有の制約が両 OS 間で有意に異なるという一次情報は確認できていません。したがって、
  **現時点では両 OS を同一優先度（Must、並行実施）のまま維持**し、片方を先行させる根拠は
  ないと判断します。
  - PoC-2 実証範囲（TASK-3.1・Issue #56）は
    4 ターゲット（watchOS・visionOS の実機・シミュレータ各 2）すべてでコンパイル成功済みであり、
    実証実績の観点でも両 OS 間に優劣はありません。
  - 市場性等の定性的な優先順位判断が必要になった場合は、spec 側の要件変更に相当するため、
    本書では確定させず「8. スコープ外・要ユーザー報告事項」に記録します。
- **再評価トリガー**（優先度・網羅範囲の見直しを要する条件）:
  1. upstream で `winit` の UIKit バックエンド（watchOS/visionOS 対応）が安定版として完了した
     ことを示す一次情報が確認された場合（Issue #1497 の解決を含む）。
  2. `accesskit` に watchOS/visionOS 向けの公式アダプタが追加されたことを示す一次情報が
     確認された場合。
  3. 上記いずれかと合わせて `wgpu`/`libloading` の制約（`docs/operations/upstream-watch-wgpu-libloading.md`
     が監視対象とする既存の Won't 事項）も解消方向に変化した場合。
  - いずれの場合も、独自レンダラ系スタックの watchOS/visionOS 対応再検討は REQ-3 の受け入れ
    基準・Won't 判定の変更に相当するため、spec リポジトリ側の検討事項としてユーザーへ報告し
    ます（本リポジトリでは spec を変更しません）。

## 5. 監視運用との接続

- `winit`/`accesskit` の watchOS/visionOS 対応は NFR-6（`docs/spec/04-requirements.md`）が
  定める継続監視対象の一部です。`docs/operations/upstream-watch-wgpu-libloading.md` は
  §1 で明記のとおり `wgpu`/`libloading`（watchOS・visionOS）のみを対象とし、`winit`/`accesskit`
  は本書（TASK-8.6・Issue #212）が監視ベースラインを持つ運用としています（重複管理の回避）。
- 監視ベースラインは以下のとおりです。

| クレート | 対象 OS | 現状ベースライン | 確認日 | 確認方法 |
|---------|--------|------------------|--------|---------|
| `winit` | watchOS・visionOS | UIKit バックエンド対応「進行中」。Tracking issue #1497（IME/composition）は open | 2026-07 | [winit Issue #1497](https://github.com/rust-windowing/winit/issues/1497) の状態確認、[winit - crates.io](https://crates.io/crates/winit) のリリースノート確認 |
| `accesskit` | watchOS・visionOS | 公式アダプタ未確認 | 2026-07 | [accesskit - crates.io](https://crates.io/crates/accesskit) のリリースノート確認、`accesskit` リポジトリの対応プラットフォーム一覧確認 |

- **確認サイクル**: `docs/operations/upstream-watch-wgpu-libloading.md` §4 の運用（M3・M4 期間中は
  月 1 回、M5 以降は四半期 1 回）に相乗りします。同文書の「10. 監視ログ」表と同一の形式
  （日付・確認者・バージョン・対応状況・判断・次アクション）で、本書末尾「7. 監視ログ」に
  記録します（同一表への統合はせず、対象クレートが異なるため別表として管理し、対応する
  Issue 番号〈#74 と #212〉で紐付けます）。
- **役割分担**: `docs/operations/upstream-watch-wgpu-libloading.md` §7 と同様に、一次担当は
  人間（ユーザー）とし、Claude Code は確認作業の実行支援（reference-researcher への外部調査
  委譲を含む）・監視ログ表への追記支援を担います。監視結果の解釈・対応方針の決定は行いません。

## 6. REQ-8 受け入れ基準・M3 完了基準との整合表

| 基準 | 内容 | 本書での対応 |
|------|------|-------------|
| REQ-8 受け入れ基準 3（`04-requirements.md` L121） | spec の当該項目を参照 | 本書「3. REQ-3 の網羅範囲への反映」「4. 優先度への反映」で対応（**本書のスコープ**） |
| REQ-3 受け入れ基準（`04-requirements.md` L57〜62） | SwiftUI バインド方式の採用・型安全なブリッジ層設計・監視ルール策定 | 本書は REQ-3 の既存確定事項（ハイブリッド構成の採用）を追認・強化する位置づけであり、`swiftui-bridge-architecture.md`（ブリッジ層設計）・`docs/operations/upstream-watch-wgpu-libloading.md`（監視ルール）と矛盾しない（本書「3.2」「5」で接続を明示） |
| `06-roadmap.md` M3 完了基準（「M3」節、L60〜） | spec の当該項目を参照（TASK-3.3 対応） | 本書が確定させる「独自レンダラ系除外・SwiftUI バインド必須化」の方針は M3 完了基準が前提とする実装方式と一致する。本書自体はドキュメントのみでありコード実装は伴わないため、M3 完了基準の充足そのものは TASK-3.3 側が担う |
| `06-roadmap.md` M3 リスク項（L116、`wgpu`/`libloading` の対応状況変化で TASK-3.1〜3.4 の前提が変わるリスク） | TASK-3.6 の監視ルールで早期検知を図る（`wgpu`/`libloading` が対象。本書は対象外） | `winit`/`accesskit` 側の早期検知は本書「5. 監視運用との接続」の監視ベースラインが担う（対象クレートが異なるため row を分けて引用） |
| `06-roadmap.md` リスクと対策表（L258、`wgpu`/`libloading`/`accesskit` の watchOS/visionOS/Android 対応状況悪化） | 「悪化が確認された場合は REQ-3 の網羅範囲（watchOS・visionOS のどちらを優先するか）を見直します」 | 本書「4. 優先度への反映」の再評価トリガーで `winit`/`accesskit` 側にも同様の枠組みを定義し、両 OS 同一優先度を維持する現状判断を記録した |

## 7. 監視ログ（日付付き記録）

本節は監視サイクルごとの確認結果を追記する記録であり、他節と異なり日付付きで残す。

| 日付 | 確認者 | `winit` | `accesskit` | 対応状況 | 判断 | 次アクション |
|------|-------|---------|-------------|---------|------|-------------|
| 2026-07-31 | Claude Code（初回記録、`crate-feature-matrix.md` 2026-07 調査の引用） | Issue #1497 open。UIKit バックエンド対応「進行中」 | watchOS/visionOS 向け公式アダプタ未確認 | 両クレートとも watchOS/visionOS 対応が発展途上のまま。PoC-7 時点からの判定変更を裏付ける一次情報なし | 独自レンダラ系スタック（`wgpu`/`winit`/`accesskit`）を REQ-3 の網羅範囲から除外し、SwiftUI バインド必須方針を維持 | 次回監視サイクル（TASK-3.6 の監視運用、月 1 回）で `winit` Issue #1497 の状態・`accesskit` リリースノートを再確認する |

## 8. スコープ外・要ユーザー報告事項

- **watchOS・visionOS の市場性等に基づく優先順位判断**: 本書「4. 優先度への反映」で述べた
  とおり、現時点では両 OS を同一優先度とする以上の踏み込んだ優先順位判断（例: 市場性・
  ユーザー基盤の違いによる先行順位）は行っていません。これが必要になった場合、REQ-3 の
  受け入れ基準・Won't 判定の変更に相当するため、spec リポジトリ
  （Fandhe-AI/fandhe-multi-platform-spec、非公開）側の検討事項としてユーザーへ別途報告します。
- **`winit`/`accesskit` の監視 CI 自動化**: `docs/operations/upstream-watch-wgpu-libloading.md`
  §9 が記録する「監視の CI 自動化」の対象外事項と同様、本書が定めるベースライン確認の
  自動化は本書のスコープ外です。`.github/workflows/update-external.yml` は上流
  `Fandhe-AI/actions` の reusable workflow を SHA 固定で呼び出す wrapper であり、`uses:` で
  呼び出すジョブへ直接ステップ・ジョブを追加することはできないため、自動化する場合は
  監視専用の別 workflow ファイルを新設するか、上流 reusable workflow 側へ機能追加を
  提案するかのいずれかによります。追加する場合は `ci.md`（GitHub ホスト runner・最小権限）に
  従う必要があり、Issue 起票はユーザー承認後に行います。

## 9. 出典

- `docs/spec/04-requirements.md` L53〜66（REQ-3）、L114〜124（REQ-8、特に L121 受け入れ基準 3）
- `docs/spec/05-tasks.md` L288〜294（TASK-8.6）
- `docs/spec/06-roadmap.md` L60〜135（M3、REQ-3・REQ-8 該当タスクと M3 リスク項）
- `docs/spec/03-poc/watchos-visionos/README.md`（PoC-2）
- `docs/architecture/crate-feature-matrix.md`（TASK-8.1 成果物、2 節・5 節、`winit`/`accesskit`
  の 2026-07 再調査結果の引用元）
- `docs/architecture/swiftui-bridge-architecture.md`（TASK-3.2 成果物、FFI 境界設計・
  責務分界との接続）
- `docs/operations/upstream-watch-wgpu-libloading.md`（TASK-3.6 成果物、監視運用の相乗り先）
- [Tracking issue for IME / composition support - winit Issue #1497 - GitHub](https://github.com/rust-windowing/winit/issues/1497)
- [winit - crates.io](https://crates.io/crates/winit)
- [accesskit - crates.io](https://crates.io/crates/accesskit)
- RustSec advisory（`https://rustsec.org`）: `crate-feature-matrix.md` §7 の確認結果を引用
  （2026-07 時点で該当する勧告は確認できず、要再検証）
