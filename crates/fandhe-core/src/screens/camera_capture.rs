//! Phase 8（端末機能、親 #752）の型サンプル「カメラ撮影」（Issue #749）。「撮影」ボタン
//! → 端末カメラ起動 → サムネイル表示 + 「撮り直す」、権限拒否・未対応環境では文言表示の
//! みで済ませる 1 画面。`FANDHE_PROXY_APP=camera_capture` で選ばれ、
//! `crates/fandhe-egui-proxy/src/screens/registry.rs` の `effect` 区画から呼び出される
//! （`screens/mod.rs` の `pub mod camera_capture;` と対）。
//!
//! 状態機械: `Idle` → （撮影ボタン）→ `Requested` → （effect ドライバが撮影開始を
//! 検知）→ `Capturing` → （完了）→ `Captured(Thumbnail)` / `Failed(CaptureFailure)`。
//! `Requested → Capturing` の二段階遷移は、`egui-proxy` 側の effect ドライバが関数
//! ポインタ（状態を持てない）で駆動されるため、「起動要求が来た」ことを状態側で
//! 検知できるようにする契約（HTTP 取得画面 `#748` と同型、`docs/ai/SCHEMA.md`
//! パターン 9 参照）。`Capturing` 以外で届いた完了は無視し、二重発火・古い完了による
//! 状態の巻き戻しを防ぐ。
//!
//! `fandhe-core` は端末依存ゼロを維持するため、実カメラアクセス（`fandhe-device` の
//! `Camera` トレイト呼び出し）はここでは行わない。`crates/fandhe-egui-proxy/src/device/
//! camera_capture.rs` が `fandhe_device::camera::Camera` を呼び、結果をこのモジュールの
//! 型（`Thumbnail` / `CaptureFailure`）へ変換して `CompletionSender` 経由で反映する
//! （依存方向はプロキシ → コアの一方向、coding-rust.md 準拠）。
//!
//! 撮影結果のサムネイルは実画素で表示する。`thumbnail_from_rgba` が長辺
//! [`THUMB_MAX_SIDE`] ピクセルに収まるようブロック平均で縮小し、検証済みの
//! [`RgbaPixels`] として状態に保持、`current_screen` が `Screen::rgba_image`
//! （`Node::RgbaImage`）で描画させる（Issue #5。egui-proxy は GPU テクスチャ、
//! web-dom は `<img src="data:image/png;base64,…">` として出す）。状態には生の RGBA
//! （最大 64 MiB、`fandhe-device` の `MAX_IMAGE_BYTES` 参照）を持たず、縮小後の
//! 最大 160x160x4 = 100 KiB に抑える（UI 状態・ヘッドレス出力の肥大を避ける設計判断）。

use crate::ui::{Node, RgbaPixels, Screen, WidgetId};

/// サムネイルの長辺の上限（ピクセル）。
pub const THUMB_MAX_SIDE: u32 = 160;

/// 元画像 `width` x `height` を縦横比を保って長辺 [`THUMB_MAX_SIDE`] 以下へ縮小した寸法。
/// すでに収まる寸法はそのまま返す（冪等。web 側は canvas の寸法決めにも使う）。
/// 0 を含む入力は `(0, 0)`。
pub fn thumbnail_size(width: u32, height: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0);
    }
    let long = width.max(height);
    if long <= THUMB_MAX_SIDE {
        return (width, height);
    }
    let scale = |v: u32| ((v as u64 * THUMB_MAX_SIDE as u64 / long as u64) as u32).max(1);
    (scale(width), scale(height))
}

/// 「撮影」ボタンの `WidgetId`（`Idle` のときのみ表示）。
const CAPTURE_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 「撮り直す」ボタンの `WidgetId`（`Captured`/`Failed` のときのみ表示）。
const RETAKE_BUTTON_ID: WidgetId = WidgetId::from_raw(2);

/// 縮小済みの実画素サムネイル（モジュール冒頭のコメント参照）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thumbnail {
    /// 元画像の幅（ピクセル、表示文言用）。
    pub width: u32,
    /// 元画像の高さ（ピクセル、表示文言用）。
    pub height: u32,
    /// 縮小後の画素（長辺 [`THUMB_MAX_SIDE`] 以下）。
    pub pixels: RgbaPixels,
}

/// RGBA（行優先・パディングなし、`fandhe_device::Image` と同じレイアウト）を
/// [`thumbnail_size`] の寸法へブロック平均（全チャンネル）で縮小する。
///
/// `width`/`height` が 0、または `rgba.len()` が `width * height * 4`
/// （`checked_mul` でオーバーフロー検査済み）と一致しない場合は `None`
/// （panic を契約にしない、coding-rust.md 準拠）。
pub fn thumbnail_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<Thumbnail> {
    if width == 0 || height == 0 {
        return None;
    }
    let w = width as usize;
    let h = height as usize;
    let expected_len = w.checked_mul(h)?.checked_mul(4)?;
    if rgba.len() != expected_len {
        return None;
    }

    let (ow, oh) = thumbnail_size(width, height);
    let (ow, oh) = (ow as usize, oh as usize);
    let mut out = Vec::with_capacity(ow * oh * 4);
    for oy in 0..oh {
        let y0 = oy * h / oh;
        let y1 = ((oy + 1) * h / oh).max(y0 + 1).min(h);
        for ox in 0..ow {
            let x0 = ox * w / ow;
            let x1 = ((ox + 1) * w / ow).max(x0 + 1).min(w);
            let mut sum = [0u64; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let idx = (y * w + x) * 4;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += rgba[idx + c] as u64;
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u64;
            out.extend(sum.iter().map(|s| (s / count) as u8));
        }
    }
    Some(Thumbnail {
        width,
        height,
        pixels: RgbaPixels::new(ow as u32, oh as u32, out)?,
    })
}

/// 撮影失敗の理由。端末非依存（`fandhe_device::DeviceError` からの変換は
/// `crates/fandhe-egui-proxy/src/device/camera_capture.rs` の責務）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureFailure {
    /// カメラの権限が拒否された（`fandhe_device::DeviceError::PermissionDenied`）。
    PermissionDenied,
    /// この環境ではカメラを利用できない（`DeviceError::Unsupported` 等）。
    Unsupported,
    /// 上記以外の失敗。文言は `DeviceError` の `Display` 転送のみ（内部構造を漏らさない、
    /// security.md A09 準拠）。
    Other(String),
}

/// 撮影の進行状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CapturePhase {
    /// 未撮影（初期状態）。
    #[default]
    Idle,
    /// 「撮影」/「撮り直す」ボタンが押され、effect ドライバの起動待ち。
    Requested,
    /// effect ドライバが起動を検知し、端末カメラの応答待ち。
    Capturing,
    /// 撮影成功。サムネイルを保持する。
    Captured(Thumbnail),
    /// 撮影失敗。
    Failed(CaptureFailure),
}

/// カメラ撮影画面が保持する状態。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CameraCaptureState {
    pub phase: CapturePhase,
}

/// 「撮影」「撮り直す」共通のハンドラ。`Requested`/`Capturing` 中の押下は無視する
/// （二重起動防止、`Idle`/`Captured`/`Failed` からのみ遷移可能）。
pub fn start_capture(state: &mut CameraCaptureState) {
    if matches!(
        state.phase,
        CapturePhase::Requested | CapturePhase::Capturing
    ) {
        return;
    }
    state.phase = CapturePhase::Requested;
}

/// effect ドライバが起動を検知した直後に呼ぶ（`Requested → Capturing` のみ許可）。
/// それ以外の状態からの呼び出しは無視する（`Idle` 中の誤呼び出し等を無害化）。
pub fn mark_capturing(state: &mut CameraCaptureState) {
    if matches!(state.phase, CapturePhase::Requested) {
        state.phase = CapturePhase::Capturing;
    }
}

/// ユーザーによる撮影キャンセルを反映する。`Capturing` 以外で届いた場合は無視する
/// （`finish_capture` と同じ順序契約）。`CaptureFailure` を経由させず `Idle` へ戻す
/// （`UIImagePickerController` 等のキャンセルは正常なユーザー操作であり、失敗として
/// 表示すべきではない。呼び出し元は `crates/fandhe-egui-proxy/src/device/
/// camera_capture.rs` の `spawn_capture`、PR #791 レビュー指摘対応）。
pub fn cancel_capture(state: &mut CameraCaptureState) {
    if matches!(state.phase, CapturePhase::Capturing) {
        state.phase = CapturePhase::Idle;
    }
}

/// 撮影完了を反映する。`Capturing` 以外で届いた完了は無視する（古い完了・二重発火が
/// 状態を巻き戻さないための防御、`app::EffectFn` の順序契約参照）。
pub fn finish_capture(state: &mut CameraCaptureState, result: Result<Thumbnail, CaptureFailure>) {
    if !matches!(state.phase, CapturePhase::Capturing) {
        return;
    }
    state.phase = match result {
        Ok(thumbnail) => CapturePhase::Captured(thumbnail),
        Err(failure) => CapturePhase::Failed(failure),
    };
}

/// 状態に応じた案内文言（ダミー文言、フレームワーク・実端末の挙動を示唆しない表現）。
pub fn status_text(state: &CameraCaptureState) -> String {
    match &state.phase {
        // Web（`getUserMedia`）・Android/iOS（`fandhe-device::camera::Camera` の実装）は
        // 実カメラへアクセスするが、macOS/Windows/Linux は引き続き `DummyCamera`
        // （固定の市松模様画像）を使う（`fandhe-egui-proxy/src/device/camera_capture.rs`
        // の `platform_camera` 参照）。`state` はどの実装が動いているかを知らない
        // （共通コアはプラットフォーム固有クレートへ依存しない、coding-rust.md
        // 「クレート構成」節）ため、実カメラ/ダミーの両方に当てはまる表現にする
        // （Codex 指摘 #998）。
        CapturePhase::Idle => "「撮影」を押すと 1 枚撮影します".to_string(),
        CapturePhase::Requested | CapturePhase::Capturing => "撮影中…".to_string(),
        CapturePhase::Captured(thumb) => format!("撮影結果（{}×{}）", thumb.width, thumb.height),
        CapturePhase::Failed(CaptureFailure::PermissionDenied) => {
            "カメラの権限が拒否されています。端末の設定から許可してください".to_string()
        }
        CapturePhase::Failed(CaptureFailure::Unsupported) => {
            "この環境ではカメラを利用できません".to_string()
        }
        CapturePhase::Failed(CaptureFailure::Other(msg)) => format!("撮影に失敗しました（{msg}）"),
    }
}

/// 一覧区画の展開（`Screen::list`）。`Captured` のときのみ実画素ノードを返し、それ以外は空。
/// 描画ごとに評価されるため、Web の `mount_with_effect`（画面ツリーを初回に 1 度だけ構築）
/// でも撮影完了後に画像が現れる。
fn expand_thumbnail(state: &CameraCaptureState) -> Vec<Node<CameraCaptureState>> {
    match &state.phase {
        CapturePhase::Captured(thumb) => vec![Node::RgbaImage {
            pixels: thumb.pixels.clone(),
            alt: "撮影結果のサムネイル".to_string(),
        }],
        _ => Vec::new(),
    }
}

/// カメラ撮影画面。`state.phase` に応じてボタンを出し分ける
/// （`Idle`: 「撮影」、`Captured`/`Failed`: 「撮り直す」、`Requested`/`Capturing`:
/// ボタンなし＝二重起動を UI 側からも防ぐ）。`crates/fandhe-egui-proxy/src/screens/
/// registry.rs` の `effect` 区画から `fn(&S) -> Screen<S>` として毎フレーム呼ばれる契約
/// （`docs/ai/SCHEMA.md` パターン 9）。
pub fn current_screen(state: &CameraCaptureState) -> Screen<CameraCaptureState> {
    let screen = Screen::new("カメラ撮影")
        .dynamic_text(status_text)
        .list(expand_thumbnail);
    match state.phase {
        CapturePhase::Idle => screen.button(CAPTURE_BUTTON_ID, "撮影", start_capture),
        CapturePhase::Captured(_) | CapturePhase::Failed(_) => {
            screen.button(RETAKE_BUTTON_ID, "撮り直す", start_capture)
        }
        CapturePhase::Requested | CapturePhase::Capturing => screen,
    }
}
