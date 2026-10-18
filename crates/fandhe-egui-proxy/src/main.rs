//! デスクトップ相当（native, glow バックエンド）の起動エントリ（TASK-1.2）。
//!
//! `FANDHE_PROXY_AUTOSHOT=<path>` を指定すると、数フレーム後に
//! `ViewportCommand::Screenshot` を要求し、応答を受け取り次第 PNG として `<path>` へ
//! 保存してウィンドウを閉じる（`xvfb-run` 上での実描画確認・エビデンス取得用、
//! `scripts/` からは呼ばず
//! 検証手順から手動実行する）。未指定時は通常のインタラクティブ起動になる。
//!
//! wasm32 / Android ターゲットでは本 bin はビルド対象に含めるが中身は no-op とし、
//! 実際のエントリはそれぞれ `web.rs`（`wasm_bindgen(start)`）・`android.rs`
//! （`android_main`、TASK-1.5-S2）が担う。Android は実行ファイル形式の `main` ではなく
//! `android-activity` が探す固定シンボルを要求するため、この bin ターゲット自体は
//! cargo-ndk の検証コマンド（`--lib` 指定）ではビルド対象外になる想定だが、
//! ワークスペース全体のビルド（`cargo build --workspace`）に混ざっても壊れないよう
//! no-op 化しておく。
//!
//! iOS ターゲットでは glow バックエンドが非対応のため、本 bin の中身は
//! `ios.rs` の wgpu（Metal）向けエントリ（`fandhe_egui_proxy::ios::run`）へ委譲する
//! （TASK-1.3-S2）。自動撮影（`FANDHE_PROXY_AUTOSHOT`）は iOS 側へは配線しない。
//!
//! Issue #1215: macOS ターゲット（feature `device` 有効時）では、通常の画面起動の
//! `run_native` より前に `device::apple_url_scheme::register` で URL scheme の
//! Apple Event ハンドラを登録する（起動時に届く URL を取りこぼさないため）。
//!
//! Issue #1216: Windows ターゲット（feature `device` 有効時）では、URL scheme 起動の判定と
//! 起動済みインスタンスへの転送（`device::windows_url_scheme::win::startup`）を、通常の画面起動より
//! 前に行う。URL 起動で `FANDHE_PROXY_APP` が未設定なら `url_scheme_show` 画面を既定にする。
//!
//! Issue #1044: Windows ターゲット（feature `device` 有効時）では、通常の画面
//! 起動より前に `device::windows_background_task::parse_scheduled_args` で
//! コマンドライン引数を見る。Windows タスク スケジューラが専用引数付きで本 exe を
//! 再起動した場合はそちらに一致し、`run_scheduled` が画面を出さずに実行結果を
//! 書いて終了する（`windows_background_task.rs` モジュール doc 参照）。
//!
// TASK-2.3（#54）計測マーカー: プロトタイプ描画対象切替（TASK-2.2-S5 / #52）の
// モジュール doc。プラットフォーム別コードとして計上する。
// fandhe-prototype:begin
//! `FANDHE_PROXY_APP=prototype` を指定すると、既定の `sample_screen()`
//! （[`fandhe_egui_proxy::ProxyApp`]）の代わりにプロトタイプアプリ
//! （[`fandhe_egui_proxy::PrototypeApp`]、TASK-2.2-S5 / #52）を描画対象として起動する。
//! 既定（未指定時）は `sample_screen()` を維持し（TASK-1.2 の検証・
//! 既存テスト `tests/desktop_render.rs` の前提を壊さない）、自動撮影
//! （`FANDHE_PROXY_AUTOSHOT`）は sample 側のみへ配線する（プロトタイプ側は対象外）。
//!
//! `FANDHE_PROXY_APP` に `prototype`/`tv-focus` 以外の値（`screens.rs` の
//! `generated_screens!` レジストリ登録名。例: `schema_example`）を指定すると、
//! 画面レジストリに登録した画面（`fandhe_core::screens::*`）を
//! [`fandhe_egui_proxy::build_app`] 経由で描画する
//! （Issue #715）。未登録名の場合は標準エラーへ 1 行警告を出し、sample へ倒す。
// fandhe-prototype:end

// TASK-5.3（REQ-5 / M4、Issue #238）: `frame-metrics` feature 有効時は計測専用ビルドへ
// エントリそのものを差し替える（`frame_metrics.rs` モジュール doc の「feature ゲート
// 契約」参照）。自動撮影（`FANDHE_PROXY_AUTOSHOT`）・プロトタイプ切替
// （`FANDHE_PROXY_APP`）とは独立した別ビルドのため、下の通常分岐は
// `not(feature = "frame-metrics")` で無効化し、両方の `fn main` が同時定義されないようにする。
// TASK-5.1（REQ-5、Issue #236 / #593）: macOS デスクトップ native 向けのコールドスタート
// 計測エントリ。`cold-start-probe` feature（既定 off）有効時のみこの `fn main` が使われる。
// `frame-metrics` と同時に有効化した場合は iOS 側（`ios.rs`）と同様に `cold-start-probe` を
// 優先する（`cfg` の並び順で `frame-metrics` 分岐より前に評価させるため、下 2 分岐の
// `cfg` には `not(feature = "cold-start-probe")` を追加している）。
#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")),
    feature = "cold-start-probe"
))]
fn main() -> eframe::Result<()> {
    let start = std::time::Instant::now();
    // 計測条件を通常起動と揃えるため、`NativeOptions` は下の通常分岐と同一内容を組む
    // （ウィンドウサイズ 480x360・バックエンドは eframe 既定）。分岐ごとに `fn main` が
    // 独立しているため設定そのものは共有できず、ここで同じ内容を再掲している。
    // iOS 側 `ios::run` の `metal_native_options` のような専用バックエンド指定は
    // デスクトップでは不要。
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        "fandhe-egui-proxy",
        native_options,
        Box::new(move |cc| {
            // 計測後に自動でウィンドウを閉じてプロセスを終了させる
            // （`close_after_log = true`。複数回試行して中央値を取る計測運用のため、
            // 手動でウィンドウを閉じる必要があると使えない。Issue #593 依頼事項 2）。
            let app = fandhe_egui_proxy::cold_start::ColdStartProbe::new(
                fandhe_egui_proxy::ProxyApp::new(cc),
                start,
                true,
            );
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
}

#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")),
    feature = "frame-metrics",
    not(feature = "cold-start-probe")
))]
fn main() -> eframe::Result<()> {
    fandhe_egui_proxy::frame_metrics::run_desktop()
}

#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "android", target_os = "ios")),
    not(feature = "frame-metrics"),
    not(feature = "cold-start-probe")
))]
fn main() -> eframe::Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    // Issue #1044: Windows のタスク スケジューラが専用引数付きで本 exe を
    // 再起動したときの入口。通常の画面起動（`FANDHE_PROXY_APP` の解決等）より
    // 前に判定し、一致したら画面を出さずに実行結果を書いて終了する
    // （`device/windows_background_task.rs` モジュール doc「契約の簡略化」参照）。
    // feature `device` 無効時はこの経路自体が存在しない。
    #[cfg(all(target_os = "windows", feature = "device"))]
    {
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if fandhe_egui_proxy::device::windows_background_task::parse_scheduled_args(&args) {
            std::process::exit(fandhe_egui_proxy::device::windows_background_task::run_scheduled());
        }
    }

    // Issue #1216: カスタム URL scheme（Windows）。URL 起動で既存インスタンスがあれば
    // pipe で転送してここで終える。自分が受ける側なら URL を受け口へ入れて続行する
    // （`device/windows_url_scheme.rs` モジュール doc 参照）。
    #[cfg(all(target_os = "windows", feature = "device"))]
    let url_launch = {
        use fandhe_egui_proxy::device::windows_url_scheme as wus;
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        if let Some(code) = wus::win::startup(&args) {
            std::process::exit(code);
        }
        wus::parse_url_args(&args).is_some()
    };
    #[cfg(not(all(target_os = "windows", feature = "device")))]
    let url_launch = false;

    // Issue #1215: カスタム URL scheme の Apple Event ハンドラはイベントループ開始前
    // （`run_native` より前）に main thread で登録する。起動時の URL を取りこぼさないため
    // （`device/apple_url_scheme.rs`・`macos/fandhe_macos_url_scheme.m` 参照）。
    #[cfg(all(target_os = "macos", feature = "device"))]
    fandhe_egui_proxy::device::apple_url_scheme::register();

    // TASK-2.3（#54）計測マーカー: 以下は描画対象切替ロジック（TASK-2.2-S5 / #52）。
    // 描画対象の名前解決。固定文字列との等値比較のみに使い、パス・コマンド・クエリへは
    // 展開しない（A03 インジェクション対策、security.md 準拠）。
    // fandhe-prototype:begin
    let requested_app = std::env::var("FANDHE_PROXY_APP")
        .ok()
        .filter(|value| !value.is_empty())
        // Issue #1216: レジストリのコマンドは環境変数を渡せないため、URL 起動の既定は型サンプル画面。
        .or_else(|| url_launch.then(|| "url_scheme_show".to_string()));
    // プロトタイプアプリ（TASK-2.2-S5 / #52）への描画対象切替。
    let use_prototype = requested_app.as_deref() == Some("prototype");
    // TASK-17.2（REQ-17）: TV フォーカスナビゲーション検証用アプリへの描画対象切替。
    // 矢印キー/Enter がデスクトップの物理キーボードから同じ経路で駆動できるため、
    // Android エミュレータ以外でもローカル検証できる（`tv_focus.rs` モジュール doc 参照）。
    let use_tv_focus = requested_app.as_deref() == Some("tv-focus");

    // 自動撮影は prototype/tv-focus 専用アプリでは配線しない（`ProxyAppDyn` を実装せず
    // 撮影経路を持たないため）。それ以外（sample・レジストリ経由の名前指定画面）は
    // `FANDHE_PROXY_AUTOSHOT` を共通の `ProxyAppDyn::set_autoshot` 経由で有効化する
    // （Issue #715: 生成画面の PNG エビデンス取得を機械的に行いたい要望への対応）。
    let autoshot_path = if use_prototype || use_tv_focus {
        None
    } else {
        std::env::var("FANDHE_PROXY_AUTOSHOT").ok()
    };
    // fandhe-prototype:end
    let is_autoshot = autoshot_path.is_some();
    // `ProxyApp::logic` は戻り値でプロセス終了コードへ介入できないため、
    // PNG 保存失敗をこの共有フラグ経由で受け取り、`run_native` 完了後に判定する
    // （Bugbot 指摘: 保存失敗時も成功終了していた不整合の是正）。
    let save_failed = Arc::new(AtomicBool::new(false));
    let save_failed_for_closure = Arc::clone(&save_failed);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 360.0]),
        ..Default::default()
    };

    eframe::run_native(
        "fandhe-egui-proxy",
        native_options,
        Box::new(move |cc| {
            // TASK-2.3（#54）計測マーカー: プロトタイプアプリ分岐
            // （if use_prototype { ... }、TASK-2.2-S5 / #52）。
            // fandhe-prototype:begin
            let app: Box<dyn eframe::App> = if use_prototype {
                Box::new(fandhe_egui_proxy::PrototypeApp::new(cc))
            } else if use_tv_focus {
                Box::new(fandhe_egui_proxy::TvFocusApp::new(cc))
            } else {
                // Issue #715: レジストリ（`screens::build_app`）経由の画面名選択と、
                // 既定の sample 画面を `ProxyAppDyn` の同一経路へ統一し、
                // `FANDHE_PROXY_AUTOSHOT`（`ProxyAppDyn::set_autoshot`）をどちらにも
                // 配線できるようにする。未登録名は sample へ倒し、経緯を stderr へ残す。
                let mut app = requested_app
                    .as_deref()
                    .and_then(|name| fandhe_egui_proxy::build_app(name, cc))
                    .unwrap_or_else(|| {
                        if let Some(name) = requested_app.as_deref() {
                            eprintln!(
                                "[fandhe-egui-proxy] 未登録の画面名 '{name}'、sample を表示する。"
                            );
                        }
                        Box::new(fandhe_egui_proxy::ProxyApp::new(cc))
                    });
                if let Some(path) = autoshot_path {
                    app.set_autoshot(std::path::PathBuf::from(path), save_failed_for_closure);
                }
                app
            };
            // fandhe-prototype:end
            Ok(app)
        }),
    )?;

    if is_autoshot && save_failed.load(Ordering::SeqCst) {
        eprintln!("[fandhe-egui-proxy] 自動撮影の PNG 保存に失敗したため、非ゼロで終了する。");
        std::process::exit(1);
    }

    Ok(())
}

#[cfg(any(target_arch = "wasm32", target_os = "android"))]
fn main() {
    // wasm32 の実エントリは web.rs（wasm_bindgen(start)）、
    // Android の実エントリは android.rs（android_main）。
}

#[cfg(target_os = "ios")]
fn main() -> eframe::Result<()> {
    // Issue #27: UIScene 化シム（`ios/fandhe_ios_scene.m`）をリンクに残す。本番・計測の全経路をここで覆う。
    fandhe_egui_proxy::ios_scene::install();

    // Issue #1045: `BGTaskScheduler` の launch handler は起動完了前（main thread・
    // `UIApplicationMain` 呼び出し前）に登録する契約（Apple ドキュメント要件、
    // 起動後の登録はできない）。`ios::run()`（内部で `UIApplicationMain` を呼ぶ）
    // より前にここで 1 回だけ呼ぶ（本番経路・計測 2 feature 経路の 3 つを
    // この 1 箇所で覆う、実装計画 §3.5）。feature `device`（既定 on）無効時は
    // `fandhe-device` への依存自体が無いため呼ばない。
    #[cfg(feature = "device")]
    fandhe_egui_proxy::device::ios_background_task::register();

    // Issue #390: バックグラウンドダウンロードの登録口（現状は no-op のスタブ。b-ios #391 が
    // `handleEventsForBackgroundURLSession` の受け口を埋める）。起動完了前に呼ぶ契約に揃える。
    #[cfg(feature = "device")]
    fandhe_egui_proxy::device::ios_background_download::register();

    // Issue #1217: カスタム URL scheme の observer も `UIApplicationMain` より前に登録する。
    // `UIApplicationDidFinishLaunchingNotification` に間に合わないと、コールドスタートの
    // URL と delegate の差し込みを取り逃がす（`ios/fandhe_ios_url_scheme.m` 参照）。
    #[cfg(feature = "device")]
    fandhe_egui_proxy::device::apple_url_scheme::register();

    // iOS の実エントリは ios.rs（wgpu/Metal バックエンド初期化、TASK-1.3-S2）。
    fandhe_egui_proxy::ios::run()
}
