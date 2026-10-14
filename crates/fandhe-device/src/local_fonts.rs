//! Phase 11 の拡充ループ（親 #1145、Issue #1376）: 端末に入っているフォントの「ファミリー名」を
//! 一覧で 1 回読む契約。`fandhe-egui-proxy` の `device/local_fonts.rs` が「一覧を取得」ボタン押下ごとに
//! [`LocalFonts::list_families`] を 1 回だけ呼び、各 OS 実装（macOS: AppKit `NSFontManager`、
//! Windows: DirectWrite、iOS: UIKit `UIFont.familyNames`、Android: #1378）が本トレイトを実装する。Web は
//! `fandhe-web-dom` 側（#1379）で `queryLocalFonts()` を使い、本トレイトは経由しない。
//!
//! 範囲は「ファミリー名を一覧で出す」だけ。選んで描画に使うこと、書体（太さ・斜体・PostScript 名）、
//! ファイルパス、フォントのバイト列は扱わない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: [`LocalFontFamilies`]。`families` が空は「OS が 0 件を返した」正常な結果
//! - 非対応: `Err(DeviceError::Unsupported("local_fonts"))`（手段が無い OS・版。Android API 29 未満を含む）
//! - 失敗: `Err(DeviceError::Internal("local_fonts list"))`（OS 呼び出しの失敗、または 1 件以上
//!   あるのに名前を 1 件も取れなかった場合。[`require_named`]）
//!
//! `PermissionDenied` は本トレイトでは返さない。拒否が実在するのは Web だけで、Web は本トレイトを
//! 通らず画面側が直接 `PermissionDenied` に写す。キャンセルと「正常終了 / 失敗」の三値は無い
//! （同期の 1 回読み。Windows の期限切れは呼び出し側が失敗で確定する）。
//!
//! # 取らないもの
//!
//! fullName・PostScript 名・スタイル・ファイルパス・バイト列・書体数は読まない・保持しない。
//! ファミリー名はフィンガープリントになりうるため、ログ・エラー文言に出さず画面状態にだけ置く。
//!
//! # 整形・重複除去・並び・上限
//!
//! 全 OS 実装は生の名前を [`families_from`] に渡すだけにする。名前を [`clean_text`]
//! （制御文字除去・128 文字）に通し、名前の無いものを捨て、整形後の完全一致で重複を除き、
//! `str` 昇順（コードポイント順）に並べ、先頭 [`MAX_FAMILIES`] 件だけ残す。並べてから切るので
//! 切り捨ては入力順に依らず決定的で、切ったときは `truncated` を立てる。名前は OS が既定で返す
//! 正規（英語）名で、ローカライズ名への変換や大文字小文字・全半角の畳み込みはしない。
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 嘘の値を出さないこと（`Ok` の空と `Err` の区別、`truncated`、失敗を成功へ畳まない、
//! 名前を作り替えない）、ファミリー名以外を取らない・保持しない・ログに出さないこと、1 プロセス内の取得、
//! Windows で列挙が戻らないときに UI を固めないこと（5 秒）、Web の許可拒否を失敗と区別すること。
//! 守らない範囲: OS 間・ブラウザ間の一覧の一致、ダウンロード型・アプリ同梱フォントが載るか、
//! 読み取り後のフォントの追加・削除（再取得まで）、複数タブ・複数プロセス、ロケール順、
//! ローカライズ名、[`MAX_FAMILIES`] 超の件数。
//!
//! 並行性: 読み取りは 1 要求 1 回で、購読・コールバックを持たない。集約すべき並行経路は無い。
//!
//! # 既知の天井
//!
//! 並びはロケール照合でなくコードポイント順（日本語の五十音順にはならない）。

use std::collections::BTreeSet;

use crate::DeviceError;
use crate::device_info::clean_text;

/// 返すファミリー名の最大件数。仮置きの値で、実機で足りなければ上げる。
pub const MAX_FAMILIES: usize = 1000;

/// フォントのファミリー名の一覧。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LocalFontFamilies {
    /// 整形・重複除去済みで昇順のファミリー名（最大 [`MAX_FAMILIES`] 件）。
    pub families: Vec<String>,
    /// [`MAX_FAMILIES`] を超えて切り捨てたか。
    pub truncated: bool,
}

/// 端末のフォントのファミリー名を 1 回読む。
pub trait LocalFonts: Send + Sync {
    /// 契約はモジュール doc 参照。
    fn list_families(&self) -> Result<LocalFontFamilies, DeviceError>;
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedLocalFonts;

impl LocalFonts for UnsupportedLocalFonts {
    fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
        Err(DeviceError::Unsupported("local_fonts"))
    }
}

/// OS が返した生の名前の列を契約の値にする（整形・重複除去・昇順・上限）。全 OS 実装が共用する。
pub fn families_from(names: impl IntoIterator<Item = String>) -> LocalFontFamilies {
    let set: BTreeSet<String> = names.into_iter().filter_map(|n| clean_text(&n)).collect();
    let truncated = set.len() > MAX_FAMILIES;
    LocalFontFamilies {
        families: set.into_iter().take(MAX_FAMILIES).collect(),
        truncated,
    }
}

/// `attempted`（OS が返したファミリー数）が 1 以上なのに名前が 1 件も残らなければ `Internal`
/// （全件が名前なしのとき嘘の 0 件にしない）。
pub fn require_named(
    attempted: usize,
    f: LocalFontFamilies,
) -> Result<LocalFontFamilies, DeviceError> {
    if attempted > 0 && f.families.is_empty() {
        Err(DeviceError::Internal("local_fonts list"))
    } else {
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedLocalFonts.list_families(),
            Err(DeviceError::Unsupported("local_fonts"))
        ));
    }

    #[test]
    fn cleans_and_drops_nameless() {
        let f = families_from(s(&[" Fo\u{0}nt\n1 ", "", " \t\n"]));
        assert_eq!(f.families, ["Font1"]);
        assert!(!f.truncated);
    }

    #[test]
    fn dedups_after_cleaning() {
        let f = families_from(s(&["Font A", " Font A\n", "Font A"]));
        assert_eq!(f.families, ["Font A"]);
    }

    #[test]
    fn sorts_by_code_point_without_case_folding() {
        let f = families_from(s(&["b", "B", "a"]));
        assert_eq!(f.families, ["B", "a", "b"]);
    }

    #[test]
    fn cap_boundary_and_determinism() {
        let make = |n: usize| (0..n).map(|i| format!("f{i:05}")).collect::<Vec<_>>();
        assert!(!families_from(make(MAX_FAMILIES)).truncated);
        let over = families_from(make(MAX_FAMILIES + 1));
        assert!(over.truncated);
        assert_eq!(over.families.len(), MAX_FAMILIES);
        assert_eq!(
            over.families[MAX_FAMILIES - 1],
            format!("f{:05}", MAX_FAMILIES - 1)
        );
        // 入力順に依らず同じ先頭が残る。
        let mut rev = make(MAX_FAMILIES + 1);
        rev.reverse();
        assert_eq!(families_from(rev), over);
    }

    #[test]
    fn cap_counts_unique_names_only() {
        let mut v: Vec<String> = (0..MAX_FAMILIES).map(|i| format!("f{i:05}")).collect();
        v.push("f00000".into());
        assert!(!families_from(v).truncated);
    }

    #[test]
    fn require_named_rejects_only_all_nameless() {
        let empty = LocalFontFamilies::default();
        assert!(require_named(0, empty.clone()).is_ok());
        assert!(matches!(
            require_named(3, empty),
            Err(DeviceError::Internal("local_fonts list"))
        ));
        assert!(require_named(3, families_from(s(&["A"]))).is_ok());
    }
}
