//! `ImageWidget` の実画素を描画側へ届ける、識別子（キー）ごとの保管所（レンダラ非依存）。
//!
//! `fandhe-core` の `ImageWidget` は不透明なソース識別子しか持たない。端末機能の effect ドライバ
//! （写真選択・書類スキャン等）や利用者の effect が、復号確認済みの画素を識別子付きで [`put`] し、
//! レンダラ実装（`fandhe-render-egui` の `image_store::texture`、`fandhe-web-dom` の `img` 変換）が
//! [`get`] で識別子が一致するときだけ引いて描く。一致しない識別子は描かせない
//! （古い結果が新しい表示へ混ざらない）。
//!
//! 一覧の行ごとに別の画像を出すため、識別子ごとに複数枚を持つ（旧版は 1 枠で、後から置いた画像が
//! 先の画像を消していた。#1291）。行の識別子は利用者が決める（例: `"row:42"`）。
//!
//! # 容量・追い出し・解放
//!
//! - 合計の CPU 側画素は [`BUDGET_BYTES`]（64 MiB）を目安に保つ。超えるときは、最後に [`get`] / [`put`] された
//!   のが古い画像から捨てる（LRU）。ただし今 [`put`] した 1 枚は捨てないので、実際の上限は
//!   **「64 MiB + 直前に置いた 1 枚（最大約 16 MiB）」**。
//! - 「画面外の行から消える」のは `virtual_list` のように見えている行だけを描く一覧の話。`Node::List`（`.list()`）は
//!   毎フレーム**全行**を描く（= 全行を [`get`] する）ため、全行の合計が 64 MiB を超えると、描いている最中の行が
//!   互いを追い出し合う。`.list()` の行の画像は全行の合計を 64 MiB に収め（縮小済みの小さな画素にする）、
//!   収まらないなら `.virtual_list()` を使う。
//! - 追い出された識別子を描く行は、その間 alt を文字で出す。再び [`put`] すれば戻る。置き直す必要があるかは
//!   [`contains`]（順位を上げない読み取り）で確かめる。
//! - 利用者が不要になった画像は [`remove`]・[`remove_prefix`]・[`clear`] で明示的に捨てる。
//!   写真選択など世代付きの識別子（`fandhe.photo:<世代>`）は [`remove_prefix`] で前の世代をまとめて解放する。
//! - 捨てるたびに [`set_on_remove`] のフックが呼ばれ、レンダラが GPU へ上げた分も同時に捨てる。
//!
//! # 識別子の名前空間
//!
//! `fandhe.` で始まる識別子はフレームワーク（機能 crate のドライバ。`fandhe.photo:`・`fandhe.scan:`・`fandhe.scanner:`）が使う。
//! 利用者は `fandhe.` で始まらない識別子（例: `"row:42"`）を使う。
//!
//! # 守らない範囲
//!
//! 写真選択・書類スキャン・スキャナーの画像（`fandhe.photo:`・`fandhe.scan:`・`fandhe.scanner:`）も同じ保管所にあるので、
//! 別の画面が行の画像を多く置くと追い出されうる。ドライバは置き直さないため、追い出された画像は**次に選ぶ・撮るまで alt の文字のまま**
//! になる（データの消失ではなく表示だけの欠け。保管所の予算に収まる範囲で使うことで避ける）。
//!
//! # 書き方（復号を別スレッドで行う effect）
//!
//! effect は毎フレーム呼ばれる。`send` の結果は次のフレームまで状態へ届かないので、状態に持った印で「要求済み」を
//! 判断すると二重に起動する。判断は保管所（[`contains`]）と、スレッドが生きている間の印（`pending`）、失敗した識別子の印
//! （`failed`）で行う。追い出されたときは `contains` が `false` に戻るので、そのまま置き直しに入る。
//!
//! - スレッドは `Guard` を持ち、**panic しても** `Drop` で `pending` から外す（外れないと、その行が永久に alt のままになる）。
//!   成功の印を立てずに `Guard` が落ちたとき（復号の失敗・panic）は `failed` に入れ、読み直さない
//!   （ずっと失敗する画像を毎フレーム復号し直して、描画と CPU を回し続けない）。
//! - 判断は `pending` / `failed` のロックを取ってから `contains` を見る（見てから取ると、完了とすれ違って二重に起動する）。
//!
//! ```
//! use std::collections::HashSet;
//! use std::sync::Mutex;
//! use fandhe_core::RgbaPixels;
//! use fandhe_runtime::{CompletionSender, image_store};
//!
//! struct State { keys: Vec<String> }
//!
//! #[derive(Default)]
//! struct Book { pending: HashSet<String>, failed: HashSet<String> }
//! static BOOK: Mutex<Option<Book>> = Mutex::new(None);
//!
//! fn decode(_key: &str) -> Option<RgbaPixels> { RgbaPixels::new(1, 1, vec![0u8; 4]) }
//!
//! /// スレッドの中で持つ。落ちたとき（panic を含む）に `pending` から外し、成功していなければ `failed` に入れる。
//! struct Guard { key: String, ok: bool }
//! impl Drop for Guard {
//!     fn drop(&mut self) {
//!         let mut g = BOOK.lock().unwrap_or_else(|e| e.into_inner());
//!         let book = g.get_or_insert_with(Book::default);
//!         book.pending.remove(&self.key);
//!         if !self.ok {
//!             book.failed.insert(self.key.clone());
//!         }
//!     }
//! }
//!
//! fn effect(s: &State, sender: &CompletionSender<State>) {
//!     for key in &s.keys {
//!         {
//!             let mut g = BOOK.lock().unwrap_or_else(|e| e.into_inner());
//!             let book = g.get_or_insert_with(Book::default);
//!             if book.pending.contains(key) || book.failed.contains(key) || image_store::contains(key) {
//!                 continue;
//!             }
//!             book.pending.insert(key.clone());
//!         }
//!         let (key, sender) = (key.clone(), sender.clone());
//!         std::thread::spawn(move || {
//!             let mut guard = Guard { key, ok: false };
//!             if let Some(px) = decode(&guard.key) {
//!                 image_store::put(guard.key.clone(), px);
//!                 guard.ok = true;
//!             }
//!             drop(guard);
//!             // 描画を起こす（置いた画像は次のフレームで描かれる）。
//!             let _ = sender.send(Box::new(|_: &mut State| {}));
//!         });
//!     }
//! }
//! # let _ = effect;
//! ```
//!
//! 画素は `fandhe_core::RgbaPixels`（検証済み・`Arc` 保持）で持つ。GPU へ上げる処理は
//! レンダラ側の責務で、本モジュールは egui を参照しない。

use std::collections::HashMap;
use std::sync::Mutex;

use fandhe_core::RgbaPixels;

/// 保管所が保持する CPU 側画素の合計上限（バイト）。1 枚の上限は `RgbaPixels` の一辺上限による約 16 MiB。
pub const BUDGET_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    /// [`put`] のたびに増える。レンダラが「同じ画素か」を識別子の再利用と区別して判定するために使う。
    serial: u64,
    /// 最後に触れた時刻（保管所内の通し。追い出しの順序に使う）。
    used: u64,
    pixels: RgbaPixels,
}

/// ロック内で完結する本体。グローバルの [`STORE`] と、上限を変えた単体テストの両方が使う。
struct Store {
    map: HashMap<String, Entry>,
    serial: u64,
    tick: u64,
    bytes: usize,
    budget: usize,
}

impl Store {
    fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            serial: 0,
            tick: 0,
            bytes: 0,
            budget,
        }
    }

    fn bump(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// 置く。同じ識別子は置き換え（通し番号が変わる）。上限を超えた分の追い出しで消えた識別子を返す。
    fn put(&mut self, source: String, pixels: RgbaPixels) -> Vec<String> {
        self.serial += 1;
        let used = self.bump();
        let size = pixels.rgba().len();
        let entry = Entry {
            serial: self.serial,
            used,
            pixels,
        };
        if let Some(old) = self.map.insert(source.clone(), entry) {
            self.bytes -= old.pixels.rgba().len();
        }
        self.bytes += size;
        let mut evicted = Vec::new();
        while self.bytes > self.budget {
            // 今置いた 1 枚は残す（単独で上限を超える画像は置けた時点で受け入れる。上限は一辺の上限で抑えられている）。
            let Some(victim) = self
                .map
                .iter()
                .filter(|(k, _)| **k != source)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.take(&victim);
            evicted.push(victim);
        }
        evicted
    }

    fn take(&mut self, source: &str) -> bool {
        match self.map.remove(source) {
            Some(e) => {
                self.bytes -= e.pixels.rgba().len();
                true
            }
            None => false,
        }
    }

    fn contains(&self, source: &str) -> bool {
        self.map.contains_key(source)
    }

    fn get(&mut self, source: &str) -> Option<(u64, RgbaPixels)> {
        let used = self.bump();
        let e = self.map.get_mut(source)?;
        e.used = used;
        Some((e.serial, e.pixels.clone()))
    }

    fn remove_prefix(&mut self, prefix: &str) -> Vec<String> {
        let keys: Vec<String> = self
            .map
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect();
        for k in &keys {
            self.take(k);
        }
        keys
    }
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(|| Store::new(BUDGET_BYTES)))
}

/// 捨てられた識別子ごとに呼ばれる、レンダラ側の GPU 資源を捨てるフック（レンダラが 1 つ登録する）。
static ON_REMOVE: Mutex<Option<fn(&str)>> = Mutex::new(None);

/// 画像が捨てられる（[`remove`]・[`remove_prefix`]・[`clear`]・容量超過の追い出し）たびに、その識別子で
/// 呼ぶフックを登録する。レンダラが GPU へ上げた分を、画像ウィジェットが描かれなくなっても確実に解放するために使う。
/// 同じ識別子の置き換え（[`put`]）では呼ばれない（通し番号の変化でレンダラが上げ直す）。
pub fn set_on_remove(hook: fn(&str)) {
    *ON_REMOVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(hook);
}

fn notify(removed: &[String]) {
    // 保管所のロックを手放してから呼ぶ（フックが `get` を呼んでも詰まらない）。
    let hook = *ON_REMOVE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = hook {
        removed.iter().for_each(|s| h(s));
    }
}

/// 識別子つきで画素を置く。同じ識別子は置き換え、別の識別子は残す。合計が [`BUDGET_BYTES`] を超えるときは
/// 古く使われた画像から捨てる（モジュール doc 参照）。
pub fn put(source: String, pixels: RgbaPixels) {
    let evicted = with(|s| s.put(source, pixels));
    notify(&evicted);
}

/// 識別子の画像を 1 枚捨てる。あれば `true`。
pub fn remove(source: &str) -> bool {
    let removed = with(|s| s.take(source));
    if removed {
        notify(&[source.to_string()]);
    }
    removed
}

/// `prefix` で始まる識別子の画像をすべて捨てる。世代付きの識別子（`fandhe.photo:<世代>`）で前の世代を解放する用途。
pub fn remove_prefix(prefix: &str) {
    let removed = with(|s| s.remove_prefix(prefix));
    notify(&removed);
}

/// 保持中の画像をすべて捨てる。登録済みのフックでレンダラ側のテクスチャも捨てさせる。
pub fn clear() {
    remove_prefix("");
}

/// `source` の画像があるか。[`get`] と違い、最近使った扱いにしない（effect が「置き直しが要るか」を毎フレーム
/// 確かめても、描かれていない画像の追い出し順を変えない）。
pub fn contains(source: &str) -> bool {
    with(|s| s.contains(source))
}

/// `source` と識別子が一致するときだけ `(置いた通し番号, 画素)` を返す。返した画像は「最近使った」扱いになる。
pub fn get(source: &str) -> Option<(u64, RgbaPixels)> {
    with(|s| s.get(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(side: u32) -> RgbaPixels {
        RgbaPixels::new(side, side, vec![255u8; (side * side * 4) as usize]).unwrap()
    }

    /// 複数の識別子が同時に引ける（旧版は後から置いた 1 枚だけが残った）。置き換えでだけ通し番号が変わる。
    #[test]
    fn keeps_each_source_and_serial_changes_only_on_replace() {
        let mut s = Store::new(BUDGET_BYTES);
        s.put("a".into(), px(2));
        s.put("b".into(), px(3));
        let (sa, pa) = s.get("a").unwrap();
        assert_eq!((pa.width(), s.get("b").unwrap().1.width()), (2, 3));
        assert!(s.get("c").is_none(), "識別子が違えば返さない");
        s.put("a".into(), px(2));
        assert_ne!(sa, s.get("a").unwrap().0, "置き直せば通し番号が変わる");
        assert_eq!(s.get("b").unwrap().1.width(), 3, "他の識別子は残る");
        assert_eq!(s.bytes, 2 * 2 * 4 + 3 * 3 * 4, "置き換えで二重計上しない");
    }

    /// 上限を超えると、最後に使われたのが古い識別子から捨てる。`get` で触れた画像は残る。
    #[test]
    fn evicts_least_recently_used_over_budget() {
        let one = 4 * 4 * 4; // 4x4 の 1 枚
        let mut s = Store::new(one * 3);
        for k in ["a", "b", "c"] {
            assert!(s.put(k.into(), px(4)).is_empty());
        }
        s.get("a"); // a を最近使ったことにする。最古は b
        assert_eq!(s.put("d".into(), px(4)), vec!["b".to_string()]);
        assert!(s.get("b").is_none());
        assert!(s.get("a").is_some() && s.get("c").is_some() && s.get("d").is_some());
        assert_eq!(s.bytes, one * 3);
    }

    /// 上限より大きい 1 枚でも、置いた本人は残す（空振りにしない）。
    #[test]
    fn keeps_the_new_entry_even_if_alone_over_budget() {
        let mut s = Store::new(10);
        s.put("a".into(), px(2));
        assert_eq!(s.put("b".into(), px(2)), vec!["a".to_string()]);
        assert!(s.get("b").is_some());
    }

    /// `contains` は存在を返すだけで、最近使った扱いにしない（追い出し順を変えない）。
    #[test]
    fn contains_does_not_refresh_recency() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 2);
        s.put("a".into(), px(4));
        s.put("b".into(), px(4));
        assert!(s.contains("a") && !s.contains("z"));
        // `contains` は used を触らない。a が最古のまま追い出される（`get` なら b が追い出される）。
        assert_eq!(s.put("c".into(), px(4)), vec!["a".to_string()]);
    }

    #[test]
    fn remove_and_prefix_release_bytes() {
        let mut s = Store::new(BUDGET_BYTES);
        s.put("fandhe.photo:1".into(), px(2));
        s.put("fandhe.photo:2".into(), px(2));
        s.put("row:1".into(), px(2));
        let mut gone = s.remove_prefix("fandhe.photo:");
        gone.sort();
        assert_eq!(
            gone,
            vec!["fandhe.photo:1".to_string(), "fandhe.photo:2".to_string()]
        );
        assert!(s.get("row:1").is_some(), "接頭辞の違う行は残る");
        assert!(s.take("row:1") && !s.take("row:1"));
        assert_eq!(s.bytes, 0);
    }

    /// グローバルの入口（フック・clear）。static を共有するため 1 テストにまとめて並列実行の干渉を避ける。
    #[test]
    fn global_api_notifies_hook_for_each_removed_source() {
        static LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let removed = || LOG.lock().unwrap().len();
        set_on_remove(|s| LOG.lock().unwrap().push(s.to_string()));
        clear();
        LOG.lock().unwrap().clear();
        put("g:a".into(), px(2));
        put("g:b".into(), px(2));
        assert!(contains("g:b") && !contains("g:none"));
        put("g:a".into(), px(2)); // 置き換えは通知しない
        assert_eq!(removed(), 0);
        assert!(remove("g:a") && !remove("g:a"));
        remove_prefix("g:");
        clear();
        assert_eq!(removed(), 2);
        assert!(get("g:b").is_none());

        // 予算を超える `put` は、追い出した識別子でフックを呼ぶ（レンダラが GPU 側を捨てられる）。
        LOG.lock().unwrap().clear();
        let big = |c: u8| RgbaPixels::new(2048, 2048, vec![c; 2048 * 2048 * 4]).unwrap();
        for k in 0..5 {
            put(format!("g:big:{k}"), big(k));
        }
        assert_eq!(
            *LOG.lock().unwrap(),
            vec!["g:big:0".to_string()],
            "最古の 1 枚だけが追い出され、フックが呼ばれる"
        );
        clear();
    }
}
