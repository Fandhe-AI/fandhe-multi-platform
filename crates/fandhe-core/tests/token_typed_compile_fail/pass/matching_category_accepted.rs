// 受理されるべきケース: カテゴリが一致する型付き参照はそのまま渡せることを固定化する
// （compile_fail 側の制約が過剰拒否していないことの確認）。
use fandhe_core::{ColorTokenRef, TypedTokenRef};

fn requires_color(_token_ref: ColorTokenRef) {}

fn main() {
    let color_ref: ColorTokenRef = TypedTokenRef::from_dotted("color.primary").expect("正当なパス");
    requires_color(color_ref);
}
