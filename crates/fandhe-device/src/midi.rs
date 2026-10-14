//! Phase 11 の拡充ループ（親 #1137、Issue #1398）: 接続中の MIDI 入力から届いたノートオン / オフを
//! 受ける契約。`fandhe-egui-proxy` の `device/midi.rs`（effect ドライバ）が [`MidiInput::start`] /
//! [`MidiInput::stop`] を呼び、各 OS 実装（macOS: CoreMIDI = #1398、iOS: #1399、Android: #1400、
//! Windows: #1402）が本トレイトを実装する。Web は `fandhe-web-dom` 側（#1401）の Web MIDI を使い、
//! 本トレイトは経由しない。
//!
//! 範囲は「ノートオン / オフを新しい順に一覧へ出す」ことのみ。出力、SysEx、CC などの他メッセージ、
//! MIDI 2.0 の高分解能値、デバイス名、タイムスタンプ、BLE ペアリング UI は扱わない。
//!
//! # 結果の種類（契約表 6・10）
//!
//! - 成功: `start` が `Ok`、続いて [`MidiEvent::Started`]。接続中の入力が 0 台でも成功（正常）
//! - 非対応: `Err(DeviceError::Unsupported("midi"))`。成功値へ倒さない
//! - 失敗: `Err(DeviceError::Internal("midi start"))`、または非同期の [`MidiEvent::Failed`]
//! - 利用者の停止は失敗ではない（[`MidiInput::stop`]）。`PermissionDenied` は Web 専用で本トレイトを通らない
//!
//! # 守る範囲 / 守らない範囲（契約表 11）
//!
//! 守る: 1 プロセス内の順序、失敗を成功値へ畳まないこと、停止後に通知を届けないこと、保留の上限
//! （超過は捨てて数える。ドライバ側）、登録の解放。
//! 守らない範囲: 複数プロセス・タブ間の一致、同一ミリ秒内の順序、停止との競合で 1 件だけ遅れて届く
//! 通知（ドライバが世代で捨てる）、背景遷移中の受信継続、OS 側の抜き差し確定との時間差による
//! 件数表示の一時的な食い違い。
//!
//! 並行性の集約先は OS ごとに 1 本（macOS は `.m` の 1 ロックと受信スレッドから sink への 1 経路）。
//! 受信内容（ノート・ベロシティ・バイト列）はログに出さない（security.md A09）。
//!
//! 本ファイルの定数は仮置き値で、実機の確認（#1397）で見直しうる。

use std::sync::Arc;
use std::time::Duration;

use crate::DeviceError;

/// 画面に残す直近の件数。
pub const MAX_SHOWN_EVENTS: usize = 100;
/// ドライバが画面へ未適用のまま溜めてよい通知（OS のコールバック 1 回 = 1 件）の上限。
pub const MAX_PENDING_BATCHES: usize = 256;
/// `start` から [`MidiEvent::Started`] / [`MidiEvent::Failed`] までの期限。
pub const START_DEADLINE: Duration = Duration::from_secs(5);

/// ノートオン / オフ 1 件。値域は [`NoteEvent::from_wire`] が保証する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEvent {
    /// true = ノートオン。
    pub on: bool,
    /// チャンネル（1..=16）。
    pub channel: u8,
    /// ノート番号（0..=127）。
    pub note: u8,
    /// ベロシティ（0..=127）。オフは届いた値のまま。
    pub velocity: u8,
}

impl NoteEvent {
    /// MIDI 1.0 の生の値からノートイベントを作る唯一の入口（全 OS 実装が共用）。
    /// `0x9` は on（ベロシティ 0 は off）、`0x8` は off。他の種別と範囲外の値は丸めず `None`。
    /// チャンネルの `+1` はここだけで行う。
    pub fn from_wire(status_nibble: u8, channel0: u8, note: u8, velocity: u8) -> Option<Self> {
        if channel0 > 15 || note > 127 || velocity > 127 {
            return None;
        }
        let on = match status_nibble {
            0x9 => velocity != 0,
            0x8 => false,
            _ => return None,
        };
        Some(Self {
            on,
            channel: channel0 + 1,
            note,
            velocity,
        })
    }
}

/// 実装から画面側（ドライバ）へ流す通知。
#[derive(Debug)]
pub enum MidiEvent {
    /// 開始が確定した。`sources == 0` も成功。
    Started {
        /// 接続中の入力数。
        sources: usize,
        /// 接続できなかった入力数（受信は残りで続ける部分成功。0 台 + 失敗は成功にしない）。
        failed: usize,
    },
    /// 抜き差しで接続中の入力数が変わった。
    Sources {
        /// 接続中の入力数。
        sources: usize,
        /// 接続できていない入力数。
        failed: usize,
    },
    /// OS のコールバック 1 回ぶんのノート。
    Notes(Vec<NoteEvent>),
    /// 非同期の開始失敗（Windows が使う）。
    Failed(DeviceError),
}

/// 通知の受け口。`false` は「受け手がいないので購読を止めてよい」の意味。
pub type MidiSink = Arc<dyn Fn(MidiEvent) -> bool + Send + Sync>;

/// MIDI 入力の購読。
pub trait MidiInput: Send + Sync {
    /// 購読を始める。即時に確定する失敗は戻り値で返す。同期で確定できる実装は `start` の中で
    /// `sink(Started{..})` を呼んでよい。非同期の実装は後から `Started` か `Failed` を流す
    /// （期限 [`START_DEADLINE`] はドライバが持つ）。
    fn start(&self, sink: MidiSink) -> Result<(), DeviceError>;
    /// 購読を止める。冪等。登録を確実に外し、以後 sink を呼ばない。
    fn stop(&self);
}

/// 非対応 OS 向けの実装。常に `Unsupported` を返し、成功値へは倒さない。
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsupportedMidiInput;

impl MidiInput for UnsupportedMidiInput {
    fn start(&self, _sink: MidiSink) -> Result<(), DeviceError> {
        Err(DeviceError::Unsupported("midi"))
    }
    fn stop(&self) {}
}

/// バイト列（MIDI 1.0）の組み立て器。入力ソースごとに 1 つ持つ（Android・Web が使う）。
#[derive(Debug, Default)]
pub struct MidiParser {
    /// running status（0x80..=0xEF）。
    status: Option<u8>,
    data: [u8; 2],
    have: usize,
    in_sysex: bool,
    /// システムコモンの後続データバイトを捨てている。
    skipping: bool,
}

impl MidiParser {
    /// 1 バイト与える。ノートオン / オフが完成したときだけ `Some`。
    pub fn feed(&mut self, byte: u8) -> Option<NoteEvent> {
        if byte >= 0xF8 {
            return None; // リアルタイムは状態に触れない
        }
        if byte >= 0x80 {
            self.have = 0;
            self.skipping = false;
            self.in_sysex = byte == 0xF0;
            self.status = match byte {
                0x80..=0xEF => Some(byte),
                _ => None,
            };
            self.skipping = matches!(byte, 0xF1..=0xF6);
            return None;
        }
        if self.in_sysex || self.skipping {
            return None;
        }
        let status = self.status?;
        self.data[self.have] = byte;
        self.have += 1;
        let need = if matches!(status >> 4, 0xC | 0xD) {
            1
        } else {
            2
        };
        if self.have < need {
            return None;
        }
        self.have = 0;
        NoteEvent::from_wire(status >> 4, status & 0xF, self.data[0], self.data[1])
    }
}

/// UMP の 1 メッセージのワード数（MT = 先頭ワード上位 4 bit）。M2-104-UM の表に従う。
fn ump_len(mt: u32) -> usize {
    match mt {
        0x0..=0x2 | 0x6 | 0x7 => 1,
        0x3 | 0x4 | 0x8..=0xA => 2,
        0xB | 0xC => 3,
        _ => 4,
    }
}

/// UMP ワード列から MIDI 1.0 チャンネルボイス（MT 2）のノートオン / オフを取り出す。
/// 他の MT は長さぶん読み飛ばす（MIDI 2.0 の MT 4 も (a) では読み飛ばす）。末尾が途中切れなら打ち切る。
pub fn notes_from_ump_words(words: &[u32]) -> Vec<NoteEvent> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let mt = w >> 28;
        let len = ump_len(mt);
        if i + len > words.len() {
            break;
        }
        if mt == 0x2 {
            out.extend(NoteEvent::from_wire(
                ((w >> 20) & 0xF) as u8,
                ((w >> 16) & 0xF) as u8,
                ((w >> 8) & 0xFF) as u8,
                (w & 0xFF) as u8,
            ));
        }
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_wire_validates_before_converting() {
        assert_eq!(NoteEvent::from_wire(0x9, 16, 60, 1), None);
        assert_eq!(NoteEvent::from_wire(0x9, 0, 128, 1), None);
        assert_eq!(NoteEvent::from_wire(0x9, 0, 60, 128), None);
        assert_eq!(NoteEvent::from_wire(0xB, 0, 60, 1), None);
        let e = NoteEvent::from_wire(0x9, 15, 127, 127).unwrap();
        assert_eq!((e.on, e.channel, e.note, e.velocity), (true, 16, 127, 127));
    }

    #[test]
    fn note_on_velocity_zero_is_off_and_off_keeps_velocity() {
        assert!(!NoteEvent::from_wire(0x9, 0, 60, 0).unwrap().on);
        let off = NoteEvent::from_wire(0x8, 0, 60, 40).unwrap();
        assert!(!off.on);
        assert_eq!(off.velocity, 40);
    }

    fn feed_all(p: &mut MidiParser, bytes: &[u8]) -> Vec<NoteEvent> {
        bytes.iter().filter_map(|b| p.feed(*b)).collect()
    }

    #[test]
    fn parser_running_status_and_realtime() {
        let mut p = MidiParser::default();
        let got = feed_all(&mut p, &[0x90, 60, 100, 0xF8, 62, 0xFE, 90]);
        assert_eq!(got.len(), 2);
        assert_eq!((got[1].note, got[1].velocity), (62, 90));
    }

    #[test]
    fn parser_skips_sysex_and_clears_running_status() {
        let mut p = MidiParser::default();
        assert_eq!(
            feed_all(&mut p, &[0x90, 60, 100, 0xF0, 1, 2, 3, 0xF7, 60, 100]).len(),
            1
        );
    }

    #[test]
    fn parser_system_common_clears_running_status() {
        let mut p = MidiParser::default();
        assert!(feed_all(&mut p, &[0x90, 0xF2, 1, 2, 60, 100]).is_empty());
    }

    #[test]
    fn parser_drops_incomplete_and_other_messages() {
        let mut p = MidiParser::default();
        // データ欠落のまま次の状態バイト、CC、プログラムチェンジ
        let got = feed_all(&mut p, &[0x90, 60, 0x80, 61, 0, 0xB0, 7, 100, 0xC0, 5]);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].on, got[0].note), (false, 61));
        // 状態バイト無しのデータバイトは無視
        assert!(feed_all(&mut MidiParser::default(), &[60, 100]).is_empty());
    }

    #[test]
    fn ump_note_on_off() {
        let on = 0x2090_3C64u32; // ch1 note60 vel100
        let off = 0x2081_3C00u32; // ch2 note60 off
        let got = notes_from_ump_words(&[on, off]);
        assert_eq!(got.len(), 2);
        assert!(got[0].on && got[0].channel == 1);
        assert!(!got[1].on && got[1].channel == 2);
    }

    #[test]
    fn ump_skips_unknown_by_length_and_truncated_tail() {
        let on = 0x2090_3C64u32;
        // MT4 (2 words), MT5 (4 words), MT B (3 words) を読み飛ばす
        let mut w = vec![
            0x4090_3C00,
            0x2090_3C64,
            0x5000_0000,
            0,
            0,
            0,
            0xB000_0000,
            0,
            0,
            on,
        ];
        assert_eq!(notes_from_ump_words(&w).len(), 1);
        // 途中切れ
        w.push(0x5000_0000);
        w.push(0);
        assert_eq!(notes_from_ump_words(&w).len(), 1);
        assert!(notes_from_ump_words(&[]).is_empty());
    }

    #[test]
    fn ump_word_counts_per_mt() {
        let expect = [1, 1, 1, 2, 2, 4, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4];
        for (mt, n) in expect.iter().enumerate() {
            assert_eq!(ump_len(mt as u32), *n, "MT {mt:#x}");
        }
    }

    #[test]
    fn unsupported_never_succeeds() {
        let sink: MidiSink = Arc::new(|_| true);
        assert!(matches!(
            UnsupportedMidiInput.start(sink),
            Err(DeviceError::Unsupported("midi"))
        ));
    }
}
