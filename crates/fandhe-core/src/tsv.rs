//! タブ区切り値（TSV）の純粋関数パーサ（親 #979、Issue #980）。
//!
//! myIme 設定画面のユーザー辞書（読み・単語・品詞・コメント）のような TSV を読み込む
//! 前提機能。端末依存（`fandhe-device`・OS API）を一切持たない純粋関数として実装し、
//! ファイルの取得は既存の `FilePicker`（`fandhe-device`）や
//! `screens::file_pick`/`crates/fandhe-egui-proxy/src/device/file_pick.rs` の
//! 橋渡しをそのまま使う（新しい trait は作らない）。`screens::tsv_import` 画面から
//! 呼ばれるほか、myIme 設定画面からの再利用も想定する。
//!
//! # パーサの契約
//!
//! 1. 入力は UTF-8 のバイト列。先頭の BOM（`EF BB BF`）は除く。UTF-8 として読めなければ
//!    `Err`（別の文字コードを化けたまま通さない）
//! 2. 改行は `\n` と `\r\n` を受け付ける。空行（空文字列、または `\r` だけの行）は
//!    読み飛ばす
//! 3. 区切り文字はタブだけ。引用符やエスケープは解釈しない（フィールド内のタブや改行は
//!    扱わない）
//! 4. 列数が行によって違う場合は、行番号（物理行、1 始まり、空行も数える）を付けて
//!    `Err` を返す。その行を捨てて成功扱いにはしない
//! 5. 上限は [`MAX_TSV_BYTES`]（1 MiB）・[`MAX_TSV_ROWS`]（1 万行）。超えたら `Err`
//! 6. 0 行は `Ok`（空の結果）として返す。画面側では成功とも失敗とも別の表示にする
//!    （`screens::tsv_import` 参照）

use std::fmt;

/// 読み込める TSV の最大バイト数（1 MiB）。超過は [`TsvError::TooLarge`]。
pub const MAX_TSV_BYTES: usize = 1024 * 1024;
/// 読み込める TSV の最大データ行数（空行を除く）。超過は [`TsvError::TooManyRows`]。
pub const MAX_TSV_ROWS: usize = 10_000;

/// UTF-8 の BOM（バイトオーダーマーク）。
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 解析済みの TSV。行ごとに列（`String`）を持つ（引用符・エスケープは解釈しないため、
/// 生の `split('\t')` の結果がそのまま列になる、モジュール doc「パーサの契約」3 参照）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TsvTable {
    /// データ行（空行を除く）。各要素が 1 行ぶんの列。
    pub rows: Vec<Vec<String>>,
}

impl TsvTable {
    /// データ行数。
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 列数（先頭行の列数。0 行なら 0。契約 4 により全行で同じ列数になっている）。
    pub fn column_count(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }

    /// データ行が 0 行かどうか。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// TSV 解析の失敗理由。表示文言は日本語（`japanese-style.md` 準拠）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TsvError {
    /// [`MAX_TSV_BYTES`] を超えるバイト数。
    TooLarge,
    /// UTF-8 として読めなかった。
    NotUtf8,
    /// 列数が行によって違う（`line` は物理行番号、1 始まり、空行も数える）。
    ColumnMismatch {
        line: usize,
        expected: usize,
        found: usize,
    },
    /// [`MAX_TSV_ROWS`] を超えるデータ行数。
    TooManyRows,
}

impl fmt::Display for TsvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TsvError::TooLarge => {
                write!(
                    f,
                    "{} MiB を超える TSV は読み込めません",
                    MAX_TSV_BYTES / (1024 * 1024)
                )
            }
            TsvError::NotUtf8 => write!(f, "UTF-8 の TSV ではありません"),
            TsvError::ColumnMismatch {
                line,
                expected,
                found,
            } => write!(
                f,
                "{line} 行目の列数が {found} です（先頭行は {expected} 列）"
            ),
            TsvError::TooManyRows => {
                write!(f, "{MAX_TSV_ROWS} 行を超える TSV は読み込めません")
            }
        }
    }
}

impl std::error::Error for TsvError {}

/// TSV をバイト列から解析する（モジュール doc「パーサの契約」参照）。
///
/// 処理順: (1) バイト数上限判定 → (2) BOM 除去 → (3) UTF-8 デコード → (4) 改行分割 +
/// 空行読み飛ばし → (5) タブ分割して列数照合 → (6) 行数上限判定。
pub fn parse_tsv(bytes: &[u8]) -> Result<TsvTable, TsvError> {
    if bytes.len() > MAX_TSV_BYTES {
        return Err(TsvError::TooLarge);
    }

    let without_bom = bytes.strip_prefix(BOM).unwrap_or(bytes);
    let text = std::str::from_utf8(without_bom).map_err(|_| TsvError::NotUtf8)?;

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut expected_columns: Option<usize> = None;

    for (idx, raw_line) in text.split('\n').enumerate() {
        let line_number = idx + 1;
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }

        let columns: Vec<String> = line.split('\t').map(str::to_string).collect();
        match expected_columns {
            None => expected_columns = Some(columns.len()),
            Some(expected) if expected != columns.len() => {
                return Err(TsvError::ColumnMismatch {
                    line: line_number,
                    expected,
                    found: columns.len(),
                });
            }
            Some(_) => {}
        }

        rows.push(columns);
        if rows.len() > MAX_TSV_ROWS {
            return Err(TsvError::TooManyRows);
        }
    }

    Ok(TsvTable { rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_tsv() {
        let table = parse_tsv(b"a\tb\nc\td\n").unwrap();
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.rows[0], vec!["a", "b"]);
        assert_eq!(table.rows[1], vec!["c", "d"]);
    }

    #[test]
    fn strips_leading_bom() {
        let mut bytes = BOM.to_vec();
        bytes.extend_from_slice(b"a\tb\n");
        let table = parse_tsv(&bytes).unwrap();
        assert_eq!(table.rows[0], vec!["a", "b"]);
        assert!(!table.rows[0][0].starts_with('\u{feff}'));
    }

    #[test]
    fn accepts_crlf_line_endings() {
        let table = parse_tsv(b"a\tb\r\nc\td\r\n").unwrap();
        assert_eq!(table.row_count(), 2);
        assert_eq!(table.rows[1], vec!["c", "d"]);
    }

    #[test]
    fn skips_blank_lines_including_cr_only() {
        let table = parse_tsv(b"a\tb\n\n\r\nc\td\n").unwrap();
        assert_eq!(table.row_count(), 2);
    }

    #[test]
    fn trailing_newline_does_not_add_empty_row() {
        let table = parse_tsv(b"a\tb\n").unwrap();
        assert_eq!(table.row_count(), 1);
    }

    #[test]
    fn empty_fields_count_as_columns() {
        let table = parse_tsv(b"a\t\tb\n").unwrap();
        assert_eq!(table.column_count(), 3);
        assert_eq!(table.rows[0], vec!["a", "", "b"]);
    }

    #[test]
    fn quotes_are_treated_as_plain_characters() {
        let table = parse_tsv(b"\"a\tb\"\n").unwrap();
        assert_eq!(table.column_count(), 2);
        assert_eq!(table.rows[0], vec!["\"a", "b\""]);
    }

    #[test]
    fn rejects_non_utf8_bytes() {
        assert_eq!(parse_tsv(&[0xFF, 0xFE]), Err(TsvError::NotUtf8));
    }

    #[test]
    fn rejects_shift_jis_bytes() {
        // Shift_JIS の「あ」(0x82 0xA0) は不正な UTF-8 シーケンス。
        assert_eq!(parse_tsv(&[0x82, 0xA0]), Err(TsvError::NotUtf8));
    }

    #[test]
    fn rejects_column_mismatch_with_physical_line_number() {
        let err = parse_tsv(b"a\tb\nc\n").unwrap_err();
        assert_eq!(
            err,
            TsvError::ColumnMismatch {
                line: 2,
                expected: 2,
                found: 1,
            }
        );
    }

    #[test]
    fn column_mismatch_line_number_counts_blank_lines() {
        let err = parse_tsv(b"a\tb\n\nc\n").unwrap_err();
        assert_eq!(
            err,
            TsvError::ColumnMismatch {
                line: 3,
                expected: 2,
                found: 1,
            }
        );
    }

    #[test]
    fn rejects_over_byte_limit() {
        let bytes = vec![b'a'; MAX_TSV_BYTES + 1];
        assert_eq!(parse_tsv(&bytes), Err(TsvError::TooLarge));
    }

    #[test]
    fn rejects_over_row_limit() {
        let mut text = String::new();
        for _ in 0..=MAX_TSV_ROWS {
            text.push_str("a\n");
        }
        assert_eq!(parse_tsv(text.as_bytes()), Err(TsvError::TooManyRows));
    }

    #[test]
    fn empty_input_is_ok_and_empty() {
        let table = parse_tsv(b"").unwrap();
        assert!(table.is_empty());
        assert_eq!(table.column_count(), 0);
    }

    #[test]
    fn bom_only_is_ok_and_empty() {
        let table = parse_tsv(BOM).unwrap();
        assert!(table.is_empty());
    }

    #[test]
    fn blank_lines_only_is_ok_and_empty() {
        let table = parse_tsv(b"\n\r\n\n").unwrap();
        assert!(table.is_empty());
    }
}
