//! 動画・音声の変換の抽象（Phase 11 の拡充ループ、対応表
//! `docs/device/capabilities/media-transcode.json`、親 #91、Issue #311）。
//!
//! 扱う範囲は「実行時に作った 5 秒・モノラル・48 kHz・PCM 16bit の音声（[`crate::media_playback::demo_pcm`]）を
//! AAC-LC（m4a）へ 1 回変換する」ことだけ。動画・利用者が持ち込むファイル・進捗・取り消し・
//! 形式の選択・保存は扱わない。対象は macOS / iOS（AVAudioFile）、Windows（MediaTranscoder）、
//! Android（MediaCodec + MediaMuxer）、Web（WebCodecs。コンテナなしの AAC / Opus）。
//! 一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。
//!
//! # 契約
//!
//! - **成功**: 出力を読み直して [`check_m4a`] を満たしたものだけ。0 バイトでない・m4a として
//!   解析できる・符号形式が `mp4a`・長さが入力 ±[`DURATION_TOLERANCE`] の中、をすべて満たす
//!   （`device-bridge-contracts.md`「6. 嘘の成功」）
//! - **失敗の区別**: 非対応 → `Err(Unsupported("media_transcode"))`、期限切れ → `Internal(`[`TIMEOUT`]`)`、
//!   出力を解析できない・符号形式が違う → `Internal(`[`UNREADABLE`]`)`、長さの不一致 →
//!   `Internal(`[`MISMATCH`]`)`、出力が 0 バイト → `Internal(`[`EMPTY`]`)`、入力が空 → `InvalidInput`、
//!   それ以外 → `Internal`
//! - **結果の三値**（契約表 10）: 成功 / 中断 / 失敗。中断は Web の `pagehide`（ページを離れた）のみで、
//!   画面側が失敗とは別の状態として扱う。**取り消しの操作と OS の権限は存在しない**ため、
//!   キャンセル・権限拒否の失敗系は無い
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`TRANSCODE_DEADLINE`]）。起点は
//!   要求を観測した時の単調時計 `Instant`。Android と Windows は OS 側で中断する。Apple は
//!   中断できないため走り切らせ、期限後に届いた結果はドライバが捨てる。一時ファイル・コーデックは
//!   結果を捨てる前に解放する
//! - **並行性**（契約表 2）: 世代は画面側が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ
//!   適用する。処理中の再押下は無視する
//! - **ライフサイクル**（契約表 1）: 変換は短いため背景遷移では中断しない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、一時ファイルとコーデックの解放。
//! 守らない範囲: OS ごとの所要時間とビットレートの違い、Apple で期限の後も書き込みが走り切ること、
//! 複数プロセス・複数タブ。出力のバイト列・OS のエラー文はログへ出さない。

use std::time::Duration;

use crate::DeviceError;

pub use crate::media_playback::CLIP_SECONDS;

/// 要求を観測してから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const TRANSCODE_DEADLINE: Duration = Duration::from_secs(10);
/// 入力 PCM のサンプルレート。
pub const SAMPLE_RATE: u32 = 48_000;
/// 出力の長さと入力の長さの許容差（AAC のプライミング・パディング分）。
pub const DURATION_TOLERANCE: Duration = Duration::from_millis(200);

/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "media_transcode timeout";
/// 出力を m4a として解析できない・符号形式が違うときの `Internal` の識別子（SSOT）。
pub const UNREADABLE: &str = "media_transcode unreadable";
/// 出力の長さが入力と合わないときの `Internal` の識別子（SSOT）。
pub const MISMATCH: &str = "media_transcode mismatch";
/// 出力が 0 バイトのときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "media_transcode empty";

/// 音声変換の契約（モジュール doc の「契約」節）。
pub trait MediaTranscode: Send + Sync {
    /// モノラル PCM 16bit を AAC-LC の m4a へ変換して返す。`remaining` は期限までの残り時間。
    /// Windows は実装の内側で `media_playback::wav_bytes` を使って WAV に包む。
    fn transcode(
        &self,
        pcm: &[i16],
        sample_rate: u32,
        remaining: Duration,
    ) -> Result<Vec<u8>, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedMediaTranscode;

impl MediaTranscode for UnsupportedMediaTranscode {
    fn transcode(&self, _: &[i16], _: u32, _: Duration) -> Result<Vec<u8>, DeviceError> {
        Err(DeviceError::Unsupported("media_transcode"))
    }
}

/// 入力を OS を呼ぶ前に検査する。空の PCM・サンプルレート 0 は `InvalidInput`。
pub fn validate_input(pcm: &[i16], rate: u32) -> Result<(), DeviceError> {
    if pcm.is_empty() || rate == 0 {
        Err(DeviceError::InvalidInput("media_transcode input"))
    } else {
        Ok(())
    }
}

/// PCM の長さ。`rate == 0` は 0。
pub fn input_duration(samples: usize, rate: u32) -> Duration {
    if rate == 0 {
        return Duration::ZERO;
    }
    Duration::from_nanos((samples as u128 * 1_000_000_000 / u128::from(rate)) as u64)
}

/// m4a から読み取った、検証に要る最小の情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct M4aInfo {
    /// `stsd` の先頭エントリの型（AAC は `mp4a`）。
    pub codec: [u8; 4],
    /// `mdhd` の時間軸。
    pub timescale: u32,
    /// `mdhd` の長さ（`timescale` 単位）。
    pub duration: u64,
    /// `stsz` のサンプル数。
    pub sample_count: u32,
}

impl M4aInfo {
    /// 再生時間。
    pub fn length(&self) -> Duration {
        let nanos = u128::from(self.duration) * 1_000_000_000 / u128::from(self.timescale.max(1));
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

/// box の `(型, 本体)`。
pub(crate) type BoxEntry<'a> = ([u8; 4], &'a [u8]);

fn unreadable() -> DeviceError {
    DeviceError::Internal(UNREADABLE)
}

pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

pub(crate) fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// 並んだ box を `(型, 本体)` の列にする。size が 1 なら 64bit の largesize、0 なら末尾まで。
/// header 長未満・残りを超える size は失敗（panic しない）。
pub(crate) fn boxes(mut data: &[u8]) -> Result<Vec<BoxEntry<'_>>, DeviceError> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let size = be32(data, 0).ok_or_else(unreadable)?;
        let kind: [u8; 4] = data
            .get(4..8)
            .and_then(|t| t.try_into().ok())
            .ok_or_else(unreadable)?;
        let (header, total) = match size {
            0 => (8usize, data.len() as u64),
            1 => (16usize, be64(data, 8).ok_or_else(unreadable)?),
            n => (8usize, u64::from(n)),
        };
        let total = usize::try_from(total).map_err(|_| unreadable())?;
        if total < header || total > data.len() {
            return Err(unreadable());
        }
        out.push((kind, &data[header..total]));
        data = &data[total..];
    }
    Ok(out)
}

pub(crate) fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], DeviceError> {
    boxes(data)?
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, body)| body)
        .ok_or_else(unreadable)
}

/// `stsz` が示すサンプルのデータ量が `mdat`（トップレベルの合計）に収まることを確かめる。
/// 音声データ本体が無い・参照範囲より小さい出力は成功にしない。動画の読み直しも共用する。
pub(crate) fn check_samples_fit(
    stsz: &[u8],
    sample_count: u32,
    top: &[BoxEntry<'_>],
) -> Result<(), DeviceError> {
    let sample_size = be32(stsz, 4).ok_or_else(unreadable)?;
    // サンプルが参照するデータ量（固定長なら size * count、可変長なら表の総和）。
    let sample_bytes = if sample_size != 0 {
        u64::from(sample_size) * u64::from(sample_count)
    } else {
        let table = stsz.get(12..).ok_or_else(unreadable)?;
        let n = usize::try_from(sample_count).map_err(|_| unreadable())?;
        if table.len() / 4 < n {
            return Err(unreadable());
        }
        table
            .as_chunks::<4>()
            .0
            .iter()
            .take(n)
            .map(|c| u64::from(u32::from_be_bytes([c[0], c[1], c[2], c[3]])))
            .sum()
    };
    let mdat_len: u64 = top
        .iter()
        .filter(|(k, _)| k == b"mdat")
        .map(|(_, b)| b.len() as u64)
        .sum();
    if sample_bytes == 0 || sample_bytes > mdat_len {
        return Err(unreadable());
    }
    Ok(())
}

/// m4a（ISO BMFF）を解析し、符号形式・長さ・サンプル数を取り出す。ブランドは見ない
/// （OS ごとに違い偽の失敗になるため。符号形式は `stsd` で見る）。範囲外の読みで panic しない。
/// `moov` が無い（fragmented 等）・`timescale == 0`・`sample_count == 0`・`mdat` が無い、または
/// `stsz` が示すデータ量が `mdat` に収まらないものは `Internal(UNREADABLE)`。
pub fn parse_m4a(bytes: &[u8]) -> Result<M4aInfo, DeviceError> {
    let top = boxes(bytes)?;
    if !top.iter().any(|(k, _)| k == b"ftyp") {
        return Err(unreadable());
    }
    let moov = top
        .iter()
        .find(|(k, _)| k == b"moov")
        .map(|(_, b)| *b)
        .ok_or_else(unreadable)?;
    let mdia = child(child(moov, b"trak")?, b"mdia")?;
    let mdhd = child(mdia, b"mdhd")?;
    let (timescale, duration) = match mdhd.first() {
        Some(0) => (be32(mdhd, 12), be32(mdhd, 16).map(u64::from)),
        Some(1) => (be32(mdhd, 20), be64(mdhd, 24)),
        _ => (None, None),
    };
    let (timescale, duration) = timescale.zip(duration).ok_or_else(unreadable)?;
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let stsd = child(stbl, b"stsd")?;
    let codec: [u8; 4] = stsd
        .get(12..16)
        .and_then(|t| t.try_into().ok())
        .ok_or_else(unreadable)?;
    let stsz = child(stbl, b"stsz")?;
    let sample_count = be32(stsz, 8).ok_or_else(unreadable)?;
    if timescale == 0 || sample_count == 0 {
        return Err(unreadable());
    }
    check_samples_fit(stsz, sample_count, &top)?;
    Ok(M4aInfo {
        codec,
        timescale,
        duration,
        sample_count,
    })
}

/// 測った長さを入力の長さと照合する。差が [`DURATION_TOLERANCE`] を超えたら `Internal(MISMATCH)`
/// （境界は含む）。Web の実装も使う。
pub fn check_duration(measured: Duration, input: Duration) -> Result<Duration, DeviceError> {
    let diff = measured.abs_diff(input);
    if diff <= DURATION_TOLERANCE {
        Ok(measured)
    } else {
        Err(DeviceError::Internal(MISMATCH))
    }
}

/// 出力を読み直して成功の条件を確かめる。空 → `EMPTY`、解析できない・符号形式が `mp4a`
/// でない → `UNREADABLE`、長さが外れている → `MISMATCH`。成功なら出力の長さを返す。
pub fn check_m4a(bytes: &[u8], input: Duration) -> Result<Duration, DeviceError> {
    if bytes.is_empty() {
        return Err(DeviceError::Internal(EMPTY));
    }
    let info = parse_m4a(bytes)?;
    if &info.codec != b"mp4a" {
        return Err(unreadable());
    }
    check_duration(info.length(), input)
}

/// box を 1 つ組む（`size` + 型 + 本体）。テスト・フェイク用の MP4 組み立てが共用する。
pub(crate) fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}

/// テスト・フェイク実装用に、解析に要る最小の m4a を組む（`ftyp` + `moov/trak/mdia/{mdhd, minf/stbl/{stsd, stsz}}`）。
/// 実際に再生できる音声ではない（`mdat` は `samples` バイトの 0）。`version1` は `mdhd` を 64bit 形式にする。
pub fn minimal_m4a(
    codec: &[u8; 4],
    timescale: u32,
    duration: u64,
    samples: u32,
    version1: bool,
) -> Vec<u8> {
    let mut mdhd = Vec::new();
    if version1 {
        mdhd.extend_from_slice(&[1, 0, 0, 0]);
        mdhd.extend_from_slice(&[0; 16]);
        mdhd.extend_from_slice(&timescale.to_be_bytes());
        mdhd.extend_from_slice(&duration.to_be_bytes());
    } else {
        mdhd.extend_from_slice(&[0, 0, 0, 0]);
        mdhd.extend_from_slice(&[0; 8]);
        mdhd.extend_from_slice(&timescale.to_be_bytes());
        mdhd.extend_from_slice(&(duration as u32).to_be_bytes());
    }
    let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
    stsd.extend_from_slice(&bx(codec, &[0; 8]));
    let mut stsz = vec![0u8; 8];
    stsz[7] = 1; // 固定長 1 バイト。mdat は samples バイト
    stsz.extend_from_slice(&samples.to_be_bytes());
    let stbl = [bx(b"stsd", &stsd), bx(b"stsz", &stsz)].concat();
    let mdia = [bx(b"mdhd", &mdhd), bx(b"minf", &bx(b"stbl", &stbl))].concat();
    [
        bx(b"ftyp", b"M4A \0\0\0\0M4A "),
        bx(b"moov", &bx(b"trak", &bx(b"mdia", &mdia))),
        bx(b"mdat", &vec![0; samples as usize]),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: Duration = Duration::from_secs(5);

    fn good() -> Vec<u8> {
        minimal_m4a(b"mp4a", 48_000, 240_000, 235, false)
    }

    #[test]
    fn parses_v0_and_v1() {
        for v1 in [false, true] {
            let info = parse_m4a(&minimal_m4a(b"mp4a", 48_000, 240_000, 235, v1)).unwrap();
            assert_eq!(&info.codec, b"mp4a");
            assert_eq!(info.length(), INPUT);
            assert_eq!(info.sample_count, 235);
        }
    }

    #[test]
    fn largesize_and_size_zero() {
        let m = good();
        // 先頭の ftyp を largesize（size == 1）形式へ書き換える。
        let ftyp_len = u32::from_be_bytes(m[0..4].try_into().unwrap()) as usize;
        let mut large = 1u32.to_be_bytes().to_vec();
        large.extend_from_slice(b"ftyp");
        large.extend_from_slice(&((ftyp_len + 8) as u64).to_be_bytes());
        large.extend_from_slice(&m[8..ftyp_len]);
        large.extend_from_slice(&m[ftyp_len..]);
        assert!(parse_m4a(&large).is_ok());
        // 末尾の mdat（8 + 235 バイト）を size == 0（末尾まで）にする。
        let mut zero = m.clone();
        let mdat_at = m.len() - (8 + 235);
        zero[mdat_at..mdat_at + 4].copy_from_slice(&0u32.to_be_bytes());
        assert!(parse_m4a(&zero).is_ok());
    }

    #[test]
    fn truncated_input_fails_without_panic() {
        let m = good();
        for n in 0..m.len() {
            assert!(parse_m4a(&m[..n]).is_err(), "len {n}");
        }
    }

    #[test]
    fn malformed_boxes_fail() {
        // size が残りより大きい
        let mut big = good();
        big[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse_m4a(&big).is_err());
        // size が header 長未満
        let mut small = good();
        small[0..4].copy_from_slice(&4u32.to_be_bytes());
        assert!(parse_m4a(&small).is_err());
        // largesize が header 長未満
        let mut tiny = 1u32.to_be_bytes().to_vec();
        tiny.extend_from_slice(b"ftyp");
        tiny.extend_from_slice(&2u64.to_be_bytes());
        assert!(parse_m4a(&tiny).is_err());
        // moov が無い（fragmented 相当）
        let m = good();
        let ftyp_len = u32::from_be_bytes(m[0..4].try_into().unwrap()) as usize;
        assert!(parse_m4a(&m[..ftyp_len]).is_err());
        // mdat が無い（moov までで切る）
        let mdat_at = m.len() - (8 + 235);
        assert!(parse_m4a(&m[..mdat_at]).is_err());
        // mdat がサンプルの参照範囲より小さい
        let mut short = m[..mdat_at].to_vec();
        short.extend_from_slice(&9u32.to_be_bytes());
        short.extend_from_slice(b"mdat");
        short.push(0);
        assert!(parse_m4a(&short).is_err());
        // ゴミ
        assert!(parse_m4a(&[0xff; 64]).is_err());
    }

    #[test]
    fn zero_timescale_and_zero_samples_fail() {
        assert!(parse_m4a(&minimal_m4a(b"mp4a", 0, 1, 1, false)).is_err());
        assert!(parse_m4a(&minimal_m4a(b"mp4a", 48_000, 1, 0, false)).is_err());
    }

    #[test]
    fn check_m4a_conditions() {
        assert_eq!(check_m4a(&good(), INPUT).unwrap(), INPUT);
        assert!(matches!(
            check_m4a(&[], INPUT),
            Err(DeviceError::Internal(EMPTY))
        ));
        assert!(matches!(
            check_m4a(b"garbage", INPUT),
            Err(DeviceError::Internal(UNREADABLE))
        ));
        let opus = minimal_m4a(b"Opus", 48_000, 240_000, 235, false);
        assert!(matches!(
            check_m4a(&opus, INPUT),
            Err(DeviceError::Internal(UNREADABLE))
        ));
    }

    #[test]
    fn duration_boundary() {
        let at = |ms: i64| {
            let ts = 1000u32;
            let d = (5000 + ms) as u64;
            check_m4a(&minimal_m4a(b"mp4a", ts, d, 1, false), INPUT)
        };
        assert!(at(200).is_ok());
        assert!(at(-200).is_ok());
        assert!(matches!(at(201), Err(DeviceError::Internal(MISMATCH))));
        assert!(matches!(at(-201), Err(DeviceError::Internal(MISMATCH))));
    }

    #[test]
    fn validate_input_rejects_empty_and_zero_rate() {
        assert!(validate_input(&[1], 48_000).is_ok());
        assert!(matches!(
            validate_input(&[], 48_000),
            Err(DeviceError::InvalidInput(_))
        ));
        assert!(matches!(
            validate_input(&[1], 0),
            Err(DeviceError::InvalidInput(_))
        ));
    }

    #[test]
    fn input_duration_of_demo_clip() {
        assert_eq!(input_duration(240_000, SAMPLE_RATE), INPUT);
        assert_eq!(input_duration(1, 0), Duration::ZERO);
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedMediaTranscode.transcode(&[1], SAMPLE_RATE, Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
