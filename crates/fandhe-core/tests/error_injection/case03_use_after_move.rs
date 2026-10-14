// ケース03（PoC-6 case03 対応）: use-after-move（所有権違反）。
// `Screen<S>` のビルダーメソッド（`.text()` 等）は `self` を値で受け取り新しい
// `Screen<S>` を返すため、元の変数は呼び出し時にムーブされる。ムーブ後に元の変数を
// 使用するとコンパイル段階で拒否されることを確認する（E0382 想定）。
use fandhe_core::Screen;

struct FormState {
    memo: String,
}

fn main() {
    let screen: Screen<FormState> = Screen::new("フォーム");
    let screen_with_text = screen.text("見出し"); // `screen` はここでムーブされる
    // `screen` はすでにムーブ済みのため使用できない
    println!("{}", screen.title());
    let _ = screen_with_text;
}
