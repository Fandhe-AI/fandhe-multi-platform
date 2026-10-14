// ケース07（PoC-6 case07 対応）: トレイト境界の不足（`Default` を実装していない
// 状態型に `..Default::default()` を使用）。`fandhe_core::StateStore` の初期状態として
// 構築しようとする状態型が `Default` を実装していない場合、コンパイル段階で
// 拒否されることを確認する（E0277 想定）。
use fandhe_core::StateStore;

struct LoginState {
    email: String,
    password: String,
    login_attempts: u32,
}

fn main() {
    // LoginState は `#[derive(Default)]` を付けていないため `Default::default()` を使えない
    let state = LoginState {
        email: "a@example.com".to_string(),
        ..Default::default()
    };
    let store = StateStore::new(state);
    println!(
        "{} {} {}",
        store.state().email,
        store.state().password,
        store.state().login_attempts
    );
}
