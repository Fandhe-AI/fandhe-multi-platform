//! アプリの起動口。eframe の `run_native`・`WebRunner` とバックエンドの選択をここに閉じる。
//!
//! 利用者のコード（`fandhe-egui-proxy` の各 OS の入口を含む）は [`EguiApp`] を実装した値を
//! `build` クロージャで返すだけで、eframe の型（`eframe::App`・`CreationContext`・
//! `NativeOptions`・`Frame`・`eframe::Result`）には触れない。`impl eframe::App` は本モジュールの
//! 非公開アダプタ 1 つだけに書く（受け入れ条件: 公開 API に eframe の型を出さない、#1164）。
//!
//! バックエンドは OS ごとに固定する（`docs/architecture/responsibility-boundary.md`）。
//! デスクトップは eframe 既定（glow）、iOS は Metal、Android は Vulkan、Web は WebGL2。

/// 起動から終了までの 1 アプリ。eframe の `App` から `Frame` 引数を除いた最小の形で、
/// 全実装が `Frame` を読まずに下へ渡していたため、これで足りる。
pub trait EguiApp {
    /// 1 フレーム分の更新と描画。
    fn update(&mut self, ctx: &egui::Context);

    /// ウィンドウを閉じる経路で呼ばれる終了処理。モバイルでは OS がプロセスを止めるため届かない。
    fn on_exit(&mut self) {}

    /// 描画先全体を塗る背景色（sRGB ガンマ空間の 0..=1。`Color32::to_normalized_gamma_f32` の値）。
    /// 既定は eframe の既定と同じ（ほぼ黒、わずかに透過）。iOS は safe area 外（ノッチ周辺）を
    /// パネル背景色に揃えるために上書きする（Issue #800）。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::from_rgba_unmultiplied(12, 12, 12, 180).to_normalized_gamma_f32()
    }
}

/// 起動に失敗したことを表す。原因は eframe 側のエラーを文字列にして持つ（eframe の型を出さない）。
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct LaunchError(String);

impl LaunchError {
    fn from_debug(err: impl std::fmt::Debug) -> Self {
        Self(format!("{err:?}"))
    }
}

/// `build` クロージャへ渡す、起動直後のコンテキスト。
///
/// egui の `Context` と、（Windows のみ）ウィンドウハンドルを取り出す口を出す。
/// 中身の `eframe::CreationContext` は公開しない。
pub struct LaunchContext<'a> {
    cc: &'a eframe::CreationContext<'a>,
}

impl<'a> LaunchContext<'a> {
    fn new(cc: &'a eframe::CreationContext<'a>) -> Self {
        Self { cc }
    }

    /// 実描画に使う egui の `Context`。
    pub fn egui_ctx(&self) -> &egui::Context {
        &self.cc.egui_ctx
    }
}

/// Windows の共有シート・生体認証が HWND を要求するため、ウィンドウハンドルを出す。
#[cfg(target_os = "windows")]
impl raw_window_handle::HasWindowHandle for LaunchContext<'_> {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.cc.window_handle()
    }
}

/// [`EguiApp`] を eframe の `App` へ載せる唯一のアダプタ。
struct Adapter(Box<dyn EguiApp>);

impl eframe::App for Adapter {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.update(ctx);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        self.0.clear_color(visuals)
    }

    // `on_exit` のシグネチャは eframe 自身の "glow" feature の有無で変わる。本クレートが eframe へ
    // "glow" を渡すのは native デスクトップと wasm32（`not(any(ios, android))`）なので、
    // 同じ集合で分岐する（Cargo.toml の target 分岐と対）。
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.0.on_exit();
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    fn on_exit(&mut self) {
        self.0.on_exit();
    }
}

/// `build` を eframe の `AppCreator` へ包む。
fn creator<F>(build: F) -> eframe::AppCreator<'static>
where
    F: FnOnce(&LaunchContext<'_>) -> Box<dyn EguiApp> + 'static,
{
    Box::new(move |cc| {
        let app = build(&LaunchContext::new(cc));
        Ok(Box::new(Adapter(app)) as Box<dyn eframe::App>)
    })
}

/// デスクトップ（macOS / Windows / Linux）で起動する。バックエンドは eframe 既定（glow）。
/// `inner_size` が `Some` ならウィンドウの内側の大きさ（論理 px）にする。
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn run_desktop<F>(
    title: &str,
    inner_size: Option<[f32; 2]>,
    build: F,
) -> Result<(), LaunchError>
where
    F: FnOnce(&LaunchContext<'_>) -> Box<dyn EguiApp> + 'static,
{
    let mut viewport = egui::ViewportBuilder::default();
    if let Some(size) = inner_size {
        viewport = viewport.with_inner_size(size);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(title, native_options, creator(build)).map_err(LaunchError::from_debug)
}

/// iOS で起動する。glow（glutin）は iOS 非対応のため wgpu の Metal に固定する。
/// Vulkan / GL へ `wgpu` が誤ってフォールバックしないよう、許可するバックエンドを Metal だけにする。
/// iOS はフルスクリーン前提のため、ウィンドウの大きさは指定しない。
#[cfg(target_os = "ios")]
pub fn run_ios<F>(title: &str, build: F) -> Result<(), LaunchError>
where
    F: FnOnce(&LaunchContext<'_>) -> Box<dyn EguiApp> + 'static,
{
    let wgpu_setup = eframe::egui_wgpu::WgpuSetupCreateNew {
        instance_descriptor: eframe::wgpu::InstanceDescriptor {
            backends: eframe::wgpu::Backends::METAL,
            ..Default::default()
        },
        ..Default::default()
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default(),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(wgpu_setup),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(title, native_options, creator(build)).map_err(LaunchError::from_debug)
}

/// Android で使うバックエンド。REQ-1 は「Android = Vulkan」を要求する。
#[cfg(target_os = "android")]
fn android_wgpu_backends() -> eframe::wgpu::Backends {
    eframe::wgpu::Backends::VULKAN
}

/// Android（GameActivity）で起動する。`app` は `android_main` が受け取った `AndroidApp`。
/// バックエンドは Vulkan に固定する。
#[cfg(target_os = "android")]
pub fn run_android<F>(
    app: winit::platform::android::activity::AndroidApp,
    title: &str,
    build: F,
) -> Result<(), LaunchError>
where
    F: FnOnce(&LaunchContext<'_>) -> Box<dyn EguiApp> + 'static,
{
    let native_options = eframe::NativeOptions {
        android_app: Some(app),
        renderer: eframe::Renderer::Wgpu,
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            wgpu_setup: eframe::egui_wgpu::WgpuSetup::CreateNew(
                eframe::egui_wgpu::WgpuSetupCreateNew {
                    instance_descriptor: eframe::wgpu::InstanceDescriptor {
                        backends: android_wgpu_backends(),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(title, native_options, creator(build)).map_err(LaunchError::from_debug)
}

/// Web で `canvas` へアタッチして起動する。WebGL2 に固定する（TASK-4.3 / REQ-4 / 条件 4）。
///
/// `WebOptions::default()` の既定 `BestFirst` は WebGL2 の生成に失敗すると WebGL1 へ暗黙に
/// フォールバックする。PoC-5（`docs/spec/03-poc/web-wasm/README.md` 検証方法 4）で
/// WebGPU → WebGL2 の自動フォールバックが `panic!` で失敗することを確認済みで、条件 4 は
/// 自動フォールバックを実装しないことを求めるため、WebGL1 への暗黙フォールバックも無効にする。
/// 方針と再評価条件は `docs/architecture/web-rendering-policy.md`。
#[cfg(target_arch = "wasm32")]
pub async fn start_web<F>(canvas: web_sys::HtmlCanvasElement, build: F) -> Result<(), LaunchError>
where
    F: FnOnce(&LaunchContext<'_>) -> Box<dyn EguiApp> + 'static,
{
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions {
                webgl_context_option: eframe::WebGlContextOption::WebGl2,
                ..Default::default()
            },
            creator(build),
        )
        .await
        .map_err(LaunchError::from_debug)
}

#[cfg(all(test, target_os = "android"))]
mod tests {
    use super::*;

    /// REQ-1: Android は Vulkan に固定する。
    #[test]
    fn android_backend_is_vulkan() {
        assert_eq!(android_wgpu_backends(), eframe::wgpu::Backends::VULKAN);
    }
}
