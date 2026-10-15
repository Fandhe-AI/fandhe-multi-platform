//! iOS 向けメール・SMS 作成画面（Issue #1385）。`device/compose_mail_sms.rs` の iOS 経路
//! （`begin_compose`）から呼ばれ、MessageUI 実装（`ios/fandhe_ios_compose_mail_sms.m`）を
//! `unsafe extern "C"` 1 関数で呼ぶ。`unsafe` はこの呼び出しと結果コールバックに閉じる。
//!
//! 結果は `.m` の delegate から [`fandhe_rust_compose_mail_sms_outcome`] が呼ばれ、
//! `compose_mail_sms::bridge`（pull 型・`RequestSlot`）へ置く。コード表は
//! `compose_mail_sms::outcome_from_ios_code`。
//!
//! 契約表の対応: 1・2（delegate は static 強参照、`done` フラグと `request_id` 照合で二重・古い通知を捨てる）、
//! 4（コールバック直後に `request_repaint`）、5（main queue・`@autoreleasepool`）、
//! 6（Failed・提示失敗・canSend false を成功に畳まない）、10（Cancelled は失敗にしない）。
//! 守らない範囲: 作成画面を開いた後の利用者の操作、背景遷移中の作成画面の扱い（OS に任せる）。
//! 添付（Issue #32）は一時ファイルを作らず `addAttachmentData:` へ NSData で渡す（契約表 9）。
//! 宛先・件名・本文・添付はログへ出さない。

use fandhe_device::compose_mail_sms::{ComposeKind, ComposeRequest};
#[cfg(not(target_os = "ios"))]
use fandhe_device::compose_mail_sms::{ComposeMailSms, UnsupportedComposeMailSms};
use fandhe_device::DeviceError;

/// `.m` 側へ渡す種別コード（0 = メール、1 = SMS）。
#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
fn kind_code(kind: ComposeKind) -> i32 {
    match kind {
        ComposeKind::Mail => 0,
        ComposeKind::Sms => 1,
    }
}

// SAFETY: `ios/fandhe_ios_compose_mail_sms.m` が定義する C ABI。契約は `.m` 冒頭コメントに集約する。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
unsafe extern "C" {
    fn fandhe_ios_compose_mail_sms_present(
        request_id: u64,
        kind: i32,
        to: *const std::os::raw::c_char,
        cc: *const std::os::raw::c_char,
        bcc: *const std::os::raw::c_char,
        subject: *const std::os::raw::c_char,
        body: *const std::os::raw::c_char,
        att_data: *const u8,
        att_len: usize,
        att_mime: *const std::os::raw::c_char,
        att_name: *const std::os::raw::c_char,
    ) -> i32;
}

/// 作成画面の表示を依頼する。`Err` は依頼自体が受理されなかった（呼び出し元がその場で
/// `deliver` する）。`Ok` なら結果は後で [`fandhe_rust_compose_mail_sms_outcome`] が届ける。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
pub(crate) fn begin_compose(request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    use std::ffi::CString;
    const PRESENT: DeviceError = DeviceError::Internal("compose present");
    // `ComposeRequest` が NUL を含む制御文字を弾いているため通常は失敗しない。
    // 各リストは `\n` 連結で渡す（`\n` は検証で拒否される文字で宛先に現れない。`.m` 側で配列へ戻す）。
    let (Ok(to), Ok(cc), Ok(bcc), Ok(subject), Ok(body)) = (
        CString::new(req.to().join("\n")),
        CString::new(req.cc().join("\n")),
        CString::new(req.bcc().join("\n")),
        CString::new(req.subject()),
        CString::new(req.body()),
    ) else {
        return Err(PRESENT);
    };
    // 添付は一時ファイルを作らず、バイト列・MIME・名前をそのまま渡す（添付なしは長さ 0・NULL）。
    // MIME と名前は検証で NUL を含まないため通常は失敗しない。
    let att = req.attachment();
    let (Ok(att_mime), Ok(att_name)) = (
        CString::new(att.map_or("", |a| a.mime_type())),
        CString::new(att.map_or("", |a| a.file_name())),
    ) else {
        return Err(PRESENT);
    };
    let att_data = att.map_or(&[][..], |a| a.data());
    // SAFETY: 7 つの `CString` と `att_data` のスライスは呼び出しが返るまで生存する有効な領域
    // （`att_data` は `att_len` バイト、長さ 0 のとき `.m` は読まない）。`.m` 側は return 前に
    // `NSString` / `NSData` へコピーするため、所有権は移らず返却後は参照されない。
    let code = unsafe {
        fandhe_ios_compose_mail_sms_present(
            request_id,
            kind_code(req.kind()),
            to.as_ptr(),
            cc.as_ptr(),
            bcc.as_ptr(),
            subject.as_ptr(),
            body.as_ptr(),
            att_data.as_ptr(),
            att_data.len(),
            att_mime.as_ptr(),
            att_name.as_ptr(),
        )
    };
    if code == 0 {
        Ok(())
    } else {
        Err(PRESENT)
    }
}

/// ホストのテストビルドでは実体が無いため非対応を返す。
#[cfg(not(target_os = "ios"))]
pub(crate) fn begin_compose(_request_id: u64, req: &ComposeRequest) -> Result<(), DeviceError> {
    UnsupportedComposeMailSms.compose(req).map(|_| ())
}

/// `.m` 側の delegate から呼ばれる。コードの解釈は `compose_mail_sms::outcome_from_ios_code`。
/// 結果を置いた直後に次フレームを起こす（egui は入力が無いと描画せず、画面が処理中のまま止まるため）。
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
#[no_mangle]
extern "C" fn fandhe_rust_compose_mail_sms_outcome(request_id: u64, code: i32) {
    super::compose_mail_sms::bridge::deliver(
        request_id,
        super::compose_mail_sms::outcome_from_ios_code(code),
    );
    crate::ios::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_code_matches_m_contract() {
        assert_eq!(kind_code(ComposeKind::Mail), 0);
        assert_eq!(kind_code(ComposeKind::Sms), 1);
    }

    #[cfg(not(target_os = "ios"))]
    #[test]
    fn host_is_unsupported() {
        let req = ComposeRequest::sms("", "b").unwrap();
        assert!(matches!(
            begin_compose(1, &req),
            Err(DeviceError::Unsupported("compose"))
        ));
    }
}
