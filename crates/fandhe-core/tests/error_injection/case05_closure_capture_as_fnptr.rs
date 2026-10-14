// ケース05（PoC-6 case05 対応）: 外部変数を `move` キャプチャするクロージャを
// `Screen::button` の `on_click: fn(&mut S)` へ渡す。PoC-6 で実証済みの拒否パターンを
// 踏襲する（E0308 想定）。分類上は既存 `tests/compile_fail.rs`（TASK-13.1-S3、クロージャ
// 環境キャプチャ拒否に特化した 9 バリエーション網羅スイート）と重なるが、本ケースは
// PoC-6 の 12 誤り分類のうち 1 項目として本スイートに含める設計判断であり、重複起票ではない
// （`error_injection.rs` の責務分界コメント参照）。
use fandhe_core::{Screen, WidgetId};

struct Counter {
    count: i32,
}

fn main() {
    let bonus = 3;
    let screen: Screen<Counter> =
        Screen::new("画面").button(WidgetId::from_raw(1), "加算", move |state| {
            state.count += bonus
        });
    let _ = screen;
}
