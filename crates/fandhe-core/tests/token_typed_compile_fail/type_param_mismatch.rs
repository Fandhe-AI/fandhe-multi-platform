// ケース 1: SpacingTokenRef を ColorTokenRef が要る関数の引数へ渡す。
// 型パラメータの不一致として拒否されるべきことを確認する（REQ-13、設計文書 §5.3）。
use fandhe_core::{ColorTokenRef, SpacingTokenRef, TypedTokenRef};

fn requires_color(_token_ref: ColorTokenRef) {}

fn main() {
    let spacing_ref: SpacingTokenRef =
        TypedTokenRef::from_dotted("spacing.md").expect("正当なパス");
    requires_color(spacing_ref);
}
