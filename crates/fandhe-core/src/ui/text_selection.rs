//! カーソル・選択範囲管理モジュール（TASK-8.5-S4 / REQ-8、Issue #185）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）の 30 分割 sub-issue（4/30）。
//! `docs/architecture/ime-architecture.md`（S1・#182）§4.6「文字インデックス正規化方針」を
//! 実装へ落とす。winit は byte offset（UTF-8）、iOS/Android は UTF-16 code unit offset、
//! Web はカーソル位置非公開という各プラットフォームの差異を、`fandhe-core` 契約の
//! **char 単位オフセット**（[`CharOffset`]、`chars().count()` 基準）へ正規化する変換層を
//! 提供する。既存 `text_field.rs` の `TextFieldWidget::cursor`（char 単位 `usize`）と型を
//! 揃えており、両者は同じ座標系で扱える。
//!
//! - **呼び出し文脈**: プラットフォーム層（S7・S15・S18・S21、winit/UIKit/
//!   `InputConnection`/DOM の各イベントハンドラ）が本モジュールの変換関数を呼び出し、
//!   ネイティブ表現のオフセットを [`CharOffset`] へ変換してからコアへ渡す（コアへ
//!   winit/UIKit 等のプラットフォーム型を一切露出させない契約、設計文書 §4.1）。
//!   コア側の利用側は S5（preedit 表示、#186）・S8（`TextFieldWidget` への選択範囲配線、
//!   #189）。
//! - **設計文書 §4.6 の実装対象**: `str::char_indices()` を使い panic を API 契約にしない
//!   （範囲外・不正境界は文字境界へクランプ）。サロゲートペア中間点等の不正オフセットは
//!   `Result` を返す変換関数として設計する（既存 `text_field.rs` の `byte_index_at` の
//!   「境界外は末尾へフォールバック」設計判断を継承）。
//!
//! **機微情報の露出防止**（`.claude/rules/security.md` / 設計文書 §7）: [`OffsetError`] /
//! [`SelectionRange`] の `Debug` / `Display` 出力には対象文字列の内容を一切含めない
//! （数値オフセット・長さのみ）。変換関数はすべて `&str` を借用するのみで内容の複製・
//! 保持・ログ出力は行わない。
//!
//! **スコープ外**（out-of-scope-tracking.md 準拠）: `TextFieldWidget<S>` への選択範囲の
//! 配線・Shift+矢印等の選択操作入力は S8（#189）、preedit 状態モデル本体は S2/S3
//! （#183/#184）、winit 等からの実イベント変換呼び出しは S7/S15/S18/S21。
//!
//! ## grapheme cluster 境界（TASK-8.5-S26 / REQ-8、Issue #207）
//!
//! 親 issue #181 の sub-issue 26/30。上記 char 単位オフセット契約
//! （[`CharOffset`]・設計文書 §4.6）はプラットフォーム境界の座標系として維持しつつ、
//! `text_field.rs` の編集操作（削除・カーソル移動）が grapheme cluster を分断する
//! 不具合（肌色修飾子つき絵文字・ZWJ 連結絵文字・Regional Indicator 国旗・結合文字が
//! char 単位の削除で壊れる）を是正するため、[`prev_grapheme_boundary`] /
//! [`next_grapheme_boundary`] / [`is_grapheme_boundary`] を追加する。UAX #29 準拠の
//! 判定は `unicode-segmentation` クレート（`grapheme_indices`）に委譲し、自前実装
//! しない（正しさのリスクと工数を回避、`Cargo.toml` の依存追加コメント参照）。
//! いずれも入出力は char 単位オフセットのまま（byte offset を呼び出し側へ漏らさない）で、
//! `text_field.rs` の `DeleteBackward` / `DeleteForward` / `MoveCursorLeft` /
//! `MoveCursorRight` から呼ばれる。

/// バイト/UTF-16 表現に依存しない char 単位オフセット（`ime-architecture.md` §4.1・§4.6）。
///
/// 既存 `text_field.rs` の `TextFieldWidget::cursor: usize`（char 単位）契約と型を揃える。
pub type CharOffset = usize;

/// オフセット変換で発生し得るエラー（panic を API 契約にしない、coding-rust.md）。
///
/// `thiserror` は使わず、既存 `catalog::CatalogError` と同じ手書きパターンを踏襲する
/// （新規依存を追加しない設計判断）。**`Debug` / `Display` には対象文字列の内容を
/// 一切含めない**（機微情報の露出防止、`.claude/rules/security.md`・設計文書 §7）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffsetError {
    /// 指定オフセットが `char` 境界上にない（マルチバイト文字・サロゲートペアの中間点）。
    /// `clamped` には直近の文字境界へクランプした場合のフォールバック値を保持する。
    NotCharBoundary {
        /// クランプ済みフォールバック値（char 単位オフセット）。
        clamped: CharOffset,
    },
    /// 指定オフセットが文字列長を超過している。
    OutOfRange {
        /// 対象文字列の char 数（`chars().count()`）。
        len: CharOffset,
    },
}

impl OffsetError {
    /// バリアントによらずフォールバック値（クランプ済み char 単位オフセット）を
    /// 取り出す。`NotCharBoundary { clamped }` と `OutOfRange { len }` は保持する
    /// フィールド名が異なるため、両アームを毎回 match させずに済ませる利便アクセサ
    /// （`char_offset_from_utf16` 等、呼び出し側で両アームを吸収する箇所向け）。
    pub fn fallback(&self) -> CharOffset {
        match self {
            OffsetError::NotCharBoundary { clamped } => *clamped,
            OffsetError::OutOfRange { len } => *len,
        }
    }
}

impl std::fmt::Display for OffsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OffsetError::NotCharBoundary { clamped } => {
                write!(f, "文字境界上にないオフセットです（クランプ値: {clamped}）")
            }
            OffsetError::OutOfRange { len } => {
                write!(f, "文字列長を超過したオフセットです（長さ: {len}）")
            }
        }
    }
}

impl std::error::Error for OffsetError {}

/// テキスト内の選択範囲（`anchor` = 選択開始点、`active` = カーソル側の可動端）。
///
/// `anchor == active` のときはキャレット（幅ゼロ選択）を表す。両フィールドとも
/// char 単位オフセット（[`CharOffset`]）で、`anchor` が `active` より大きい場合も
/// 許容する（選択方向を保持するため。範囲としての正規化が必要な場面は
/// [`SelectionRange::start`] / [`SelectionRange::end`] を使う）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectionRange {
    anchor: CharOffset,
    active: CharOffset,
}

impl SelectionRange {
    /// 選択範囲のないキャレット（`anchor == active == offset`）を構築する。
    pub fn caret(offset: CharOffset) -> Self {
        Self {
            anchor: offset,
            active: offset,
        }
    }

    /// 任意の `anchor` / `active` から選択範囲を構築する（方向を保持する）。
    pub fn new(anchor: CharOffset, active: CharOffset) -> Self {
        Self { anchor, active }
    }

    /// 選択開始点（ドラッグ・Shift 選択の起点）。
    pub fn anchor(&self) -> CharOffset {
        self.anchor
    }

    /// カーソル側の可動端（次の入力・削除が作用する位置）。
    pub fn active(&self) -> CharOffset {
        self.active
    }

    /// 範囲としての開始位置（`anchor` / `active` の小さい方）。
    pub fn start(&self) -> CharOffset {
        self.anchor.min(self.active)
    }

    /// 範囲としての終了位置（`anchor` / `active` の大きい方）。
    pub fn end(&self) -> CharOffset {
        self.anchor.max(self.active)
    }

    /// 選択範囲がないキャレット状態か（`anchor == active`）。
    pub fn is_caret(&self) -> bool {
        self.anchor == self.active
    }

    /// 選択範囲の char 単位長さ（`end - start`）。
    pub fn char_len(&self) -> CharOffset {
        self.end() - self.start()
    }

    /// `offset` が選択範囲内（`[start, end)`）に含まれるか。
    ///
    /// キャレット状態（`is_caret`）の場合は常に `false` を返す（幅ゼロ範囲は
    /// いかなるオフセットも含まない、と定義する）。
    pub fn contains(&self, offset: CharOffset) -> bool {
        !self.is_caret() && offset >= self.start() && offset < self.end()
    }

    /// バッファ長（char 数）へ両端をクランプする。
    ///
    /// テキスト編集（削除等）でバッファが短くなった際に、選択範囲が末尾を超えて
    /// panic の原因にならないよう呼び出し側が呼ぶことを想定する（`byte_index_at` と
    /// 同じ「境界外は末尾へフォールバック」契約）。
    pub fn clamped_to(&self, char_count: CharOffset) -> Self {
        Self {
            anchor: self.anchor.min(char_count),
            active: self.active.min(char_count),
        }
    }

    /// 選択を解除し、範囲の開始位置にキャレットを置く。
    pub fn collapse_to_start(&self) -> Self {
        Self::caret(self.start())
    }

    /// 選択を解除し、範囲の終了位置にキャレットを置く。
    pub fn collapse_to_end(&self) -> Self {
        Self::caret(self.end())
    }
}

/// `text` の char 数（`chars().count()`）。範囲外判定の共通ヘルパー。
fn char_count(text: &str) -> CharOffset {
    text.chars().count()
}

/// winit `Ime::Preedit` 等が渡す byte offset（UTF-8）を char 単位オフセットへ変換する
/// （`ime-architecture.md` §4.4・§4.6）。
///
/// `byte_offset` が `char` 境界上になければ `Err(NotCharBoundary)`、`text.len()` を
/// 超過していれば `Err(OutOfRange)` を返す（panic を API 契約にしない）。いずれの
/// エラーも直近の文字境界へクランプしたフォールバック値を保持する。
pub fn try_char_offset_from_byte(
    text: &str,
    byte_offset: usize,
) -> Result<CharOffset, OffsetError> {
    if byte_offset > text.len() {
        return Err(OffsetError::OutOfRange {
            len: char_count(text),
        });
    }
    if !text.is_char_boundary(byte_offset) {
        let clamped = char_offset_from_byte(text, byte_offset);
        return Err(OffsetError::NotCharBoundary { clamped });
    }
    // byte_offset は境界確認済みのため、char_indices 上の何番目かを数えるだけで良い。
    let offset = text[..byte_offset].chars().count();
    Ok(offset)
}

/// `byte_offset` を「境界外は末尾へ、非境界は含む char の先頭へ」クランプして
/// char 単位オフセットへ変換する下位関数（`try_char_offset_from_byte` が
/// `NotCharBoundary` のフォールバック値を得るために内部で呼び出す。
/// 依存方向は `try_char_offset_from_byte` → 本関数であり、逆ではない）。
///
/// 呼び出し側が異常（範囲外・非境界）を無視してよい場面では本関数を直接
/// 使ってよい。挙動は 2 通りに分かれる（1 文で兼用しない）。
/// - `byte_offset >= text.len()`（末尾ちょうど・範囲外）: 全 char 数へフォールバックする
///   （`text_field.rs` の `byte_index_at` の「境界外は末尾へ」契約の逆変換版）。
/// - それ以外で非境界の場合: `byte_offset` を含む char の先頭側へフロアする
///   （末尾フォールバックではない、§4.6 の契約）。
pub fn char_offset_from_byte(text: &str, byte_offset: usize) -> CharOffset {
    if byte_offset >= text.len() {
        return char_count(text);
    }
    // `byte_offset` を含む char（境界上ならその char 自身）の開始位置以下の
    // char 数を数え、1 引くことでその char の char 単位インデックスを得る
    // （非境界の場合は「含む char」の先頭側へクランプする、§4.6 の契約）。
    text.char_indices()
        .take_while(|(start, _)| *start <= byte_offset)
        .count()
        .saturating_sub(1)
}

/// iOS `NSRange` / Android `CharSequence` が渡す UTF-16 code unit offset を
/// char 単位オフセットへ変換する（`ime-architecture.md` §4.5・§4.6）。
///
/// サロゲートペアの中間点を指す `utf16_offset` や、文字列の UTF-16 長を超過する
/// オフセットに対しては `Err` を返す（panic を API 契約にしない）。いずれの
/// エラーも直近の文字境界へクランプしたフォールバック値を保持する。
pub fn try_char_offset_from_utf16(
    text: &str,
    utf16_offset: usize,
) -> Result<CharOffset, OffsetError> {
    let mut utf16_pos = 0usize;
    for (char_index, ch) in text.chars().enumerate() {
        if utf16_pos == utf16_offset {
            return Ok(char_index);
        }
        let unit_len = ch.len_utf16();
        if utf16_pos < utf16_offset && utf16_offset < utf16_pos + unit_len {
            // サロゲートペア（2 code unit）の中間点を指している。
            return Err(OffsetError::NotCharBoundary {
                clamped: char_index,
            });
        }
        utf16_pos += unit_len;
    }
    if utf16_pos == utf16_offset {
        return Ok(char_count(text));
    }
    Err(OffsetError::OutOfRange {
        len: char_count(text),
    })
}

/// [`try_char_offset_from_utf16`] の結果をクランプ値で吸収する利便ラッパ。
///
/// `Err` の場合も直近の文字境界へクランプした値を返す（`char_offset_from_byte` と
/// 同じ契約）。フォールバック値の取り出しは [`OffsetError::fallback`] に委譲する。
pub fn char_offset_from_utf16(text: &str, utf16_offset: usize) -> CharOffset {
    match try_char_offset_from_utf16(text, utf16_offset) {
        Ok(offset) => offset,
        Err(err) => err.fallback(),
    }
}

/// char 単位オフセットから byte offset（UTF-8）への逆変換。
///
/// コア → プラットフォームへのカーソル矩形・範囲応答（S6・S15 の前提）が用いる。
/// `char_offset` が `text` の char 数を超える場合は末尾（`text.len()`）へ
/// フォールバックする（既存 `text_field.rs` の `byte_index_at` と同じ契約。
/// panic を API 契約にしない）。
pub fn byte_offset_at(text: &str, char_offset: CharOffset) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(text.len())
}

/// char 単位オフセットから UTF-16 code unit offset への逆変換。
///
/// コア → プラットフォームへのカーソル矩形・範囲応答（S6・S15 の前提）が用いる。
/// `char_offset` が `text` の char 数を超える場合は末尾（全体の UTF-16 長）へ
/// フォールバックする（`byte_offset_at` と同じ契約。panic を API 契約にしない）。
pub fn utf16_offset_at(text: &str, char_offset: CharOffset) -> usize {
    text.chars().take(char_offset).map(char::len_utf16).sum()
}

/// `char_offset` が `text` の grapheme cluster 境界上にあるか（TASK-8.5-S26 / #207）。
///
/// 文字列先頭・末尾は常に境界とする（`unicode_segmentation::UnicodeSegmentation` の
/// `grapheme_indices` が返す byte offset 集合に `text.len()`（末尾）を加えた集合に
/// `char_offset` を byte offset へ変換した値が含まれるかで判定する）。`char_offset` が
/// `text` の char 数を超える場合も境界として扱う（`byte_offset_at` と同じ「境界外は
/// 末尾へ」契約を継承し、呼び出し側での事前クランプを不要にする）。
pub fn is_grapheme_boundary(text: &str, char_offset: CharOffset) -> bool {
    use unicode_segmentation::UnicodeSegmentation;

    let byte_offset = byte_offset_at(text, char_offset);
    if byte_offset >= text.len() {
        return true;
    }
    text.grapheme_indices(true)
        .any(|(start, _)| start == byte_offset)
}

/// `char_offset` より厳密に小さい直近の grapheme cluster 境界を返す（`DeleteBackward` /
/// `MoveCursorLeft` から呼ばれる、TASK-8.5-S26 / #207）。
///
/// `char_offset` が既に文字列先頭（`0`）の場合はクランプして `0` を返す（no-op、
/// panic を API 契約にしない）。呼び出し側（`text_field.rs`）はカーソルを常に
/// grapheme 境界へスナップし続けるため `char_offset` 自身が境界であることを前提に
/// できるが、本関数は境界か否かによらず「厳密に手前」の境界を返す（境界判定に
/// 依存しない単純な定義とし、不変条件が崩れた場合でも安全側に倒す）。
pub fn prev_grapheme_boundary(text: &str, char_offset: CharOffset) -> CharOffset {
    use unicode_segmentation::UnicodeSegmentation;

    if char_offset == 0 {
        return 0;
    }
    let byte_offset = byte_offset_at(text, char_offset);
    // `grapheme_indices` は各クラスタの開始 byte offset を昇順で返す。
    // `byte_offset` 未満の最後の境界（= 1 個前のクラスタの開始）を探す。
    text.grapheme_indices(true)
        .rfind(|(start, _)| *start < byte_offset)
        .map(|(start, _)| char_offset_from_byte(text, start))
        .unwrap_or(0)
}

/// `char_offset` より厳密に大きい直近の grapheme cluster 境界を返す（`DeleteForward` /
/// `MoveCursorRight` から呼ばれる、TASK-8.5-S26 / #207）。
///
/// `char_offset` が既に文字列末尾以上の場合はクランプして `text` の char 数を返す
/// （no-op、panic を API 契約にしない）。`prev_grapheme_boundary` と対称に、境界か
/// 否かによらず「厳密に後方」の境界を返す単純な定義とする（呼び出し側は `char_offset`
/// が境界であることを前提にしてよいが、本関数自体は不変条件に依存しない）。
pub fn next_grapheme_boundary(text: &str, char_offset: CharOffset) -> CharOffset {
    use unicode_segmentation::UnicodeSegmentation;

    let char_count = char_count(text);
    if char_offset >= char_count {
        return char_count;
    }
    let byte_offset = byte_offset_at(text, char_offset);
    text.grapheme_indices(true)
        .map(|(start, _)| start)
        .find(|start| *start > byte_offset)
        .map(|start| char_offset_from_byte(text, start))
        .unwrap_or(char_count)
}

/// `(start, end)` の byte offset 範囲を [`SelectionRange`] へ変換する
/// （`ImeEvent::Preedit { cursor }` へのマッピング補助、winit 等の byte offset 系）。
///
/// `start` / `end` いずれかが非境界・範囲外の場合は `Err` を返す（構築を諦める）。
/// 呼び出し側が「一部だけでも反映したい」場合は [`try_char_offset_from_byte`] を
/// 直接使うこと。
pub fn try_selection_from_byte_range(
    text: &str,
    range: (usize, usize),
) -> Result<SelectionRange, OffsetError> {
    let anchor = try_char_offset_from_byte(text, range.0)?;
    let active = try_char_offset_from_byte(text, range.1)?;
    Ok(SelectionRange::new(anchor, active))
}

/// `(start, end)` の UTF-16 offset 範囲を [`SelectionRange`] へ変換する
/// （`ImeEvent::Preedit { cursor }` へのマッピング補助、iOS/Android の UTF-16 offset 系）。
///
/// `start` / `end` いずれかがサロゲートペア中間点・範囲外の場合は `Err` を返す
/// （`try_selection_from_byte_range` と同じ契約）。
pub fn try_selection_from_utf16_range(
    text: &str,
    range: (usize, usize),
) -> Result<SelectionRange, OffsetError> {
    let anchor = try_char_offset_from_utf16(text, range.0)?;
    let active = try_char_offset_from_utf16(text, range.1)?;
    Ok(SelectionRange::new(anchor, active))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- SelectionRange ---

    #[test]
    fn caret_has_zero_length_and_no_containment() {
        let sel = SelectionRange::caret(3);
        assert!(sel.is_caret());
        assert_eq!(sel.char_len(), 0);
        assert!(!sel.contains(3));
        assert_eq!(sel.start(), 3);
        assert_eq!(sel.end(), 3);
    }

    #[test]
    fn anchor_greater_than_active_normalizes_via_start_end() {
        let sel = SelectionRange::new(5, 2);
        assert_eq!(sel.anchor(), 5);
        assert_eq!(sel.active(), 2);
        assert_eq!(sel.start(), 2);
        assert_eq!(sel.end(), 5);
        assert_eq!(sel.char_len(), 3);
        assert!(sel.contains(2));
        assert!(sel.contains(4));
        assert!(!sel.contains(5));
    }

    #[test]
    fn clamped_to_clamps_both_ends() {
        let sel = SelectionRange::new(10, 20);
        let clamped = sel.clamped_to(5);
        assert_eq!(clamped.anchor(), 5);
        assert_eq!(clamped.active(), 5);
    }

    #[test]
    fn collapse_to_start_and_end() {
        let sel = SelectionRange::new(2, 7);
        assert_eq!(sel.collapse_to_start(), SelectionRange::caret(2));
        assert_eq!(sel.collapse_to_end(), SelectionRange::caret(7));
    }

    #[test]
    fn offset_error_display_and_debug_omit_text_content() {
        let err = OffsetError::NotCharBoundary { clamped: 1 };
        let display = format!("{err}");
        let debug = format!("{err:?}");
        assert!(display.contains('1'));
        assert!(!display.contains("秘密"));
        assert!(!debug.contains("秘密"));
    }

    // --- byte offset 変換（ASCII） ---

    #[test]
    fn byte_offset_roundtrip_ascii() {
        let text = "hello";
        for char_offset in 0..=text.chars().count() {
            let byte = byte_offset_at(text, char_offset);
            let back = char_offset_from_byte(text, byte);
            assert_eq!(back, char_offset);
        }
    }

    #[test]
    fn byte_offset_roundtrip_multibyte() {
        let text = "こんにちは";
        for char_offset in 0..=text.chars().count() {
            let byte = byte_offset_at(text, char_offset);
            let back = char_offset_from_byte(text, byte);
            assert_eq!(back, char_offset);
        }
    }

    #[test]
    fn try_char_offset_from_byte_non_boundary_clamps() {
        let text = "こんにちは";
        // 'こ' は UTF-8 で 3 byte。byte offset 1 は非境界。
        let result = try_char_offset_from_byte(text, 1);
        assert_eq!(result, Err(OffsetError::NotCharBoundary { clamped: 0 }));
    }

    #[test]
    fn try_char_offset_from_byte_out_of_range() {
        let text = "abc";
        let result = try_char_offset_from_byte(text, 100);
        assert_eq!(result, Err(OffsetError::OutOfRange { len: 3 }));
    }

    #[test]
    fn try_char_offset_from_byte_empty_string_boundary_zero() {
        let text = "";
        assert_eq!(try_char_offset_from_byte(text, 0), Ok(0));
    }

    #[test]
    fn try_char_offset_from_byte_end_boundary() {
        let text = "abc";
        assert_eq!(try_char_offset_from_byte(text, 3), Ok(3));
    }

    // --- UTF-16 offset 変換（サロゲートペア） ---

    #[test]
    fn utf16_offset_roundtrip_with_surrogate_pair_emoji() {
        // U+1F44D (👍) は UTF-8 4 byte・UTF-16 2 code unit・char としては 1 個。
        let text = "a👍b";
        for char_offset in 0..=text.chars().count() {
            let utf16 = utf16_offset_at(text, char_offset);
            let back = char_offset_from_utf16(text, utf16);
            assert_eq!(back, char_offset);
        }
    }

    #[test]
    fn try_char_offset_from_utf16_surrogate_pair_midpoint_clamps() {
        let text = "a👍b";
        // 'a' が utf16 offset 0、👍 が 1..3（サロゲートペア）、'b' が 3。
        let result = try_char_offset_from_utf16(text, 2);
        assert_eq!(result, Err(OffsetError::NotCharBoundary { clamped: 1 }));
    }

    #[test]
    fn try_char_offset_from_utf16_out_of_range() {
        let text = "abc";
        let result = try_char_offset_from_utf16(text, 100);
        assert_eq!(result, Err(OffsetError::OutOfRange { len: 3 }));
    }

    #[test]
    fn try_char_offset_from_utf16_empty_string_boundary_zero() {
        let text = "";
        assert_eq!(try_char_offset_from_utf16(text, 0), Ok(0));
    }

    #[test]
    fn try_char_offset_from_utf16_end_boundary() {
        let text = "abc";
        assert_eq!(try_char_offset_from_utf16(text, 3), Ok(3));
    }

    // --- grapheme cluster 境界（TASK-8.5-S26 / #207）---
    //
    // 代表文字セット（計画書で統一）: 𩸽（U+29E3D、非 BMP・サロゲートペア）・
    // 👍🏽（肌色修飾子列、2 char）・🇯🇵（Regional Indicator 2 個）・
    // 👨‍👩‍👧‍👦（ZWJ 連結、7 char）・é（e + U+0301 結合）・か + U+3099（結合濁点）。

    #[test]
    fn grapheme_boundary_non_bmp_supplementary_char_is_single_cluster() {
        // 𩸽 は 1 char（非 BMP）だが UTF-16 ではサロゲートペア。grapheme としても
        // 単一クラスタであり、char 境界と grapheme 境界が一致することを確認する。
        let text = "a𩸽b";
        assert_eq!(text.chars().count(), 3);
        assert!(is_grapheme_boundary(text, 0));
        assert!(is_grapheme_boundary(text, 1));
        assert!(is_grapheme_boundary(text, 2));
        assert!(is_grapheme_boundary(text, 3));
        assert_eq!(prev_grapheme_boundary(text, 2), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 2);
    }

    #[test]
    fn grapheme_boundary_skin_tone_modifier_emoji_is_single_cluster() {
        // 👍🏽 = U+1F44D (👍) + U+1F3FD（肌色修飾子）の 2 char だが grapheme としては
        // 1 クラスタ。中間点（char offset 2）は境界ではない。
        let text = "a👍🏽b";
        assert_eq!(text.chars().count(), 4);
        assert!(is_grapheme_boundary(text, 0));
        assert!(is_grapheme_boundary(text, 1));
        assert!(!is_grapheme_boundary(text, 2)); // クラスタ中間点
        assert!(is_grapheme_boundary(text, 3));
        assert!(is_grapheme_boundary(text, 4));
        assert_eq!(prev_grapheme_boundary(text, 3), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 3);
    }

    #[test]
    fn grapheme_boundary_regional_indicator_flag_is_single_cluster() {
        // 🇯🇵 = 2 個の Regional Indicator（'J' + 'P' に対応）の 2 char で 1 クラスタ。
        let text = "a🇯🇵b";
        assert_eq!(text.chars().count(), 4);
        assert!(!is_grapheme_boundary(text, 2)); // RI 中間点
        assert_eq!(prev_grapheme_boundary(text, 3), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 3);
    }

    #[test]
    fn grapheme_boundary_zwj_family_emoji_is_single_cluster() {
        // 👨‍👩‍👧‍👦 = 4 人 + ZWJ 3 個 = 7 char で 1 クラスタ。
        let text = "a👨\u{200d}👩\u{200d}👧\u{200d}👦b";
        let char_count = text.chars().count();
        assert_eq!(char_count, 9); // 'a' + 7 char の family + 'b'
        assert!(is_grapheme_boundary(text, 1)); // クラスタ開始
        assert!(is_grapheme_boundary(text, 8)); // クラスタ終了（次の 'b' の開始）
        for offset in 2..8 {
            assert!(
                !is_grapheme_boundary(text, offset),
                "offset {offset} は family emoji クラスタの中間点であるべき"
            );
        }
        assert_eq!(prev_grapheme_boundary(text, 7), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 8);
    }

    #[test]
    fn grapheme_boundary_combining_acute_accent_is_single_cluster() {
        // 'e' + U+0301（結合鋭アクセント）= é の 2 char で 1 クラスタ。
        let text = "a\u{65}\u{301}b";
        assert_eq!(text.chars().count(), 4);
        assert!(!is_grapheme_boundary(text, 2));
        assert_eq!(prev_grapheme_boundary(text, 3), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 3);
    }

    #[test]
    fn grapheme_boundary_combining_dakuten_is_single_cluster() {
        // 'か' + U+3099（結合濁点）= が の 2 char で 1 クラスタ。
        let text = "a\u{304b}\u{3099}b";
        assert_eq!(text.chars().count(), 4);
        assert!(!is_grapheme_boundary(text, 2));
        assert_eq!(prev_grapheme_boundary(text, 3), 1);
        assert_eq!(next_grapheme_boundary(text, 1), 3);
    }

    #[test]
    fn grapheme_boundary_empty_string() {
        assert!(is_grapheme_boundary("", 0));
        assert_eq!(prev_grapheme_boundary("", 0), 0);
        assert_eq!(next_grapheme_boundary("", 0), 0);
    }

    #[test]
    fn grapheme_boundary_out_of_range_offset_clamps() {
        let text = "ab";
        assert!(is_grapheme_boundary(text, 100));
        assert_eq!(prev_grapheme_boundary(text, 100), 1);
        assert_eq!(next_grapheme_boundary(text, 100), 2);
    }

    #[test]
    fn grapheme_boundary_prev_at_start_is_noop() {
        let text = "abc";
        assert_eq!(prev_grapheme_boundary(text, 0), 0);
    }

    #[test]
    fn grapheme_boundary_next_at_end_is_noop() {
        let text = "abc";
        assert_eq!(next_grapheme_boundary(text, 3), 3);
    }

    #[test]
    fn grapheme_boundary_simple_ascii_every_char_is_boundary() {
        let text = "abc";
        for offset in 0..=3 {
            assert!(is_grapheme_boundary(text, offset));
        }
    }

    // --- selection range mapping ---

    #[test]
    fn try_selection_from_byte_range_ok() {
        let text = "こんにちは";
        // 'こ' (0..3), 'ん' (3..6)
        let sel = try_selection_from_byte_range(text, (0, 6)).expect("valid byte range");
        assert_eq!(sel, SelectionRange::new(0, 2));
    }

    #[test]
    fn try_selection_from_byte_range_err_on_non_boundary() {
        let text = "こんにちは";
        let result = try_selection_from_byte_range(text, (0, 1));
        assert!(result.is_err());
    }

    #[test]
    fn try_selection_from_utf16_range_ok() {
        let text = "a👍b";
        let sel = try_selection_from_utf16_range(text, (0, 3)).expect("valid utf16 range");
        assert_eq!(sel, SelectionRange::new(0, 2));
    }

    #[test]
    fn try_selection_from_utf16_range_err_on_surrogate_midpoint() {
        let text = "a👍b";
        let result = try_selection_from_utf16_range(text, (0, 2));
        assert!(result.is_err());
    }
}
