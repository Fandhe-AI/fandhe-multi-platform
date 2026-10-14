# egui/eframe cargo feature 監査（TASK-6.1）

## 1. 目的・位置づけ

本書は TASK-6.1（`docs/spec/05-tasks.md` L214〜219）の成果物であり、REQ-6（`docs/spec/04-requirements.md`
L93〜104、Must・Conditional Go 条件 6）の受け入れ基準 1「`egui`/`eframe`（または本実装で採用する基盤クレート
組み合わせ）の cargo feature を監査し、フレームワークが使用しない機能を無効化する」のうち、**監査（洗い出し）
部分のみ**を満たすために作成しました。

- **契機**: PoC-4（`docs/spec/03-poc/performance/README.md` L97・L105・L114・L124）で、macOS プロキシ実測での
  アイドル時メモリ増分が SwiftUI（ネイティブ実装代替）比 +146%（約 37.7MB）となり、REQ-5 のメモリ増分基準
  （ネイティブ比 +50% 以内・絶対値 20MB 以内）を超過しました。要因として `egui`/`eframe` が同梱する
  `accesskit`・`image`・ICU（`icu_*`）・`arboard`・`webbrowser` 等の依存クレートが挙げられていました
  （PoC-4 発見事項 1、`04-requirements.md` REQ-6 詳細）。
- **本書のスコープ**: cargo feature・依存クレートの**監査のみ**。無効化の実装は TASK-6.2、削減前後のメモリ
  比較計測・REQ-5 基準への近接度確認は TASK-6.3 のスコープであり、いずれも本書には含みません。
- 本書は `egui`/`eframe` の feature 構成を対象とした監査であり、
  `docs/architecture/crate-feature-matrix.md` の対象整理と対をなします。
- **`docs/spec/` との関係**: `docs/spec/` は読み取り専用サブモジュールのため、本書は本リポジトリ側
  （`docs/architecture/`）の新規ドキュメントとして作成しています。spec 自体の変更はこのリポジトリでは
  行いません。
- **現行の採用バージョン・実装先**: `crates/fandhe-egui-proxy/Cargo.toml`（および `Cargo.toml` の
  `[workspace.dependencies]`）で `egui`/`eframe` とも `0.33.3` を採用している（§6.1）。

## 2. 監査方法

- **対象バージョン**: PoC-4 の実測構成である `eframe = { version = "0.29", default-features = false,
  features = ["glow", "accesskit"] }` + `egui = "0.29"`（`docs/spec/03-poc/performance/egui-app/Cargo.toml`）を
  ベースラインとし、比較対象として crates.io 上の最新安定版（本監査実施時点 2026-07-29 で `0.35.0`）を並行調査
  しました。**egui/eframe は頻繁にリリースされるため、本書のバージョン判定は本監査実施日の一次情報に基づく
  一時点のスナップショットです。** 採用版 `0.33.3` での再確認結果は §6.1 を参照してください。
- **実施日**: 2026-07-29
- **実行環境**: `cargo 1.96.0`（Linux, x86_64）。iOS/Android/watchOS/visionOS/Windows 向けの依存差分は
  本監査のスコープ外（target 別 cfg 分岐は eframe の Cargo.toml 記載を確認するに留め、実機/エミュレータでの
  検証は行っていません）。
- **手順（再現可能な形で記録）**:
  1. scratchpad に検証用 cargo プロジェクトを 2 つ作成（本リポジトリへは一切コミットしない）。
     - (a) PoC-4 構成: `cargo new poc4 && cd poc4 && cargo add eframe@0.29 --no-default-features
       --features glow,accesskit && cargo add egui@0.29`
     - (b) 最新安定版フル既定構成: `cargo new latest && cd latest && cargo add eframe && cargo add egui`
       （`--dry-run` で `0.35.0` が解決されることを確認済み）
     - (c) 6 節の推奨構成（同一バージョン 0.35.0 で feature のみ絞り込み。フル既定構成との比較のため
       バージョンを揃えた）: `cargo new reduced && cd reduced && cargo add eframe@0.35
       --no-default-features --features accesskit,default_fonts,glow,wayland,x11 && cargo add egui@0.35
       --no-default-features --features default_fonts`
  2. `cargo tree -e features` で feature 有効化の経路を出力し、`cargo tree -i <crate>`（`image`・`icu_properties`・
     `icu_normalizer`・`arboard`・`webbrowser`・`accesskit`・`quick-xml@<version>`）で依存クレートの逆引き
     （どの feature・どの中間クレート経由で引き込まれるか）を確認した。
  3. `~/.cargo/registry/src/index.crates.io-*/eframe-<version>/Cargo.toml`・`egui-winit-<version>/Cargo.toml`・
     `egui-<version>/Cargo.toml`（cargo が展開した正規化済み Cargo.toml。`cargo add`/`cargo tree` 実行後に
     ローカルにキャッシュされる一次情報）を直接読み、`[features]` セクションと各依存の `optional`/
     `default-features` 設定を確認した。
  4. `cargo tree --prefix none | sort -u | wc -l` で unique 依存クレート数を比較した。
  5. `cargo audit`（RustSec advisory-db、2026-07-29 時点のデータベース）を各構成で実行し、既知脆弱性・
     unmaintained warning の有無を確認した。
  6. 実験プロジェクトは crates.io 公式レジストリのみを参照し（git 依存・非公式ミラー不使用）、リポジトリには
     コミットしていない（scratchpad のみに存在。OWASP A08 整合性の不備の回避）。

## 3. 主要な発見: PoC-4 の前提（「フル機能有効化」）は事実と異なる

**REQ-6 詳細・PoC-4 発見事項 1 は「`egui`/`eframe` が同梱する依存クレートがフル機能有効化で静的リンクされる」
ことを推定要因としていますが、PoC-4 が実際に測定した `egui-app/Cargo.toml` は既に
`eframe = { default-features = false, features = ["glow", "accesskit"] }` という絞り込み済み構成でした。**
つまり 37.7MB の増分は「フル機能版」ではなく「最小限に絞ったつもりの構成」で測定された値です。

`image`（PNG デコード）・`arboard`（クリップボード）・`webbrowser`（リンクオープン）は
**eframe 側で top-level feature による無効化手段が提供されておらず**（後述 4 節）、`default-features = false`
を指定しても外れません。したがって、**PoC-4 発見事項が推定するメモリ増分要因（`accesskit`・`image`・
`icu_*`・`arboard`）に限れば、TASK-6.2 で見込める削減余地は REQ-6 が想定するよりも小さい**可能性が高く、
この点を本書で明示しておく必要があります。同一バージョン内での feature 絞り込み自体には約 12% の依存
クレート数削減効果を確認済みですが、その内訳は主に `wgpu` 系バックエンドの除外によるものであり、上記 4
クレートは含まれません（削減効果の詳細は 6 節）。

## 4. feature 監査結果一覧

`eframe` 0.29.1／0.35.0 の正規化済み `Cargo.toml`（`[features]` セクション・依存宣言の `optional`/
`default-features`）と `cargo tree -i` の逆引き結果に基づく監査結果です。「導入経路」列は実際に
`cargo tree -e features` / `cargo tree -i` で確認した経路（一次情報: ローカル展開された Cargo.toml）です。
以下の表は監査実施日（2026-07-29）時点の判定であり、「判定」列の末尾に採用版 `0.33.3`
（`crates/fandhe-egui-proxy/Cargo.toml`、§6.1）での現行の採用状況を付記する。

| 対象 | 種別 | 導入経路（実測） | フレームワークでの使用有無・根拠 | 判定 |
|------|------|-----------------|--------------------------------|------|
| `accesskit`（`accesskit`・`accesskit_unix`・`accesskit_atspi_common`・`accesskit_consumer`・`accesskit_winit`） | eframe feature `accesskit`（top-level・optional で無効化可） | `eframe` feature `accesskit` → `egui-winit/accesskit`（0.35 では直接、0.29 では `egui/accesskit` も追加で有効化） | REQ-9（macOS アクセシビリティ）・REQ-8（採用基盤クレートとしてのアクセシビリティ橋渡し）の必須要素。メモリ増分の推定要因ではあるが、**安全側に倒し「維持」と判定**する。安易な無効化は REQ-9 の受け入れ基準を満たせなくなる | **維持**（feature フラグとしては存在するため、テスト用ビルド等で一時的に切ることは可能だが、製品ビルドでは常時有効とする）。**現行 0.33.3**: 通常ビルドは macOS 分岐（TASK-9.1-S1）・Android 分岐・dev-dependencies のみ有効。Linux/Windows/iOS/wasm32 の通常ビルドでは無効 |
| `image`（PNG デコード。`moxcms`・`png`・`crc32fast`・`fdeflate`・`flate2`・`byteorder-lite` 等の推移的依存を含む） | **eframe 側で無効化不可**。`target.cfg(not(wasm32)).dependencies.image`（`features = ["png"]`, `default-features = false`）として非 optional 宣言されている（0.29・0.35 共通、アイコン読み込み用途と推定） | `cargo tree -e features -i image` で `image feature "png"` が `eframe feature "accesskit"` と `eframe feature "glow"` の両方から到達することを確認（実体は eframe 自身の非 optional 依存）。0.35 では `arboard`（クリップボード画像）経由でも追加される | ウィンドウアイコン読み込み等に使用されている可能性が高いが、PNG 1 フォーマットのみに既に限定されている（feature 側は既に最小）。フレームワークが実際に画像アセットを扱う予定（REQ-8 のデザイントークン・ウィジェットセット）があるため、フォーマット追加時は必要な形式のみに限定する方針とする | **維持（構造的に無効化不可。フォーマットは png のみで既に最小）**。**現行 0.33.3**: `cargo tree -i image` で同経路（`egui-winit` → `eframe`）を確認済み。判定は変わらず |
| `arboard`（クリップボード、`clipboard-win`/`x11rb`/`smithay-clipboard` 等のプラットフォーム別バックエンドを含む） | **eframe 側で無効化不可**。`eframe` が `egui-winit` 依存を `features = ["clipboard", "links"]`, `default-features = false` で**非 optional・固定指定**しており、`egui-winit` 側は `clipboard` feature で `arboard`/`smithay-clipboard` を optional 化しているが、eframe がこの feature を常時有効化するため、アプリ側からは無効化できない | `cargo tree -i arboard` → `egui-winit` → `eframe`（0.29・0.35 共通）。`egui-winit-<ver>/Cargo.toml` の `clipboard = ["arboard", "smithay-clipboard"]` を確認 | テキスト編集ウィジェット（REQ-8 の IME 統合・ウィジェットセット）でコピー&ペーストは必須機能と想定されるため、機能要件上は「使用する」。ただし現状 eframe をそのまま採用する限り無効化の余地がない点を明記する | **維持（機能要件上必要。かつ構造的に無効化不可）**。**現行 0.33.3**: `cargo tree -i arboard` で同経路を確認済み。判定は変わらず |
| `webbrowser`（リンククリック時の既定ブラウザ起動。`url`・`idna`・`idna_adapter` 等の推移的依存を含む） | **eframe 側で無効化不可**。上記と同じ理由（`egui-winit` の `links` feature を eframe が固定指定） | `cargo tree -i webbrowser` → `egui-winit` → `eframe`。`egui-winit-<ver>/Cargo.toml` の `links = ["webbrowser"]` を確認 | `egui::Hyperlink` 等のリンク機能を使う場合に必要。Linux をターゲットに含む（CLAUDE.md 記載の対応プラットフォーム）ため、対応自体は妥当。ただし無効化不可の制約を明記する | **維持（構造的に無効化不可）**。**現行 0.33.3**: `cargo tree -i webbrowser` で同経路を確認済み。判定は変わらず |
| `icu_*`（`icu_properties`・`icu_normalizer`・`icu_collections`・`icu_locale_core`・`icu_provider` 等、Unicode/IDNA 処理用） | フレームワークの feature 選択とは無関係に**必ず入る**。`webbrowser` が使う `url` クレートの IDNA（国際化ドメイン名）処理経路 | `cargo tree -i icu_properties` / `-i icu_normalizer` → `idna_adapter` → `idna` → `url` → `webbrowser` → `egui-winit` → `eframe`（0.29・0.35 共通） | PoC-4 発見事項が推定した「ICU（国際化データ）」はテキストシェーピング用ではなく、**リンクオープン機能（`webbrowser`→`url`）の URL 正規化のための IDNA 実装**である。テキスト描画・CJK フォント（REQ-10）とは無関係。`webbrowser`/`links` を無効化できない限り連動して残る | **維持（`webbrowser` に従属。単体では無効化不可）**。**現行 0.33.3**: `cargo tree -i icu_properties` で `idna_adapter` → `idna` → `url` → `webbrowser` → `egui-winit` → `eframe` の同経路を確認済み。判定は変わらず |
| `default_fonts`（`egui`/`epaint` 同梱のプロポーショナルフォント） | eframe feature `default_fonts`（top-level・optional で無効化可） | `eframe` の `default` feature 経由（`default_fonts = ["egui/default_fonts"]`）。**PoC-4 の `egui-app/Cargo.toml` は独自に `egui = "0.29"`（デフォルト feature 有効）を直接依存に追加しており、この経路でも `default_fonts` が有効化されていた**（eframe 側で `default-features = false` にしても、アプリの直接依存側で feature 統一により再度有効化される） | REQ-10（CJK フォント同梱、M3）で独自フォントパイプラインに置き換わる可能性がある。M2 段階（プロキシ実描画）では暫定的に同梱フォントで代替描画するため必要 | **条件付き維持**（M2 は維持。M3 で REQ-10 の独自フォントパイプライン確定時に無効化を再検討。**TASK-6.2 実装時は eframe 側だけでなく、アプリの直接 `egui` 依存側の feature 指定も揃えて `default-features = false` にすること** — さもないと本監査と同じ理由で feature 統一により無効化が効かない）。**現行 0.33.3**: `egui`（workspace 定義）側で維持している。`fandhe-fonts` の Noto Sans JP（REQ-10、TASK-10.1）が既定フォントであり、`default_fonts` は読込失敗時のフォールバック用途 |
| `wayland`・`x11`（Linux ウィンドウバックエンド） | eframe feature（top-level・optional で無効化可） | `eframe` の `default` feature 経由 | 対応プラットフォームに Linux を含む（CLAUDE.md 記載）ため、Linux ターゲットでは少なくとも一方が必要。プラットフォーム別 feature フラグとして加法的に構成すべき（coding-rust.md の additive 原則） | **維持（Linux ビルドでは必須。ただし加法的にプラットフォーム別へ切り出す設計は TASK-6.2 で検討）**。**現行 0.33.3**: デスクトップ分岐（not wasm32/ios/android）で `glow`・`x11`・`wayland` として採用済み |
| `glow`（OpenGL レンダリングバックエンド、0.29 の PoC-4 採用構成） | eframe feature（top-level・optional で無効化可） | `eframe` の `glow` feature（`egui_glow`・`glow`・`glutin`・`glutin-winit` を dep: 経由で有効化） | PoC-4 実測構成の採用バックエンド。0.35 では `default` feature が `wgpu` に切り替わっており（後述）、`glow` は非 default になった | **維持（0.29 系を使う限り）。TASK-1.2 が採用する実バージョン・バックエンドと整合させること**。**現行 0.33.3**: デスクトップ分岐・wasm32 分岐（WebGL2 固定）で採用済み |
| `wgpu`（`egui-wgpu` 経由の WebGPU/Metal/Vulkan バックエンド） | eframe feature（top-level・optional で無効化可） | 0.35 で `default` feature の一部に変更（0.29 では非 default の opt-in） | PoC-4・現行案は `glow` を採用しているため不使用。TASK-1.2 の実描画方式次第で採用可能性あり | **未使用（現行 M2 方針では `glow` を採用するため opt-in しない）**。**現行 0.33.3**: iOS 分岐（`metal`）・Android 分岐（`vulkan`）で採用済み。デスクトップ・wasm32 は `glow` のまま |
| `persistence`（`ron`・`serde`・`home` を追加） | eframe feature（top-level・optional で無効化可、非 default） | 非 default のため明示的に有効化しない限り入らない | 状態永続化の要件は未確定 | **未使用（無効のまま維持。必要になった時点で個別検討）**。**現行 0.33.3**: 引き続き未使用 |
| `web_screen_reader`（Web 版のスクリーンリーダー連携） | eframe feature（top-level・optional で無効化可、default） | `eframe` の `default` feature 経由 | ネイティブビルドでは `web-sys` 依存のため事実上ノーオペレーション。Web ターゲット（wasm32）採用時は REQ-9 相当のアクセシビリティ要件に関連しうる | **維持（Web ターゲットのみ実効。ネイティブでは実害なし）**。**現行 0.33.3**: wasm32 分岐では無効（WebGL2 固定の `glow` のみ採用） |
| `android-game-activity` / `android-native-activity` | eframe feature（top-level・optional、非 default） | 非 default のため明示的に有効化しない限り入らない | Android 統合（TASK-1.3 以降）で `winit` の Android ライフサイクル方式選択に必要になる見込み | **未使用（Android 統合時に選択検討）**。**現行 0.33.3**: `android-game-activity` を Android 分岐で採用済み（`winit/android-game-activity` も直接依存として連動） |
| `puffin`（プロファイラ計装） | eframe feature（top-level・optional、非 default） | 非 default | 開発時プロファイリング用途。製品ビルドには不要 | **未使用（開発時のみ opt-in を検討）**。**現行 0.33.3**: 引き続き未使用 |

## 5. RustSec 勧告の確認結果（`cargo audit`、2026-07-29 時点データベース）

| 構成 | 検出された勧告 | 内容 |
|------|---------------|------|
| PoC-4 構成（`eframe` 0.29.1、`glow`+`accesskit`） | **RUSTSEC-2026-0194 / RUSTSEC-2026-0195（`quick-xml` 0.30.0、severity 7.5 High）**、RUSTSEC-2024-0436（`paste`、unmaintained）、RUSTSEC-2026-0192（`ttf-parser`、unmaintained） | `quick-xml` 0.30.0 は `accesskit_atspi_common` → `zbus-lockstep` → `zbus_xml` 経由で導入される（Linux の AT-SPI／D-Bus 経由アクセシビリティ実装の推移的依存）。**上限緩和（`>=0.41.0` への更新）で解消可能な既知の高深刻度脆弱性が、`accesskit` を有効化した時点で 0.29.1 系には内在する** |
| 最新安定版構成（`eframe` 0.35.0、フル既定 feature） | RUSTSEC-2026-0192（`ttf-parser`、unmaintained）のみ | `quick-xml` は `0.41.0` に解決され、上記 2 件の脆弱性は再現しない |
| 6 節の推奨 feature 構成（`eframe` 0.35.0、`accesskit`・`default_fonts`・`glow`・`wayland`・`x11` のみ） | RUSTSEC-2026-0192（`ttf-parser`、unmaintained）のみ | フル既定 feature 構成と同様。`quick-xml` の脆弱性は再現しない |
| **現行 `Cargo.lock`（2026-09-05、cargo-audit 0.22.2、advisory-db 1239 件、430 crate）** | **脆弱性 0 件。unmaintained 警告 2 件**: RUSTSEC-2024-0436（`paste` 1.0.15）、RUSTSEC-2026-0192（`ttf-parser` 0.25.1） | `quick-xml` は `0.41.0` に解決されており、上記 2 件の高深刻度脆弱性は該当しない |

**この結果は、0.29 → 0.33 系以降へのバージョン更新がセキュリティ上の理由（A06 脆弱な依存）からも
妥当であったことを示しています。** 採用版 0.33.3 は MSRV 1.88 の制約（CI ランナーの
rustc 1.88.0 固定、6.1 節参照）から選定されたものであり、既知脆弱性 0 件を維持しています。
`ttf-parser` の unmaintained warning は現行版でも解消されておらず、更新時の再確認が必要です。

## 6. 監査時点の推奨構成と現行構成

- **推奨バージョン**: 本監査時点の最新安定版（`0.35.0`）を基準に検討することを推奨する。0.29 系には
  `accesskit`（Linux AT-SPI 経路）に起因する RustSec 高深刻度脆弱性 2 件が内在し、0.35 系ではこれが解消されて
  いることを確認済みのため（5 節）。**ただし TASK-1.2（#23）が既に採用しているバージョン・バックエンド
  （`glow`/`wgpu`）がある場合は、そちらとの整合を優先し、本書の推奨はあくまで参考値として扱うこと。**
- **推奨 feature 構成の例**（4 節の判定に基づく。ネイティブ・非 wasm32 ターゲット想定）:
  ```toml
  eframe = { version = "0.35", default-features = false, features = ["accesskit", "default_fonts", "glow", "wayland", "x11"] }
  egui = { version = "0.35", default-features = false, features = ["default_fonts"] }
  ```
  - `egui` 側も `default-features = false` を明示すること（4 節「`default_fonts`」の feature 統一に関する注記）。
  - `image`・`arboard`・`webbrowser`・`icu_*` は eframe 経由で常に入るため、上記構成でも依存クレート数の
    大幅な削減効果は見込めない（3 節）。
  - **現行 `Cargo.toml` の実定義**（`[workspace.dependencies]`）: `egui = { version = "0.33",
    default-features = false, features = ["default_fonts"] }`、
    `eframe = { version = "0.33", default-features = false }`（top-level feature は
    `crates/fandhe-egui-proxy/Cargo.toml` 側の target 分岐で個別に指定。分岐一覧は §6.1）。
- **無効化不可な依存を本当に削減したい場合の代替案（TASK-6.2 での検討候補として記録。本書では判定しない）**:
  `eframe` を使わず `egui` + `egui-winit`（`default-features = false` で `clipboard`/`links` を明示的に外す）+
  独自の `winit`/描画バックエンド初期化コードを組む構成にすれば、`arboard`・`webbrowser`・`icu_*` を完全に
  外せる可能性がある。ただし `eframe` が肩代わりしているウィンドウライフサイクル管理・イベントループ配線を
  自前実装することになり、工数・保守コストとのトレードオフが生じる。**この設計判断自体は TASK-6.2 のスコープ
  であり、本書では選択肢の提示に留める。**
- **期待削減効果（参考値・依存クレート数）**:

  | 構成 | バージョン | unique 依存クレート数（`cargo tree --prefix none \| sort -u \| wc -l`） |
  |------|-----------|------------------------------------------------------------------|
  | フル既定 feature | 0.35.0 | 265 |
  | 6 節の推奨 feature 構成（`accesskit`・`default_fonts`・`glow`・`wayland`・`x11` のみ。バックエンドは `wgpu` → `glow` に変更） | 0.35.0（同一バージョン） | 233（-32、約 12% 減） |
  | 参考: PoC-4 実測構成（`default-features=false`、`glow`+`accesskit`。バージョンが異なるため上 2 行とは単純比較不可） | 0.29.1 | 223 |
  | **現行 workspace（0.33.3、macOS ホスト、`fandhe-egui-proxy` の target 分岐込み）** | **0.33.3** | **254（`-e normal`: 189）** |
  | **現行 `fandhe-egui-proxy` 単体（`-p fandhe-egui-proxy`）** | **0.33.3** | **227（`-e normal`: 174）** |

  現行 workspace の値はターゲット（macOS ホスト）・バージョン（0.33.3）とも監査時点の各構成と異なるため、
  上記の 265/233/223 とは単純比較できない（参考値として併記する）。

  同一バージョン（0.35.0）内でフル既定 feature と推奨構成を比較すると、feature 絞り込み自体には
  32 クレート（約 12%）の削減効果があることを確認した。ただし `image`/`arboard`/`webbrowser`/`icu_*`
  （4 節）は feature 選択に関わらず残るため、削減の内訳は主に `wgpu` 系バックエンド一式（`wgpu`・
  `egui-wgpu`・関連 GPU 抽象化クレート）と `web_screen_reader`（`web-sys` 系、非 wasm32 では未使用）が
  外れたことによるものであり、**PoC-4 が計測したメモリ増分の主要因（`accesskit`・`image`・`icu_*`・
  `arboard`）そのものは今回の削減には含まれていない**。したがって「削減余地は限定的だが無視できるほど
  小さくはない（依存クレート数ベースで約 12%）」というのが本書の結論であり、**この削減がメモリ増分
  （37.7MB）にどの程度効くかは TASK-6.3 の実測でのみ判断できる**旨を申し送る。

### 6.1 現行構成（`crates/fandhe-egui-proxy/Cargo.toml`、2026-09-05 時点）

`egui`/`eframe` とも `0.33.3` を採用し、target 分岐ごとに `eframe` の feature を個別に指定している。

| 分岐 | `eframe` features | 補足 |
|------|-------------------|------|
| デスクトップ（not wasm32/ios/android） | `glow`, `x11`, `wayland` | `default-features = false` |
| macOS（デスクトップ分岐と union） | `+ accesskit` | TASK-9.1-S1 |
| iOS | `wgpu` | `wgpu` を直接依存として追加し `metal` feature を加算 |
| Android | `wgpu`, `android-game-activity`, `accesskit` | `accesskit_winit/accesskit_android`・`wgpu/vulkan`・`winit/android-game-activity`・`libc` を直接依存として追加 |
| wasm32 | `glow` | WebGL2 固定（`web_screen_reader` は無効） |
| dev-dependencies（Android 以外） | `accesskit` | `egui_kittest`（`wgpu` 依存）との feature 整合用 |

`crates/fandhe-egui-proxy` 自身の crate feature（依存を増やさない既定 off）: `frame-metrics` /
`debug-proxy-app` / `cold-start-probe`。

例外として `device`（Phase 8 / Issue #746、既定 **on**）がある。`fandhe-device`（`ureq` 経由で
依存を増やす）への optional 依存を切り替える feature で、spec 要件化が否決された
場合に crate ごと削除して切り離す前提のため、他の feature と異なり既定 off ではなく既定 on
にしている（切り離し手順は README.md 参照）。wasm32 では `fandhe-device` 自体が
`target.'cfg(not(target_arch = "wasm32"))'.dependencies` に置かれているため、この feature を
有効にしても依存は増えない。

直接依存の補完（`[workspace.dependencies]` に定義済み・target 分岐で個別に有効化）: `wgpu`
（iOS/Android 分岐）・`accesskit_winit`（Android 分岐のみ。macOS は `eframe` の
`accesskit` feature 経由で間接的に取り込む）・`winit`（`fandhe-egui-proxy` 自身は Android
分岐のみで直接依存。全 native 分岐で使われる `winit` 本体は `fandhe-winit-ime` 側の定義）・
`ordered-float`（`fandhe-egui-proxy` の非 wasm32 native 分岐。wgpu-hal の推移依存バージョンを
固定するための宣言のみで本クレート自身は未使用）。クレート単位の直接依存（workspace 定義に
無いもの）: `fandhe-egui-proxy` の `libc = "0.2"`（Android 分岐）・`js-sys = "0.3"`（wasm32 分岐）。

`persistence` / `puffin` は引き続き未使用。

## 7. REQ-6 受け入れ基準・M2 完了基準との整合確認

| 基準 | 内容 | 本書での対応 |
|------|------|-------------|
| REQ-6 受け入れ基準 1 | `egui`/`eframe` の cargo feature を監査し、フレームワークが使用しない機能を無効化する | **監査は本書で完了**（4 節の feature 監査結果一覧）。**無効化の実装は TASK-6.2 のスコープ**であり本書には含まない |
| REQ-6 受け入れ基準 2 | feature 絞り込み前後でアイドル時メモリ増分を比較計測し、削減率を記録する | **TASK-6.3 のスコープ**。本書は依存クレート数の参考値（6 節）のみ提供し、メモリ実測は行わない |
| REQ-6 受け入れ基準 3 | 削減後の値が REQ-5 のメモリ増分基準にどこまで近づいたかを実測で確認する | **TASK-6.3 のスコープ**。本書 3 節・6 節で「削減余地は限定的」との見通しを申し送るに留める |
| Issue #25 受け入れ条件 1 | feature 監査結果一覧 | 4 節で対応 |
| Issue #25 受け入れ条件 2 | REQ-6 の受け入れ基準・M2 完了基準との整合確認 | 本節で対応 |
| `06-roadmap.md` M2 完了基準（REQ-6 該当行） | feature 監査・基盤クレート機能マトリクス更新（REQ-6・REQ-8）の完成 | REQ-6 側は本書（TASK-6.1）の作成をもって完了。REQ-8 側は `docs/architecture/crate-feature-matrix.md`（TASK-8.1）で別途対応済み |

## 8. スコープ分界・後続タスクからの参照

- **TASK-6.2-S1（accesskit・image 系 feature 無効化実装、Issue #90）**: 完了済み。本書 4 節の
  判定を 0.33.3（実採用バージョン）で再検証し、`crates/fandhe-egui-proxy/Cargo.toml` の
  accesskit を通常ビルドから除外（テスト・opt-in feature・macOS/Android 分岐では維持）した。
- **TASK-6.2-S2（icu/arboard 系の追加調査）**: 完了済み。`arboard`/`webbrowser`/
  `icu_*` が構造的に無効化不可であることを再確認した（4 節と同じ結論）。
- **TASK-6.2-S3（feature 絞り込みのビルド反映）**: 完了済み。§6.1 の現行構成（採用バージョン
  0.33.3、バックエンド `glow`/`wgpu`）へ反映済み。
- **TASK-6.3（削減前後のメモリ比較計測）**: 完了済み。
- **TASK-1.2（#23、egui/eframe プロキシ実描画）**: 採用バージョン・レンダリングバックエンドは
  §6.1 のとおり確定済み（`glow`/`wgpu` の target 分岐使い分け）。
- **TASK-8.1（`docs/architecture/crate-feature-matrix.md`）**: REQ-8 の基盤クレート機能マトリクスとは対象が
  異なる（TASK-8.1 は `wgpu`/`vello`/`taffy`/`cosmic-text`/`winit`/`accesskit` の M3 以降の直接統合候補、
  本書は M2 段階で採用する `egui`/`eframe` プロキシの feature 監査）。両書は独立して維持する。

## 9. 出典・一次情報

- [eframe - crates.io](https://crates.io/crates/eframe)（バージョン一覧・`0.35.0` が最新安定版であることの確認、2026-07-29 時点）
- ローカルに展開された正規化済み `Cargo.toml`（`cargo add`/`cargo tree` 実行時に crates.io から取得。一次情報）:
  `~/.cargo/registry/src/index.crates.io-*/eframe-0.29.1/Cargo.toml`、
  `~/.cargo/registry/src/index.crates.io-*/eframe-0.35.0/Cargo.toml`、
  `~/.cargo/registry/src/index.crates.io-*/egui-winit-0.29.1/Cargo.toml`、
  `~/.cargo/registry/src/index.crates.io-*/egui-winit-0.35.0/Cargo.toml`、
  `~/.cargo/registry/src/index.crates.io-*/egui-0.35.0/Cargo.toml`
- `cargo tree -e features` / `cargo tree -i <crate>` の実行結果（本監査で scratchpad 上で実施、2026-07-29）
- `cargo audit`（RustSec advisory-db、2026-07-29 時点のデータベース。[RUSTSEC-2026-0194](https://rustsec.org/advisories/RUSTSEC-2026-0194)、
  [RUSTSEC-2026-0195](https://rustsec.org/advisories/RUSTSEC-2026-0195)、
  [RUSTSEC-2024-0436](https://rustsec.org/advisories/RUSTSEC-2024-0436)、
  [RUSTSEC-2026-0192](https://rustsec.org/advisories/RUSTSEC-2026-0192)）
- `docs/spec/03-poc/performance/README.md`（PoC-4、egui-app/Cargo.toml の実際の構成の確認）
- `docs/spec/04-requirements.md`（REQ-6・REQ-5・REQ-9）、`docs/spec/05-tasks.md`（TASK-6.1〜6.3）、
  `docs/spec/06-roadmap.md`（M2）
- `docs/architecture/crate-feature-matrix.md`（TASK-8.1、体裁・スコープ分界の参照元）
- `Cargo.toml`・`Cargo.lock`・`crates/fandhe-egui-proxy/Cargo.toml`（§6.1 現行構成の一次情報）
- `cargo audit`（2026-09-05、cargo-audit 0.22.2、advisory-db 1239 件、Cargo.lock 430 crate）
