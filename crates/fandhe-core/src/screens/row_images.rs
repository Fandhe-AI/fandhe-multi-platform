//! 一覧の行ごとの画像と、時間で変わる表示の確認用画面（Issue #1291）。
//! 各行に別の画像を出し、「コピー」を押すと文言が 2 秒後に戻り、秒カウンタが 1 秒ごとに進む。
//!
//! `FANDHE_PROXY_APP=row-images` で選ばれ、`crates/fandhe-gallery/src/screens/registry.rs` の `effect` 区画から
//! 呼ばれる（`screens/mod.rs` の `pub mod row_images;` と対）。画素の登録と時間の予約は effect（registry.rs の
//! `row_images_effect`）が `fandhe_runtime` の `image_store::contains` / `put` と `CompletionSender::after` で行う。
//! 本モジュールは端末にも `fandhe-runtime` にも依存しない（`docs/ai/SCHEMA.md`「新規画面ファイルの置き場」）。
//!
//! # 行ごとの画像の書き方
//!
//! - 行の画像は `ImageWidget::fixed(行ごとの WidgetId, 行ごとの識別子, alt)` を `list` の `expand` の中で作る。
//!   `expand` は状態を受け取るので、識別子は状態から決めてよい（並べ替え・削除しても行に追従する）
//! - 識別子は effect が `image_store::put` した画素と一致したときだけ画素が出る。まだ（または追い出された後）は alt が文字で出る。
//!   effect は毎フレーム `image_store::contains` で確かめ、無い行だけ置き直す（追い出しにも追従する）
//! - 時間で変わる表示は、状態に「今こうなっている」だけを持たせ、effect が「その状態なら N 秒後に戻す」を予約する

use crate::ui::{ImageWidget, Node};
use crate::{RgbaPixels, Screen, WidgetId};

/// 画像の識別子の接頭辞。行 `i` の識別子は [`image_key`]。
pub const KEY_PREFIX: &str = "row-images:"; // `fandhe.` で始まらない（フレームワーク予約と衝突しない）

/// 「コピーしました」を元へ戻すまでの時間（秒）。
pub const COPIED_RESET_SECS: u64 = 2;

const COPY_BUTTON_ID: WidgetId = WidgetId::from_raw(1);
/// 行 `i` の画像の `WidgetId` は `ROW_ID_BASE + i`。
const ROW_ID_BASE: u64 = 100;

/// 行ごとの画像の確認用状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowImagesState {
    /// 行の題名。
    pub names: Vec<String>,
    /// 「コピーしました」を出している間 `true`。effect が [`COPIED_RESET_SECS`] 後に戻す。
    pub copied: bool,
    /// 1 秒ごとに進むカウンタ（定期更新の例）。
    pub ticks: u32,
}

impl Default for RowImagesState {
    fn default() -> Self {
        RowImagesState {
            names: ["赤", "緑", "青"].iter().map(|s| s.to_string()).collect(),
            copied: false,
            ticks: 0,
        }
    }
}

/// 行 `i` の画像の識別子（`image_store` のキー）。
pub fn image_key(row: usize) -> String {
    format!("{KEY_PREFIX}{row}")
}

/// 行 `i` の確認用の画素（一辺 32、行ごとに色が違う単色）。
pub fn demo_pixels(row: usize) -> RgbaPixels {
    let rgb = match row % 3 {
        0 => [220, 40, 40],
        1 => [40, 180, 60],
        _ => [50, 80, 220],
    };
    let rgba: Vec<u8> = (0..32 * 32)
        .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    RgbaPixels::new(32, 32, rgba).expect("32x32 の RGBA は常に有効")
}

/// 「コピー」ボタン: 文言を「コピーしました」にする（戻すのは effect の予約）。
pub fn copy(state: &mut RowImagesState) {
    state.copied = true;
}

/// effect が予約から呼ぶ: 文言を元へ戻す。
pub fn reset_copied(state: &mut RowImagesState) {
    state.copied = false;
}

/// effect が予約から呼ぶ: カウンタを 1 進める。
pub fn tick(state: &mut RowImagesState) {
    state.ticks += 1;
}

fn rows(state: &RowImagesState) -> Vec<Node<RowImagesState>> {
    state
        .names
        .iter()
        .enumerate()
        .map(|(i, name)| Node::Row {
            children: vec![
                Node::Widget(Box::new(ImageWidget::fixed(
                    WidgetId::from_raw(ROW_ID_BASE + i as u64),
                    image_key(i),
                    format!("{name}の画像"),
                ))),
                Node::Text(name.clone()),
            ],
            tap: None,
        })
        .collect()
}

fn status(state: &RowImagesState) -> String {
    if state.copied {
        "コピーしました".to_string()
    } else {
        "コピーできます".to_string()
    }
}

fn counter(state: &RowImagesState) -> String {
    format!("経過 {} 秒", state.ticks)
}

/// 画面を組み立てる（状態から毎フレーム引き直す `current_screen` 形）。
pub fn current_screen(_state: &RowImagesState) -> Screen<RowImagesState> {
    Screen::new("行ごとの画像")
        .list(rows)
        .button(COPY_BUTTON_ID, "コピー", copy)
        .dynamic_text(status)
        .dynamic_text(counter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_row_has_its_own_key_widget_id_and_color() {
        let state = RowImagesState::default();
        let nodes = rows(&state);
        let mut ids = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            let Node::Row { children, .. } = n else {
                panic!("行");
            };
            let Node::Widget(w) = &children[0] else {
                panic!("画像");
            };
            let img = w
                .as_any()
                .unwrap()
                .downcast_ref::<ImageWidget<RowImagesState>>()
                .unwrap();
            assert_eq!(img.source_text(), image_key(i));
            ids.push(w.id());
        }
        ids.sort_by_key(|i| i.raw());
        ids.dedup();
        assert_eq!(ids.len(), 3, "WidgetId は行ごとに一意");
        assert_ne!(demo_pixels(0).rgba(), demo_pixels(1).rgba());
    }

    #[test]
    fn copy_then_reset_round_trip() {
        let mut s = RowImagesState::default();
        assert_eq!(status(&s), "コピーできます");
        copy(&mut s);
        assert_eq!(status(&s), "コピーしました");
        reset_copied(&mut s);
        assert_eq!(status(&s), "コピーできます");
        tick(&mut s);
        assert_eq!(counter(&s), "経過 1 秒");
    }
}
