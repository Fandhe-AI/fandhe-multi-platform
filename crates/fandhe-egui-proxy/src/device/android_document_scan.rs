//! Android 向け書類スキャン（ML Kit Document Scanner、親 #145、Issue #1125。契約は
//! `fandhe_device::document_scan`）。`device/document_scan.rs` の Android 経路（dispatch）から
//! [`implementation`] が呼ばれる。Java 側 `DocumentScanHelper` が Google Play services の撮影 UI を出し、
//! 1 ページ目を長辺 2048 以下の JPEG にして返す。
//!
//! 流れ: `scan` → JNI `MainActivity.requestDocumentScan(id, maxBytes)` → Java が UI スレッドで前面・
//! Google Play services を確認し、撮影 UI の起動直前に `nativeTryBeginPresent`（本ファイルの export が
//! `document_scan::try_begin_present` を呼ぶ）で期限切れ・差し替えと排他にする → 結果を worker で JPEG 化し
//! ページのファイルを消してから `nativeOnDocumentScanResult(id, kind, pageCount, jpeg)` →
//! [`map_result`] が契約の結果へ写して `document_scan::complete_from_os` へ 1 回だけ渡す。
//!
//! 結果コードの写像（Java `DocumentScanHelper.KIND_*` と同じ値）: `KIND_SCANNED` は
//! `ScannedPage::from_os`（1 段目の検査）を通した成功、`KIND_CANCELLED` は `Ok(Cancelled)`、
//! 他は同名の失敗、`KIND_INTERNAL` と未知のコードは `Internal`（成功へ倒さない）。
//! `PermissionDenied` は Android では返さない（撮影 UI は Play services 側でカメラ権限が不要）。
//! `TimedOut` は bridge の期限だけが確定する。成功値は bridge が表示開始を記録した要求にしか認めない。
//! 上限バイト数は `MAX_PAGE_BYTES` を正とし、Java へ引数で渡す（複製しない）。
//!
//! 守る範囲: 1 プロセス内の要求順序・嘘の成功の禁止・Activity 再生成時の旧要求の破棄
//! （`android.rs` が `reset_pending_tracking` を呼ぶ）と、旧要求のページファイルの削除。
//! 守らない範囲: 2 ページ目以降・PDF・撮影 UI の表示中のプロセス死・複数プロセス・初回のモジュール
//! ダウンロードが期限（10 秒）を超えること。画像と OS の文言はログに出さない。
//!
//! no-op スタブは置かない: Java の native は Rust の `requestDocumentScan` を経た後でしか呼ばれない
//! （`android_companion_device.rs` と同じ判断）。

use std::sync::Arc;

use fandhe_device::document_scan::{
    DocumentScanFailure, DocumentScanOutcome, DocumentScanner, ScannedPage,
};

/// 結果コード（Java 側 `DocumentScanHelper.KIND_*` と対）。
const KIND_SCANNED: i32 = 0;
const KIND_CANCELLED: i32 = 1;
const KIND_UNSUPPORTED: i32 = 2;
const KIND_EMPTY: i32 = 3;
const KIND_TOO_LARGE: i32 = 4;
const KIND_UNDECODABLE: i32 = 5;
const KIND_NOT_FOREGROUND: i32 = 6;
const KIND_INTERRUPTED: i32 = 7;
const KIND_INTERNAL: i32 = 8;

/// Java から届いた結果コードを契約の結果へ写像する。未知のコードは `Internal`。
/// 画像は `KIND_SCANNED` のときだけ使い、`ScannedPage::from_os` を通す。
fn map_result(
    kind: i32,
    page_count: i32,
    bytes: Option<Vec<u8>>,
) -> Result<DocumentScanOutcome, DocumentScanFailure> {
    use DocumentScanFailure as F;
    match kind {
        KIND_SCANNED => {
            let Some(bytes) = bytes else {
                return Err(F::Internal);
            };
            ScannedPage::from_os(bytes, u32::try_from(page_count).unwrap_or(0))
                .map(DocumentScanOutcome::Scanned)
        }
        KIND_CANCELLED => Ok(DocumentScanOutcome::Cancelled),
        KIND_UNSUPPORTED => Err(F::Unsupported),
        KIND_EMPTY => Err(F::Empty),
        KIND_TOO_LARGE => Err(F::TooLarge),
        KIND_UNDECODABLE => Err(F::Undecodable),
        KIND_NOT_FOREGROUND => Err(F::NotForeground),
        KIND_INTERRUPTED => Err(F::Interrupted),
        KIND_INTERNAL => Err(F::Internal),
        // 未知のコードも成功へ倒さない。
        _ => Err(F::Internal),
    }
}

/// Android の `DocumentScanner` 実装を返す。
pub(crate) fn implementation() -> Arc<dyn DocumentScanner> {
    Arc::new(AndroidDocumentScanner)
}

struct AndroidDocumentScanner;

impl DocumentScanner for AndroidDocumentScanner {
    /// 受理は「UI スレッドへ渡した」だけを意味し、結果は後から届く。
    fn scan(&self, request_id: u64) -> Result<(), DocumentScanFailure> {
        imp::scan(request_id)
    }
}

#[cfg(target_os = "android")]
mod imp {
    use fandhe_device::document_scan::MAX_PAGE_BYTES;
    use jni::objects::JValue;

    use super::super::android::JniCallFailed;
    use super::DocumentScanFailure;

    /// `MainActivity` のメソッドを呼ぶ。Activity 未確立・JNI 失敗は `None`（呼び出しの例外は握る）。
    #[allow(unsafe_code)]
    fn call_activity<T>(
        f: impl FnOnce(&mut jni::Env, &jni::objects::JObject) -> Result<T, JniCallFailed>,
    ) -> Option<T> {
        let app = crate::android::android_app()?;
        // SAFETY: `app.vm_as_ptr()` は android-activity が管理する有効な `JavaVM*` で、
        // `app` は本呼び出しの間生存する。
        let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };
        vm.attach_current_thread(|env: &mut jni::Env| -> Result<T, JniCallFailed> {
            let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
            // SAFETY: 未所有のグローバル参照を所有権なしでラップするだけで delete しない。
            let activity = unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
            f(env, &activity)
        })
        .ok()
    }

    pub(super) fn scan(id: u64) -> Result<(), DocumentScanFailure> {
        if crate::android::android_app().is_none() {
            return Err(DocumentScanFailure::NotForeground);
        }
        let max_bytes = i32::try_from(MAX_PAGE_BYTES).unwrap_or(i32::MAX);
        call_activity(|env, a| {
            env.call_method(
                a,
                jni::jni_str!("requestDocumentScan"),
                jni::jni_sig!((long, int) -> ()),
                &[JValue::Long(id as jni::sys::jlong), JValue::Int(max_bytes)],
            )?;
            Ok(())
        })
        .ok_or(DocumentScanFailure::Internal)
    }
}

/// `device.rs` が Android 限定で載せるため、ホストでは使われない。呼ばれても成功へ倒さない。
#[cfg(not(target_os = "android"))]
#[allow(dead_code)]
mod imp {
    use super::DocumentScanFailure;

    pub(super) fn scan(_id: u64) -> Result<(), DocumentScanFailure> {
        Err(DocumentScanFailure::Unsupported)
    }
}

/// `DocumentScanHelper.nativeTryBeginPresent` から呼ばれる JNI エクスポート。撮影 UI を起動する直前に
/// 呼ばれ、`false`（期限切れ確定済み・古い要求）なら Java は起動しない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 本クレートは cdylib として 1 つの Android 実行単位にのみリンクされるため他シンボルと衝突しない
/// （既存の JNI エクスポートと同じ根拠）。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_DocumentScanHelper_nativeTryBeginPresent<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
) -> jni::sys::jboolean {
    // JVM 境界を越えて panic させない（`with_env` が `catch_unwind` 済み）。
    let outcome = unowned_env.with_env(|_env| -> Result<bool, jni::errors::Error> {
        Ok(super::document_scan::try_begin_present(request_id as u64))
    });
    matches!(outcome.into_outcome(), jni::Outcome::Ok(true))
}

/// `DocumentScanHelper.nativeOnDocumentScanResult` から呼ばれる JNI エクスポート。ページのファイルを
/// 消した後に 1 回だけ呼ばれ、結果を bridge へ渡して次フレームを起こす（契約表 4 節）。
/// JPEG は長さを `MAX_PAGE_BYTES` と比べてからコピーし、超過は `KIND_TOO_LARGE` に差し替える。
/// 値はログへ出さない。
///
/// # Safety
/// この関数自体は `unsafe` 操作を含まない。`#[no_mangle]` は JNI 命名規約上の固定シンボル名で、
/// 既存の JNI エクスポートと同じ根拠。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_DocumentScanHelper_nativeOnDocumentScanResult<
    'caller,
>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    request_id: jni::sys::jlong,
    kind: jni::sys::jint,
    page_count: jni::sys::jint,
    jpeg: jni::objects::JByteArray<'caller>,
) {
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let mut kind = kind;
        let bytes = if jpeg.is_null() {
            None
        } else if jpeg.len(env)? > fandhe_device::document_scan::MAX_PAGE_BYTES {
            kind = KIND_TOO_LARGE;
            None
        } else {
            Some(env.convert_byte_array(&jpeg)?)
        };
        super::document_scan::complete_from_os(
            request_id as u64,
            map_result(kind, page_count, bytes),
        );
        Ok(())
    });
    let _ = outcome.into_outcome();
    // 入力が無いと egui が描画せず「処理中」で止まるため次フレームを起こす。
    crate::android::request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;
    use DocumentScanFailure as F;

    const PNG_HEAD: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    const JPEG_HEAD: [u8; 4] = [0xFF, 0xD8, 0xFF, 0xE0];

    #[test]
    fn map_result_covers_every_code() {
        let table = [
            (KIND_UNSUPPORTED, F::Unsupported),
            (KIND_EMPTY, F::Empty),
            (KIND_TOO_LARGE, F::TooLarge),
            (KIND_UNDECODABLE, F::Undecodable),
            (KIND_NOT_FOREGROUND, F::NotForeground),
            (KIND_INTERRUPTED, F::Interrupted),
            (KIND_INTERNAL, F::Internal),
        ];
        for (kind, failure) in table {
            assert_eq!(map_result(kind, 1, None), Err(failure));
        }
        assert_eq!(
            map_result(KIND_CANCELLED, 0, None),
            Ok(DocumentScanOutcome::Cancelled)
        );
    }

    #[test]
    fn unknown_codes_never_become_success() {
        for kind in [9, 99, -1, i32::MAX, i32::MIN] {
            assert_eq!(
                map_result(kind, 1, Some(PNG_HEAD.to_vec())),
                Err(F::Internal)
            );
        }
    }

    #[test]
    fn scanned_requires_bytes_and_pages() {
        assert_eq!(map_result(KIND_SCANNED, 1, None), Err(F::Internal));
        for pages in [0, -1, i32::MIN] {
            assert_eq!(
                map_result(KIND_SCANNED, pages, Some(PNG_HEAD.to_vec())),
                Err(F::Empty)
            );
        }
    }

    #[test]
    fn scanned_validates_image() {
        assert_eq!(
            map_result(KIND_SCANNED, 1, Some(vec![1, 2, 3, 4])),
            Err(F::Undecodable)
        );
        let mut big = PNG_HEAD.to_vec();
        big.resize(fandhe_device::document_scan::MAX_PAGE_BYTES + 1, 0);
        assert_eq!(map_result(KIND_SCANNED, 1, Some(big)), Err(F::TooLarge));
        for head in [PNG_HEAD.to_vec(), JPEG_HEAD.to_vec()] {
            let Ok(DocumentScanOutcome::Scanned(p)) = map_result(KIND_SCANNED, 3, Some(head))
            else {
                panic!("成功になるはず");
            };
            assert_eq!(p.page_count(), 3);
        }
    }

    #[test]
    fn bytes_ignored_for_non_scanned() {
        assert_eq!(
            map_result(KIND_CANCELLED, 1, Some(PNG_HEAD.to_vec())),
            Ok(DocumentScanOutcome::Cancelled)
        );
        assert_eq!(
            map_result(KIND_INTERNAL, 1, Some(PNG_HEAD.to_vec())),
            Err(F::Internal)
        );
    }

    #[test]
    fn host_scan_never_succeeds() {
        assert!(implementation().scan(1).is_err());
    }

    #[test]
    fn debug_hides_bytes() {
        let r = map_result(KIND_SCANNED, 1, Some(PNG_HEAD.to_vec()));
        assert!(!format!("{r:?}").contains("137"));
    }
}
