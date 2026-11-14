//! Wi-Fi の電源状態の判定（Windows の wifi-info と wifi-connect が共有する純関数）。
//!
//! 機能の契約（`WifiInfo`・`Ssid`・`normalize_nearby` など）は `fandhe-cap-wifi-info` が持つ。ここに残すのは、
//! 2 機能以上が使う OS に触れない部品だけ（`docs/architecture/consumer-app-structure.md` §8）。機能 crate 同士を
//! 依存させないため、`fandhe-cap-wifi-info` と `fandhe-cap-wifi-connect` はどちらもここを参照する。

/// `dot11_radio_state_on`。
pub const RADIO_ON: i32 = 1;
/// `dot11_radio_state_off`（ソフト・ハードどちらかが明示的にオフ）。0 は unknown で、オフではない。
pub const RADIO_STATE_OFF: i32 = 2;

/// `(ソフト, ハード)` の電源状態で、明示的にオフの PHY が 1 つ以上あり、かつ両方オンの PHY が
/// 無いときだけ true。unknown（0）だけ・空は判定不能として false（偽の失敗を避ける）。
/// Windows の wifi-info と wifi-connect が共有する（複製して手で合わせない）。
pub fn radio_off(states: &[(i32, i32)]) -> bool {
    let on = states.iter().any(|&(s, h)| s == RADIO_ON && h == RADIO_ON);
    let off = states
        .iter()
        .any(|&(s, h)| s == RADIO_STATE_OFF || h == RADIO_STATE_OFF);
    off && !on
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_states() {
        assert!(radio_off(&[(2, 1), (1, 2)]));
        assert!(!radio_off(&[(0, 1), (1, 0)]));
        assert!(!radio_off(&[(0, 0)]));
        assert!(radio_off(&[(2, 1), (0, 0)]));
        assert!(radio_off(&[(1, 2)]));
        assert!(!radio_off(&[(0, 0), (1, 1)]));
        assert!(!radio_off(&[(2, 2), (1, 1)]));
        assert!(!radio_off(&[]));
    }
}
