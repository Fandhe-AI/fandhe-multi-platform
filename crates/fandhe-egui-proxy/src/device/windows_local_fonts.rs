//! Windows 向け OS 搭載フォント一覧実装（Issue #1376）。DirectWrite のシステムフォントコレクション
//! （`IDWriteFactory::GetSystemFontCollection`）から各ファミリーの英語名（`en-us`、無ければ先頭の
//! ロケール）を読む。`device/local_fonts.rs` の Windows 経路（`drive_worker`）からワーカースレッド上で
//! 呼ばれ、契約は `fandhe_device::local_fonts`。
//!
//! # スレッドと COM
//!
//! `ComInit` は付けない。`DWriteCreateFactory` は `dwrite.dll` の export で `CoCreateInstance` を経ず、
//! COM の初期化を要しない。戻らない場合は呼び出し側の 5 秒の期限で失敗を確定し、残った呼び出しは
//! プロセス終了で OS が片付ける。
//!
//! # 取らないもの
//!
//! 書体（フェイス名・スタイル）・ファイルパス・フォントのバイト列は読まない。名前はログに出さない。
//!
//! # 失敗の扱い
//!
//! ファクトリ・コレクションの失敗は `Internal`。ファミリー単位の失敗はそのファミリーだけ捨て、
//! 1 件以上あるのに名前が 1 件も残らなければ `require_named` が `Internal` にする。
//!
//! # unsafe
//!
//! COM 呼び出しに閉じ、各呼び出しの直前に `// SAFETY:` を付ける（`lib.rs` の Windows
//! `deny(unsafe_code)` は関数単位の `allow` で緩める）。COM 参照は `windows` クレートの `Drop` で解放される。
//!
//! # 既知の天井
//!
//! - 名前は英語名で、ローカライズ名ではない。ダウンロード型フォントは対象外の見込み
//! - unpackaged アプリでの動作は実機で未確認（#1375）

use std::sync::Arc;

use fandhe_device::local_fonts::{families_from, require_named, LocalFontFamilies, LocalFonts};
use fandhe_device::DeviceError;
use windows::core::{w, BOOL};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, DWRITE_FACTORY_TYPE_SHARED,
};

struct WindowsLocalFonts;

/// 1 ファミリーの英語名。取れなければ `None`（そのファミリーだけ捨てる）。
#[allow(unsafe_code)]
fn family_name(collection: &IDWriteFontCollection, index: u32) -> Option<String> {
    // SAFETY: `collection` は有効な COM 参照で、`index` は `GetFontFamilyCount` 未満。
    let family = unsafe { collection.GetFontFamily(index) }.ok()?;
    // SAFETY: `family` は有効な COM 参照。
    let names = unsafe { family.GetFamilyNames() }.ok()?;
    let (mut idx, mut exists) = (0u32, BOOL(0));
    // SAFETY: `names` は有効な COM 参照で、出力先は呼び出しの間有効なローカル変数。
    // 見つからないとき（失敗または `exists` が偽）は先頭のロケールを使う。
    if unsafe { names.FindLocaleName(w!("en-us"), &mut idx, &mut exists) }.is_err()
        || !exists.as_bool()
    {
        idx = 0;
    }
    // SAFETY: `names` は有効な COM 参照。
    let len = unsafe { names.GetStringLength(idx) }.ok()? as usize;
    // NUL 終端の分を含めて確保する（`GetString` は終端を書く）。
    let mut buf = vec![0u16; len + 1];
    // SAFETY: `buf` は `len + 1` 要素の有効な排他領域で、`GetString` は `buf.len()` を超えて書かない。
    unsafe { names.GetString(idx, &mut buf) }.ok()?;
    String::from_utf16(&buf[..len]).ok()
}

impl LocalFonts for WindowsLocalFonts {
    #[allow(unsafe_code)]
    fn list_families(&self) -> Result<LocalFontFamilies, DeviceError> {
        const FAIL: DeviceError = DeviceError::Internal("local_fonts list");
        // SAFETY: 共有ファクトリの生成のみ。型とインターフェースは windows クレートの定義。
        let factory: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }.map_err(|_| FAIL)?;
        let mut collection: Option<IDWriteFontCollection> = None;
        // SAFETY: `factory` は有効な COM 参照で、出力先は呼び出しの間有効なローカル変数。
        unsafe { factory.GetSystemFontCollection(&mut collection, false) }.map_err(|_| FAIL)?;
        let collection = collection.ok_or(FAIL)?;
        // SAFETY: `collection` は有効な COM 参照。
        let count = unsafe { collection.GetFontFamilyCount() };
        let names = (0..count).filter_map(|i| family_name(&collection, i));
        require_named(count as usize, families_from(names))
    }
}

/// Windows の `LocalFonts` 実装を返す（`device/local_fonts.rs` の dispatch から呼ばれる）。
pub(crate) fn implementation() -> Arc<dyn LocalFonts> {
    Arc::new(WindowsLocalFonts)
}
