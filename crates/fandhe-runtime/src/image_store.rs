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
//!   のが古い画像から捨てる（LRU）。上限は、描画側のフレームの区切り（[`begin_frame`]）の有無で次のようになる。
//!   - **呼ばれていない**: 今 [`put`] した 1 枚は捨てないので、実際の上限は**「64 MiB + 直前に置いた 1 枚（最大約 16 MiB）」**。
//!     [`put`] の直後は [`contains`] が真。
//!   - **呼ばれている（`ProxyApp`）**:
//!     - 今のフレームで [`get`] された（描かれた）画像は追い出さない。
//!     - 前のフレームだけで描かれた画像（スクロールで外れた行）は、2 段目の追い出し候補になる（1 段目は描かれていない画像）。
//!     - 拒否するのは、今のフレームで描かれた画像の合計に今置く画像を足すと予算を超えるときだけ。**何も追い出さずに今置く画像のほうを捨て**
//!       （[`PutResult::RejectedOverBudget`]。この場合は [`put`] の直後でも [`contains`] が偽）、前のフレームだけの画像は拒否の根拠にしない。
//!     - 上限は**「今のフレームで描かれた画像の合計と予算の大きいほう」**
//!       （描かれた画像の差し替えで今のフレームの合計が予算を超えたときだけ、超えて保持する）。
//! - 「画面外の行から消える」のは `virtual_list` のように見えている行だけを描く一覧の話。`Node::List`（`.list()`）は
//!   毎フレーム**全行**を描く（= 全行を [`get`] する）ため、全行の合計が 64 MiB を超えると、描いている最中の行が
//!   互いを追い出し合う（フレームの区切りがあれば、超えた分は置けず alt のままになる）。`.list()` の行の画像は全行の合計を
//!   64 MiB に収め（縮小済みの小さな画素にする）、収まらないなら `.virtual_list()` を使う。
//! - 描画側が [`begin_frame`] でフレームの区切りを知らせている間は、**今のフレームで描かれた画像は追い出さない**。
//!   フレーム末尾の effect が `put` しても、見えている行が今置いた画像に押し出されて alt との間でちらつくことはない。
//!   追い出す順は 2 段: 描かれていない画像（古い順）、足りなければ前のフレームだけで描かれた画像（スクロールで外れた行。古い順）。
//! - [`put`] の戻り値（[`PutResult`]）で、置けたかを知る。置けなかった識別子を毎フレーム復号し直すと、復号と再描画が止まらなくなる。
//!   失敗として扱い、読み直さない（下の例）。置けないのは、**今のフレームで描かれた画像だけで予算いっぱい**のときに限る
//!   （1 行スクロールした直後のように、前のフレームだけで描かれた行は追い出して置ける）。機能 crate のドライバ（写真選択・
//!   書類スキャン・スキャナー）は、置けなかったとき画像を見せられないので成功にせず失敗として確定する。見えている画像が減っていれば
//!   次の選択・撮影で置ける。モバイルで OS の選択画面を出している間は描画ループが止まり `frame` が進まないため、出す直前に
//!   見えていた画像が「今のフレーム」のまま残る（それらで予算いっぱいでなければ影響しない）。
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
//!   成功の印を立てずに `Guard` が落ちたとき（復号の失敗・panic・`put` が置けなかった）は `failed` に入れ、読み直さない
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
//!                 // 置けたときだけ成功にする。`RejectedOverBudget`（予算に収まらない）は `ok` を立てず、
//!                 // `Guard` が `failed` に入れる（読み直さない）。
//!                 guard.ok = image_store::put(guard.key.clone(), px) == image_store::PutResult::Stored;
//!             }
//!             let stored = guard.ok;
//!             drop(guard);
//!             if stored {
//!                 // 描画を起こす（置いた画像は次のフレームで描かれる）。置けなかったときは起こさない。
//!                 let _ = sender.send(Box::new(|_: &mut State| {}));
//!             }
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

/// [`put`] の結果。**置けなかった**ときを呼び出し側が知るための値で、`#[must_use]`（捨てると警告）。
///
/// 置けない（`RejectedOverBudget`）のは、描画側が [`begin_frame`] でフレームの区切りを知らせていて、**今のフレームで描かれた画像だけで
/// 予算いっぱい**（その合計に今置く画像を足すと予算を超える）のときだけ。前のフレームだけで描かれた画像（スクロールで外れた行）は
/// 拒否の根拠にならず、追い出して置く。置けないとき画像は保管所に入らず、その識別子を描く行は alt のままになる。
/// 呼び出し側は、置けなかった識別子を毎フレーム復号し直さない（見えている画像が変わらない間は同じ結果になる。復号と再描画が止まらなくなる）。
/// 失敗として扱い、再試行は条件が変わってから（見えている行が減った・別の画像を `remove` した等）にする。
#[must_use = "RejectedOverBudget は画像が保管所に入っていないことを表す"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutResult {
    /// 保管所に置いた（以後 [`contains`] は、追い出されるまで真）。
    Stored,
    /// 予算を超えるため置かなかった（[`contains`] は偽のまま）。
    RejectedOverBudget,
}

/// 保管所が保持する CPU 側画素の合計上限（バイト）。1 枚の上限は `RgbaPixels` の一辺上限による約 16 MiB。
pub const BUDGET_BYTES: usize = 64 * 1024 * 1024;

struct Entry {
    /// [`put`] のたびに増える。レンダラが「同じ画素か」を識別子の再利用と区別して判定するために使う。
    serial: u64,
    /// 最後に触れた時刻（保管所内の通し。追い出しの順序に使う）。
    used: u64,
    /// 最後に [`get`] された（= 描かれた）フレーム。`0` は一度も描かれていない。追い出しの優先順位に使う（2 段）:
    /// 描かれていない・2 フレーム以上前に描かれた画像が先、足りなければ前のフレームだけで描かれた画像（古い順）。
    /// **今のフレームで描かれた画像は追い出さず**、拒否の判定（`put` が置けない条件）も今のフレームの合計だけで行う。
    drawn: u64,
    pixels: RgbaPixels,
}

/// ロック内で完結する本体。グローバルの [`STORE`] と、上限を変えた単体テストの両方が使う。
struct Store {
    map: HashMap<String, Entry>,
    serial: u64,
    tick: u64,
    /// [`begin_frame`] の回数。`0` は「フレームの区切りを知らない」（描画側が呼んでいない）で、純粋な LRU になる。
    frame: u64,
    bytes: usize,
    budget: usize,
}

impl Store {
    fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            serial: 0,
            tick: 0,
            frame: 0,
            bytes: 0,
            budget,
        }
    }

    fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// 追い出しの優先順位（小さいほど先に追い出す）。`2` は今のフレームで描かれた画像（追い出さない）、
    /// `1` は前のフレームだけで描かれた画像（スクロールで外れたかもしれない。足りないときだけ追い出す）、
    /// `0` はそれ以外（描かれていない・2 フレーム以上前）。フレームの区切りを知らないとき（`frame == 0`）は常に `0`。
    fn rank(&self, e: &Entry) -> u8 {
        if self.frame == 0 || e.drawn == 0 {
            0
        } else if e.drawn >= self.frame {
            2
        } else if e.drawn + 1 == self.frame {
            1
        } else {
            0
        }
    }

    /// 今のフレームで描かれた画像のうち `except` 以外の合計バイト数。
    fn current_bytes_except(&self, except: &str) -> usize {
        self.map
            .iter()
            .filter(|(k, e)| k.as_str() != except && self.rank(e) == 2)
            .map(|(_, e)| e.pixels.rgba().len())
            .sum()
    }

    fn bump(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// 置く。同じ識別子は置き換え（通し番号が変わる）。置けたかと、追い出し・捨てて消えた識別子を返す。
    ///
    /// フレームの区切りを知っている（`frame > 0`）ときは、今のフレームで描かれた画像を追い出さない。描かれていない新しい 1 枚が、
    /// **今のフレームで描かれた画像の合計**に足すと予算を超えるなら（= 今見えている行だけで予算いっぱい）、**何も追い出さずに**
    /// その 1 枚だけを捨てる（`RejectedOverBudget`）。前のフレームだけで描かれた画像（スクロールで外れた行）は拒否の根拠にせず、
    /// 追い出し候補の 2 段目にする（1 行スクロールした直後の `put` を置けるようにする）。
    /// 知らないとき（`frame == 0`）は純粋な LRU で、今置いた 1 枚は残す。
    fn put(&mut self, source: String, pixels: RgbaPixels) -> (PutResult, Vec<String>) {
        let size = pixels.rgba().len();
        // 置き換えは、描かれた印を引き継ぐ（描かれている画像を差し替えても、そのフレームでは追い出さない）。
        let old_drawn = self.map.get(&source).map_or(0, |e| e.drawn);
        let new_drawn = self.frame > 0 && old_drawn >= self.frame;
        // 新しく置く画像は「前のフレームで描かれた」扱いにする（`frame == 0` は今の挙動のまま）。順位 0（描かれていない）のままだと、
        // ページ送りのように見える行がまとめて入れ替わるとき、置いたばかりの画像どうしが 1 段目で追い出し合ってしまう。
        // 外れた行（前のフレームで描かれた画像）と同じ 2 段目に入れ、古い順（外れた行が先）に追い出す。
        let drawn = if self.frame > 0 {
            old_drawn.max(self.frame - 1)
        } else {
            old_drawn
        };
        if self.frame > 0 && !new_drawn {
            let protected = self.current_bytes_except(&source);
            if protected > 0 && protected + size > self.budget {
                // 今見えている画像を落とさず、新しい 1 枚だけを捨てる。同じ識別子の古い画像（今のフレームでは描かれていない）が
                // 残ると古い画素を見せ続けるので、それも捨てる。
                let mut gone = Vec::new();
                if self.take(&source) {
                    gone.push(source);
                }
                return (PutResult::RejectedOverBudget, gone);
            }
        }
        self.serial += 1;
        let used = self.bump();
        let entry = Entry {
            serial: self.serial,
            used,
            drawn,
            pixels,
        };
        if let Some(old) = self.map.insert(source.clone(), entry) {
            self.bytes -= old.pixels.rgba().len();
        }
        self.bytes += size;
        let mut evicted = Vec::new();
        while self.bytes > self.budget {
            // 追い出す順: 1 段目は描かれていない（2 フレーム以上前を含む）画像で最後に使ったのが古いもの、
            // 2 段目は前のフレームだけで描かれた画像で古いもの。今置いた 1 枚と、今のフレームで描かれた画像は追い出さない。
            let victim = self
                .map
                .iter()
                .filter(|(k, e)| **k != source && self.rank(e) < 2)
                .min_by_key(|(_, e)| (self.rank(e), e.used))
                .map(|(k, _)| k.clone());
            let Some(victim) = victim else {
                // 残りは今のフレームで描かれた画像だけ（描かれた画像の差し替えで合計が予算を超えた）。追い出さずに超えて保持する。
                break;
            };
            self.take(&victim);
            evicted.push(victim);
        }
        (PutResult::Stored, evicted)
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
        let frame = self.frame;
        let e = self.map.get_mut(source)?;
        e.used = used;
        e.drawn = frame;
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
/// 古く使われた画像から捨てる。置けたかを [`PutResult`] で返す（モジュール doc 参照）。置けなかった（`RejectedOverBudget`）ときは
/// 保管所に入らず、同じ識別子の古い画像があれば捨てる。
pub fn put(source: String, pixels: RgbaPixels) -> PutResult {
    let (result, evicted) = with(|s| s.put(source, pixels));
    notify(&evicted);
    result
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

/// 描画のフレームの区切りを知らせる。描画側（`ProxyApp::update` の冒頭）が毎フレーム 1 回呼ぶ。
///
/// 呼ばれている間、**このフレームですでに [`get`] された（描かれた）画像**は、[`put`]（フレーム末尾の effect など）では
/// 追い出されない。見えている行の合計が予算に収まるなら、`put` しても見えている画像は alt との間でちらつかない。
/// 守らない範囲: `begin_frame` の後、行がまだ描かれていない（まだ [`get`] されていない）間に届いた `put`（フレームの途中の復号スレッド）は、
/// その行を前のフレームだけで描かれた画像（2 段目の候補）として扱う。見えている行が一時的に alt になりうる。追い出す順は 2 段: 描かれていない画像（古い順）、足りなければ前のフレームだけで描かれた画像（古い順）。
/// 今のフレームで描かれた画像に今置く 1 枚を足すと予算を超えるときは、何も追い出さずに今置く画像のほうを捨てる
/// （[`PutResult::RejectedOverBudget`]。呼び出し側は失敗として扱い、読み直さない。前のフレームだけの画像は拒否の根拠にならない）。
/// 描かれている画像の差し替えで今のフレームの合計が予算を超えたときだけ、追い出さずに予算を超えて保持する（差し替えに限る。
/// まだ無い行の画像は捨てられ、alt のまま。`.list()` は全行の合計を予算に収める、長い一覧は `.virtual_list()`）。
/// 呼ばれなければ純粋な LRU（`render_screen` を自前のループで呼ぶ利用者など）。プロセスに 1 つの描画ループを前提にする。
pub fn begin_frame() {
    with(|s| s.begin_frame());
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

    /// `Store::put` の追い出し・捨てた識別子だけを返す（結果は `sput_result` で見る）。
    fn sput(s: &mut Store, k: &str, p: RgbaPixels) -> Vec<String> {
        s.put(k.to_string(), p).1
    }

    fn sput_result(s: &mut Store, k: &str, p: RgbaPixels) -> PutResult {
        s.put(k.to_string(), p).0
    }

    fn px(side: u32) -> RgbaPixels {
        RgbaPixels::new(side, side, vec![255u8; (side * side * 4) as usize]).unwrap()
    }

    /// 複数の識別子が同時に引ける（旧版は後から置いた 1 枚だけが残った）。置き換えでだけ通し番号が変わる。
    #[test]
    fn keeps_each_source_and_serial_changes_only_on_replace() {
        let mut s = Store::new(BUDGET_BYTES);
        sput(&mut s, "a", px(2));
        sput(&mut s, "b", px(3));
        let (sa, pa) = s.get("a").unwrap();
        assert_eq!((pa.width(), s.get("b").unwrap().1.width()), (2, 3));
        assert!(s.get("c").is_none(), "識別子が違えば返さない");
        sput(&mut s, "a", px(2));
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
            assert!(sput(&mut s, k, px(4)).is_empty());
        }
        s.get("a"); // a を最近使ったことにする。最古は b
        assert_eq!(sput(&mut s, "d", px(4)), vec!["b".to_string()]);
        assert!(s.get("b").is_none());
        assert!(s.get("a").is_some() && s.get("c").is_some() && s.get("d").is_some());
        assert_eq!(s.bytes, one * 3);
    }

    /// 上限より大きい 1 枚でも、置いた本人は残す（空振りにしない）。
    #[test]
    fn keeps_the_new_entry_even_if_alone_over_budget() {
        let mut s = Store::new(10);
        sput(&mut s, "a", px(2));
        assert_eq!(sput(&mut s, "b", px(2)), vec!["a".to_string()]);
        assert!(s.get("b").is_some());
    }

    /// 見えている画像（描かれた）は、フレーム末尾の `put` が予算を超えても追い出されない。今置く画像のほうが捨てられ、
    /// 戻り値で置けなかったと分かる。旧版（純粋な LRU）は、見えている最古の行を追い出していた。
    #[test]
    fn put_at_frame_end_does_not_evict_drawn_images_and_reports_rejection() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 3);
        s.begin_frame();
        for k in ["a", "b", "c"] {
            sput(&mut s, k, px(4));
        }
        s.begin_frame();
        for k in ["a", "b", "c"] {
            assert!(s.get(k).is_some()); // 見えている 3 行（予算ちょうど）
        }
        assert_eq!(
            sput_result(&mut s, "n", px(4)),
            PutResult::RejectedOverBudget,
            "置けなかったと戻り値で分かる"
        );
        assert!(s.contains("a") && s.contains("b") && s.contains("c") && !s.contains("n"));
        assert_eq!(s.bytes, one * 3);
    }

    /// 描かれていない古い画像があれば、そちらから追い出す。次のフレームでは前のフレームの印が「前のフレームだけ」（2 段目の候補）に下がり、
    /// 2 フレーム描かれなければ 1 段目の候補になる。
    #[test]
    fn stale_images_go_first_and_protection_ends_after_two_frames() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 3);
        s.begin_frame();
        for k in ["a", "b", "c"] {
            sput(&mut s, k, px(4));
        }
        s.begin_frame();
        s.get("a");
        s.get("b"); // c は見えていない
        assert_eq!(sput_result(&mut s, "n", px(4)), PutResult::Stored);
        assert!(s.contains("a") && s.contains("b") && s.contains("n") && !s.contains("c"));
        // 次のフレームは何も描かれない: a・b は前のフレームだけで描かれた（2 段目の候補）ので、1 段目の n から追い出す。
        s.begin_frame();
        assert_eq!(sput_result(&mut s, "m", px(4)), PutResult::Stored);
        assert!(
            s.contains("a") && s.contains("b") && !s.contains("n"),
            "描かれていない n から追い出す"
        );
        // もう 1 フレーム描かれなければ、a・b の保護も外れる。最後に使われたのが古い a から追い出せる。
        s.begin_frame();
        assert_eq!(sput(&mut s, "k", px(4)), vec!["a".to_string()]);

        // 1 行スクロールした直後: 見えていた a・b・c（予算ちょうど）のうち c が外れ、新しい行 d が見え始める。
        // 前のフレームだけで描かれた c は拒否の根拠にならず、d は置け、見えなくなった c が追い出される。
        // （旧版は前のフレームの画像も守るので、d が置けず alt のままになり、failed に入れる書き方だと戻らなかった。）
        let mut s = Store::new(one * 3);
        s.begin_frame();
        for k in ["a", "b", "c"] {
            sput(&mut s, k, px(4));
        }
        s.begin_frame();
        for k in ["a", "b", "c"] {
            s.get(k); // 見えている 3 行
        }
        s.begin_frame(); // スクロール: c が外れ、d が見え始める
        s.get("a");
        s.get("b");
        assert_eq!(
            sput_result(&mut s, "d", px(4)),
            PutResult::Stored,
            "スクロール直後の d は置ける"
        );
        assert!(s.contains("a") && s.contains("b") && s.contains("d") && !s.contains("c"));
    }

    /// 描かれた画像の合計に新しい 1 枚を足すと予算を超えるときは、先に判定して何も追い出さず、新しい 1 枚だけを捨てる
    /// （描かれていない古い画像を、置けない画像のために追い出し損にしない）。
    #[test]
    fn rejection_is_decided_before_evicting_anything() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 3);
        s.begin_frame();
        for k in ["a", "b", "c"] {
            sput(&mut s, k, px(4));
        }
        s.begin_frame();
        s.begin_frame(); // c だけが、描かれていない（2 フレーム前）
        s.get("a");
        s.get("b");
        // a・b（描かれた 2 枚 = one * 2）に 2 枚ぶんの画像（one * 2）を足すと予算（one * 3）を超える: 置けない。c は残る。
        let big = RgbaPixels::new(4, 8, vec![0u8; 4 * 8 * 4]).unwrap();
        assert_eq!(sput_result(&mut s, "n", big), PutResult::RejectedOverBudget);
        assert!(
            s.contains("a") && s.contains("b") && s.contains("c"),
            "追い出し損が無い"
        );
        // 1 枚ぶん（one）なら置け、描かれていない c が追い出される。
        assert_eq!(sput(&mut s, "m", px(4)), vec!["c".to_string()]);
    }

    /// 置けなかった識別子の古い画像（同じ識別子・描かれていない）は残さない。古い画素を見せ続けない。
    #[test]
    fn rejected_put_also_drops_the_stale_image_of_the_same_key() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 2 + 2 * 2 * 4); // a・b と、小さな old が収まる
        s.begin_frame();
        sput(&mut s, "old", px(2));
        sput(&mut s, "a", px(4));
        sput(&mut s, "b", px(4));
        s.begin_frame();
        s.begin_frame(); // old は描かれていない
        s.get("a");
        s.get("b");
        let (r, gone) = s.put("old".to_string(), px(4));
        assert_eq!(
            (r, gone),
            (PutResult::RejectedOverBudget, vec!["old".to_string()])
        );
        assert!(!s.contains("old"));
    }

    /// ページ送り: 見える行が a..d から e..h へまとめて入れ替わっても、置いたばかりの e..h が互いを追い出し合わず、
    /// 外れた a..d が古い順に追い出される。（旧版は新しい画像が順位 0 で、置いたばかりの画像が先に追い出されて b c d h が残った。）
    #[test]
    fn page_turn_keeps_the_new_rows_and_evicts_the_scrolled_out_ones() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 4);
        s.begin_frame();
        for k in ["a", "b", "c", "d"] {
            sput(&mut s, k, px(4));
        }
        s.begin_frame();
        for k in ["a", "b", "c", "d"] {
            s.get(k); // 見えている a..d（予算ちょうど）
        }
        s.begin_frame(); // ページ送り: e..h が見え始める（まだ保管所に無い）
        for k in ["e", "f", "g", "h"] {
            assert_eq!(sput_result(&mut s, k, px(4)), PutResult::Stored, "{k}");
        }
        for k in ["e", "f", "g", "h"] {
            assert!(s.contains(k), "{k} は残る");
        }
        for k in ["a", "b", "c", "d"] {
            assert!(!s.contains(k), "{k} は外れたので追い出される");
        }
    }

    /// 描かれている画像を同じ識別子で置き換えても、そのフレームでは追い出されない（差し替えは置ける）。
    #[test]
    fn replacing_a_drawn_image_keeps_it_protected() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 2);
        s.begin_frame();
        sput(&mut s, "a", px(4));
        sput(&mut s, "b", px(4));
        s.begin_frame();
        s.get("a");
        assert_eq!(sput_result(&mut s, "a", px(4)), PutResult::Stored); // 描かれた a の差し替え
        s.get("b");
        assert_eq!(
            sput_result(&mut s, "n", px(4)),
            PutResult::RejectedOverBudget
        );
        assert!(s.contains("a") && s.contains("b"));
    }

    /// 描かれた画像の差し替えで描かれた合計が予算を超えたときだけ、追い出さずに超えて保持する。
    #[test]
    fn drawn_images_over_budget_are_kept() {
        let mut s = Store::new(4 * 4 * 4 * 2);
        s.begin_frame();
        sput(&mut s, "a", px(4));
        sput(&mut s, "b", px(4));
        s.begin_frame();
        s.get("a");
        s.get("b");
        // 描かれている a を大きな画像へ差し替える（予算を超える）。b も a も描かれているので追い出さない。
        assert_eq!(sput_result(&mut s, "a", px(8)), PutResult::Stored);
        assert!(s.contains("a") && s.contains("b"));
        assert!(s.bytes > s.budget);
    }

    /// `contains` は存在を返すだけで、最近使った扱いにしない（追い出し順を変えない）。
    #[test]
    fn contains_does_not_refresh_recency() {
        let one = 4 * 4 * 4;
        let mut s = Store::new(one * 2);
        sput(&mut s, "a", px(4));
        sput(&mut s, "b", px(4));
        assert!(s.contains("a") && !s.contains("z"));
        // `contains` は used を触らない。a が最古のまま追い出される（`get` なら b が追い出される）。
        assert_eq!(sput(&mut s, "c", px(4)), vec!["a".to_string()]);
    }

    #[test]
    fn remove_and_prefix_release_bytes() {
        let mut s = Store::new(BUDGET_BYTES);
        sput(&mut s, "fandhe.photo:1", px(2));
        sput(&mut s, "fandhe.photo:2", px(2));
        sput(&mut s, "row:1", px(2));
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
        let _ = put("g:a".into(), px(2));
        let _ = put("g:b".into(), px(2));
        assert!(contains("g:b") && !contains("g:none"));
        let _ = put("g:a".into(), px(2)); // 置き換えは通知しない
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
            assert_eq!(put(format!("g:big:{k}"), big(k)), PutResult::Stored);
        }
        assert_eq!(
            *LOG.lock().unwrap(),
            vec!["g:big:0".to_string()],
            "最古の 1 枚だけが追い出され、フックが呼ばれる"
        );
        clear();
    }
}
