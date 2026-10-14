// ケース 2: SizeTokenRef と RadiusTokenRef は同じ TypedTokenRef<K> の型エイリアス
// だが K（SizeKind / RadiusKind）が異なるため取り違えると拒否されるべきことを確認する
// （SizeValue / RadiusValue が同一の DimensionValue を共有する実体でも、型パラメータ
// レベルでの取り違え検出は独立して機能する必要がある）。
use fandhe_core::{RadiusTokenRef, SizeTokenRef, TypedTokenRef};

fn requires_radius(_token_ref: RadiusTokenRef) {}

fn main() {
    let size_ref: SizeTokenRef = TypedTokenRef::from_dotted("size.md").expect("正当なパス");
    requires_radius(size_ref);
}
