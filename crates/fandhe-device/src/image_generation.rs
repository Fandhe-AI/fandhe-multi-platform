//! 端末内での画像生成の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/image-generation.json`、親 #191、Issue #975）。
//!
//! 扱う範囲は「OS の画像生成画面（Image Playground）を出し、利用者が確定した 1 枚を表示する」ことまで。
//! 画面なしの生成（`ImageCreator`）・複数枚・元画像の指定・人物の取り込み・保存・共有は扱わない。
//! 生成の元にする文は固定の [`PROMPT`] で、外部入力を受けない。対象は macOS（#975）と iOS（#976）。
//! Android・Windows・Web は対応表で `no` のため OS 別モジュールを置かず `Unsupported`。
//! `fandhe-egui-proxy` の effect ドライバ（`device/image_generation.rs`）が本モジュールの trait を呼び、
//! OS 層が実装する。
//!
//! # 契約
//!
//! - **結果の三値**（契約表 10）: 「生成した」（[`ImageGenerationOutcome::Generated`]）/
//!   「キャンセル」（[`ImageGenerationOutcome::Cancelled`]。利用者が閉じた。失敗にしない）/
//!   失敗（[`ImageGenerationFailure`] の理由別、非対応を含む）。OS の画面の中で起きた失敗は
//!   利用者が閉じれば `Cancelled` になり、失敗として区別しない（守らない範囲）
//! - **失敗の理由と OS 層の戻りコード**（[`outcome_from_apple_code`] が写像の SSOT。iOS 欄は #976 への指示）:
//!
//! | コード | 状況 | macOS | iOS | 返す値 |
//! | --- | --- | --- | --- | --- |
//! | 0 | 生成した | `didCreateImageAt` | 同左 | `Ok(Generated)` |
//! | 1 | 利用者が閉じた | `DidCancel` | 同左 | `Ok(Cancelled)` |
//! | 2 | 非対応 | `isAvailable == false`、OS が古い、SDK に Image Playground が無い | 同左 | `Unsupported` |
//! | 3 | 表示先がない | 親窓が無い | 提示元の VC が無い | `NoWindow` |
//! | 4 | 復号不可 | 寸法 0・復号失敗 | 同左 | `Undecodable` |
//! | 5 | 大きすぎる | 一辺が [`MAX_SIDE`] 超（復号の前に判定） | 同左 | `TooLarge` |
//! | 6 | 前面でない | — | アプリが前面でない | `NotForeground` |
//! | 7・未知 | その他 | — | — | `Internal` |
//!
//!   表示前の期限切れは `TimedOut`（Rust の期限）。
//! - **期限**（契約表 3）: 生成画面が出るまでの [`PRESENT_DEADLINE`]。起点は要求を観測した時の
//!   `Instant`（単調時計）。画面が出始めた後は期限を持たない（生成は利用者の操作を待つ）。
//!   期限切れと表示開始は同じロックで排他にし、期限切れを確定した後は OS に画面を出させない。
//!   値は仮で、Mac 目視（#966）で見直す
//! - **同時 1 件と世代**（契約表 2）: 処理中の再押下は無視する。古い要求の完了は要求 id と
//!   画面の世代の両方で捨てる
//! - **成功値の条件**（契約表 6）: `Ok(Generated)` と `Ok(Cancelled)` は表示開始（bridge の
//!   `try_begin_present`）を記録した要求にだけ認める。記録のない要求の成功値は `Internal`。
//!   成功として見せてよいのは [`GeneratedImage::from_os_rgba`] の検査を通った画素だけで、
//!   縮小して成功にはしない
//! - **OS 層の義務**（契約表 1・5・9）:
//!   - 画面の提示は main queue で行い、提示直前に `try_begin_present` を呼ぶ（`false` なら出さない）
//!   - `done` 通知は 1 要求につき 1 回だけ。画面を閉じてから通知する
//!   - `didCreateImageAt` の一時ファイルは成功・失敗・寸法超過の全経路で消す
//!   - 寸法は復号の前に読んで検査する（0 は 4、[`MAX_SIDE`] 超は 5）
//!   - VC とデリゲートは通知の後に解放する
//!   - iOS は受け口で結果を共有状態へ入れた直後に `crate::ios::request_repaint` を呼ぶ
//! - **秘密値**（契約表 8）: プロンプト・一時ファイルのパス・画素・OS のエラー文を
//!   ログ・画面のエラー文・`Debug` に出さない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と結果の順序、嘘の成功の禁止（非対応・復号不可・寸法超過・
//! 表示開始なしの成功を成功にしない）、一時ファイルの削除と画面の解放を全経路で行うこと。
//! 守らない範囲: 生成内容、OS の画面内での失敗理由、`isAvailable` と実際に生成できるかの食い違い、
//! 生成が端末内で行われるか（公式頁で未確認）、画面の表示中に Rust 側から閉じること、
//! 複数プロセス、プロセスが落ちた後の結果。

use std::fmt;
use std::time::Duration;

/// 要求を観測してから生成画面を出すまでの期限（Rust のドライバだけが持つ）。
/// 仮の値で、Mac 目視（#966）で見直す。
pub const PRESENT_DEADLINE: Duration = Duration::from_secs(10);

/// 受け取る画像の一辺の上限（画素）。OS 層は復号の前にこの値で寸法を検査する。
/// `fandhe-core` の `RGBA_MAX_SIDE` 以下に保つ（`fandhe-egui-proxy` に const assert を置く）。
pub const MAX_SIDE: u32 = 2048;

/// 生成の元にする固定文（英語。外部入力を受けない）。
pub const PROMPT: &str = "a lighthouse by the sea at sunset";

/// 生成された 1 枚（RGBA8・行優先・パディングなし）。[`GeneratedImage::from_os_rgba`] を通ったものだけが
/// 作れる。`Debug` は寸法と長さだけを出し、画素は出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl GeneratedImage {
    /// OS から届いた画素を検査する。寸法 0 と `rgba.len() != w*h*4`（オーバーフロー含む）は
    /// `Undecodable`、一辺が [`MAX_SIDE`] 超は `TooLarge`。
    pub fn from_os_rgba(
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    ) -> Result<Self, ImageGenerationFailure> {
        if width == 0 || height == 0 {
            return Err(ImageGenerationFailure::Undecodable);
        }
        if width > MAX_SIDE || height > MAX_SIDE {
            return Err(ImageGenerationFailure::TooLarge);
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4));
        if expected != Some(rgba.len()) {
            return Err(ImageGenerationFailure::Undecodable);
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// 幅（画素）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 高さ（画素）。
    pub fn height(&self) -> u32 {
        self.height
    }

    /// RGBA8 画素列（長さは常に `width * height * 4`）。
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// 画素列を取り出す。
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
    }
}

impl fmt::Debug for GeneratedImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeneratedImage")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("len", &self.rgba.len())
            .finish()
    }
}

/// 成功側の結果（失敗ではない 2 値）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGenerationOutcome {
    /// 生成した。
    Generated(GeneratedImage),
    /// 利用者が生成画面を閉じた（失敗にしない）。
    Cancelled,
}

/// 生成できなかった理由（画像・OS のエラー文は持たない）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageGenerationFailure {
    /// 非対応 OS・端末（Apple Intelligence が使えない場合を含む）。
    Unsupported,
    /// 生成画面の表示先（親窓など）がない。
    NoWindow,
    /// 画像が 0 寸法・復号できない・画素の長さが合わない。
    Undecodable,
    /// 画像が上限を超えた。
    TooLarge,
    /// 前面でない。
    NotForeground,
    /// 生成画面を出すまでの期限切れ。
    TimedOut,
    /// 未知のコードなど。
    Internal,
}

/// OS 層の完了（戻りコードと画素）を結果へ写す純関数（モジュール doc の表が SSOT）。
/// コード 0 で画素が無い・検査に落ちたときは失敗にする（成功にしない）。
pub fn outcome_from_apple_code(
    code: i32,
    rgba: Option<Vec<u8>>,
    width: u32,
    height: u32,
) -> Result<ImageGenerationOutcome, ImageGenerationFailure> {
    match code {
        0 => {
            let rgba = rgba.ok_or(ImageGenerationFailure::Undecodable)?;
            GeneratedImage::from_os_rgba(rgba, width, height).map(ImageGenerationOutcome::Generated)
        }
        1 => Ok(ImageGenerationOutcome::Cancelled),
        2 => Err(ImageGenerationFailure::Unsupported),
        3 => Err(ImageGenerationFailure::NoWindow),
        4 => Err(ImageGenerationFailure::Undecodable),
        5 => Err(ImageGenerationFailure::TooLarge),
        6 => Err(ImageGenerationFailure::NotForeground),
        _ => Err(ImageGenerationFailure::Internal),
    }
}

/// 画像生成の OS 実装。`generate` は「受理」だけを同期で返し、結果は OS 層が
/// `fandhe-egui-proxy` の bridge（`complete_from_os`）へ 1 回だけ非同期に届ける。
pub trait ImageGenerator: Send + Sync {
    /// `request_id` の要求として生成画面の表示を OS へ依頼する。
    /// 表示の直前に bridge の `try_begin_present` を呼び、`false` なら表示しない。
    fn generate(&self, request_id: u64) -> Result<(), ImageGenerationFailure>;
}

/// 非対応 OS 向け。常に `Unsupported`（成功を返さない）。
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedImageGenerator;

impl ImageGenerator for UnsupportedImageGenerator {
    fn generate(&self, _request_id: u64) -> Result<(), ImageGenerationFailure> {
        Err(ImageGenerationFailure::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_never_succeeds() {
        assert_eq!(
            UnsupportedImageGenerator.generate(1),
            Err(ImageGenerationFailure::Unsupported)
        );
    }

    #[test]
    fn zero_dimension_and_length_mismatch_are_undecodable() {
        for (len, w, h) in [(0, 0, 1), (0, 1, 0), (7, 1, 2), (9, 1, 2)] {
            assert_eq!(
                GeneratedImage::from_os_rgba(vec![0; len], w, h),
                Err(ImageGenerationFailure::Undecodable),
                "{len} {w} {h}"
            );
        }
    }

    #[test]
    fn over_max_side_is_too_large_and_overflow_is_safe() {
        assert_eq!(
            GeneratedImage::from_os_rgba(Vec::new(), MAX_SIDE + 1, 1),
            Err(ImageGenerationFailure::TooLarge)
        );
        assert_eq!(
            GeneratedImage::from_os_rgba(Vec::new(), 1, MAX_SIDE + 1),
            Err(ImageGenerationFailure::TooLarge)
        );
        assert_eq!(
            GeneratedImage::from_os_rgba(vec![0; 4], u32::MAX, u32::MAX),
            Err(ImageGenerationFailure::TooLarge)
        );
    }

    #[test]
    fn valid_pixels_pass() {
        let img = GeneratedImage::from_os_rgba(vec![1; 8], 2, 1).unwrap();
        assert_eq!((img.width(), img.height(), img.rgba().len()), (2, 1, 8));
    }

    #[test]
    fn debug_hides_pixels() {
        let img = GeneratedImage::from_os_rgba(vec![177; 8], 2, 1).unwrap();
        let d = format!(
            "{img:?} {:?}",
            ImageGenerationOutcome::Generated(img.clone())
        );
        assert!(!d.contains("177"), "{d}");
    }

    #[test]
    fn code_mapping() {
        assert_eq!(
            outcome_from_apple_code(1, None, 0, 0),
            Ok(ImageGenerationOutcome::Cancelled)
        );
        for (code, f) in [
            (2, ImageGenerationFailure::Unsupported),
            (3, ImageGenerationFailure::NoWindow),
            (4, ImageGenerationFailure::Undecodable),
            (5, ImageGenerationFailure::TooLarge),
            (6, ImageGenerationFailure::NotForeground),
            (7, ImageGenerationFailure::Internal),
            (99, ImageGenerationFailure::Internal),
            (-1, ImageGenerationFailure::Internal),
        ] {
            assert_eq!(outcome_from_apple_code(code, None, 0, 0), Err(f), "{code}");
        }
    }

    #[test]
    fn success_code_requires_valid_pixels() {
        assert!(outcome_from_apple_code(0, Some(vec![0; 4]), 1, 1).is_ok());
        assert_eq!(
            outcome_from_apple_code(0, None, 1, 1),
            Err(ImageGenerationFailure::Undecodable)
        );
        assert_eq!(
            outcome_from_apple_code(0, Some(vec![0; 3]), 1, 1),
            Err(ImageGenerationFailure::Undecodable)
        );
        assert_eq!(
            outcome_from_apple_code(0, Some(vec![]), MAX_SIDE + 1, 1),
            Err(ImageGenerationFailure::TooLarge)
        );
    }
}
