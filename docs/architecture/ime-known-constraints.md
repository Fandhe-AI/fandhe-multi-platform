# IME 統合 既知制約・プラットフォーム差異（TASK-8.5-S29 統合文書）

## 1. 目的・位置づけ

本文書は #181（TASK-8.5「IME 統合実装」、REQ-8、M3）の sub-issue #210
（TASK-8.5-S29「既知制約・プラットフォーム差異の文書化」）の成果物であり、
`ime-architecture.md` §1 分担表が S29 に定義する役割
「既知制約・プラットフォーム差異を扱う」に対応する。

プラットフォーム差異・既知制約は `ime-architecture.md` §9、
`docs/architecture/ime-android-input-connection.md` §9、各クレートの
モジュール doc に分散する。**本文書がそれらを一元化する SSOT** である。

### 他文書との役割分担（重複記載しない）

- **`ime-architecture.md`**: コア⇔プラットフォーム層の契約設計（`ImeEvent`/`ImeRequest`
  のシグネチャ・責務分界・プラットフォーム別マッピング表）。**契約の詳細は本書では
  再掲せず参照に留める**（`code-comment-style.md` の「陳腐化しやすい実装詳細の重複を
  避ける」原則に従う）。
- **`docs/architecture/ime-android-input-connection.md`**: Android
  `InputConnection`/GameTextInput 統合の詳細設計（S18・#199）。
- **本書（`ime-known-constraints.md`）**: 上記へ分散した制約・差異を横断的に一覧化し、
  §4 に IME 統合の確定設計判断を置く。**アプリ開発者・後続 sub-issue（S30 使用ガイド等）が
  「今何が制約か」を一箇所で把握するための参照点**と位置づける。

## 2. プラットフォーム差異マトリクス

行に機能軸、列に対象プラットフォームを取る。詳細な契約定義は `ime-architecture.md` §4 を参照。

| 機能軸 | macOS | Windows | Linux X11 | Linux Wayland | iOS | Android（NativeActivity） | Web（DOM） | watchOS / visionOS |
|--------|-------|---------|-----------|---------------|-----|-------------------------------|-----------|---------------------|
| `Ime::Preedit`（未確定文字列の表示） | 対応 | 対応 | 対応（IM サーバーが XIM on-the-spot スタイルを広告する場合のみ。fcitx5 は既定 off、§3.14） | 対応 | **非対応**（commit 型のみ、§3.1） | **非対応**（commit 型のみ、§3.2） | `compositionupdate` 経由（コア描画へは未反映、§3.9） | 未検証（§3.12） |
| `Ime::Enabled`/`Disabled` 発火タイミング | コンポジション開始/終了まで**遅延発火** | コンポジション開始/終了まで**遅延発火**、かつ確定毎に高頻度再サイクル | **即時発火**（`SetEnabled`/フォーカス変化に同期） | **即時発火**（同左） | ソフトキーボード表示/非表示に連動（`Ime::*` 自体は発火しない） | ソフトキーボード表示/非表示に連動（`Ime::*` 自体は発火しない） | 該当 API なし（DOM に明示的な enable/disable API がない） | 未検証 |
| `SetPurpose`（`ImePurpose`）の実効性 | **no-op** | **no-op** | **no-op** | **有効**（`ContentHint`/`ContentPurpose` へマップ） | no-op（winit 側 warn ログのみ） | no-op（winit 側空実装） | 未実装 | 未検証 |
| `SetCursorArea`（候補ウィンドウ位置） | 対応 | 対応 | 対応（ただし freedesktop 既知バグで IME 側が無視しうる） | 対応 | `UITextInput` プロトコルメソッド実装形態（明示 API ではない） | no-op（winit 側空実装） | ブラウザ既定の候補ウィンドウに委任（コア側は制御不能） | 未検証 |
| `SetEnabled` の意味論 | IME 許可/不許可 | IME 許可/不許可 | IME 許可/不許可 | IME 許可/不許可 | **ソフトウェアキーボードの表示/非表示** | **ソフトウェアキーボードの表示/非表示** | 該当なし（no-op） | 未検証 |
| preedit のカーソル位置 | byte offset（`Option<(usize, usize)>`） | byte offset | byte offset（**常に `start == end` の単一点**） | byte offset | 該当なし（preedit 非対応） | 該当なし（preedit 非対応） | **非公開**（`data` 文字列のみ） | 未検証 |
| 文字インデックス基準（生 API） | UTF-8 byte offset | UTF-8 byte offset | UTF-8 byte offset | UTF-8 byte offset | UTF-16 code unit offset（`NSRange`、未使用） | UTF-16 code unit offset（`CharSequence`、未使用） | 非公開 | 未検証 |

## 3. 既知制約一覧

各項目に「内容 / 根拠 / 影響 / 緩和策 / 状態」を記載する。

### 3.1 iOS: インライン preedit（marked text）未対応

- **内容**: winit 0.30 系の iOS バックエンド（`platform_impl/ios/{view.rs,window.rs}`）は
  `UITextInput` プロトコル（marked text API）を実装しておらず、`UIKeyInput`
  （`insertText:`/`deleteBackward`）のみに対応する。したがって `WindowEvent::Ime(Ime::*)`
  は iOS では一切発火しない。
- **根拠**: TASK-8.5-S16（#197）ソースレベル検証。
  `ime-architecture.md` §4.5「iOS（UIKit）」・§9。
- **影響**: 変換途中の未確定文字列（preedit）をコア側の状態として表示できない。
  確定済み文字列のみが `insertText:` 経由で 1 文字ずつ届く commit 型入力に限定される。
- **緩和策**: `fandhe-winit-ime` の `text_field_input_from_key` が commit 型入力を
  `TextFieldInput::InsertText`/`DeleteBackward`（既存の編集入力契約）へ変換する。
  `ImeRequest::SetEnabled` はキーボードの表示/非表示（`becomeFirstResponder`/
  `resignFirstResponder`）として作用する。カーソル矩形通知・`SetPurpose` は no-op。
- **状態**: **winit バックエンド側は未対応（恒久制約）**。解消には winit 側の `UITextInput`
  対応、または本フレームワーク独自の `UITextInput` ビュー実装が必要（本文書の対象範囲外の
  設計判断）。**egui-proxy での迂回は 2026-09-07 に実機確認済み**:
  #795（PR #798）で egui-proxy の iOS 経路に ObjC ランタイムのバイパス方式を組み込み、
  零サイズの `UITextField` を first responder にすることで日本語の変換・確定・Backspace を
  egui のテキスト欄に到達させることを確認。ただし **egui-proxy は検証用プロキシに過ぎず**、
  本実装レンダラ層（REQ-8）では winit の制約が依然残る。**S15（#196）が独自 `UITextInput`
  ビュー実装の詳細設計を具体化済み**（`ImeRequest::SetCursorArea` の pull 型アダプタ変換・
  `UITextInputStringTokenizer` 既定実装の採用・`Enabled`/`Disabled` イベントを UIKit から
  発火しない契約。`docs/architecture/ime-ios-uikit-text-input.md` §4.1・§9 参照）。本実装
  レンダラの採用は採用条件（REQ-10 観点の重大性判断・winit 側進展）を満たした場合に
  ユーザー承認を経て別 issue として起票する設計判断である。

### 3.2 Android: インライン preedit（`setComposingText` の反映）未対応

- **内容**: winit 0.30 系の Android バックエンド（`platform_impl/android/mod.rs`）は
  `InputEvent::KeyEvent` のみを処理し、`InputEvent::TextEvent`（`InputConnection`/
  GameTextInput 由来の composing text）を無視する。さらに `InputEvent::TextEvent` 自体、
  android-activity 0.6.1 では GameActivity バックエンド限定でのみ発火し、本リポジトリが
  採用する NativeActivity 方式では到達経路が存在しない。
- **根拠**: TASK-8.5-S18（#199）・S19（#200）ソースレベル検証、
  `docs/architecture/ime-android-input-connection.md`。
  `ime-architecture.md` §4.5「Android（`InputConnection`）」・§9。
- **影響**: iOS と同型。commit 型（KeyEvent 経由の 1 文字ずつの確定文字列挿入）に限定される。
- **緩和策**: `fandhe-winit-ime` の `text_field_input_from_key`（iOS と同一の変換規則）。
  `InputMethodManager.show_soft_input`/`hide_soft_input` に `ImeRequest::SetEnabled` を委譲。
- **状態**: **winit バックエンド側は未対応（恒久制約）**。解消には GameActivity +
  `GameTextInput` 方式への移行（既存 issue #241・TASK-9.3、`ime-android-input-connection.md`
  §4.1 で設計済みだが実装は S19 のスコープ外）、または独自の `InputConnection` 実装が必要。
  **egui-proxy での迂回は 2026-09-07 に実機確認済み**: #796（PR #797）で egui-proxy の
  Android 経路に GameActivity の `GameTextInput` ポーリング機構を組み込み、毎フレーム状態を
  確認して日本語 12 キー入力（「あいう」→「愛兎」確定）・英字・Backspace を egui のテキスト
  欄に到達させることを確認。ただし **egui-proxy は検証用プロキシに過ぎず**、本実装レンダラ層
  （REQ-8）では winit の制約が依然残る。なお、複数文字が 1 回の `KeyEvent` にまとまって
  届いた場合に分割されず原子的に挿入される契約は結合テストで確認済み。

### 3.3 `SetPurpose`（`ImePurpose::Password`）の学習・予測変換抑止がプラットフォーム間で不均一

**セキュリティ注意（重要）**: 本制約はパスワード等の機微情報入力に直接関わる。

- **内容**: `Window::set_ime_purpose` は winit 0.30 系で macOS・Windows・Linux X11・iOS・
  Android・Web では **no-op** であり、`ImeRequest::SetPurpose(Password)` を渡してもこれらの
  プラットフォームでは IME の学習・予測変換抑止が保証されない。**Linux Wayland のみ**
  `ContentHint`/`ContentPurpose` へ実際にマップされ、コンポジタへ伝わる（ヒントに留まり
  反映を保証しない）。
- **根拠**: TASK-8.5-S9（#190）・S11（#192）・S13（#194）ソースレベル検証。
  `ime-architecture.md` §9。
- **設計上の禁則**: `SetPurpose(Password)` に `SetEnabled(false)` を紐づけてはならない。
  iOS・Android では `SetEnabled` が学習・予測変換ではなく**ソフトウェアキーボードの
  表示/非表示そのもの**に委譲されるため、パスワードフィールドへのフォーカスで
  キーボードごと隠れて入力できなくなる。現行コアは `Password` でも `SetEnabled` を他の
  purpose と同様に扱い（常に `true` を要求）、`SetPurpose(Password)` の意図表明のみを
  プラットフォーム層へ送る。
- **影響**: **Wayland 以外のプラットフォームでは、パスワード等の機微情報フィールドで
  IME の学習・予測変換候補に入力内容が残る可能性がある**。アプリケーション開発者は
  `ImePurpose::Password` の指定を「IME 学習・予測変換抑止の保証」として前提にしては
  ならない。
- **緩和策**: `TextFieldWidget::set_ime_purpose` / `apply_input` 実装が
  `SetPurpose(Password)` の意図表明のみを送る（`SetEnabled` は入力対象でなくなる
  `FocusLost`/`set_disabled(true)` 等の経路でのみ無効化）。
- **状態**: プラットフォームごとに対応が分かれる（Issue #586）。
  - **Android**: `fandhe-winit-ime::android_editor_info::apply_ime_purpose_android`
    が `AndroidApp::set_ime_editor_info` へ `TYPE_TEXT_VARIATION_PASSWORD` +
    `TYPE_TEXT_FLAG_NO_SUGGESTIONS` + `IME_FLAG_NO_PERSONALIZED_LEARNING` を反映する
    （winit を介さず `android-activity` API へ直接委譲。`SetEnabled` には触れない）。実機での
    学習辞書残留の最終確認は Issue #586 / #588 へ人間担当として申し送り（`TextFieldWidget` を実行アプリへ配線する
    前提が別途必要）。
  - **macOS**: `fandhe-winit-ime::sensitive_input::SensitiveInputTracker` が
    `SetPurpose`/`SetEnabled` を観測し、パスワードフィールドへ実際にフォーカスがある
    区間のみ Carbon HIToolbox の `EnableSecureEventInput`/`DisableSecureEventInput`
    （`NSSecureTextField` と同じ機構）を呼ぶ。IME 自体が無効化される点に注意
    （`NSSecureTextField` と同挙動。`docs/ime/ime-usage-guide.md` §3.5 参照）。
  - **Windows**: 設計のみ記録し実装は別 issue（TSF `SetInputScope(HWND, IS_PASSWORD)`。
    開発ホストに Windows ターゲット未導入・CI に Windows ジョブなしのため実装・検証が
    できない）。
  - **iOS / Linux X11 / Web**: **対応不能**（iOS は winit の `UITextInputTraits`
    が空実装で独自 `UITextInput` ビューが前提、X11 に相当 API なし、Web の DSL
    （`fandhe-web-dom`）に secure 属性の配線がない）。開発者はこれらのプラットフォームで
    `ImePurpose::Password` を学習・予測変換抑止の保証として前提にしてはならない。

### 3.4 フォーカス中の Password 切替による preedit の遅延 Commit 混入

- **内容**: `Password` へのフォーカス中切替はローカルの composition を（確定せず）破棄するが、
  §3.3 の設計（`SetEnabled(false)` を送らない）により、切替直前の preedit がその後
  `Commit`（特に Windows の `WM_IME_ENDCOMPOSITION`）として遅延到着する可能性が残る。
- **根拠**: `SetEnabled(false)` を送らない設計（§3.3）と `ImeEvent` の相関トークン不在。
- **影響**: purpose 切替直後に意図しない文字列が確定挿入されうる。
- **緩和策**: `TextFieldWidget` がワンショットの専用ガード（`ime_purpose_switch_guard`）
  を導入し、破棄直後に届く最初の `Commit` のみを飲み込む。`Enabled`/`Disabled`/新しい
  `Preedit` のいずれかが先に届いた場合はガードを解除する（`text_field.rs` の `apply_ime`
  実装参照）。
- **状態**: ワンショットガードで緩和。構造的限界は「本当に stale か」を厳密には
  区別できないこと（§3.6 のセッション識別子不在と同根）。

### 3.5 Windows: `Disabled`→`Enabled` 高頻度再サイクル

- **内容**: winit の Windows バックエンドは `Ime::Disabled` をフォーカス喪失時だけでなく、
  コンポジションが確定するたび（`WM_IME_ENDCOMPOSITION`）に発火する。macOS
  （入力ソース切替時のみ）より高頻度であり、日本語・韓国語入力等で変換を確定するたびに
  `Disabled`→`Enabled` の再サイクルが発生しうる（韓国語 2 ボル式は 1 打鍵ごとに音節が
  完成しうるため頻度がさらに高い、S25 所見）。
- **根拠**: TASK-8.5-S11（#192）・S12（#193）・S25（#206）検証。
- **影響**: 状態の巻き戻し・ちらつきのリスク。
- **緩和策**: `TextFieldWidget::apply_ime` の `Disabled` 処理が composition のクリア
  のみで冪等なため、高頻度な再サイクルを状態モデル変更なく許容できることを結合テスト
  （`crates/fandhe-winit-ime/tests/windows_japanese_input.rs`、6 シナリオ）で確認済み。
- **状態**: 確認済み・対応不要（既存状態モデルで許容）。実機（Microsoft IME）での実操作
  検証は Issue #588 の対象。

### 3.6 フォーカス遷移直後の旧フィールド由来の遅延 Commit 混入

- **内容**: `dispatch_ime` は `WidgetId` のみでルーティングするため、フィールド A → B の
  フォーカス移動直後に A の IME セッション由来の遅延 `Commit` が届くと、B 単体のフォーカス
  ガードではこれを判別・破棄できず、B への誤った入力挿入になりうる。
- **根拠**: `dispatch_ime` の `WidgetId` 単独ルーティング（S24・#205）。`ime-architecture.md` §4.8。
- **影響**: 複数テキストフィールドを持つ画面で、フォーカス切替直後に誤入力が混入しうる。
- **緩和策（二段構え、Issue #587）**: 第一段としてセッション識別子照合を導入した。
  `ImeFocusCoordinator::focus`（`crates/fandhe-core/src/ui/ime_focus.rs`）が新フィールドを
  IME 有効化するたびに `ImeSessionId`（`ui::ime::ImeSessionId`）を払い出し、新フィールドの
  `SetEnabled(true)` 等より前に `ImeRequest::BeginSession(id)` を発行する。`dispatch_ime` は
  `fields`・`session: Option<ImeSessionId>`・`event` を受け取り、`self.session()` と一致しない
  イベント（`Enabled`/`Preedit`/`Commit`、`None` を含む）を `Disabled` 以外すべて破棄する
  （`Disabled` はセッション不問で全フィールドへ配る §4.6 契約を維持）。プラットフォーム層
  （`fandhe-winit-ime::session::ImeSessionTracker`）が `BeginSession`/`Ime::*` の観測列から
  帰属セッションを算出し、`ime-architecture.md` §4.8 の付け替え規則（`Disabled`/`Enabled` を
  昇格契機とする「Enabled ゲート」、旧フィールドが blur 要求済みの間は非空 `Preedit` を昇格
  契機にしない）で旧セッションのイベントを新セッションと誤認しない。第二段として、旧来の
  `Commit` 専用ゲート（`armed`。「`Commit` の直前には必ず空文字列の `Preedit` が届く」契約を
  利用した 1 ショットゲート）を撤去せず多層防御として維持する。
- **状態**: **Windows 形（`Disabled` 到達前に旧セッションの「空 `Preedit` → `Commit`」が届く、
  本制約が主対象としていた形）は識別子照合で解決した**（
  `crates/fandhe-winit-ime/tests/focus_transition_session.rs` で結合テスト済み）。
  **Enabled ゲートの根拠**: blur 要求済みの間に非空 `Preedit` を昇格契機にすると、native が
  キューイング済みの旧フィールドのイベントが `Enabled` を伴わずに届くプラットフォーム
  （Windows 等）で誤帰属するため、`Enabled` のみを開始信号とする
  （`crates/fandhe-winit-ime/src/session.rs` モジュール doc・
  `blurred_previous_session_delayed_preedit_does_not_leak_commit_into_next_session` で
  固定）。**残存限界（Wayland）**: `zwp_text_input_v3` の `done` ハンドラが
  `ime_allowed`/`serial` を見ずに `Enabled` 発火**後**の in-flight バッチ（空 `Preedit` →
  `Commit` → 次の `Preedit`）を送出する形は、Enabled ゲートでも判別できない（winit 側の
  実装上の限界、`session` モジュール doc §4.8 参照）。この残存経路は upstream（winit）側で
  `Done` ハンドラが serial/`ime_allowed` を判定するよう修正される必要がある（追跡 Issue
  **#696**）。
- **残存限界（Windows）**: winit 0.30.13 の
  `set_ime_allowed`（`src/platform_impl/windows/event_loop.rs`）は `ImmAssociateContextEx`
  を呼ぶだけで `ime_state` をリセットせず、`WM_IME_*` ハンドラ（`ime.rs`）は到着時点の
  `ime_allowed`/`ime_state` しか見ない。IMM32 単独駆動なら旧フィールド A のキー入力由来の
  `WM_IME_*` は対応する `WM_KEYDOWN` の処理中に同期配送されるため、A→B フォーカス遷移前に
  必ず届き切る。しかし TSF（Text Services Framework）経由の通知タイミングを保証する一次
  情報がなく、A の `WM_IME_ENDCOMPOSITION → WM_IME_STARTCOMPOSITION → WM_IME_COMPOSITION`
  という `Disabled→Enabled` 再サイクルが `BeginSession(B)` の**後**に届く可能性を否定
  できない。届いた場合、`Disabled` で B へ昇格・後続の `Enabled` が「B 自身の確認」として
  通過し、続く A の `Preedit`/`Commit` が B へ誤って stamp される（Enabled ゲートは
  `Disabled`/`Enabled` の組自体が「A の再サイクル」か「A→B 遷移」かを区別する情報を
  持たないため判別できない）。macOS（`set_ime_allowed(false)` が同期的に `ime_state` を
  `Disabled` へ落とす）・Linux X11（フォーカス遷移で XIC が破棄・再生成される）では
  この経路は発生しない。是正は winit 側（`set_ime_allowed(false)` での `ime_state`
  リセット・旧コンテキスト由来メッセージの破棄）が必要（追跡 Issue **#696**、Wayland の
  残存限界と同一 Issue で扱う）。

### 3.7 Linux（X11・Wayland 共通）: `Enabled`/`Disabled` 発火タイミングが macOS/Windows と逆方向

- **内容**: winit の Linux バックエンドは `Ime::Enabled`/`Ime::Disabled` を
  `ImeRequest::SetEnabled` 呼び出し・フォーカス変化へ**同期的・即時**に発火する。macOS・
  Windows がコンポジション開始/終了まで遅延発火するのとは逆方向（第三のパターン）。
- **根拠**: TASK-8.5-S13（#194）・S14（#195）検証。
- **影響**: コンポジション状態モデル・テキストフィールド配線がプラットフォーム間で
  発火タイミングの向きが異なることを許容する必要がある。
- **緩和策**: 結合テスト（`crates/fandhe-winit-ime/tests/linux_japanese_input.rs`、
  7 シナリオ）で既存状態モデルが変更なく許容できることを確認済み。
- egui プロキシ経路（`fandhe-egui-proxy` の `text_edit_singleline`）は、fcitx5 既定設定の
  XIM（`PreeditNothing`）で `Preedit` が一切届かないと `egui::TextEdit` 側の
  `ime_cursor_range`（`Enabled`/`Preedit` 受信時のみ更新）が既定値のまま残り、カーソルが
  先頭以外のときに `Commit` が無言で破棄されていたが（Issue #616 症状 B）、`Commit` 直前へ
  同一文字列の `Preedit` を合成注入して同期する対処で解消した（#663、
  `crates/fandhe-egui-proxy/src/lib.rs` の `inject_preedit_before_ime_commit`）。
- **状態**: 確認済み・対応不要（Enabled/Disabled 発火タイミング自体）。egui プロキシ経路は
  #663 で対応済み。実機（ibus-mozc/fcitx5-mozc）検証は Issue #588 の対象。

### 3.8 Linux X11: `SetCursorArea` が IME に無視されうる既知バグ

- **内容**: X11 では freedesktop 側の既知バグにより、`set_ime_cursor_area` で通知した
  カーソル矩形を IME（XIM 実装側）が無視し、候補ウィンドウが意図しない位置に表示される
  ことがある。
- **根拠**: TASK-8.5-S13（#194）検証。
- **影響**: X11 環境での変換候補ウィンドウの表示位置がずれる可能性がある（見た目の問題、
  入力内容の正しさには影響しない）。
- **緩和策**: なし（winit・XIM 実装側の既知の制約であり、フレームワーク側での
  回避策はない）。
- **状態**: 監視事項（winit・XIM 側の改善を待つ）。

### 3.9 Web（DOM）: `compositionupdate` のカーソル位置非公開・二重表示回避のための未反映設計

- **内容**: W3C UI Events の composition イベントはカーソル範囲を公開しないため、
  `ImeEvent::Preedit.cursor` は Web 経路では常に `None` になる。また既存
  `fandhe-web-dom/src/mount.rs` は「DOM の `<input>` 要素へブラウザの IME を素通しする」
  方式であり、ブラウザ自身が preedit の視覚表現を提供する。二重表示を避けるため、S21（#202）
  の実装は `compositionstart`/`compositionupdate` をコア描画へ反映せず no-op のままとし、
  `compositionend`（`Commit`）到達時のみ既存の commit 経路を呼ぶ設計とした。
- **根拠**: `ime-architecture.md` §4.5「Web」・§4.6・§9、TASK-8.5-S21 検証。
- **影響**: 独自レンダラ方式（macOS/iOS/Android/デスクトップ Linux/Windows）ほど精密な
  候補ウィンドウ位置計算は Web では成立しない。コア側は「preedit 文字列の末尾にキャレットを
  置く」既定動作へフォールバックする契約だが、Web では preedit 自体を描画反映しないため
  この既定動作は事実上使われない。
- **緩和策**: `compositionend` を commit の正とし、`input.value()` を全文置換で
  反映する既存方式を維持。
- **状態**: 設計判断として確定済み（ブラウザの既定候補ウィンドウに委ねる）。

### 3.10 Android: `finishComposingText()` の意味論

- **内容**: `finishComposingText()` は Android 仕様上「テキストは残したままコンポジション
  状態のみ解除する」動作であるため、`ImeEvent::Commit(現在の preedit text)` 相当として扱い、
  `Disabled` は発火しないことを確定した（`Disabled` 相当＝クリアして破棄とすると変換確定済み
  文字列が失われるため不採用）。
- **根拠**: TASK-8.5-S18（#199）で確定済み、
  `docs/architecture/ime-android-input-connection.md` §4.4。
- **影響**: なし（設計判断として確定済み、実装 S19 に反映済み）。
- **状態**: 確定済み。

### 3.11 `fandhe-web-dom` の Closure リーク（既知制約、S21 スコープ外）

- **内容**: `fandhe-web-dom/src/mount.rs` の `attach_input` が保持する `wasm-bindgen`
  `Closure` は、既存実装のリソース解放方針の対象外であり、IME 統合（`ime.rs` の追加）
  自体はこの既存の制約を変更していない。
- **根拠**: TASK-8.5-S21 検証（既知の制約として記載）。
- **影響**: 長時間・繰り返しマウント/アンマウントを行うページでのメモリ使用量増加リスク
  （IME 統合固有の新規リスクではなく、既存 `mount.rs` の設計に起因）。
- **状態**: 本 issue のスコープ外（`fandhe-web-dom` の既存設計の見直しが必要、§5 参照）。

### 3.12 watchOS / visionOS: `winit`/`accesskit` 対応が発展途上

- **内容**: `winit`/`accesskit` の watchOS・visionOS 対応は発展途上であり
  （`crate-feature-matrix.md` §5「watchOS/visionOS 制約セクション」）、キーボード入力自体の
  UIKit 経由対応を含め IME 統合はこれらのプラットフォームでは未着手。
- **根拠**: `ime-architecture.md` §2 REQ-8 受け入れ基準 3・§9、`crate-feature-matrix.md` §5。
- **影響**: watchOS/visionOS では本文書のマッピング表（§2）の対象外。
- **状態**: TASK-8.6（別 issue、REQ-3 反映）の結果を待って本書へ追加するかを判断する。
  spec の要件変更に相当する事項であり、本 issue では扱わない。

### 3.13 `winit` Issue #1497 の進展監視

- **内容**: IME/composition support の Tracking issue が 2026-07 時点でも open のまま。
- **根拠**: [`winit` Issue #1497](https://github.com/rust-windowing/winit/issues/1497)、
  `crate-feature-matrix.md` §3.4。
- **影響**: winit が `Ime` 列挙・`set_ime_*` API を変更した場合、`ime-architecture.md` §4.4
  のマッピング表・本書 §2・§3 の追随更新が必要になる。
- **状態**: 監視事項（upstream 変更時に追随、`docs/operations/upstream-watch-*` の前例に
  倣った追随手順を検討する）。

### 3.14 Linux X11（XIM）: preedit のインライン表示は IM サーバーの on-the-spot 広告に依存

- **内容**: デスクトップ版（eframe 分岐、`fandhe-egui-proxy`）の Linux X11 経路は
  `winit → egui-winit → egui::TextEdit` を直結しており、`fandhe-winit-ime`（§3.7 が対象とする
  クレート）は経路上にない。winit の X11 IME 実装（`x11/ime/input_method.rs`）は IM サーバーが
  広告するスタイル一覧から `XIMPreeditCallbacks|XIMStatusNothing`（on-the-spot）を優先し、
  無ければ `XIMPreeditNothing|XIMStatusNothing` へフォールバックする。`x11/ime/context.rs` は
  on-the-spot のときのみ `preedit_start`/`draw`/`done`/`caret` コールバックを登録するため、
  フォールバック時は `winit::event::Ime::Preedit` が一切発行されず、IME 側のポップアップ
  ウィンドウにのみ未確定文字列が表示される（症状 A、Issue #616）。
- **根拠**: winit-0.30.13（`x11/ime/input_method.rs` L61-72・L90-92、`x11/ime/context.rs`
  L208-223、`x11/event_processor.rs` L107-122）。fcitx5（`src/frontend/xim/xim.cpp`）は
  on-the-spot スタイル配列を設定 `useOnTheSpot`（既定 `false`）が true のときのみ広告する。
  egui-winit-0.33.3（`src/lib.rs` L367-390）は Linux では `winit::Ime::Enabled` を無視し、
  最初の `Ime::Preedit(_, Some)` 受信時にのみ `egui::ImeEvent::Enabled` を合成する。
  egui-0.33.3（`src/widgets/text_edit/builder.rs` L1067-1106）の `TextEdit` 状態は
  `Enabled`/`Preedit` でのみ `ime_cursor_range` を更新する。Issue #616 実測、Issue #662
  実装時の回帰テスト（`crates/fandhe-egui-proxy/tests/ime_preedit_render.rs`）で固定。
- **影響**: 既定設定の fcitx5 + winit の組み合わせでは常に `PreeditNothing` へ後退し、
  症状 A が構造的に発生する。加えて egui-winit の Linux 特例により `Enabled` も合成されず
  `ime_cursor_range` が既定値（index 0）のまま残るため、カーソルが index 0 以外での
  `Commit`（確定）が黙って破棄される（症状 B、Issue #663）。逆に Preedit が TextEdit へ届けば
  （本項目が解消すれば）、`Enabled`/`Preedit` 受信のたびに `ime_cursor_range` が現在の
  カーソル位置へ正しく更新されるため、後続の `Commit` は `cursor_range` との一致判定を通過し、
  症状 B も連鎖的に解消する（Commit の確定文字列と直前の Preedit 残留文字列が異なる場合でも
  一致判定を通過した Commit 自体の値が適用されることを、上記回帰テストの
  `preedit_is_displayed_inline_and_commit_applies_after_existing_text` が固定する）。
  `fandhe-core`/`fandhe-winit-ime` の状態モデルはこの経路に一切関与しない
  （`crates/fandhe-core/src/ui/text_field.rs` の IME 処理は本経路では使われない）。
- **緩和策**（利用者環境側）: fcitx5 の XIM フロントエンド設定で on-the-spot を
  有効化する（`~/.config/fcitx5/conf/xim.conf` に `UseOnTheSpot=True`を追加、または
  `fcitx5-configtool` → Addons → X Input Method Frontend → 「Use On The Spot Style」を有効化）
  後、fcitx5 を再起動する（`fcitx5 -r -d`）。設定キー名・UI 文言は fcitx5 のバージョンに
  依存するため実機で最終確認する。WSLg 環境では `WAYLAND_DISPLAY` を unset し
  `XMODIFIERS=@im=fcitx` の X11 経路を使うことが前提（Issue #616 記載の再現条件）。
  proxy・コア側から XIM スタイル交渉へ介入する手段はない（winit 内部の実装であり、上流
  winit への `XIMPreeditPosition` 対応提案は未起票の検討事項として残す）。
- **状態**: ソースレベル確認済み・環境設定で緩和・実機検証は Issue #588 の対象。
  実機確認手順（チェックリスト）: (1) 既定設定で症状 A/B が再現することを対照として確認する
  → (2) `UseOnTheSpot=True` 設定後、既存テキスト末尾に日本語 preedit がインライン表示され、
  確定（変換候補確定）が反映されることを確認する。変換候補ウィンドウの位置ずれ自体は §3.8 の
  既知事項であり本項目のスコープ外。

## 4. S29 で確定する設計判断

### 4.1 コンポジション状態のアクセシビリティツリー非公開（確定）

コンポジション状態はアクセシビリティツリーへ公開しない。確定内容・根拠は以下のとおり。

- **確定内容**: コンポジション中のテキストフィールドは既存 `text_field.rs`/`text_input.rs`
  の `AccessibilityRole::TextInput` のまま扱い、preedit（未確定文字列）自体を
  `accesskit` のアクセシビリティツリーへ個別ノードとして追加公開**しない**。
- **根拠**:
  1. `accesskit` はプラットフォームのアクセシビリティ API へアクセシビリティツリーを
     公開する共通抽象を提供するが（`crate-feature-matrix.md` §3）、コンポジション
     （preedit）状態を表現する専用 API・慣行の有無は本 issue の調査範囲（同文書・
     `ime-architecture.md`）では確認できていない。安定した専用 API がないまま追加公開する
     判断は時期尚早であり、確認できていないこと自体を「非公開を維持する」根拠とする
     （安全側に倒す判断、以下 2〜3 と合わせて総合判断）。
  2. デスクトップ 3 プラットフォーム（macOS・Windows・Linux）ではネイティブ IME 自身が
     スクリーンリーダーへの読み上げ・通知を担う慣行があり（`NSTextInputClient`・
     IMM32/TSF・ibus/fcitx がそれぞれ OS のアクセシビリティ層と連携する）、
     アプリケーション側から preedit を二重に公開すると読み上げの重複・矛盾が生じる
     リスクがある。
  3. iOS・Android は commit 型入力（preedit 自体が存在しない、§3.1・§3.2）のため、
     現状はそもそも公開すべき状態がない。
- **将来の再検討条件**: 以下のいずれかが成立した場合に本判断を再検討する。
  - `accesskit` がコンポジション・preedit 表現の安定 API を提供した場合。
  - iOS（S15・#196）・Android（GameActivity 移行後）でインライン preedit 対応
    （§3.1・§3.2 の恒久制約解消）が実装され、これらのプラットフォームで
    アプリ独自レンダラが preedit を描画するようになった場合（ネイティブ IME の
    読み上げ慣行に委ねられなくなるため）。
- **本判断の影響範囲**: `crates/fandhe-core` の実装（`text_field.rs`/`ime.rs` 等）を
  変更するものではなく、既存実装の設計判断を確定・文書化するのみ。コード変更は伴わない。

### 4.2 iOS・Android インライン preedit 未対応の恒久制約整理（確定）

iOS・Android のインライン preedit 未対応は上流依存の恒久制約として扱う。確定内容・
解消経路は以下のとおり（iOS の詳細設計は `docs/architecture/ime-ios-uikit-text-input.md`
参照）。

- **確定内容**: iOS・Android のインライン preedit（未確定文字列のコア描画）は、
  **winit 0.30 系および現行 android-activity（NativeActivity 方式）に起因する
  上流依存の恒久制約（当面）**として整理する。フレームワーク独自の回避策では
  解消できない構造的制約であり、「いつか直る一時的な不具合」ではなく「アーキテクチャ上の
  既知の非対応領域」として扱う。
- **解消経路（将来課題、本書の対象範囲外）**:
  - **iOS**: winit 側が `UITextInput` プロトコル（marked text API）を実装する
    アップデートを待つ、または本フレームワークが独自の `UITextInput` 準拠ビューを
    実装して winit をバイパスする。**S15（#196）で後者（独自ビュー実装）のみが
    現実的な選択肢であると確定した**（winit 側の対応待ちはスケジュールと両立しない
    ため不採用、`ime-ios-uikit-text-input.md` §4.1・§6 参照）。独自ビューの詳細設計
    （`ImeRequest::SetCursorArea` の内部変換・`UITextInputTokenizer` 整合・
    ソフトウェアキーボードのライフサイクル）は同文書 §4.2〜§4.7 で確定済みだが、
    実装着手はユーザー承認を要する別 issue の判断であり、本恒久制約自体は解消していない。
  - **Android**: 既存 issue #241・TASK-9.3 の GameActivity + `GameTextInput` 方式への
    移行（設計は `ime-android-input-connection.md` §4.1 で完了済み、実装は同 issue の
    スコープ）、または独自の `InputConnection` 実装。
- **現状の commit 型入力で担保されている範囲**: 確定済み文字列の 1 文字ずつの挿入は
  日本語・韓国語・中国語を含む CJK 変換入力で機能する（変換候補バーでの確定操作は
  OS 側のソフトウェアキーボードが担うため）。**失われるのは「変換途中の見た目
  （下線付き未確定文字列の表示）」のみであり、最終的に確定される文字列の正しさ自体は
  損なわれない**。

## 5. 追記運用・追跡先

新たな制約は §3 へ追記する。恒久対応が必要なものは
`.claude/rules/out-of-scope-tracking.md` の規約に従って Issue 化する。

### 追跡中の Issue

- **Issue #696**: Windows/Wayland の winit 側残存限界（§3.6）。
- **Issue #241**: Android GameActivity 移行（§3.2・§4.2 の解消経路）。

### 実機検証未了チェックリスト

着手判断・PASS 条件・実施手順・結果記入欄の追跡先は Issue #588。

- macOS 日本語入力の実機確認
- Windows 実機（Microsoft IME）検証
- Linux 実機（ibus-mozc/fcitx5-mozc）検証
- iOS 実機検証
- Android 実機検証
- 韓国語・中国語 IME 実機検証（macOS 韓国語 IME・Microsoft Pinyin・ibus-hangul/
  fcitx5-rime 等）
- Web ブラウザでの実操作検証
- Linux X11 の XIM on-the-spot 依存（§3.14、症状 A/B）の WSL2 / fcitx5-mozc 実機確認

これらが完了した時点で、本書 §3 の該当項目の「状態」欄を更新する。§3.14 のソースレベルの
切り分け・回帰テスト・環境側緩和策自体は Issue #616 で確定済み。

## 6. 出典

- [`winit::event::Ime`（docs.rs、latest）](https://docs.rs/winit/latest/winit/event/enum.Ime.html)
- [`winit::window::Window`（docs.rs、latest）](https://docs.rs/winit/latest/winit/window/struct.Window.html)
- [Tracking issue for IME / composition support — `winit` Issue #1497（GitHub）](https://github.com/rust-windowing/winit/issues/1497)
- [W3C UI Events — Composition Events](https://w3c.github.io/uievents/#events-compositionevents)
- [Apple Developer Documentation: `UITextInput`](https://developer.apple.com/documentation/uikit/uitextinput)
- [Android Developers: `InputConnection`](https://developer.android.com/reference/android/view/inputmethod/InputConnection)
- `docs/architecture/ime-architecture.md`（§2・§4・§9、契約設計・未解決事項の原本）
- `docs/architecture/ime-android-input-connection.md`（Android 詳細設計）
- `docs/architecture/crate-feature-matrix.md`（`winit`/`cosmic-text` の IME 対応状況調査）
