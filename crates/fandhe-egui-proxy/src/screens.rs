//! `crates/fandhe-core/src/screens/` へ登録された画面を、端末アプリが実行時に
//! 名前で選んで描画するためのレジストリ（Issue #715）。
//!
//! [`generated_screens!`] マクロで「画面名 → 画面ビルダー関数」を列挙し、[`build_app`] が
//! 名前から [`crate::ProxyApp`]（型消去済み、[`crate::ProxyAppDyn`]）を構築する。
//! 呼び出し元は `main.rs`（デスクトップ相当）・`web.rs`・`ios.rs`・`android.rs` の各エントリで、
//! 未登録名・未指定時は `None` を返し、呼び出し元が既定の `ProxyApp::<SampleState>::new`
//! （sample 画面）へフォールバックする契約にする。
//!
//! 新規画面を追加するときは、`fandhe-core` 側の
//! `crates/fandhe-core/src/screens/mod.rs` の `pub mod` 行と対で、子モジュール
//! [`registry`]（`screens/registry.rs`）のマクロ呼び出しへ 1 行（`"画面名" => クレートパス::関数名,`）
//! を足す。`registry.rs` はマクロ呼び出しのみを持つ小さなファイルで、マクロ定義や
//! 名前解決（本ファイル）とは分けてある（Issue #715）。

/// 画面名 → 画面ビルダー関数（`fn() -> fandhe_core::Screen<S>`、`S: Default`）の対応を
/// 列挙し、[`build_app`] と `registered_names`（テスト用）を生成する。
///
/// マクロの引数形式は `"名前" => パス::関数,` の並びのみを受け付ける
/// （余計な省略形は提供しない。呼び出し例は下の登録一覧を参照）。
///
/// `nav "名前" => パス::関数,`（`関数: fn(&S) -> Screen<S>`、Issue #726）の区画も併記できる。
/// `ScreenStack<S>` のような状態駆動の画面遷移は固定 1 画面を保持できないため、
/// [`crate::app::ProxyApp::with_screen_fn`] 経由で毎フレーム引き直す
/// （`crate::app::ScreenSource::FromState` 参照。両区画で画面名が重複しなければ併記可能）。
///
/// `init "名前" => 画面ビルダー, 状態ファクトリ,`（Issue #747）の区画は状態を
/// `Default::default()` ではなく明示的な状態ファクトリ（`fn() -> S`）から構築したい
/// 画面向け。端末保存領域からの読み込みなど構築時に副作用を伴う初期化を、`fandhe-core`
/// 側の画面コードに `fandhe-device` を参照させずに配線できる（`fandhe-egui-proxy` 側の
/// `device.rs` がファクトリを提供する）。
///
/// `effect "名前" => パス::current_screen , パス::driver ,`（Issue #748/#749）の区画は
/// `nav` の後に併記できる。`current_screen: fn(&S) -> Screen<S>` は `nav` 区画と同じ
/// 状態駆動画面で、`driver: fn(&S, &CompletionSender<S>)`（[`crate::app::EffectFn`]）を
/// `ProxyApp::with_effect` 経由で追加配線する（画面のボタン → 端末 I/O 起動、
/// `crate::app::ProxyApp::update` のフレーム末尾から毎フレーム呼ばれる）。同区画には
/// 複数行を並べてよい（HTTP 取得・カメラ撮影のように独立した状態駆動画面を並記する
/// 用途、Issue #749）。
///
/// マクロパターンは非 `nav` 区画・`nav` 区画・`init` 区画・`effect` 区画のこの固定順
/// のみを受け付ける。後の区画の行を前の区画より前に書くとパターンマッチに失敗し
/// ビルドが壊れるため、新規の非 `nav` 行は既存の `nav`/`init`/`effect` 行より前
/// （無ければブロック末尾）に、新規の `nav` 行は既存の `init`/`effect` 行より前
/// （無ければブロック末尾）に、新規の `init` 行は既存の `effect` 行より前
/// （無ければブロック末尾）に追加する
/// （`docs/ai/SCHEMA.md`「新規画面ファイルの置き場と `mod` 追加の作法」参照）。
///
/// 各行には `#[cfg(...)]` 等の属性を 0 個以上付けられる（Issue #746）。
/// device 画面（#747〜#749）は `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]`
/// を行頭に付けて登録し（`fandhe-device` への依存自体が native 分岐にしか無いため、
/// feature `device` 単独では wasm32 ビルドが壊れる。`Cargo.toml` の
/// `fandhe-device` 依存コメント参照）、条件不成立のビルドではその行自体が存在しない
/// ものとして扱われる（`registered_names`・`build_app` のどちらにも現れない）。
/// 非 `nav` 行は `$(#[$attr])* "名前" => ...`、`nav` 行は
/// `nav $(#[$nav_attr])* "名前" => ...`（属性は `nav` キーワードの後）、`init` 行は
/// `init $(#[$init_attr])* "名前" => 画面ビルダー, 状態ファクトリ,`（属性は `init`
/// キーワードの後）、`effect` 行は `effect $(#[$fx_attr])* "名前" => ...`（属性は
/// `effect` キーワードの後）と、属性の位置が区画で異なる点に注意する（各区画キーワード
/// を見た時点で他区画の候補から外せるようにし、マクロの repetition 境界をあいまいさ
/// なく解決するための制約。`macro_rules!` は同じフラグメント種別の repetition が
/// 連続すると `local ambiguity` エラーになるため）。
macro_rules! generated_screens {
    (
        $( $(#[$attr:meta])* $name:literal => $builder:path , )*
        $( nav $(#[$nav_attr:meta])* $nav_name:literal => $current:path , )*
        $( init $(#[$init_attr:meta])* $init_name:literal => $init_builder:path , $init_state:path , )*
        $( effect $(#[$fx_attr:meta])* $fx_name:literal => $fx_current:path , $fx_driver:path , )*
    ) => {
        /// 登録済み画面名の一覧（テスト・診断用）。全区画（固定 1 画面・状態駆動・
        /// 状態ファクトリ付き・状態駆動 + effect）を含む。
        pub fn registered_names() -> &'static [&'static str] {
            &[
                $( $(#[$attr])* $name , )*
                $( $(#[$nav_attr])* $nav_name , )*
                $( $(#[$init_attr])* $init_name , )*
                $( $(#[$fx_attr])* $fx_name , )*
            ]
        }

        /// `name` に対応する画面から [`ProxyApp`]（型消去済み）を構築する。
        /// 未登録名は `None`（呼び出し元は既定の sample 画面へフォールバックする契約）。
        pub fn build_app(
            name: &str,
            cc: &eframe::CreationContext<'_>,
        ) -> Option<Box<dyn $crate::app::ProxyAppDyn>> {
            match name {
                $(
                    $(#[$attr])*
                    $name => {
                        let screen = $builder();
                        Some(Box::new($crate::app::ProxyApp::with_screen(
                            cc,
                            screen,
                            Default::default(),
                        )) as Box<dyn $crate::app::ProxyAppDyn>)
                    }
                )*
                $(
                    $(#[$nav_attr])*
                    $nav_name => {
                        Some(Box::new($crate::app::ProxyApp::with_screen_fn(
                            cc,
                            $current,
                            Default::default(),
                        )) as Box<dyn $crate::app::ProxyAppDyn>)
                    }
                )*
                $(
                    $(#[$init_attr])*
                    $init_name => {
                        let screen = $init_builder();
                        Some(Box::new($crate::app::ProxyApp::with_screen(
                            cc,
                            screen,
                            $init_state(),
                        )) as Box<dyn $crate::app::ProxyAppDyn>)
                    }
                )*
                $(
                    $(#[$fx_attr])*
                    $fx_name => {
                        Some(Box::new(
                            $crate::app::ProxyApp::with_screen_fn(
                                cc,
                                $fx_current,
                                Default::default(),
                            )
                            .with_effect($fx_driver),
                        ) as Box<dyn $crate::app::ProxyAppDyn>)
                    }
                )*
                _ => None,
            }
        }
    };
}

mod registry;
pub use registry::{build_app, registered_names};

/// 描画対象の画面名を解決する（Issue #715）。
///
/// 実行時 `FANDHE_PROXY_APP` を優先し、未設定（または空文字列）ならビルド時
/// `option_env!` を見る（wasm32 / Android 等、実行時の環境変数注入手段が乏しいターゲット向け。
/// Android はさらに `adb shell setprop` 経由のフォールバック（`debug-proxy-app` feature 有効時のみ）を
/// `android.rs` 側で本関数の後段に持つ）。固定文字列との比較のみに使い、パス・コマンド・クエリへは
/// 展開しない（A03 インジェクション対策、security.md 準拠）。
pub fn selected_app_name() -> Option<String> {
    std::env::var("FANDHE_PROXY_APP")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| option_env!("FANDHE_PROXY_APP").map(str::to_string))
}

#[cfg(test)]
mod tests {
    // `build_app` には `eframe::CreationContext` が要る（テストダブル生成コストが高い）ため、
    // マクロが生成する名前一覧で登録内容を検証する（最小テスト、Issue #715）。
    use super::registered_names;

    #[test]
    fn schema_example_is_registered() {
        assert!(registered_names().contains(&"schema_example"));
    }

    /// `nav` 区画（Issue #726）の登録名も `registered_names` に現れることを検証する。
    #[test]
    fn nav_registered_screen_is_registered() {
        assert!(registered_names().contains(&"list_detail"));
    }

    #[test]
    fn settings_is_registered() {
        assert!(registered_names().contains(&"settings"));
    }

    /// 通常区画（Issue #1037）で登録した checkbox-list 画面が現れることを検証する。
    #[test]
    fn checkbox_list_is_registered() {
        assert!(registered_names().contains(&"checkbox-list"));
    }

    #[test]
    fn confirm_dialog_is_registered() {
        assert!(registered_names().contains(&"confirm_dialog"));
    }

    #[test]
    fn unregistered_name_is_absent() {
        assert!(!registered_names().contains(&"nope"));
    }

    #[test]
    fn form_is_registered() {
        assert!(registered_names().contains(&"form"));
    }

    /// `effect` 区画（Issue #749）で登録した camera_capture 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn camera_capture_is_registered() {
        assert!(registered_names().contains(&"camera_capture"));
    }

    /// `effect` 区画（Issue #830）で登録した tilt_ball 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn tilt_ball_is_registered() {
        assert!(registered_names().contains(&"tilt_ball"));
    }

    /// `effect` 区画（Issue #872）で登録した file_pick 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn file_pick_is_registered() {
        assert!(registered_names().contains(&"file_pick"));
    }

    /// `effect` 区画（Issue #888）で登録した location_show 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn location_show_is_registered() {
        assert!(registered_names().contains(&"location_show"));
    }

    /// `effect` 区画（Issue #1057）で登録した geofence_show 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn geofence_show_is_registered() {
        assert!(registered_names().contains(&"geofence_show"));
    }

    /// `effect` 区画（Issue #897）で登録した share_text 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn share_text_is_registered() {
        assert!(registered_names().contains(&"share_text"));
    }

    /// `effect` 区画（Issue #922）で登録した credential_vault 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn credential_vault_is_registered() {
        assert!(registered_names().contains(&"credential_vault"));
    }

    /// `effect` 区画（Issue #954）で登録した mic_record 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn mic_record_is_registered() {
        assert!(registered_names().contains(&"mic_record"));
    }

    /// `effect` 区画（Issue #1335）で登録した screen_capture 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn screen_capture_is_registered() {
        assert!(registered_names().contains(&"screen_capture"));
    }

    /// `effect` 区画（Issue #980）で登録した tsv_import 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn tsv_import_is_registered() {
        assert!(registered_names().contains(&"tsv_import"));
    }

    /// `effect` 区画（Issue #1004）で登録した notification_send 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn notification_send_is_registered() {
        assert!(registered_names().contains(&"notification_send"));
    }

    /// `effect` 区画（Issue #1014）で登録した drag_drop 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn drag_drop_is_registered() {
        assert!(registered_names().contains(&"drag_drop"));
    }

    /// `effect` 区画（Issue #1024）で登録した speech_recognize 画面が現れることを
    /// 検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn speech_recognize_is_registered() {
        assert!(registered_names().contains(&"speech_recognize"));
    }

    /// `effect` 区画（Issue #1060）で登録した ble_scan 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn ble_scan_is_registered() {
        assert!(registered_names().contains(&"ble_scan"));
    }

    /// `effect` 区画（Issue #1039）で登録した system_tray 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn system_tray_is_registered() {
        assert!(registered_names().contains(&"system_tray"));
    }

    /// `effect` 区画（Issue #1043）で登録した background-task 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn background_task_is_registered() {
        assert!(registered_names().contains(&"background-task"));
    }

    /// `effect` 区画（Issue #1215）で登録した url_scheme_show 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn url_scheme_show_is_registered() {
        assert!(registered_names().contains(&"url_scheme_show"));
    }

    /// `effect` 区画（Issue #1065）で登録した home_widget_show 画面が現れることを検証する。
    #[test]
    #[cfg(all(feature = "device", not(target_arch = "wasm32")))]
    fn home_widget_show_is_registered() {
        assert!(registered_names().contains(&"home_widget_show"));
    }

    /// マクロが各行の `#[cfg(...)]` 属性を透過することを検証する（Issue #746）。
    /// device 画面（#747〜#749）は
    /// `#[cfg(all(feature = "device", not(target_arch = "wasm32")))]` を行頭に付けて
    /// 登録する。
    #[allow(dead_code)] // マクロが生成する `build_app` はこのテストでは呼ばない
    mod gated {
        generated_screens! {
            #[cfg(any())]
            "never" => fandhe_core::screens::form::form_screen,
            #[cfg(all())]
            "always" => fandhe_core::screens::form::form_screen,

            init "with_state" => fandhe_core::screens::form::form_screen, fandhe_core::screens::form::FormState::default,
        }

        #[test]
        fn cfg_false_row_is_absent() {
            assert!(!registered_names().contains(&"never"));
        }

        #[test]
        fn cfg_true_row_is_present() {
            assert!(registered_names().contains(&"always"));
        }

        /// `init` 区画の登録名も `registered_names` に現れることを検証する
        /// （Issue #747。`build_app` は `CreationContext` が要るため呼ばない、
        /// 既存方針どおり）。
        #[test]
        fn init_row_is_present() {
            assert!(registered_names().contains(&"with_state"));
        }
    }

    /// `effect` 区画（Issue #748/#749）の `#[cfg(...)]` 属性透過を検証する（`gated` と同型）。
    /// `build_app` はこのテストでは呼ばない（`eframe::CreationContext` を要するため）。
    #[allow(dead_code)]
    mod gated_effect {
        #[derive(Default)]
        struct DummyState;

        fn dummy_current(_state: &DummyState) -> fandhe_core::Screen<DummyState> {
            fandhe_core::Screen::new("dummy")
        }

        fn dummy_driver(_state: &DummyState, _sender: &crate::app::CompletionSender<DummyState>) {}

        generated_screens! {
            effect
            #[cfg(any())]
            "never_effect" => dummy_current,
            dummy_driver,
            effect
            #[cfg(all())]
            "always_effect" => dummy_current,
            dummy_driver,
        }

        #[test]
        fn cfg_false_effect_row_is_absent() {
            assert!(!registered_names().contains(&"never_effect"));
        }

        #[test]
        fn cfg_true_effect_row_is_present() {
            assert!(registered_names().contains(&"always_effect"));
        }
    }
}
