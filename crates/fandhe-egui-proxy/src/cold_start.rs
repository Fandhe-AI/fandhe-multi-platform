//! コールドスタート計測用の共通ラッパー（`cold-start-probe` feature、既定 off）。
//!
//! `t0`（呼び出し元の `run()`/`main()` 冒頭）から `t1`（初回フレームで
//! `eframe::App::update` が完了した時点、＝ファーストフレームの UI 構築完了時点）までの
//! 経過時間を `COLD_START_MS=<value>;` の形式で標準エラー出力へ書き込む
//! （TASK-5.1 / REQ-5、Issue #236）。もともと `ios.rs` にのみ実装されていたが、
//! macOS デスクトップ計測（TASK-5.1 / Issue #593）でも同じ計測定義・出力形式が必要になり、
//! `main.rs` の native 分岐からも使えるよう本モジュールへ切り出した（ロジック重複回避）。
//!
//! iOS とデスクトップ native で唯一異なるのは「計測後にプロセスを終了するか」。
//! iOS はシミュレータ/実機側のライフサイクルに委ねるためログ出力のみで継続動作するが、
//! デスクトップ native は計測スクリプトが複数回起動して中央値を取る運用のため、
//! ログ出力後に自動でウィンドウを閉じてプロセスを終了させる必要がある
//! （Issue #593 依頼事項 2）。`close_after_log` で呼び出し側から選べるようにする。

#[cfg(feature = "cold-start-probe")]
pub struct ColdStartProbe<A: eframe::App> {
    inner: A,
    start: std::time::Instant,
    logged: bool,
    /// 計測ログ出力の直後に `ViewportCommand::Close` を送るか。
    /// iOS 側は false（既存挙動を維持）、デスクトップ native 側は true。
    close_after_log: bool,
}

#[cfg(feature = "cold-start-probe")]
impl<A: eframe::App> ColdStartProbe<A> {
    /// `start`（`t0`）と計測対象アプリ `inner` をラップする。
    pub fn new(inner: A, start: std::time::Instant, close_after_log: bool) -> Self {
        Self {
            inner,
            start,
            logged: false,
            close_after_log,
        }
    }
}

#[cfg(feature = "cold-start-probe")]
impl<A: eframe::App> eframe::App for ColdStartProbe<A> {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // 先に委譲することで、`inner.update`（`render_screen` 等によるファーストフレームの
        // UI 構築）の完了後に計測する（t1 定義。委譲前に記録すると過小計上になる）。
        eframe::App::update(&mut self.inner, ctx, frame);
        if !self.logged {
            let elapsed_ms = self.start.elapsed().as_millis();
            // 末尾に `;` を付けて行の終端を明示する。疑似 TTY 経由の記録では pty の
            // 折り返し幅で数値の途中が改行されうるため、抽出側の正規表現は `;` を
            // 必須にして折り返し時は抽出失敗（fail-fast）に倒す契約とする。
            eprintln!("COLD_START_MS={elapsed_ms};");
            self.logged = true;
            if self.close_after_log {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}
