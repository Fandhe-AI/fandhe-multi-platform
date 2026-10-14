//! 変換候補ウィンドウの位置計算（TASK-8.5-S6 / REQ-8、Issue #187）。
//!
//! `docs/architecture/ime-architecture.md` §4.2 の契約により、コアは
//! `ImeRequest::SetCursorArea { origin, size }` で**論理ピクセル座標**のカーソル矩形を
//! プラットフォーム層へ通知する（物理ピクセル変換・スケールファクタはプラットフォーム層の
//! 責務、winit なら `Window::set_ime_cursor_area` へ委譲）。同 §4.3 の責務分界表で
//! 「変換候補ウィンドウの位置計算（カーソル座標 → 論理ピクセル矩形）」はコア担当と
//! 定義されている。
//!
//! `docs/architecture/text-input-state-model.md` §6 は、`Composition::anchor` /
//! `cursor` から `ImeRequest::SetCursorArea` へ渡す論理ピクセル座標を計算すると定める。
//! `crates/fandhe-core/src/ui/text_field.rs` モジュール doc が採った先例（既存 main の
//! 公開 API のみに依存する自己完結実装とし、後続で型統合する）を踏襲し、本モジュールは
//! `Composition` に依存せず**プリミティブ引数（`&str` / `usize`）を取る純関数**として
//! 実装する。`TextFieldWidget` への配線・`Composition` ベースへの引数型統合は
//! TASK-8.5-S8（Issue #189）へ申し送る（`.claude/rules/out-of-scope-tracking.md` 準拠）。
//!
//! # 計算意味論
//!
//! 表示合成文字列は `text-input-state-model.md` §4.4 の定義（
//! `display = buffer[..anchor] ⊕ composition.text ⊕ buffer[anchor..]`、char 単位合成）に
//! 従う。本関数は `anchor`（確定バッファ内の基準点）と `preedit`（未確定文字列）・
//! `preedit_cursor`（preedit 内のキャレット範囲、char 単位）から、この表示合成上の
//! キャレット位置を論理ピクセル矩形へ変換する。
//!
//! 暫定計測は `text_field.rs::measure`（1 文字幅 8.0 px・行高 16.0 px の等幅近似）と
//! 一致させる（本モジュールの [`CHAR_WIDTH`] / [`LINE_HEIGHT`] 定数）。CJK の実グリフ幅とは
//! 乖離するが、cosmic-text 等の正確なテキスト計測 API による置換は既存の申し送り事項
//! （crate-feature-matrix.md）と同じ扱いとし、本 issue のスコープ外とする。
//!
//! # スコープ外事項
//!
//! - `ImeRequest` / `ImeEvent` 列挙自体の定義（TASK-8.5-S7 Issue #188・S23 Issue #204）。
//! - `TextFieldWidget` への配線・`SetCursorArea` の発行タイミング制御（S8 Issue #189）。
//! - 物理ピクセル変換・スケールファクタ（プラットフォーム層 S9〜S22）。
//! - cosmic-text 等による正確なグリフ測定。**グリフ実測待ち**（cosmic-text 導入、
//!   TASK-8.5-S26・Issue #207 で確認済み・既存申し送りと同一）。CHAR_WIDTH の等幅近似は
//!   絵文字（1 char が視覚的に全角相当）でも変わらず適用されるため、preedit 内カーソルが
//!   grapheme 境界の内部を指す場合の表示上のスナップは cosmic-text 導入後に再検討する
//!   （S26 の残課題、S29・Issue #210 で文書化）。本モジュール自体は char 数ベースの
//!   計算のため、絵文字・サロゲートペアを含む preedit でも panic しないことをテストで
//!   確認済み（下記 `tests` モジュール参照）。

use super::geometry::{Point, Rect, Size};

/// 1 文字あたりの暫定幅（論理ピクセル）。`text_field.rs::measure` の暫定計測と一致させる。
const CHAR_WIDTH: f32 = 8.0;

/// 行高（論理ピクセル）。`text_field.rs::measure` の暫定計測と一致させる。
const LINE_HEIGHT: f32 = 16.0;

/// ゼロ幅キャレット矩形の最小幅（論理ピクセル）。候補ウィンドウが幅 0 の矩形に
/// 対して不安定にならないよう、非負の下限を与える。
const CARET_WIDTH: f32 = 1.0;

/// [`ime_cursor_area`] への入力パラメータ。
///
/// フィールドはすべて一時的な参照・値のみを保持し、所有権を持たない（呼び出し側の
/// `TextFieldWidget` 相当の状態を借用するだけの短命な値であることを表す）。
///
/// **機微情報の露出防止**（`.claude/rules/security.md`）: `Debug` は手動実装し、
/// `buffer` / `preedit` の内容を出力しない（char 数のみ）。パスワード入力中の
/// 未確定文字列がログへ漏れる経路を作らないため（`TextFieldWidget` の Debug マスク方針を継承）。
pub struct ImeCursorAreaParams<'a> {
    /// テキストフィールドの左上原点（論理ピクセル、レイアウト結果の座標系）。
    pub field_origin: Point,
    /// テキストフィールドのサイズ（論理ピクセル）。
    pub field_size: Size,
    /// 確定バッファ（`TextFieldWidget::buffer` 相当）。IME 由来の外部入力として扱うが、
    /// 本関数は内容を解釈せず char 数の計数のみに使う（制御文字除去等は呼び出し側の
    /// 既存経路の責務であり、本関数で重複実装しない）。
    pub buffer: &'a str,
    /// コンポジション開始時点の確定バッファ内カーソル位置（char 単位、
    /// `Composition::anchor` 相当。非コンポジション中は現在カーソル位置を渡す）。
    pub anchor: usize,
    /// 未確定文字列（`Composition::text` 相当。非コンポジション中は空文字列を渡す）。
    pub preedit: &'a str,
    /// preedit 内カーソル範囲（preedit 先頭を 0 とした char 単位、
    /// `ImeEvent::Preedit.cursor` と同座標系）。`None` は preedit 末尾へフォールバックする
    /// （Web の `compositionupdate` がカーソル位置を公開しないため必須のフォールバック）。
    pub preedit_cursor: Option<(usize, usize)>,
}

impl std::fmt::Debug for ImeCursorAreaParams<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImeCursorAreaParams")
            .field("field_origin", &self.field_origin)
            .field("field_size", &self.field_size)
            .field("buffer_char_count", &self.buffer.chars().count())
            .field("anchor", &self.anchor)
            .field("preedit_char_count", &self.preedit.chars().count())
            .field("preedit_cursor", &self.preedit_cursor)
            .finish()
    }
}

/// 変換候補ウィンドウの位置計算（TASK-8.5-S6 の核心）。
///
/// 戻り値は論理ピクセルのカーソル矩形。プラットフォーム層が `ImeRequest::SetCursorArea` →
/// （winit の場合）`Window::set_ime_cursor_area` へ変換する契約（`ime-architecture.md` §4.2、
/// 物理ピクセル変換はコアの責務外）。
///
/// 不正な入力（境界外 `anchor`・範囲外や前後逆転の `preedit_cursor`・負や NaN の
/// `field_size`）でも panic せず、フィールド矩形内へクランプした矩形を返す
/// （panic を API 契約にしない、`.claude/rules/coding-rust.md` のエラーハンドリング方針。
/// `geometry::Rect::contains` の NaN・負サイズに対する防御方針を継承する）。
pub fn ime_cursor_area(params: &ImeCursorAreaParams<'_>) -> Rect {
    let field_rect = Rect::new(params.field_origin, params.field_size);

    // 負・NaN の field_size はレイアウト不整合を示すが、コアはここで panic せず
    // フィールド原点のゼロサイズ矩形へ退避する（`Rect::contains` と同じ「比較で
    // 弾く」防御ではなく、呼び出し側へ返す矩形自体を安全な既定値にする必要があるため
    // 個別に判定する）。
    if !is_finite_non_negative_size(params.field_size) {
        return Rect::new(params.field_origin, Size::new(0.0, 0.0));
    }

    let buffer_char_count = params.buffer.chars().count();
    // 境界外 anchor は末尾へクランプする（`text_field.rs::byte_index_at` と同じ
    // 「境界外は末尾へフォールバックし panic しない」方針）。
    let anchor = params.anchor.min(buffer_char_count);

    let preedit_char_count = params.preedit.chars().count();
    let (caret_start, caret_end) = resolve_preedit_caret(params.preedit_cursor, preedit_char_count);

    // 表示合成（`display = buffer[..anchor] ⊕ preedit ⊕ buffer[anchor..]`、
    // `text-input-state-model.md` §4.4）上の char オフセットへ変換する。
    let display_start = anchor + caret_start;
    let display_end = anchor + caret_end;

    let x = params.field_origin.x + (display_start as f32) * CHAR_WIDTH;
    let width = (((display_end - display_start) as f32) * CHAR_WIDTH).max(CARET_WIDTH);
    let y = params.field_origin.y;
    let height = LINE_HEIGHT.min(params.field_size.height);

    clamp_to_field(
        Rect::new(Point::new(x, y), Size::new(width, height)),
        field_rect,
    )
}

/// `field_size` が有限かつ非負（NaN でない）かを判定する。
fn is_finite_non_negative_size(size: Size) -> bool {
    size.width.is_finite() && size.height.is_finite() && size.width >= 0.0 && size.height >= 0.0
}

/// preedit 内キャレット範囲（char 単位、`(start, end)`、`start <= end`）を決定する。
///
/// - `Some((s, e))`: `preedit_char_count` へクランプし、`s > e` なら正規化のため入れ替える。
/// - `None`: preedit 末尾のゼロ幅キャレット（`ime-architecture.md` §4.6 のフォールバック）。
///   preedit が空（非コンポジション中）の場合も同じ式で `(0, 0)` になり、
///   「anchor 位置のゼロ幅キャレット」に一致する。
fn resolve_preedit_caret(
    preedit_cursor: Option<(usize, usize)>,
    preedit_char_count: usize,
) -> (usize, usize) {
    match preedit_cursor {
        Some((s, e)) => {
            let s = s.min(preedit_char_count);
            let e = e.min(preedit_char_count);
            if s > e { (e, s) } else { (s, e) }
        }
        None => (preedit_char_count, preedit_char_count),
    }
}

/// 候補ウィンドウ矩形をフィールド矩形 `[field_origin, field_origin + field_size)` 内へ
/// クランプする。テキストがフィールド幅を超える場合に候補ウィンドウがウィジェットから
/// 乖離しない防御（単一行・水平スクロール未実装の現状に合わせた簡易版）。
///
/// **原点（表示上のキャレット x/y）は width/height の超過を理由に引き戻さない。**
/// プラットフォーム層は本関数の戻り値の原点を候補ウィンドウの基準座標として使うため
/// （`ime-architecture.md` §4.2）、原点を左へシフトすると、フィールド右端付近で
/// 幅の広い選択範囲・preedit キャレットに対して実際の合成範囲より手前を指す矩形を
/// 返してしまい、候補ウィンドウの位置がずれる（Bugbot 指摘の再発防止）。
/// 原点は「フィールド範囲そのものから外れている場合」のみクランプし、
/// 幅・高さは残りの領域に収まるよう縮めることで「フィールド内に収める」契約を満たす。
/// このため、フィールドが極端に小さい・ゼロサイズの場合は [`CARET_WIDTH`] の
/// 下限より矩形が小さくなることを許容する（「フィールド内に収める」が優先契約であり、
/// 後続 issue（S8）がこれを「キャレット最小幅の欠落」として引き戻さないよう明記する）。
fn clamp_to_field(rect: Rect, field: Rect) -> Rect {
    let field_right = field.origin.x + field.size.width;
    let field_bottom = field.origin.y + field.size.height;

    // 原点自体がフィールド範囲外の場合のみクランプする（幅・高さの超過を理由に
    // 引き戻さない。表示上のキャレット x/y を保持する）。
    let x = rect.origin.x.max(field.origin.x).min(field_right);
    let y = rect.origin.y.max(field.origin.y).min(field_bottom);

    // 幅・高さは原点から見た残り領域に収まるよう縮める（負にはしない）。
    let width = rect.size.width.min((field_right - x).max(0.0));
    let height = rect.size.height.min((field_bottom - y).max(0.0));

    Rect::new(Point::new(x, y), Size::new(width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params<'a>(
        field_size: Size,
        buffer: &'a str,
        anchor: usize,
        preedit: &'a str,
        preedit_cursor: Option<(usize, usize)>,
    ) -> ImeCursorAreaParams<'a> {
        ImeCursorAreaParams {
            field_origin: Point::new(0.0, 0.0),
            field_size,
            buffer,
            anchor,
            preedit,
            preedit_cursor,
        }
    }

    #[test]
    fn ascii_buffer_with_preedit_cursor_basic_case() {
        // buffer="ab", anchor=2, preedit="XY", cursor=(0,1) -> display offset 2..3
        let p = params(Size::new(200.0, 20.0), "ab", 2, "XY", Some((0, 1)));
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin.x, 2.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, 1.0 * CHAR_WIDTH);
        assert_eq!(rect.origin.y, 0.0);
        assert_eq!(rect.size.height, LINE_HEIGHT);
    }

    #[test]
    fn emoji_preedit_does_not_panic_and_uses_char_count() {
        // TASK-8.5-S26（#207）: ZWJ 連結絵文字（7 char・1 grapheme cluster）を
        // preedit に含めても panic せず、char 数ベースの等幅近似で計算されることを
        // 確認する（グリフ実測は cosmic-text 導入待ちの既知の残課題、モジュール doc 参照）。
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}";
        let p = params(Size::new(400.0, 20.0), "a", 1, family, Some((0, 1)));
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin.x, 1.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, 1.0 * CHAR_WIDTH);
    }

    #[test]
    fn multibyte_buffer_uses_char_count_not_byte_len() {
        // 「こんにちは」は 5 char / 15 byte。byte 数に引きずられず char 単位で計算されること。
        let buffer = "こんにちは";
        assert_eq!(buffer.chars().count(), 5);
        let p = params(Size::new(200.0, 20.0), buffer, 5, "変換", Some((0, 2)));
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin.x, 5.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, 2.0 * CHAR_WIDTH);
    }

    #[test]
    fn preedit_cursor_none_falls_back_to_preedit_tail() {
        let p = params(Size::new(200.0, 20.0), "ab", 2, "XYZ", None);
        let rect = ime_cursor_area(&p);
        // anchor(2) + preedit char count(3) = 5 のゼロ幅キャレット。
        assert_eq!(rect.origin.x, 5.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, CARET_WIDTH);
    }

    #[test]
    fn empty_preedit_is_zero_width_caret_at_anchor() {
        // 非コンポジション中（preedit 空）は anchor 位置のゼロ幅キャレット。
        let p = params(Size::new(200.0, 20.0), "abc", 3, "", None);
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin.x, 3.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, CARET_WIDTH);
    }

    #[test]
    fn out_of_bounds_anchor_clamps_to_buffer_end() {
        let p = params(Size::new(200.0, 20.0), "ab", 999, "", None);
        let rect = ime_cursor_area(&p);
        // anchor は buffer の char 数 (2) へクランプされる。
        assert_eq!(rect.origin.x, 2.0 * CHAR_WIDTH);
    }

    #[test]
    fn reversed_preedit_cursor_is_normalized() {
        let p = params(Size::new(200.0, 20.0), "", 0, "XYZ", Some((3, 1)));
        let rect = ime_cursor_area(&p);
        // (3, 1) は (1, 3) へ正規化される。
        assert_eq!(rect.origin.x, 1.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, 2.0 * CHAR_WIDTH);
    }

    #[test]
    fn out_of_range_preedit_cursor_clamps_to_preedit_len() {
        let p = params(Size::new(200.0, 20.0), "", 0, "XY", Some((0, 999)));
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.size.width, 2.0 * CHAR_WIDTH);
    }

    #[test]
    fn wide_caret_clamps_within_field_width() {
        // フィールド幅 (10.0) を超えるテキストでも矩形はフィールド内へクランプされる
        // （原点の引き戻しのみで収まるケース：ゼロ幅キャレットのため width は field 内）。
        let p = params(Size::new(10.0, 20.0), "abcdefgh", 8, "", None);
        let rect = ime_cursor_area(&p);
        let field_right = 10.0;
        assert!(rect.origin.x + rect.size.width <= field_right + f32::EPSILON);
        assert!(rect.origin.x >= 0.0);
    }

    #[test]
    fn selection_near_right_edge_keeps_origin_and_shrinks_width_only() {
        // buffer="abcde"(5 char)・anchor=5・preedit="XYZ"・cursor=(0,3) -> display 5..8。
        // x = 5*8=40, 素の width = 3*8=24 で右端 (40+24=64) がフィールド幅 (50) を
        // 超える。clamp_to_field は origin.x (表示上のキャレット x) を左へシフトさせず、
        // width のみ残り領域 (50-40=10) へ縮めること（Bugbot 指摘の再発防止）。
        let p = params(Size::new(50.0, 20.0), "abcde", 5, "XYZ", Some((0, 3)));
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin.x, 5.0 * CHAR_WIDTH);
        assert_eq!(rect.size.width, 50.0 - 5.0 * CHAR_WIDTH);
    }

    #[test]
    fn wide_selection_caret_shrinks_width_to_fit_field() {
        // preedit のキャレット範囲自体の width (3 * CHAR_WIDTH = 24.0) が
        // フィールド幅 (10.0) を超えるケース。原点の引き戻しだけでは収まらないため、
        // 幅自体を切り詰めてフィールド内に収める（advisor 指摘の再発防止）。
        let p = params(Size::new(10.0, 20.0), "", 0, "XYZ", Some((0, 3)));
        let rect = ime_cursor_area(&p);
        assert!(rect.origin.x + rect.size.width <= 10.0 + f32::EPSILON);
        assert!(rect.origin.x >= 0.0);
    }

    #[test]
    fn zero_size_field_yields_zero_size_rect_without_panic() {
        // ゼロサイズフィールドでは CARET_WIDTH の下限より矩形が小さくなることを許容し、
        // 「フィールド内に収める」契約を優先する。
        let p = params(Size::new(0.0, 0.0), "a", 1, "", None);
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.size, Size::new(0.0, 0.0));
    }

    #[test]
    fn negative_field_size_returns_safe_fallback_without_panic() {
        let p = params(Size::new(-10.0, 20.0), "ab", 1, "", None);
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.origin, Point::new(0.0, 0.0));
        assert_eq!(rect.size, Size::new(0.0, 0.0));
    }

    #[test]
    fn nan_field_size_returns_safe_fallback_without_panic() {
        let p = params(Size::new(f32::NAN, 20.0), "ab", 1, "", None);
        let rect = ime_cursor_area(&p);
        assert_eq!(rect.size, Size::new(0.0, 0.0));
    }

    #[test]
    fn debug_output_does_not_leak_buffer_or_preedit_content() {
        let p = params(Size::new(200.0, 20.0), "secret-password", 3, "hidden", None);
        let debug_str = format!("{p:?}");
        assert!(!debug_str.contains("secret-password"));
        assert!(!debug_str.contains("hidden"));
        assert!(debug_str.contains("buffer_char_count"));
        assert!(debug_str.contains("preedit_char_count"));
    }
}
