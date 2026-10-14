//! コア⇔プラットフォーム層の IME（Input Method Editor）統合境界型。
//!
//! `docs/architecture/ime-architecture.md`（S1 / #182、設計文書）
//! §4.1 のシグネチャ案を実装化したもの。`fandhe-core` は `#![forbid(unsafe_code)]` の
//! クレートであり、`winit::event::Ime` 等のプラットフォーム型に一切依存しない
//! （REQ-2 の依存方向: 最適化レイヤ → コアの一方向）。本モジュールはその境界の
//! コア側の実体であり、プラットフォーム層（`fandhe-winit-ime` 等、TASK-8.5-S7・S9〜S22）が
//! ネイティブ IME イベントを本モジュールの型へ変換して呼び出す契約になっている。
//!
//! - [`ImeEvent`]: プラットフォーム層からコアへ届く、IME コンポジションの状態遷移
//!   （§4.1「コア⇔プラットフォーム契約」）
//! - [`ImeRequest`]: コアからプラットフォーム層への要求（§4.2「コア → プラットフォームの要求契約」）
//! - [`ImePurpose`]: 入力目的ヒント（§4.1・§4.2、winit `ImePurpose` 相当）
//!
//! コンポジション状態モデル（`Composition` 等の保持・遷移管理）は本モジュールの対象外
//! （TASK-8.5-S3 / #184 のスコープ）。本モジュールは境界型定義とその変換補助のみを担う。

use super::text_selection::CharOffset;

/// IME セッション識別子（Issue #587）。フォーカス遷移直後に旧フィールドの IME セッション由来の
/// 遅延 `Preedit`/`Commit` が新フィールドへ誤配置される（機微情報混入を含む）ことを防ぐための
/// 帰属情報。`ImeFocusCoordinator::focus`（`ime_focus.rs`）が新フィールドを IME 有効化するたびに
/// 払い出し、`ImeRequest::BeginSession` でプラットフォーム層へ通知する。プラットフォーム層
/// （`fandhe-winit-ime` 等）は以降に届く `ImeEvent` へこの識別子を帰属させ、
/// `ImeFocusCoordinator::dispatch_ime` へ渡す（付け替え規則は `ime-architecture.md` §4.8）。
/// 内容は不透明な連番で、外部入力を含まない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ImeSessionId(u64);

impl ImeSessionId {
    /// 最初のセッション（`ImeFocusCoordinator` の初期値）。
    pub const FIRST: Self = Self(1);

    /// 次のセッション識別子を返す。`wrapping_add` を使うが、等値比較にのみ用いる識別子であり
    /// 実運用でのフォーカス遷移回数が `u64` を一周することは想定されないため、回り込みは無害。
    #[must_use]
    pub fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// プラットフォーム層（TASK-8.5-S7・S9〜S22）からコア（TASK-8.5-S2〜S8）へ届く、
/// IME コンポジションの状態遷移を表す非依存イベント（`ime-architecture.md` §4.1）。
///
/// 外部由来の生バイト列・キーコードを直接扱わず、意味づけ済みの列挙のみで構成する
/// （`TextFieldInput` / `InteractionInput` と同じ OWASP A03 対策の型レベル防御、
/// `.claude/rules/security.md` 準拠）。
///
/// `Preedit` / `Commit` は機微情報（パスワード入力中の未確定・確定文字列等）を
/// 含みうるため、`Debug` は手書き実装で文字数のみを出力する（§7 セキュリティ考慮、
/// 既存 `TextFieldWidget` の `buffer_len` マスク方針を継承。設計文書のシグネチャ案は
/// `#[derive(Debug)]` だが、この型のみ意図的に逸脱する）。
#[derive(Clone, PartialEq, Eq)]
pub enum ImeEvent {
    /// IME が有効化された（フォーカス獲得直後、`ImeRequest::SetEnabled(true)` への応答）。
    ///
    /// プラットフォームによっては即時応答ではない点に注意する（TASK-8.5-S9・S11・S13
    /// ソース検証で確認済み）。
    /// macOS（winit 経由）では `SetEnabled(true)` の直後ではなく、最初のコンポジション
    /// 開始時に遅延発火する。Windows も同型で `WM_IME_STARTCOMPOSITION` 時に遅延発火する。
    /// **Linux（X11・Wayland 共通）はこれらと逆方向に、`SetEnabled(true)` 呼び出しへ
    /// 同期的・即時に発火する**（コンポジション開始を待たない）。
    /// また入力ソース切替（macOS）・**コンポジション確定のたび（Windows）**に
    /// フォーカス継続中でも `Disabled` へ戻ってから再度 `Enabled` が発火する再サイクルが
    /// 起こりうる契約として扱う。Windows では変換確定ごとに毎回発生しうるため、
    /// macOS より高頻度な再サイクルを想定する必要がある。呼び出し元の状態モデルは
    /// プラットフォームごとに発火タイミングの向き（遅延 or 即時）が異なることを
    /// 許容する必要がある。
    Enabled,
    /// コンポジション（未確定文字列）が更新された。
    Preedit {
        /// 現在の未確定文字列全体（空文字列はコンポジションのクリアを表す）。
        text: String,
        /// `text`（未確定文字列）内でのカーソル範囲（`CharOffset` 単位、§4.6 参照）。
        /// `TextFieldWidget::cursor`（確定済み `buffer` 内の位置）とは別の座標系である点に
        /// 注意 — 本フィールドは常に `text` の先頭を 0 とした相対オフセットであり、
        /// `buffer` へのマッピングは S5（プレビュー描画）・S8（テキストフィールド配線）が担う。
        /// `None` はカーソルを表示しないことを表す（winit `Preedit.1 == None` に対応）。
        cursor: Option<(CharOffset, CharOffset)>,
    },
    /// コンポジションが確定した（挿入すべき文字列を伴う）。
    Commit(String),
    /// IME が無効化された（フォーカス喪失、または `ImeRequest::SetEnabled(false)` への応答）。
    /// 次の `Enabled` まで `Preedit` / `Commit` は発生しない契約とする。
    ///
    /// Windows（winit 経由）では、フォーカス喪失時に加えて **コンポジションが確定する
    /// たびに `WM_IME_ENDCOMPOSITION` で発火する**（TASK-8.5-S11 ソース検証）。
    /// macOS の入力ソース切替時のみの発火より高頻度であり、フォーカスが継続していても
    /// 変換確定のたびに `Disabled`→`Enabled` の再サイクルが起こりうる点を、この型を
    /// 受け取るコンポジション状態モデル（TASK-8.5-S3・S8）は許容する必要がある。
    /// Linux（X11・Wayland 共通、TASK-8.5-S13 ソース検証）では `ImeRequest::SetEnabled(false)` 呼び出しへ同期的・即時に
    /// 発火し、コンポジション終了を待たない点が macOS・Windows と逆方向である。
    Disabled,
}

impl std::fmt::Debug for ImeEvent {
    /// `Preedit` / `Commit` の文字列内容は出力せず、文字数のみ記録する
    /// （§7「機微情報の露出防止」。パスワード入力中の未確定・確定文字列がログへ
    /// 漏れることを防ぐ、`TextFieldWidget` の Debug マスクと同じ方針）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Enabled => f.write_str("Enabled"),
            Self::Preedit { text, cursor } => f
                .debug_struct("Preedit")
                .field("text_len", &text.chars().count())
                .field("cursor", cursor)
                .finish(),
            Self::Commit(text) => f
                .debug_tuple("Commit")
                .field(&format_args!("<{} chars>", text.chars().count()))
                .finish(),
            Self::Disabled => f.write_str("Disabled"),
        }
    }
}

/// コア（TASK-8.5-S5〜S8・S23）からプラットフォーム層（TASK-8.5-S9〜S22）への要求
/// （`ime-architecture.md` §4.2）。双方向の要求チャネル（コールバック / イベントキュー等の
/// 実装形態）はプラットフォーム層に委ねる。
#[derive(Debug, Clone, PartialEq)]
pub enum ImeRequest {
    /// IME 有効/無効を切り替える（フォーカスされたテキストフィールドの有無に連動、§4.7）。
    SetEnabled(bool),
    /// 変換候補ウィンドウの位置計算に使うカーソル矩形を通知する（論理ピクセル座標）。
    /// プラットフォーム層が物理ピクセルへの変換・座標系変換を担う（コアはウィンドウの
    /// スケールファクタを知らない、REQ-2 の境界）。
    SetCursorArea {
        /// カーソル矩形の左上原点（論理ピクセル）。
        origin: (f32, f32),
        /// カーソル矩形のサイズ（論理ピクセル）。
        size: (f32, f32),
    },
    /// 入力目的のヒント（S23 の前提）。パスワード等では IME 無効化・学習抑止を要求できる。
    ///
    /// 担保できるかはプラットフォーム実装依存である点に注意する。macOS・Windows・
    /// **Linux の X11** を含む winit 0.30 系の一部プラットフォームでは
    /// `Window::set_ime_purpose` が no-op であることが TASK-8.5-S9（macOS）・
    /// TASK-8.5-S11（Windows）・TASK-8.5-S13（Linux X11）のソース検証で判明しており、
    /// `Password` を渡しても IME 学習・予測変換の抑止が保証されない。**Linux の
    /// Wayland のみ唯一の対応プラットフォームであり**、`ContentHint::SensitiveData`/
    /// `ContentPurpose::Password` としてコンポジタへ実際に伝わる（同 §3.2 表 #5。
    /// 反映を保証するものではなくヒントに留まる）。
    ///
    /// **S23（#204）時点では `ImePurpose::Password` 設定時に本バリアントへ加えて
    /// `ImeRequest::SetEnabled(false)` を発行し IME 自体を無効化する方針だったが、
    /// iOS/Android では `SetEnabled` がソフトウェアキーボードの表示/非表示に直結し
    /// キーボードごと隠れる欠陥（PR #457 Bugbot 指摘）があり撤回済みである。現行の
    /// [`crate::ui::text_field::TextFieldWidget::set_ime_purpose`] / `apply_input` は
    /// `Password` でも常に本バリアントのみを発行し、`SetEnabled` は他の purpose と
    /// 同様に扱う（`text_field.rs` モジュール doc「Password 時のポリシー」参照）。
    /// ヒントが no-op なプラットフォームでの代替担保はプラットフォーム層
    /// （`fandhe-winit-ime`）に委ねる方針へ変更し、Issue #586 で macOS
    /// （SecureEventInput）・Android（`EditorInfo`）向けに実装した
    /// （`docs/architecture/ime-known-constraints.md` §3.3 参照。iOS・Linux X11・
    /// Windows・Web は引き続き非保証）。複数フィールド間の調停は S24（#205）の
    /// スコープであり未解決のまま。
    SetPurpose(ImePurpose),
    /// 新しい IME セッションの開始宣言（Issue #587）。`ImeFocusCoordinator::focus` が新フィールドの
    /// `SetEnabled(true)` 等より前に発行し、以降の `ImeEvent` をこの識別子へ帰属させるようプラット
    /// フォーム層へ促す（付け替え規則は `ime-architecture.md` §4.8）。対応する native API を持たない
    /// プラットフォーム（`fandhe-web-dom` 等）は無視してよい。
    BeginSession(ImeSessionId),
}

/// 入力目的ヒント（winit `ImePurpose` 相当、`ime-architecture.md` §4.1・§4.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImePurpose {
    /// 既定（通常のテキスト入力）。
    Normal,
    /// パスワード等の機微情報（IME の学習・予測変換を抑止するヒント）。
    Password,
    /// 単一行の終端に Enter 相当が来るヒント（Terminal 等）。
    Terminal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_preedit_masks_text_content() {
        let event = ImeEvent::Preedit {
            text: "こんにちは".to_string(),
            cursor: Some((1, 2)),
        };
        let debug = format!("{event:?}");
        assert!(!debug.contains("こんにちは"));
        assert!(debug.contains("text_len: 5"));
        assert!(debug.contains("(1, 2)"));
    }

    #[test]
    fn debug_commit_masks_text_content() {
        let event = ImeEvent::Commit("secret".to_string());
        let debug = format!("{event:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("6 chars"));
    }

    #[test]
    fn debug_enabled_disabled_have_no_payload_to_mask() {
        assert_eq!(format!("{:?}", ImeEvent::Enabled), "Enabled");
        assert_eq!(format!("{:?}", ImeEvent::Disabled), "Disabled");
    }

    #[test]
    fn ime_event_equality_ignores_debug_formatting() {
        let a = ImeEvent::Commit("x".to_string());
        let b = ImeEvent::Commit("x".to_string());
        assert_eq!(a, b);
    }

    #[test]
    fn ime_request_and_purpose_derive_debug_and_eq() {
        let request = ImeRequest::SetPurpose(ImePurpose::Password);
        assert_eq!(request, ImeRequest::SetPurpose(ImePurpose::Password));
        assert_ne!(ImePurpose::Normal, ImePurpose::Password);
        // ImeRequest / ImePurpose は機微情報を含まないため derive(Debug) のままでよい
        // （§7 のマスク対象は ImeEvent::Preedit/Commit の文字列のみ）。
        assert_eq!(format!("{:?}", ImePurpose::Terminal), "Terminal");
    }
}
