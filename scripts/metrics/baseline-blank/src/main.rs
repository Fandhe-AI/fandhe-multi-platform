//! TASK-6.3（REQ-6 / Issue #93）メモリ比較計測のベースラインアプリ。
//!
//! `docs/spec/03-poc/performance/README.md`（PoC-4）が macOS で使った
//! 「空の `NSWindow` のみを表示する最小 Cocoa アプリ」（`blank.swift`）の Linux 対応物。
//! `fandhe-egui-proxy-desktop`（glow バックエンド、egui/eframe 一式）の
//! アイドル時 RSS からウィンドウ生成そのもののコストを差し引くための参照点として、
//! `scripts/metrics/measure-idle-memory.sh` から起動される。
//!
//! **計測上の注意**:
//! 本アプリは winit のみでウィンドウを生成し、GL/Vulkan コンテキストは一切初期化しない。
//! 一方 `fandhe-egui-proxy-desktop` は glow（OpenGL、`LIBGL_ALWAYS_SOFTWARE=1` で llvmpipe
//! ソフトウェアラスタライザ経由）で描画コンテキストを持つため、本ベースラインとの RSS 差分
//! （増分）には「egui/eframe 固有のメモリ」に加えて「GL コンテキスト・llvmpipe 自体の
//! メモリ」が混在する。このため増分は参考指標とし、各リビジョンの生 RSS を主指標として扱う
//! （前後比較・削減率の算出には生 RSS を用いる）。
use std::process::ExitCode;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// アイドル時メモリ計測の対象そのもの（空ウィンドウ 1 枚を保持するだけの状態）。
#[derive(Default)]
struct BlankApp {
    window: Option<Window>,
    /// ウィンドウ生成に失敗したかどうか（`main` が `run_app` 復帰後に
    /// 終了コードへ反映するために保持する。`event_loop.exit()` はイベント
    /// ループを終えるだけで、失敗理由を呼び出し元へ伝播しないため）。
    window_creation_failed: bool,
}

impl ApplicationHandler for BlankApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // measure-idle-memory.sh が RSS サンプリング（3 秒後・10 秒後）を行う対象として、
        // ウィンドウ生成直後からプロセスが生存し続ける必要がある。ただし `Poll` は
        // イベント待機せず CPU をフル使用してビジーループしてしまい、「idle RSS」の
        // ベースラインが実際には idle でなくなる（autoshot オフ時にイベント待機する
        // fandhe-egui-proxy-desktop との非対称性も生じる、Bugbot 指摘: Busy-loop baseline
        // not idle）。`Wait` はイベントが来るまでスレッドをブロックしつつプロセス自体は
        // 生存させ続けるため、真に idle な状態でベースラインを維持できる。
        event_loop.set_control_flow(ControlFlow::Wait);
        if self.window.is_none() {
            let attrs = Window::default_attributes().with_title("baseline-blank");
            match event_loop.create_window(attrs) {
                Ok(window) => self.window = Some(window),
                Err(err) => {
                    // ウィンドウ生成不能はベースライン自体が無効な計測環境であることを示す。
                    // フラグに残したうえでイベントループを終了させる（panic はステータス
                    // コードが呼び出し元に伝わりにくいため使わない）。`main` 側で
                    // `run_app` 復帰後にこのフラグを見て `ExitCode::FAILURE` を返す。
                    eprintln!("baseline-blank: ウィンドウ生成に失敗しました: {err}");
                    self.window_creation_failed = true;
                    event_loop.exit();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if event == WindowEvent::CloseRequested {
            event_loop.exit();
        }
    }
}

fn main() -> ExitCode {
    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            eprintln!("baseline-blank: イベントループの初期化に失敗しました: {err}");
            return ExitCode::FAILURE;
        }
    };

    let mut app = BlankApp::default();
    // measure-idle-memory.sh が SIGTERM でプロセスを終了させる運用のため、
    // run_app からの復帰（ウィンドウ操作起因の正常終了）は通常発生しない想定。
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("baseline-blank: イベントループの実行に失敗しました: {err}");
        return ExitCode::FAILURE;
    }
    if app.window_creation_failed {
        // resumed() でウィンドウ生成に失敗した場合、run_app 自体は正常復帰するため
        // ここでフラグを確認し、呼び出し元（measure-idle-memory.sh）に失敗を伝播する。
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
