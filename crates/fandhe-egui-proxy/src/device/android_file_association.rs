//! Android 向けファイルの関連付け受信（Issue #418）。`device/file_association.rs::platform` の
//! Android 経路から使われ、Java 側 `FileAssociationHelper`（`MainActivity.deliverUrlIntent` が
//! `onCreate` の `getIntent()` / `onNewIntent` で `offer` する）が
//! [`Java_com_fandhe_androidapp_FileAssociationHelper_nativeOnFileOpened`] を呼ぶ。
//!
//! 継続配信型（`url_scheme` と同形）で要求 id も世代もない。受け口 [`INBOX`] はプロセス全体の
//! static で、Activity が作り直されても残り、コールドスタートで UI より先に届いた分も溜まる。
//! 検査（拡張子・MIME・名前）と読み取り（8 MiB 上限）は共通側 `FileInbox` に任せ、本モジュールは
//! JNI の橋渡しと結果コードの写像だけを持つ。名前・URI・中身はログへ出さない。
//! feature `device` 無効ビルド向けの no-op は `crate::android` に置く。
//!
//! # JNI 署名と結果コード
//!
//! `FileAssociationHelper.nativeOnFileOpened(int code, String name, String mime, int fd)`
//! （Java 宣言・no-op・本実体の 3 か所で一致させる）。code の SSOT は Java 側の定数。
//!
//! | code | 意味 | 扱い |
//! | --- | --- | --- |
//! | 0 | FILE（fd 有効） | `receive_file`（検査と読み取りは共通側） |
//! | 1 | REJECTED（`file:` など） | `reject` |
//! | 2 | UNREADABLE | `Err(Unreadable)` |
//! | 3 | FOLDER | `Err(Folder)` |
//! | その他 | 想定外 | `Err(Unreadable)`（成功に畳まない） |

use std::fs::File;

use fandhe_device::drag_drop::DropError;
use fandhe_device::file_association::{FileAssociation, FileInbox, FileSnapshot};
use fandhe_device::DeviceError;

/// 受信したファイルの受け口（プロセス全体で 1 つ）。
static INBOX: FileInbox = FileInbox::new();

/// 受信時刻（UNIX ms、表示用の壁時計。エポック前なら 0）。
#[cfg(target_os = "android")]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Java の結果コードを `inbox` への記録へ写す。`file` の drop で fd は必ず 1 回 close される。
fn deliver(
    inbox: &FileInbox,
    code: i32,
    name: Option<&str>,
    mime: Option<&str>,
    file: Option<File>,
    now_ms: u64,
) {
    match code {
        0 => match file {
            Some(file) => inbox.receive_file(name, mime, file, now_ms),
            None => inbox.record(Err(DropError::Unreadable), now_ms),
        },
        1 => inbox.reject(),
        3 => inbox.record(Err(DropError::Folder), now_ms),
        // 2 と想定外の code。
        _ => inbox.record(Err(DropError::Unreadable), now_ms),
    }
}

/// `FileAssociationHelper.nativeOnFileOpened` の実体。
///
/// # Safety
/// `fd >= 0` のとき Java 側が `detachFd()` で手放した有効な fd を渡す契約で、最初に
/// `OwnedFd` で所有する（`device/android.rs::decode_file_pick_result` と同じ根拠）。
/// `#[no_mangle]` は JNI 命名規約上の固定シンボル名。`EnvUnowned::with_env` が panic を遮断する。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FileAssociationHelper_nativeOnFileOpened<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    code: jni::sys::jint,
    name: jni::objects::JString<'caller>,
    mime: jni::objects::JString<'caller>,
    fd: jni::sys::jint,
) {
    use std::os::fd::{FromRawFd, OwnedFd};

    // 他の処理より先に所有し、どの経路でも drop で close する。
    // SAFETY: fd >= 0 は Java 側 detachFd 済みの有効 fd で、他に渡さないため二重 close は無い。
    let file = (fd >= 0).then(|| File::from(unsafe { OwnedFd::from_raw_fd(fd) }));
    let outcome = unowned_env.with_env(|env| -> Result<(), jni::errors::Error> {
        let name = (!name.is_null())
            .then(|| name.try_to_string(env).ok())
            .flatten();
        let mime = (!mime.is_null())
            .then(|| mime.try_to_string(env).ok())
            .flatten();
        deliver(
            &INBOX,
            code,
            name.as_deref(),
            mime.as_deref(),
            file,
            now_ms(),
        );
        Ok(())
    });
    let _ = outcome.into_outcome();
}

/// `FileAssociationHelper.nativeAccepts` の実体。読み取り前に共通層 `accepts` で受理可否を返す。
/// 失敗（文字列の取得失敗）は名前・MIME なし扱いで、拒否側に倒す。
#[cfg(target_os = "android")]
#[allow(unsafe_code)]
#[no_mangle]
extern "system" fn Java_com_fandhe_androidapp_FileAssociationHelper_nativeAccepts<'caller>(
    mut unowned_env: jni::EnvUnowned<'caller>,
    _class: jni::objects::JClass<'caller>,
    name: jni::objects::JString<'caller>,
    mime: jni::objects::JString<'caller>,
) -> jni::sys::jboolean {
    let outcome = unowned_env.with_env(|env| -> Result<bool, jni::errors::Error> {
        let name = (!name.is_null())
            .then(|| name.try_to_string(env).ok())
            .flatten();
        let mime = (!mime.is_null())
            .then(|| mime.try_to_string(env).ok())
            .flatten();
        Ok(fandhe_device::file_association::accepts(
            name.as_deref(),
            mime.as_deref(),
        ))
    });
    // JNI 失敗時は拒否（false）。
    outcome.into_outcome().ok().unwrap_or(false).into()
}

/// Android 向け `FileAssociation` 実装。`snapshot` は static 受け口の読み出しで常に成功する。
struct AndroidFileAssociation;

impl FileAssociation for AndroidFileAssociation {
    fn snapshot(&self) -> Result<FileSnapshot, DeviceError> {
        Ok(INBOX.snapshot())
    }
}

/// Android の実装を返す。
pub(crate) fn implementation() -> &'static dyn FileAssociation {
    &AndroidFileAssociation
}

#[cfg(test)]
mod tests {
    use super::*;
    use fandhe_device::file_association::OpenFailure;
    use std::io::Write;

    fn tmp(tag: &str) -> File {
        let p = std::env::temp_dir().join(format!("fa418-{}-{tag}", std::process::id()));
        let mut f = File::create(&p).unwrap();
        f.write_all(b"hello world, 0123456789").unwrap();
        drop(f);
        let f = File::open(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        f
    }

    fn run(code: i32, name: Option<&str>, mime: Option<&str>, file: Option<File>) -> FileSnapshot {
        let inbox = FileInbox::new();
        deliver(&inbox, code, name, mime, file, 7);
        inbox.snapshot()
    }

    #[test]
    fn accepted() {
        let s = run(0, Some("a.txt"), Some("text/plain"), Some(tmp("a")));
        let f = s.latest.unwrap().unwrap();
        assert_eq!(f.name, "a.txt");
        assert_eq!(f.head, b"hello world, 012");
        let s = run(
            0,
            Some("123"),
            Some("text/plain; charset=utf-8"),
            Some(tmp("b")),
        );
        assert!(s.latest.unwrap().is_ok());
    }

    #[test]
    fn rejected() {
        for (n, m) in [
            (None, Some("text/plain")),
            (Some("a.md"), Some("image/png")),
        ] {
            let s = run(0, n, m, Some(tmp("c")));
            assert_eq!((s.rejected, s.latest), (1, None));
        }
        assert_eq!(run(1, None, None, None).rejected, 1);
    }

    #[test]
    fn failures_are_never_success() {
        let cases = [
            (0, None, OpenFailure::Unreadable),
            (2, None, OpenFailure::Unreadable),
            (3, None, OpenFailure::Folder),
            (99, Some(tmp("d")), OpenFailure::Unreadable),
        ];
        for (code, file, want) in cases {
            let s = run(code, Some("a.txt"), Some("text/plain"), file);
            assert_eq!(s.latest, Some(Err(want)));
        }
    }
}
