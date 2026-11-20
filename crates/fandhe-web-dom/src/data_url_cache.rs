//! `ImageWidget` の data URL を `(識別子, 保管所の通し番号)` で覚える、容量つきの表（#1291）。
//!
//! DOM は作り直しのたびに全 `img` の `src` を組み直す。無圧縮 PNG の base64 を毎回作らないよう、同じ通し番号の間は
//! 覚えた URL を返す。覚えた分が膨らまないよう、**URL の合計バイト数**に上限を持たせる（件数ではない）。
//! 足すたびに、保管所に無い識別子の分を先に捨て、まだ上限を超えるなら最後に使われたのが古いものから 1 件ずつ捨てる
//! （全部は捨てない。今足した 1 件は残す）。
//!
//! 呼び出し元は `dom_tree` の `ImageWidget` 変換。保管所そのものは `fandhe_runtime::image_store`。

use std::collections::HashMap;

/// 覚えておく URL の合計バイト数の上限。保管所の予算（64 MiB）の約 1.4 倍（無圧縮 PNG の base64 の膨らみ分）。
pub(crate) const CAP_BYTES: usize = fandhe_runtime::image_store::BUDGET_BYTES / 5 * 7;

struct Entry {
    serial: u64,
    url: String,
    used: u64,
}

pub(crate) struct UrlCache {
    map: HashMap<String, Entry>,
    bytes: usize,
    tick: u64,
    cap: usize,
}

impl UrlCache {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            bytes: 0,
            tick: 0,
            cap,
        }
    }

    /// 同じ通し番号で覚えている URL を返す（最近使った扱いにする）。
    pub(crate) fn get(&mut self, source: &str, serial: u64) -> Option<String> {
        self.tick += 1;
        let used = self.tick;
        let e = self.map.get_mut(source).filter(|e| e.serial == serial)?;
        e.used = used;
        Some(e.url.clone())
    }

    /// 識別子の覚えを捨てる。
    pub(crate) fn remove(&mut self, source: &str) {
        if let Some(e) = self.map.remove(source) {
            self.bytes -= e.url.len();
        }
    }

    /// 覚えを足す。`alive(識別子)` が偽（保管所に無い）の分を先に捨て、上限を超えるなら古いものから 1 件ずつ捨てる。
    pub(crate) fn insert(
        &mut self,
        source: &str,
        serial: u64,
        url: String,
        alive: impl Fn(&str) -> bool,
    ) {
        self.remove(source);
        let gone: Vec<String> = self.map.keys().filter(|k| !alive(k)).cloned().collect();
        gone.iter().for_each(|k| self.remove(k));
        self.tick += 1;
        self.bytes += url.len();
        self.map.insert(
            source.to_string(),
            Entry {
                serial,
                url,
                used: self.tick,
            },
        );
        while self.bytes > self.cap {
            let Some(victim) = self
                .map
                .iter()
                .filter(|(k, _)| k.as_str() != source)
                .min_by_key(|(_, e)| e.used)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            self.remove(&victim);
        }
    }

    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, source: &str) -> bool {
        self.map.contains_key(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(n: usize) -> String {
        "x".repeat(n)
    }

    #[test]
    fn same_serial_hits_and_new_serial_misses() {
        let mut c = UrlCache::new(100);
        c.insert("a", 1, url(10), |_| true);
        assert_eq!(c.get("a", 1).as_deref(), Some("xxxxxxxxxx"));
        assert!(c.get("a", 2).is_none(), "通し番号が違えば使わない");
        c.insert("a", 2, url(5), |_| true);
        assert_eq!(c.bytes(), 5, "置き換えで二重に数えない");
    }

    /// 保管所に無い識別子の分は、足すたびに捨てる。
    #[test]
    fn drops_entries_missing_from_the_store_on_every_insert() {
        let mut c = UrlCache::new(1000);
        c.insert("a", 1, url(10), |_| true);
        c.insert("b", 1, url(10), |_| true);
        c.insert("c", 1, url(10), |k| k != "a");
        assert!(!c.contains("a") && c.contains("b") && c.contains("c"));
        assert_eq!(c.bytes(), 20);
    }

    /// 上限を超えたら、最後に使われたのが古いものから 1 件ずつ捨てる（全部は捨てない）。上限を外すと落ちる。
    #[test]
    fn evicts_oldest_one_by_one_when_over_the_byte_cap() {
        let mut c = UrlCache::new(30);
        for k in ["a", "b", "c"] {
            c.insert(k, 1, url(10), |_| true);
        }
        assert!(c.get("a", 1).is_some()); // a を最近使ったことにする。最古は b
        c.insert("d", 1, url(10), |_| true);
        assert!(!c.contains("b"), "最古の 1 件だけ捨てる");
        assert!(c.contains("a") && c.contains("c") && c.contains("d"));
        assert_eq!(c.bytes(), 30);
        // 大きな 1 件を足すと、必要な分だけ古い順に捨てる。足した 1 件は残る。
        c.insert("e", 1, url(25), |_| true);
        assert!(c.contains("e") && c.bytes() <= 35);
        assert!(!c.contains("c"), "古い順");
    }

    #[test]
    fn cap_is_about_one_point_four_times_the_store_budget() {
        let budget = fandhe_runtime::image_store::BUDGET_BYTES;
        assert!(CAP_BYTES > budget && CAP_BYTES <= budget * 3 / 2);
    }
}
