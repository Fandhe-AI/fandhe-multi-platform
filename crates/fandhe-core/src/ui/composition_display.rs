//! コンポジション（未確定文字列）表示合成モジュール（TASK-8.5-S5 / REQ-8、Issue #186）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）の 30 分割 sub-issue（5/30）。
//! `docs/architecture/text-input-state-model.md`（S2・#183）§4.4 が確定した表示文字列の
//! 導出（純関数 `display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]`）を、
//! 実際の描画表現（[`super::widget::PaintCommand::CompositionText`]）へ落とす。同文書
//! §6 に「S5: §4.4 の表示文字列導出を実際の描画（下線・背景色等）に落とす」と明記されて
//! いる本モジュールの役割そのものである。
//!
//! - **呼び出し文脈**: `TextFieldWidget::paint`（S8、#189）が Composing 中に本モジュールを
//!   呼び出し、`PaintCommand::CompositionText` へ変換する（配線済み）。本モジュール自体は
//!   `text_selection.rs`（S4・#185）が採った自己完結モジュール新設パターンを踏襲し、
//!   `Composition` 型・`TextFieldWidget` に一切依存しない独立した純関数・型として実装する。
//!   `Composition` 型に依存しない純関数構成を意図的に維持する（`text_field.rs` 内の
//!   `Composition` が保持する
//!   `anchor` / `text` / `cursor` を呼び出し側で本モジュールの引数へ橋渡しする形を保つ
//!   ことで、表示合成ロジックを状態モデルの内部表現から独立させ、将来
//!   `Composition` の内部表現が変わっても本モジュールの純関数契約に影響しない）。
//! - **char オフセット契約**: `anchor`・`cursor` はいずれも char 単位オフセット
//!   （[`super::text_selection::CharOffset`] と同じ座標系、`ime-architecture.md` §4.1・
//!   §4.6）。byte 変換は [`super::text_selection::byte_offset_at`] を再利用し、本モジュール
//!   内でロジックを重複させない（S4・#185）。
//! - **Web のカーソル非公開フォールバック**: `cursor: None` は「preedit 末尾にキャレット」
//!   既定動作を表す（`ime-architecture.md` §4.6、DOM の `compositionupdate` がカーソル
//!   範囲を公開しないケースの契約）。
//!
//! **機微情報の露出防止**（`.claude/rules/security.md` / `text-input-state-model.md` §5
//! 不変条件 7）: [`CompositionDisplay`] の `Debug` 出力には `before` / `preedit` / `after`
//! の内容を一切含めない（char 数のみ）。既存 `Composition`（S2 設計）・`TextFieldWidget`
//! の Debug マスク方針を継承する。
//!
//! **スコープ外**（`out-of-scope-tracking.md` 準拠）: preedit 装飾（下線・背景色）の
//! デザイントークン化は検出時にユーザー確認のうえ起票（自動起票はしない）、変換候補
//! ウィンドウ位置計算は S6（#187）、winit `Ime::*` ハンドリングは S7（#188）が担当する。
//! `TextFieldWidget` への配線は S8（#189）で完了済み。
//!
//! **grapheme cluster・サロゲートペア境界の網羅検証（TASK-8.5-S26 / #207）**:
//! 本モジュールは `anchor` / `cursor` を char 単位オフセットのまま `&str` を
//! 分割・連結するのみで、grapheme cluster 境界へのスナップは行わない（設計文書
//! §4.6 の契約どおり、char 単位オフセット契約はプラットフォーム境界で維持する。
//! grapheme 対応が必要な編集操作本体は `text_field.rs` の `handle_input` 側で行う）。
//! 絵文字・サロゲートペアを含む `anchor` / `cursor` でも `byte_offset_at`
//! （char 境界へのみ変換）経由のため panic しないことをテストで固定化する
//! （下記 `tests` モジュール参照）。

use super::text_selection::byte_offset_at;
use super::widget::PaintCommand;

/// コンポジション表示の合成結果（S2 設計文書 §4.4 の純関数の出力）。
///
/// `before ⊕ preedit ⊕ after` が表示文字列全体（`display`）を構成する。3 セグメントの
/// まま保持することで、呼び出し側（プラットフォームレイヤ）が preedit 区間にのみ
/// 下線・背景色等の装飾を適用できるようにする（S2 §4.4 の設計判断を継承）。
pub struct CompositionDisplay {
    before: String,
    preedit: String,
    after: String,
    preedit_cursor: Option<(usize, usize)>,
}

impl CompositionDisplay {
    /// `buffer`（確定バッファ）・`anchor`（char 単位挿入位置）・`preedit`・`cursor` から
    /// 表示合成を構築する（`text-input-state-model.md` §4.4 の純関数の実装）。
    ///
    /// `anchor` が `buffer` の char 数を超える場合は末尾へクランプする（panic を API
    /// 契約にしない、`byte_offset_at` と同じ設計判断・coding-rust.md 準拠）。`cursor` は
    /// `preedit` の char 数へクランプし、`start > end` の場合は正規化する（小さい方を
    /// start とする）。
    pub fn synthesize(
        buffer: &str,
        anchor: usize,
        preedit: &str,
        cursor: Option<(usize, usize)>,
    ) -> Self {
        // anchor は byte_offset_at の「境界外は末尾へフォールバック」契約により
        // 自動的に buffer の char 数へクランプされる（text_selection.rs 参照）。
        let split_at = byte_offset_at(buffer, anchor);
        let before = buffer[..split_at].to_string();
        let after = buffer[split_at..].to_string();

        let preedit_len = preedit.chars().count();
        let preedit_cursor = cursor.map(|(start, end)| {
            let start = start.min(preedit_len);
            let end = end.min(preedit_len);
            if start <= end {
                (start, end)
            } else {
                (end, start)
            }
        });

        Self {
            before,
            preedit: preedit.to_string(),
            after,
            preedit_cursor,
        }
    }

    /// 連結済み表示文字列（`display = before ⊕ preedit ⊕ after`）。
    pub fn display_string(&self) -> String {
        format!("{}{}{}", self.before, self.preedit, self.after)
    }

    /// `display_string()` の座標系（display 先頭 char を 0 とする）での preedit 区間
    /// `(start, end)`。
    pub fn preedit_range(&self) -> (usize, usize) {
        let start = self.before.chars().count();
        let end = start + self.preedit.chars().count();
        (start, end)
    }

    /// `display_string()` の座標系でのキャレット位置。
    ///
    /// `preedit_cursor` が `None` のときは preedit 末尾（`ime-architecture.md` §4.6 の
    /// 既定フォールバック契約、Web の `compositionupdate` がカーソル位置を提供しない
    /// ケース）を返す。
    pub fn caret_position(&self) -> usize {
        let (range_start, range_end) = self.preedit_range();
        match self.preedit_cursor {
            Some((_, end)) => range_start + end,
            None => range_end,
        }
    }

    /// preedit が空かどうか。空なら呼び出し側は既存の `PaintCommand::Text` 等の
    /// 通常描画にフォールバックしてよい（`Composition` を保持しない Idle 状態と
    /// 同じ扱いにするための判定補助）。
    pub fn is_preedit_empty(&self) -> bool {
        self.preedit.is_empty()
    }

    /// [`PaintCommand::CompositionText`] へ変換する。
    ///
    /// `before` / `preedit` / `after` の 3 セグメントと `preedit_cursor` をそのまま
    /// 写像する（本モジュールは表示合成のみを担い、装飾の具体表現はプラットフォーム
    /// レイヤに委ねる、`ui/widget.rs` の `PaintCommand::CompositionText` doc 参照）。
    pub fn into_paint_command(self) -> PaintCommand {
        PaintCommand::CompositionText {
            before: self.before,
            preedit: self.preedit,
            after: self.after,
            preedit_cursor: self.preedit_cursor,
        }
    }
}

impl std::fmt::Debug for CompositionDisplay {
    /// 確定バッファ・未確定文字列の内容を出力しない（char 数のみ）。既存
    /// `Composition`（S2 設計）・`TextFieldWidget` の Debug マスク方針を継承する
    /// （機微情報の露出防止、`.claude/rules/security.md`）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositionDisplay")
            .field("before_len", &self.before.chars().count())
            .field("preedit_len", &self.preedit.chars().count())
            .field("after_len", &self.after.chars().count())
            .field("preedit_cursor", &self.preedit_cursor)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_synthesis_splits_buffer_around_anchor() {
        let display = CompositionDisplay::synthesize("abcdef", 3, "かな", None);
        assert_eq!(display.display_string(), "abcかなdef");
        assert_eq!(display.preedit_range(), (3, 5));
    }

    #[test]
    fn multibyte_buffer_synthesizes_without_panicking_on_char_boundary() {
        // 確定バッファがマルチバイト（char 単位契約の検証）。
        let display = CompositionDisplay::synthesize("こんにちは", 2, "w", None);
        assert_eq!(display.display_string(), "こんwにちは");
        assert_eq!(display.preedit_range(), (2, 3));
    }

    #[test]
    fn anchor_out_of_range_clamps_to_end() {
        let display = CompositionDisplay::synthesize("abc", 100, "x", None);
        assert_eq!(display.display_string(), "abcx");
        assert_eq!(display.preedit_range(), (3, 4));
    }

    #[test]
    fn empty_preedit_is_reported_as_empty_and_display_equals_buffer() {
        let display = CompositionDisplay::synthesize("abc", 1, "", None);
        assert_eq!(display.display_string(), "abc");
        assert!(display.is_preedit_empty());
    }

    #[test]
    fn none_cursor_falls_back_to_preedit_end() {
        let display = CompositionDisplay::synthesize("ab", 1, "xyz", None);
        // preedit は display 座標系で (1, 4)。cursor 非公開時は末尾（4）。
        assert_eq!(display.caret_position(), 4);
    }

    #[test]
    fn cursor_out_of_range_clamps_and_normalizes_reversed_range() {
        let display = CompositionDisplay::synthesize("ab", 1, "xyz", Some((100, 1)));
        // start=100 は preedit 長 3 へクランプ、end=1 との大小が入れ替わり (1, 3) に正規化。
        let (start, end) = display.preedit_range();
        assert_eq!((start, end), (1, 4));
        // caret_position は正規化後の end（cursor 内の end=3）を使う。
        assert_eq!(display.caret_position(), 1 + 3);
    }

    #[test]
    fn emoji_preedit_synthesizes_as_single_char_unit() {
        // U+1F44D (👍) は char としては 1 個（サロゲートペア・多バイト境界の代表ケース）。
        let display = CompositionDisplay::synthesize("ab", 1, "👍", Some((0, 1)));
        assert_eq!(display.display_string(), "a👍b");
        assert_eq!(display.preedit_range(), (1, 2));
        assert_eq!(display.caret_position(), 2);
    }

    #[test]
    fn zwj_family_emoji_anchor_does_not_split_cluster_and_does_not_panic() {
        // TASK-8.5-S26（#207）: `anchor` が ZWJ 連結絵文字（7 char・1 grapheme
        // cluster）の char 境界を指しても panic せず、char 単位契約どおりに
        // buffer を分割・連結できることを確認する（grapheme 対応は編集操作側の
        // 責務であり、本モジュールは char 境界の範囲で安全であればよい）。
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let buffer = format!("a{family}b");
        // anchor をクラスタの char 数（7）分進めた位置（'a' の直後 + family 全体）に置く。
        let anchor = 1 + family.chars().count();
        let display = CompositionDisplay::synthesize(&buffer, anchor, "!", None);
        assert_eq!(display.display_string(), format!("a{family}!b"));
    }

    #[test]
    fn combining_accent_preedit_boundary_does_not_panic() {
        // 'e' + U+0301（結合鋭アクセント）を含む buffer への preedit 挿入が
        // panic しないことを確認する（TASK-8.5-S26 / #207）。
        let buffer = "a\u{65}\u{301}b"; // a, e, ́, b の 4 char
        let display = CompositionDisplay::synthesize(buffer, 3, "x", Some((0, 1)));
        assert_eq!(display.display_string(), "a\u{65}\u{301}xb");
    }

    #[test]
    fn debug_output_omits_buffer_and_preedit_content() {
        let display = CompositionDisplay::synthesize("秘密の文字列", 2, "非公開", None);
        let debug = format!("{display:?}");
        assert!(!debug.contains("秘密"));
        assert!(!debug.contains("非公開"));
        assert!(debug.contains("before_len"));
        assert!(debug.contains("preedit_len"));
    }

    // 以下、TASK-8.5-S27（#208）が追加する境界ケース。
    // `docs/architecture/text-input-state-model.md` §4.4 の純関数が anchor の
    // 両端・collapsed cursor・全区間マルチバイト・空 buffer で破綻しないことを
    // 棚卸しして補う（プロダクションコードは無変更）。

    #[test]
    fn anchor_zero_leaves_before_empty() {
        // anchor == 0（buffer 先頭）では before が空になる。
        let display = CompositionDisplay::synthesize("abc", 0, "x", None);
        assert_eq!(display.display_string(), "xabc");
        assert_eq!(display.preedit_range(), (0, 1));
    }

    #[test]
    fn anchor_at_buffer_end_leaves_after_empty() {
        // anchor == buffer の char 数（末尾）では after が空になる。
        let display = CompositionDisplay::synthesize("abc", 3, "x", None);
        assert_eq!(display.display_string(), "abcx");
        assert_eq!(display.preedit_range(), (3, 4));
    }

    #[test]
    fn collapsed_cursor_places_caret_at_the_single_point() {
        // start == end（範囲を持たない単一キャレット位置）。
        let display = CompositionDisplay::synthesize("ab", 1, "xyz", Some((2, 2)));
        assert_eq!(display.caret_position(), 1 + 2);
    }

    #[test]
    fn all_segments_multibyte_keep_char_based_coordinates_consistent() {
        // before・preedit・after のすべてがマルチバイトの場合でも、display 座標系
        // （char 単位）が一貫していることを確認する。
        let display = CompositionDisplay::synthesize("あいう", 2, "かきく", Some((1, 2)));
        assert_eq!(display.display_string(), "あいかきくう");
        assert_eq!(display.preedit_range(), (2, 5));
        assert_eq!(display.caret_position(), 2 + 2);
    }

    #[test]
    fn empty_buffer_with_non_empty_preedit_synthesizes_preedit_only() {
        // 空 buffer への確定挿入前（初回コンポジション開始時）の合成。
        let display = CompositionDisplay::synthesize("", 0, "かな", Some((0, 2)));
        assert_eq!(display.display_string(), "かな");
        assert_eq!(display.preedit_range(), (0, 2));
        assert_eq!(display.caret_position(), 2);
    }

    #[test]
    fn into_paint_command_maps_all_fields() {
        let display = CompositionDisplay::synthesize("abcdef", 3, "かな", Some((0, 1)));
        match display.into_paint_command() {
            PaintCommand::CompositionText {
                before,
                preedit,
                after,
                preedit_cursor,
            } => {
                assert_eq!(before, "abc");
                assert_eq!(preedit, "かな");
                assert_eq!(after, "def");
                assert_eq!(preedit_cursor, Some((0, 1)));
            }
            other => panic!("CompositionText を期待したが {other:?} が返された"),
        }
    }
}
