//! スマートホーム（登録済みの家と機器の名前を先頭 20 件だけ読む）の抽象（Phase 11 の拡充ループ、
//! 対応表 `docs/device/capabilities/smart-home.json`、親 #147、Issue #1079）。
//!
//! 扱う範囲は「ボタンを押すと、OS に登録済みの家の数と（家の名前, 機器の名前）の組を OS の返す
//! 順で先頭 [`MAX_ACCESSORIES`] 件だけ読む」ことだけ。機器の操作・状態・部屋・シーン・家の追加は
//! 扱わない。対象は iOS（HomeKit）のみ。Android は GMS 依存のため保留、macOS / Windows / Web は
//! 対応表で `no`。`fandhe-egui-proxy` の `device/smart_home.rs` がワーカースレッドで
//! [`SmartHome::list_accessories`] を呼ぶ（ブロッキングの契約）。全 OS が [`accessories_from`] を
//! 通して整形する。
//!
//! # 契約
//!
//! - 結果は成功（家 0 件・機器 0 件を含む）か [`SmartHomeFailure`]。`DeviceError` に variant を足すと
//!   既存の網羅 match が壊れるため機能固有の enum にする（`music_library::MusicLibraryFailure` と同じ判断）
//! - **家 0 件の成功（`home_count == 0`）と、家が 1 件以上あって機器 0 件の成功を区別する**
//! - **0 件を出すのは、許可済みで OS が空を返したときだけ**。iOS では `authorizationStatus` に
//!   `authorized` があり、かつ初回 `homeManagerDidUpdateHomes` の後でだけ `homes` を読む。生成直後の
//!   空の `homes` を 0 件の成功にしない（`device-bridge-contracts.md`「6. 嘘の成功」）。
//!   `determined` があって `authorized` が無い場合（`restricted` を含む）は
//!   [`SmartHomeFailure::PermissionDenied`]。期限内にどちらにも確定しなければ
//!   [`SmartHomeFailure::Timeout`]。多重起動・バッファ不正は [`SmartHomeFailure::Failed`]
//! - 期限は [`LIST_TIMEOUT`] が唯一の値で、許可ダイアログを待つ時間を含む。実装は引数で受けた
//!   `timeout` 以内に必ず戻り、起点は呼び出しの入口で `Instant` 等の単調時計で測る。期限切れで
//!   戻る前に OS 側の待ち登録を外す（類型 3）
//! - **三値**: 利用者が取り消す UI は無い。拒否は [`SmartHomeFailure::PermissionDenied`]、許可ダイアログの
//!   放置は [`SmartHomeFailure::Timeout`] で、どちらも失敗として表示する（成功にもキャンセルにも
//!   しない。類型 10）
//! - manager の寿命: iOS の `HMHomeManager` はプロセスで 1 つだけ作る。初回の要求で main queue 上に
//!   生成し、static に強参照で保持する（意図した永続で解放しない。類型 1・9）。起動時には生成しない
//! - 一覧の保持: 家の構成は Rust にも ObjC にも保持せず、要求のたびに `homes` から読み直す
//! - 家・機器の名前と OS のエラー値をログ・エラー・`Debug` に出さない（類型 8）
//!
//! # 守る範囲と守らない範囲
//!
//! 守る: 1 プロセス内で要求を 1 件ずつ処理すること、家 0 件・機器 0 件・拒否・期限切れ・失敗の区別、
//! 嘘の成功を出さないこと、期限後に待ち登録を残さないこと、名前を出力しないこと。
//!
//! 守らない:
//! - 読み取り後の構成変化と複数プロセス
//! - `HMHome` の並び順の意味
//! - 機器の到達可否
//! - 共有・招待中の家が載るか
//! - 設定アプリで許可を取り消した直後の 1 回（反映の時間差）
//! - 期限後も OS の読み込みが走り切ること（結果は捨てる）
//! - Android（保留）

use std::time::Duration;

use crate::device_info::clean_text;

/// 読む機器数の上限。
pub const MAX_ACCESSORIES: usize = 20;

/// 一覧取得の期限（Rust 側の唯一の値）。初回の許可ダイアログへの応答時間を含み、
/// 各 OS 実装は別の期限を持たない。
pub const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// 機器 1 件。値は [`clean_text`] 済みで、名前の無い（空・制御文字だけの）項目は `None`。
#[derive(Clone, PartialEq, Eq)]
pub struct Accessory {
    /// 所属する家の名前。
    pub home: Option<String>,
    /// 機器の名前。
    pub name: Option<String>,
}

// 家・機器の名前を出さない。
impl std::fmt::Debug for Accessory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Accessory")
    }
}

/// 読み取った家と機器の一覧。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct AccessoryList {
    /// 登録済みの家の数（機器 0 件との区別に使う）。
    pub home_count: usize,
    /// OS の返す順の先頭 [`MAX_ACCESSORIES`] 件まで。
    pub accessories: Vec<Accessory>,
    /// [`MAX_ACCESSORIES`] を超える機器があって切り捨てたか。
    pub truncated: bool,
}

// 件数と切り捨て有無だけを出す。
impl std::fmt::Debug for AccessoryList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessoryList")
            .field("home_count", &self.home_count)
            .field("count", &self.accessories.len())
            .field("truncated", &self.truncated)
            .finish()
    }
}

/// 一覧取得の失敗種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartHomeFailure {
    /// 端末に API が無い・対象外 OS。
    Unsupported,
    /// 利用者または OS 設定が HomeKit の利用を拒否した。
    PermissionDenied,
    /// 期限内に結果が確定しない。0 件にはしない。
    Timeout,
    /// 上記以外の OS 呼び出し失敗（多重起動・壊れたバッファを含む）。
    Failed,
}

/// スマートホーム読み取りの契約（モジュール doc の「契約」節）。
pub trait SmartHome: Send + Sync {
    /// 家の数と機器を OS の返す順に先頭 [`MAX_ACCESSORIES`] 件まで読む。呼び出し側がワーカー
    /// スレッドで呼ぶブロッキング呼び出しで、`timeout` 以内に必ず戻る。
    fn list_accessories(&self, timeout: Duration) -> Result<AccessoryList, SmartHomeFailure>;
}

/// 対象外 OS・初期化失敗時のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedSmartHome;

impl SmartHome for UnsupportedSmartHome {
    fn list_accessories(&self, _timeout: Duration) -> Result<AccessoryList, SmartHomeFailure> {
        Err(SmartHomeFailure::Unsupported)
    }
}

/// OS が返した家の数と (家の名前, 機器の名前) を整形する。全 OS 実装が通る唯一の経路。
/// 各値は [`clean_text`] を通し、[`MAX_ACCESSORIES`] 件で切り、超過があれば `truncated`。
/// 両方 `None` の行も 1 件として残す。0 件は空の一覧（正当な結果）。
pub fn accessories_from(
    home_count: usize,
    raw: impl IntoIterator<Item = (Option<String>, Option<String>)>,
) -> AccessoryList {
    let mut accessories = Vec::new();
    let mut truncated = false;
    for (home, name) in raw {
        if accessories.len() == MAX_ACCESSORIES {
            truncated = true;
            break;
        }
        accessories.push(Accessory {
            home: home.as_deref().and_then(clean_text),
            name: name.as_deref().and_then(clean_text),
        });
    }
    AccessoryList {
        home_count,
        accessories,
        truncated,
    }
}

/// C ABI の戻りコードを失敗へ写す共通表（iOS の `.m` が使う）。
/// 0 は成功なので `None`、1 = `Unsupported`、2 = `PermissionDenied`、3 = `Timeout`、
/// 4 と未知の値は `Failed`。
pub fn decode_code(code: i32) -> Option<SmartHomeFailure> {
    match code {
        0 => None,
        1 => Some(SmartHomeFailure::Unsupported),
        2 => Some(SmartHomeFailure::PermissionDenied),
        3 => Some(SmartHomeFailure::Timeout),
        _ => Some(SmartHomeFailure::Failed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(h: &str, n: &str) -> (Option<String>, Option<String>) {
        (Some(h.to_string()), Some(n.to_string()))
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedSmartHome.list_accessories(LIST_TIMEOUT),
            Err(SmartHomeFailure::Unsupported)
        );
    }

    #[test]
    fn empty_with_zero_homes_is_ok() {
        let l = accessories_from(0, []);
        assert_eq!(l.home_count, 0);
        assert!(l.accessories.is_empty());
        assert!(!l.truncated);
    }

    #[test]
    fn homes_without_accessories_is_ok_and_distinct() {
        let none = accessories_from(0, []);
        let two = accessories_from(2, []);
        assert_ne!(none, two);
        assert_eq!(two.home_count, 2);
        assert!(two.accessories.is_empty());
    }

    #[test]
    fn truncates_after_max() {
        let l = accessories_from(1, (0..MAX_ACCESSORIES).map(|i| row("h", &format!("a{i}"))));
        assert_eq!(l.accessories.len(), MAX_ACCESSORIES);
        assert!(!l.truncated);
        let l = accessories_from(1, (0..=MAX_ACCESSORIES).map(|i| row("h", &format!("a{i}"))));
        assert_eq!(l.accessories.len(), MAX_ACCESSORIES);
        assert!(l.truncated);
        assert_eq!(l.accessories[0].name.as_deref(), Some("a0"));
    }

    #[test]
    fn cleans_values_and_keeps_nameless_rows() {
        let l = accessories_from(1, [row("  A\u{0}B ", " \t"), (None, None)]);
        assert_eq!(l.accessories[0].home.as_deref(), Some("AB"));
        assert_eq!(l.accessories[0].name, None);
        assert_eq!(l.accessories.len(), 2);
    }

    #[test]
    fn decode_code_maps_all() {
        assert_eq!(decode_code(0), None);
        assert_eq!(decode_code(1), Some(SmartHomeFailure::Unsupported));
        assert_eq!(decode_code(2), Some(SmartHomeFailure::PermissionDenied));
        assert_eq!(decode_code(3), Some(SmartHomeFailure::Timeout));
        assert_eq!(decode_code(4), Some(SmartHomeFailure::Failed));
        assert_eq!(decode_code(-1), Some(SmartHomeFailure::Failed));
        assert_eq!(decode_code(99), Some(SmartHomeFailure::Failed));
    }

    #[test]
    fn debug_hides_names() {
        let l = accessories_from(1, [row("SecretHome", "SecretLamp")]);
        let s = format!("{l:?} {:?}", l.accessories[0]);
        assert!(!s.contains("Secret"), "{s}");
        assert!(s.contains("count"));
    }
}
