//! カタログの起動画面の選択（Issue #1200 で各 OS の入口から移した）。`lib.rs` の `#[fandhe::main]` の関数が呼ぶ。
//!
//! 入口（`fandhe`）は画面の名前を知らず、渡した関数が返す画面を出すだけ。カタログは検証・撮影で画面を
//! 名前で選ぶため、名前の解決（環境変数 `FANDHE_PROXY_APP` → ビルド時の `option_env!` → Android の debug システムプロパティ）と、
//! 画面ごとの準備（Android の画面ゲート・デスクトップの自動撮影）をここで行う。名前は固定文字列との比較にだけ使い、
//! パス・コマンドへは展開しない（A03 インジェクション対策、security.md）。
//!
//! 未指定は sample 画面（`ProxyApp::new`）、未登録の名前は 1 行警告して sample へ倒す。`prototype`・`tv-focus` は
//! 登録表ではなく専用のアプリ（[`crate::demo`]）を出す。

use fandhe_render_egui::app::{ProxyApp, ProxyAppDyn};

/// 起動画面を選んで作る。
pub(crate) fn app(ctx: &egui::Context) -> Box<dyn ProxyAppDyn> {
    let name = requested_app_name();
    // 他画面でのアナログ入力・他画面宛てのドロップによる再描画を抑える画面ゲートを、画面の構築前に設定する
    // （`fandhe_cap_gamepad::set_screen_active`・`fandhe_cap_drag_drop::set_screen_active` doc 参照）。
    #[cfg(target_os = "android")]
    {
        let name = name.as_deref().unwrap_or("");
        fandhe_cap_gamepad::set_screen_active(name == "gamepad_show");
        fandhe_cap_tilt_motion::set_screen_active(name == "tilt_ball");
        fandhe_cap_drag_drop::set_screen_active(name == "drag_drop");
    }
    #[allow(unused_mut)] // 自動撮影を配線するデスクトップだけが書き換える。
    let mut app: Box<dyn ProxyAppDyn> = match name.as_deref() {
        Some("prototype") => return Box::new(crate::demo::PrototypeApp::new(ctx)),
        Some("tv-focus") => return Box::new(crate::demo::TvFocusApp::new(ctx)),
        Some(name) => crate::build_app(name, ctx).unwrap_or_else(|| {
            eprintln!("[fandhe-gallery] 未登録の画面名 '{name}'、sample を表示する。");
            Box::new(ProxyApp::new(ctx))
        }),
        None => Box::new(ProxyApp::new(ctx)),
    };
    // `FANDHE_PROXY_AUTOSHOT=<path>`: 数フレーム後に撮影して PNG を保存し、ウィンドウを閉じる（デスクトップの実描画確認用。
    // `prototype`・`tv-focus` は撮影経路を持たないため対象外）。保存の失敗は `main.rs` が終了コードへ写す。
    #[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
    if let Ok(path) = std::env::var("FANDHE_PROXY_AUTOSHOT") {
        app.set_autoshot(
            std::path::PathBuf::from(path),
            std::sync::Arc::clone(&AUTOSHOT_FAILED),
        );
    }
    app
}

/// 自動撮影の PNG 保存に失敗したか（`main.rs` が `run_native` の後に見る）。`ProxyApp` は終了コードへ介入できないため共有フラグで受ける。
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
static AUTOSHOT_FAILED: std::sync::LazyLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::LazyLock::new(Default::default);

/// 自動撮影の PNG 保存に失敗したか。
#[cfg(not(any(target_os = "android", target_os = "ios", target_arch = "wasm32")))]
pub fn autoshot_failed() -> bool {
    AUTOSHOT_FAILED.load(std::sync::atomic::Ordering::SeqCst)
}

/// 起動する画面名。実行時の `FANDHE_PROXY_APP`（空は未指定）→ ビルド時の `option_env!`（wasm32・Android のように
/// 実行時に環境変数を渡しにくい OS 向け）→ OS ごとの既定（Windows の URL・ファイル起動、Android の debug プロパティ）の順。
fn requested_app_name() -> Option<String> {
    std::env::var("FANDHE_PROXY_APP")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("FANDHE_PROXY_APP").map(str::to_string))
        .or_else(os_default_name)
}

/// Windows: レジストリのコマンドは環境変数を渡せないため、URL 起動（Issue #1216）なら `url_scheme_show`、
/// ファイル起動（Issue #416）なら `file_association_show` を既定にする。起動済みインスタンスへ転送した起動は、
/// 入口の起動前の登録（`fandhe`）の中で終了済みなので、ここへ来るのは自分が受ける側のときだけ。
#[cfg(target_os = "windows")]
fn os_default_name() -> Option<String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    if fandhe_cap_url_scheme::windows::parse_url_args(&args).is_some() {
        Some("url_scheme_show".to_string())
    } else if fandhe_cap_file_association::windows::is_file_launch(&args) {
        Some("file_association_show".to_string())
    } else {
        None
    }
}

/// Android: `adb shell am start` は環境変数を渡せないため、debug システムプロパティを読む（feature `debug-proxy-app`）。
#[cfg(target_os = "android")]
fn os_default_name() -> Option<String> {
    Some(read_debug_proxy_app_property()).filter(|name| !name.is_empty())
}

#[cfg(not(any(target_os = "windows", target_os = "android")))]
fn os_default_name() -> Option<String> {
    None
}

/// bionic の `__system_property_get` を最小 FFI で呼び、
/// `debug.fandhe.proxy_app` システムプロパティの現在値を返す（TASK-17.2）。
///
/// 呼び出し元: [`os_default_name`]（`FANDHE_PROXY_APP` の後段）
/// （上記コメント参照）。`adb shell setprop debug.fandhe.proxy_app tv-focus` で
/// 値を設定できる（`scripts/android/verify-tv-focus.sh` が使う）。プロパティ未設定・
/// 取得失敗時は空文字列を返す（呼び出し元は空文字列を未指定として扱い、既定の sample 画面へ倒す）。
///
/// # Safety
/// 本関数が含む `unsafe` 操作は下記 FFI 呼び出し 1 箇所のみで、それ以外の
/// `unsafe` 操作（生ポインタ経由の追加読み書き等）は行わない（根拠は下記
/// `// SAFETY:` コメント参照。`#[allow(unsafe_code)]` は `lib.rs` の `android_main` と同じ
/// FFI 例外規定（`.claude/rules/coding-rust.md`）に基づき本関数 1 箇所に閉じる）。
#[allow(unsafe_code)]
#[cfg(all(target_os = "android", feature = "debug-proxy-app"))]
fn read_debug_proxy_app_property() -> String {
    const PROPERTY_NAME: &[u8] = b"debug.fandhe.proxy_app\0";
    // bionic のプロパティ値は PROP_VALUE_MAX（92 バイト、NUL 終端込み）を超えない。
    let mut buf = [0u8; 92];

    // SAFETY: `PROPERTY_NAME` は NUL 終端済みの `'static` バイト列を指すポインタで、
    // `__system_property_get` はこれを読み取り専用の C 文字列として扱う（書き込まない）。
    // `buf` は 92 バイト（bionic が定義する PROP_VALUE_MAX の上限）確保済みで、
    // `__system_property_get` はこの上限を超えて書き込まない契約（bionic
    // `<sys/system_properties.h>` の仕様）。戻り値は書き込んだバイト数（NUL 終端含まず、
    // 負値なし）であり、`buf` の範囲外を読み書きすることはない。
    // 注意: この 92 バイト仮定はレガシー API `__system_property_get` 固有の
    // 後方互換 ABI 契約（長い値は切り詰められる）であり、可変長対応の代替 API
    // （`__system_property_read_callback` 等）へは流用不可。
    let len = unsafe {
        libc::__system_property_get(PROPERTY_NAME.as_ptr().cast(), buf.as_mut_ptr().cast())
    };

    if len <= 0 {
        return String::new();
    }
    let len = usize::try_from(len).unwrap_or(0).min(buf.len());
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// `debug-proxy-app` feature 無効時（ストア提出物を含む既定ビルド）の
/// [`read_debug_proxy_app_property`] 代替。システムプロパティを一切読まず空文字列を返し、
/// 起動画面を既定（sample）へ落とす（Issue #718）。
#[cfg(all(target_os = "android", not(feature = "debug-proxy-app")))]
fn read_debug_proxy_app_property() -> String {
    String::new()
}

#[cfg(all(test, target_os = "android"))]
mod tests {
    use super::*;

    /// `read_debug_proxy_app_property` はエミュレータ実機での
    /// `adb shell setprop` 前提の関数であり、ホスト側単体テストでは値の内容を
    /// 検証できない（実機検証は `scripts/android/verify-tv-focus.sh`、TASK-17.2）。
    /// ここでは panic せず戻り値を返すことのみを最小確認する（`cargo ndk test` で
    /// Android ターゲット向けにコンパイル・実行できることの確認を兼ねる）。
    /// `debug-proxy-app` feature 無効時は FFI を含まないスタブ（空文字列）が対象になる
    /// （Issue #718）。FFI 版の確認には `--features debug-proxy-app` が要る。
    #[test]
    fn read_debug_proxy_app_property_does_not_panic() {
        let _ = read_debug_proxy_app_property();
    }
}
