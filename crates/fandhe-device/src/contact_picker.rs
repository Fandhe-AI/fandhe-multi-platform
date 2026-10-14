//! 連絡先を 1 件選ぶ契約（Phase 11 の拡充ループ、親 #1117、Issue #1220）。
//!
//! 利用者が OS のピッカーで選んだ 1 件の **表示名だけ** を返す。電話番号・メール・識別子の
//! 取得、連絡先の一覧取得、書き込みは扱わない。`fandhe-egui-proxy` の effect ドライバ
//! （`device/contact_pick.rs`）が呼ぶ。iOS / Android / Web の実装（#1221 / #1222 / #1223）も
//! 本書の契約に従う。
//!
//! # 結果の区別（嘘の成功を作らない）
//!
//! - 成功は [`PickedContact`] のみ。前後の空白を除くと空になる名前は成功にせず
//!   `Internal("contact_picker empty name")`（[`validate_contact_name`]）
//! - 利用者のキャンセルは `Cancelled("contact_picker")`。失敗として見せない
//! - 権限拒否・制限は `PermissionDenied("contact_picker")`
//! - ピッカーを出せなかった・未知の結果コードは `Internal`、非対応 OS は `Unsupported`
//!
//! # 要求の扱い
//!
//! - 再要求は置き換え。保留中に新しい要求が来たら OS 側のピッカーを閉じ、古い要求の完了は
//!   要求 id の照合で捨てる
//! - 完了通知は一度きり。ピッカーの提示は UI スレッドで行う
//! - モバイル / Web では、Activity 再生成やページ離脱の後に届いた結果を世代照合で捨て、
//!   最後の結果を返し続けない。デスクトップは 1 プロセス 1 画面でプロセス終了が破棄になる
//! - 名前（個人情報）はログ・エラーメッセージに出さず、永続化しない
//!
//! # 守る範囲・守らない範囲
//!
//! 守る: 1 プロセス内の要求と完了の順序、キャンセル・拒否・空名・失敗・非対応を成功に
//! 見せないこと、ピッカーの強参照の解放。守らない: 複数プロセス・複数タブでの同時操作、
//! OS 側で非同期に確定する選択との時間差で表示が一時的に食い違うこと。
//!
//! # macOS の権限
//!
//! `CNContactPicker` が TCC を要するか公式文書に明記がないため、macOS 実装は提示前に
//! 認可状態を確かめ、未決定なら許可を求める安全側で実装する。ピッカーだけなら不要と
//! 分かった場合は Mac 目視（#1213）の結果をもとに外す。

use crate::DeviceError;

/// 選ばれた連絡先。表示名だけを持つ（電話・メール・識別子は持たない）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickedContact {
    /// OS から届いた表示名（空でない。加工しない）。
    pub display_name: String,
}

/// OS から届いた名前を契約へ写す。前後の空白を除くと空なら `Internal`（成功にしない）。
/// trim は判定にだけ使い、保存する値は届いた文字列のまま。
pub fn validate_contact_name(name: String) -> Result<PickedContact, DeviceError> {
    if name.trim().is_empty() {
        return Err(DeviceError::Internal("contact_picker empty name"));
    }
    Ok(PickedContact { display_name: name })
}

/// 連絡先選択の契約。同期 API（コールバック型 OS は `fandhe-egui-proxy` の pull 型 bridge
/// を使いこのトレイトを通らない。`photo_picker` と同じ分担）。
pub trait ContactPicker: Send + Sync {
    /// ピッカーを 1 回開き、選ばれた 1 件を返す。キャンセルは `Err(Cancelled)`。
    fn pick(&self) -> Result<PickedContact, DeviceError>;
}

/// 対象外 OS 用。ダミーの成功実装へは倒さない。
pub struct UnsupportedContactPicker;

impl ContactPicker for UnsupportedContactPicker {
    fn pick(&self) -> Result<PickedContact, DeviceError> {
        Err(DeviceError::Unsupported("contact_picker"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_blank_names_are_not_success() {
        for n in ["", "   ", "\u{3000}\n"] {
            assert!(matches!(
                validate_contact_name(n.to_string()),
                Err(DeviceError::Internal("contact_picker empty name"))
            ));
        }
    }

    #[test]
    fn name_is_kept_as_received() {
        let c = validate_contact_name(" 山田 太郎 ".to_string()).unwrap();
        assert_eq!(c.display_name, " 山田 太郎 ");
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedContactPicker.pick(),
            Err(DeviceError::Unsupported("contact_picker"))
        ));
    }
}
