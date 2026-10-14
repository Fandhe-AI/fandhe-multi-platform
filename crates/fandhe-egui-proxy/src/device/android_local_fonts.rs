//! Android 向け OS 搭載フォント一覧実装（Issue #1378、親 #1145）。`device/local_fonts.rs` の
//! Android 経路（`drive_sync`）から egui スレッドでボタン押下ごとに同期で 1 回呼ばれる。
//! 契約は `fandhe_device::local_fonts`（範囲はファミリー名の一覧のみ。描画への利用は範囲外）。
//!
//! Java 側 `MainActivity.localFontsList()`（JNI シグネチャ `()[Ljava/lang/Object;`）が
//! `LocalFontsHelper.list` へ委譲し、`SystemFonts.getAvailableFonts()` の各 `Font` から
//! `getBuffer()`（direct・read-only の mmap）と `getTtcIndex()` を返す。詰め方の SSOT は
//! `LocalFontsHelper.java` 冒頭。`Font` に名前を返す公開 API は無いため、Rust が `name` テーブル
//! だけを読んでファミリー名を取り出す（純関数 `family_name`、ホストでもテストする）。
//!
//! - API 29 未満は長さ 0 の配列が返り `Unsupported`。0 件の成功にしない
//! - 名前は nameID 16（typographic family）を優先し、無ければ 1。Unicode / Windows の名前のみ
//!   使い、Windows の en-US（0x409）を優先する
//! - 壊れたフォント・`name` 無し・TTC の index 範囲外はそのフォントだけ捨てる。1 件以上あるのに
//!   1 件も名前が取れなければ `require_named` が `Internal`。null・例外・形が不正も `Internal`
//! - 整形・重複除去・並び・上限は `families_from` に任せる
//! - ファイルパス・書体情報は読まず、バイト列は保持しない。権限は宣言しない。名前はログに出さない
//! - 同期の一発読み取りで保留中の要求・コールバック・購読を持たないので、Activity 再生成時に
//!   破棄すべきものは無い（古い結果が新しい要求を消費する経路も無い。世代照合は共通コア側）
//!
//! 守る範囲: 1 プロセス内の取得、失敗の非隠蔽。守らない範囲: 読み取り後のフォント追加・削除の
//! 反映（再取得まで）、OS 間のファミリー名の一致。初回が 100 ms を超えるなら `drive_worker` へ
//! 移す判断を実機確認（#1375）へ送る。
//!
//! 一次情報:
//! <https://developer.android.com/reference/android/graphics/fonts/SystemFonts>、
//! <https://developer.android.com/reference/android/graphics/fonts/Font#getBuffer()>

#[cfg(any(target_os = "android", test))]
use ttf_parser::{name::Names, name_id, PlatformId, RawFace, Tag};

/// `id` の名前を 1 件選ぶ。Windows かつ en-US を最優先し、無ければ復号できる最初の名前。
/// Mac Roman など Unicode 系でない名前は使わない。
#[cfg(any(target_os = "android", test))]
fn pick(names: Names<'_>, id: u16) -> Option<String> {
    const WINDOWS_EN_US: u16 = 0x409;
    let mut fallback = None;
    for n in names
        .into_iter()
        .filter(|n| n.name_id == id && n.is_unicode())
    {
        let Some(s) = n.to_string() else { continue };
        if n.platform_id == PlatformId::Windows && n.language_id == WINDOWS_EN_US {
            return Some(s);
        }
        fallback.get_or_insert(s);
    }
    fallback
}

/// フォントのバイト列（TTC なら `ttc_index` 番）からファミリー名を読む。読めなければ `None`。
/// `Face::parse` は head 等を要求するため使わず、table directory と `name` だけを読む。
#[cfg(any(target_os = "android", test))]
fn family_name(data: &[u8], ttc_index: u32) -> Option<String> {
    let raw = RawFace::parse(data, ttc_index).ok()?;
    let table = raw.table(Tag::from_bytes(b"name"))?;
    let names = ttf_parser::name::Table::parse(table)?.names;
    pick(names, name_id::TYPOGRAPHIC_FAMILY).or_else(|| pick(names, name_id::FAMILY))
}

#[cfg(target_os = "android")]
mod jni_impl {
    use super::family_name;
    use crate::device::android::JniCallFailed;
    use fandhe_device::local_fonts::{families_from, require_named, LocalFontFamilies, LocalFonts};
    use fandhe_device::DeviceError;
    use jni::objects::{JByteBuffer, JIntArray, JObject, JObjectArray};

    /// Java の戻り値を読んだ結果。
    enum Outcome {
        /// API 29 未満（長さ 0 の配列）。
        Unsupported,
        /// 試みた件数と、名前が取れたフォントの名前。
        Fonts(usize, Vec<String>),
    }

    /// Android の `LocalFonts` 実装（`MainActivity.localFontsList()` を JNI で呼ぶ）。
    pub(super) struct AndroidLocalFonts;

    /// 1 要素を読む。取れない要素は `None`（そのフォントだけ捨てる）。呼び出し側が local frame で囲む。
    #[allow(unsafe_code)]
    fn read_one(env: &mut jni::Env, buffers: &JObjectArray, ttc: i32, i: usize) -> Option<String> {
        let obj = buffers.get_element(env, i).ok()?;
        let buf = env.cast_local::<JByteBuffer>(obj).ok()?;
        if buf.is_null() {
            return None;
        }
        // direct でないバッファはアドレスが取れず Err になり、捨てる。
        let ptr = env.get_direct_buffer_address(&buf).ok()?;
        let len = env.get_direct_buffer_capacity(&buf).ok()?;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: `ptr` / `len` は direct な `ByteBuffer` のアドレスと容量で、`buf` の local 参照を
        // 保持している間（この関数の間）有効。`SystemFonts` のキャッシュが `Font` を保持しており、
        // Java 側からは read-only の mmap なので書き込みと競合しない。`slice` はこの関数内で
        // `String` を作った後に捨て、外へ持ち出さない。
        let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
        family_name(slice, u32::try_from(ttc).ok()?)
    }

    impl LocalFonts for AndroidLocalFonts {
        /// 2 箇所の `unsafe` は `AndroidAudioDevices::list` と同じ根拠・同じ形。
        #[allow(unsafe_code)]
        fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
            const FAILED: DeviceError = DeviceError::Internal("local_fonts list");
            let Some(app) = crate::android::android_app() else {
                return Err(FAILED);
            };

            // SAFETY: `AndroidAudioDevices::list` と同一の根拠。`vm_as_ptr()` は
            // android-activity が管理する有効な `JavaVM*`、`app` は呼び出し中生存する。
            let vm = unsafe { jni::JavaVM::from_raw(app.vm_as_ptr().cast()) };

            let result = vm.attach_current_thread(
                |env: &mut jni::Env| -> Result<Option<Outcome>, JniCallFailed> {
                    let raw_activity = app.activity_as_ptr() as jni::sys::jobject;
                    // SAFETY: `AndroidAudioDevices::list` と同一の根拠。未所有の JNI
                    // グローバル参照を `Cast` で包むだけで delete は試みない。
                    let activity =
                        unsafe { env.as_cast_raw::<jni::objects::JObject>(&raw_activity)? };
                    let ret = env.call_method(
                        &activity,
                        jni::jni_str!("localFontsList"),
                        jni::jni_sig!(() -> java.lang.Object[]),
                        &[],
                    );
                    let ret = match ret {
                        Ok(v) => v,
                        Err(e) => {
                            // 例外は自動クリアされない（jni 0.22）。文言は読まない。
                            if env.exception_check() {
                                env.exception_clear();
                            }
                            return Err(e.into());
                        }
                    };
                    let outer = env.cast_local::<JObjectArray>(ret.l()?)?;
                    if outer.is_null() {
                        return Ok(None);
                    }
                    match outer.len(env)? {
                        0 => return Ok(Some(Outcome::Unsupported)),
                        2 => {}
                        _ => return Ok(None),
                    }
                    let b: JObject = outer.get_element(env, 0)?;
                    let t: JObject = outer.get_element(env, 1)?;
                    let buffers = env.cast_local::<JObjectArray>(b)?;
                    let ttcs = env.cast_local::<JIntArray>(t)?;
                    if buffers.is_null() || ttcs.is_null() {
                        return Ok(None);
                    }
                    let n = buffers.len(env)?;
                    if ttcs.len(env)? != n {
                        return Ok(None);
                    }
                    let mut indices = vec![0i32; n];
                    ttcs.get_region(env, 0, &mut indices)?;

                    let mut names = Vec::new();
                    for (i, ttc) in indices.into_iter().enumerate() {
                        // 要素ごとに local 参照を解放し、数百件でも上限に当たらないようにする。
                        let one = env.with_local_frame(4, |env| {
                            Ok::<_, JniCallFailed>(read_one(env, &buffers, ttc, i))
                        })?;
                        names.extend(one);
                    }
                    if env.exception_check() {
                        env.exception_clear();
                    }
                    Ok(Some(Outcome::Fonts(n, names)))
                },
            );

            match result.ok().flatten().ok_or(FAILED)? {
                Outcome::Unsupported => Err(DeviceError::Unsupported("local_fonts")),
                Outcome::Fonts(attempted, names) => require_named(attempted, families_from(names)),
            }
        }
    }
}

/// Android の `LocalFonts` 実装を返す。
#[cfg(target_os = "android")]
pub(crate) fn implementation() -> std::sync::Arc<dyn fandhe_device::local_fonts::LocalFonts> {
    std::sync::Arc::new(jni_impl::AndroidLocalFonts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UTF-16BE。
    fn u16be(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect()
    }

    /// `name` テーブル。`(platform, encoding, language, nameID, 文字列のバイト列)`。
    fn name_table(recs: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        let head = 6 + 12 * recs.len();
        let mut out = Vec::new();
        for v in [0u16, recs.len() as u16, head as u16] {
            out.extend(v.to_be_bytes());
        }
        let mut off = 0usize;
        for (p, e, l, id, s) in recs {
            for v in [*p, *e, *l, *id, s.len() as u16, off as u16] {
                out.extend(v.to_be_bytes());
            }
            off += s.len();
        }
        for (.., s) in recs {
            out.extend(s);
        }
        out
    }

    /// 1 フォント（table directory + 表）。表のオフセットは先頭 `base` からの絶対位置で書く。
    fn font(base: usize, tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let dir = 12 + 16 * tables.len();
        let mut out = Vec::new();
        out.extend(0x0001_0000u32.to_be_bytes());
        out.extend((tables.len() as u16).to_be_bytes());
        out.extend([0u8; 6]);
        let mut off = base + dir;
        for (tag, data) in tables {
            out.extend(*tag);
            out.extend(0u32.to_be_bytes());
            out.extend((off as u32).to_be_bytes());
            out.extend((data.len() as u32).to_be_bytes());
            off += data.len();
        }
        for (_, data) in tables {
            out.extend(data);
        }
        out
    }

    fn win(id: u16, lang: u16, s: &str) -> (u16, u16, u16, u16, Vec<u8>) {
        (3, 1, lang, id, u16be(s))
    }

    fn one(recs: &[(u16, u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        font(0, &[(b"name", name_table(recs))])
    }

    #[test]
    fn typographic_family_wins_over_family() {
        let d = one(&[win(1, 0x409, "Foo"), win(16, 0x409, "Foo Family")]);
        assert_eq!(family_name(&d, 0).as_deref(), Some("Foo Family"));
    }

    #[test]
    fn falls_back_to_family() {
        let d = one(&[win(1, 0x409, "Foo")]);
        assert_eq!(family_name(&d, 0).as_deref(), Some("Foo"));
    }

    #[test]
    fn en_us_preferred_over_other_language() {
        let d = one(&[win(1, 0x411, "フー"), win(1, 0x409, "Foo")]);
        assert_eq!(family_name(&d, 0).as_deref(), Some("Foo"));
        let d = one(&[win(1, 0x411, "フー")]);
        assert_eq!(family_name(&d, 0).as_deref(), Some("フー"));
    }

    #[test]
    fn mac_only_names_are_dropped() {
        let d = one(&[(1, 0, 0, 1, b"Foo".to_vec())]);
        assert_eq!(family_name(&d, 0), None);
    }

    #[test]
    fn no_name_table_or_garbage_is_none() {
        assert_eq!(family_name(&font(0, &[(b"head", vec![0; 4])]), 0), None);
        assert_eq!(family_name(&[], 0), None);
        assert_eq!(family_name(b"garbage-bytes!", 0), None);
        let mut d = one(&[win(1, 0x409, "Foo")]);
        d.truncate(20);
        assert_eq!(family_name(&d, 0), None);
    }

    #[test]
    fn ttc_selects_index_and_rejects_out_of_range() {
        let head = 12 + 4 * 2;
        let a = font(head, &[(b"name", name_table(&[win(1, 0x409, "A")]))]);
        let b = font(
            head + a.len(),
            &[(b"name", name_table(&[win(1, 0x409, "B")]))],
        );
        let mut d = Vec::new();
        d.extend(b"ttcf");
        d.extend(0x0001_0000u32.to_be_bytes());
        d.extend(2u32.to_be_bytes());
        d.extend((head as u32).to_be_bytes());
        d.extend(((head + a.len()) as u32).to_be_bytes());
        d.extend(&a);
        d.extend(&b);
        assert_eq!(family_name(&d, 0).as_deref(), Some("A"));
        assert_eq!(family_name(&d, 1).as_deref(), Some("B"));
        assert_eq!(family_name(&d, 2), None);
    }

    #[test]
    fn bundled_font_has_a_family_name() {
        let n = family_name(fandhe_fonts::NOTO_SANS_JP_REGULAR, 0).unwrap();
        assert!(n.starts_with("Noto Sans JP"), "{n}");
    }
}
