//! macOS 実機検証ハーネス（TASK-8.5-S10 / REQ-8 / M3、Issue #191）。
//!
//! 親 issue #181（TASK-8.5「IME 統合実装」）配下。S9（#190）がソースレベルで確定させた
//! macOS 実装の契約を実機（macOS + 日本語 IME）で確認するための最小 winit イベントループ。
//!
//! 実機での検証手順は人間担当。本ハーネス自体は Linux 上では `cargo check --target
//! aarch64-apple-darwin --example macos_ime_probe`（macOS SDK 不要、リンクは行わない）まで
//! しか検証できない（実行は macOS 実機が必要）。
//!
//! # セキュリティ考慮（`.claude/rules/security.md` 準拠）
//!
//! 受信した [`fandhe_core::ImeEvent`] は **イベント種別・文字数・カーソル範囲のみ**を
//! 標準出力へ記録し、入力テキスト本文・確定済みバッファ内容は一切出力しない
//! （`ImeEvent` の `Debug` 実装が既に文字数のみへマスクしているため、そのまま
//! `{:?}` へ委譲するだけで契約を満たす）。手順書には検証入力に個人情報・実パスワードを
//! 使わないことも明記する（指定のダミー文字列のみ）。
//!
//! 内容一致の検証は行わない（codex-review P0 指摘対応・Cursor Medium 指摘対応:
//! `Preedit` は変換前のローマ字・かな表記、`Commit` は変換後の確定表記であり、
//! 検証者が「これから入力する文字列」と単純比較すると変換が正しく行われた場合でも
//! 不一致になる。加えて本ハーネスはフィールドを持たないため受信内容の発生元も
//! 証明できない。本ハーネスは文字描画を持たないため（画面上にテキストが見えない）、
//! 確認はログの `Commit` イベント受信・`text_len`（確定文字数）によるイベント受信確認に
//! 限定し、実際の反映（表示）は確認しない）。
#![forbid(unsafe_code)]

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

/// ハーネス本体。ウィンドウは 1 枚のみ保持し、受信した `Ime::*` を変換・記録する。
#[derive(Default)]
struct ImeProbe {
    window: Option<Window>,
}

impl ApplicationHandler for ImeProbe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes().with_title("fandhe macOS IME probe"))
            .expect("ウィンドウ生成に失敗した（実機検証ハーネスのため unwrap を許容、coding-rust.md「例外的な初期化不変条件」）");

        // IME を有効化し、固定のカーソル矩形を通知する（実際のテキストフィールド位置への
        // 追随は本ハーネスのスコープ外。候補ウィンドウが画面内の一定位置に表示されることの
        // 確認が目的、S9 §3 表 #8）。
        window.set_ime_allowed(true);
        window.set_ime_cursor_area(
            PhysicalPosition::new(40.0, 80.0),
            PhysicalSize::new(120u32, 24u32),
        );

        self.window = Some(window);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Ime(ime) => {
                // winit → コア境界型への変換のみを行い（`fandhe-winit-ime` の役割どおり）、
                // ログ出力はコア側の Debug マスク（文字数のみ）にそのまま委ねる。
                let core_event = fandhe_winit_ime::ime_event_from_winit(ime);
                // P1'（Commit イベント受信の確認）は本ハーネスが文字描画を持たないため、
                // ログの `Commit` / `text_len`（確定文字数、`Debug` マスク由来）による
                // イベント受信確認に限定する。実際の反映（表示）は確認しない
                // （モジュール doc 参照）。
                println!("[ime] {core_event:?}");
            }
            _ => {}
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("イベントループ生成に失敗した");
    let mut app = ImeProbe::default();
    event_loop
        .run_app(&mut app)
        .expect("イベントループ実行に失敗した");
}
