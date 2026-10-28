//! 動画の変換の抽象（Phase 11 の拡充ループ、対応表 `docs/device/capabilities/video-transcode.json`、
//! 親 #340、Issue #1014）。
//!
//! 扱う範囲は「実行時に作った元動画（[`SOURCE_WIDTH`]×[`SOURCE_HEIGHT`]・[`SOURCE_FPS`] fps・
//! [`SOURCE_FRAMES`] フレーム＝2 秒・映像のみ・H.264）を OS の変換経路で小さい解像度の MP4 へ
//! 1 回書き出す」ことだけ。保存・共有・再生・進捗・取り消し・形式の選択・利用者の動画は扱わない。
//! 対象は macOS / iOS（`AVAssetWriter` + `AVAssetExportSession`）、Windows（`MediaTranscoder`）、
//! Android（`MediaCodec` + `MediaMuxer`）、Web（WebCodecs。コンテナなし）。
//! 一発呼び出し型の同期 API で、呼び出し側（`fandhe-egui-proxy` の effect ドライバ）は
//! UI を止めないようワーカースレッドから呼ぶ。元動画のフレームの絵は [`fill_source_frame_bgra`] が
//! 作り、Apple・Windows・Web が共用する。
//!
//! # 契約
//!
//! - **成功**: 出力を読み直して [`check_mp4_video`] を満たしたものだけ。0 バイトでない・映像トラックがある・
//!   幅と高さがともに 0 でなく元動画より小さい・サンプルが 1 以上で `mdat` に収まる・長さが元動画の
//!   [`source_duration`] ±[`DURATION_TOLERANCE`] の中、をすべて満たす（契約表 6「嘘の成功」）。
//!   符号形式（fourcc）は拒まず、そのまま表示する
//! - **失敗の区別**: 非対応 → `Err(Unsupported("video_transcode"))`、期限切れ → `Internal(`[`TIMEOUT`]`)`、
//!   出力が 0 バイト → [`EMPTY`]、解析できない・映像が無い・寸法 0 → [`UNREADABLE`]、
//!   幅か高さが元動画以上 → [`NOT_SMALLER`]、長さの不一致 → [`MISMATCH`]、
//!   前回の解放を確認できない → [`RELEASE_PENDING`]、それ以外 → `Internal`（段階ごとの固定ラベル。OS の文言は出さない）
//! - **結果の三値**（契約表 10）: 成功 / 中断 / 失敗。中断は Web の `pagehide` のみで、画面側が失敗とは別の
//!   状態として扱う。**取り消しの操作と OS の権限は存在しない**ため、キャンセル・権限拒否の失敗系は無い
//! - **OS を正にする**: 成否は OS の完了通知ではなく出力の読み直しで決める。出力の寸法は OS が書いた値を
//!   そのまま出す（Apple はプリセット `AVAssetExportPreset640x480` に任せる）。[`TARGET_WIDTH`]・
//!   [`TARGET_HEIGHT`] は Web と Android へ渡す指定値
//! - **期限**（契約表 3）: 期限は Rust のドライバだけが持つ（[`TRANSCODE_DEADLINE`]。実測の根拠が無い仮の値で、
//!   実機確認で見直す）。起点は要求を観測した時の単調時計 `Instant`。OS 側が中断できる実装は
//!   `remaining` で中断し、解放してから `TIMEOUT` を返す。期限後に届いた結果はドライバが捨てる
//! - **並行性**（契約表 2）: 世代は画面側が要求ごとに進め、完了は「処理中かつ同じ世代」のときだけ適用する。
//!   処理中の再押下は無視する
//! - **解放の失敗**（契約表 9）: 一時ファイルを消せなかった場合も、出力を読み終えていれば結果は返し、
//!   未解放として記録する。次の要求の開始前に再削除と前回の OS 操作の完了を確かめ、確かめられなければ
//!   OS を呼ばずに [`RELEASE_PENDING`] で失敗にする
//! - **ライフサイクル**（契約表 1）: 変換は短いため背景遷移では中断しない
//!
//! # 守る範囲と守らない範囲（契約表 11）
//!
//! 守る範囲: 1 プロセス内での要求と完了の順序、嘘の成功の禁止、一時ファイル・コーデック・export session・
//! `MediaStreamSource`・`VideoFrame` の解放。
//! 守らない範囲: OS ごとの所要時間・ビットレート・符号形式の違い、Apple のプリセットが決める出力寸法、
//! 背景へ移ったときの OS の挙動、複数プロセス・複数タブ。出力のバイト列・OS のエラー文はログへ出さない。

use std::time::Duration;

use crate::DeviceError;
use crate::media_transcode::{
    BoxEntry, be32, be64, boxes as shared_boxes, check_samples_fit, child,
};

pub use crate::media_transcode::DURATION_TOLERANCE;

/// 元動画の幅。
pub const SOURCE_WIDTH: u32 = 1280;
/// 元動画の高さ。
pub const SOURCE_HEIGHT: u32 = 720;
/// 元動画のフレームレート。
pub const SOURCE_FPS: u32 = 15;
/// 元動画のフレーム数（2 秒）。
pub const SOURCE_FRAMES: u32 = 30;
/// 出力の指定幅（Web と Android へ渡す。Apple はプリセット、Windows は OS が決める）。
pub const TARGET_WIDTH: u32 = 640;
/// 出力の指定高さ（同上）。
pub const TARGET_HEIGHT: u32 = 360;
/// 要求を観測してから結果を確定するまでの期限（Rust のドライバだけが持つ）。
pub const TRANSCODE_DEADLINE: Duration = Duration::from_secs(20);

/// 期限切れのときの `Internal` の識別子（SSOT）。
pub const TIMEOUT: &str = "video_transcode timeout";
/// 出力が 0 バイト（Web はチャンク 0）のときの `Internal` の識別子（SSOT）。
pub const EMPTY: &str = "video_transcode empty";
/// 出力を解析できない・映像が無い・寸法が 0 のときの `Internal` の識別子（SSOT）。
pub const UNREADABLE: &str = "video_transcode unreadable";
/// 出力の幅か高さが元動画以上のときの `Internal` の識別子（SSOT）。
pub const NOT_SMALLER: &str = "video_transcode not smaller";
/// 出力の長さが元動画と合わないときの `Internal` の識別子（SSOT）。
pub const MISMATCH: &str = "video_transcode mismatch";
/// 前回の解放（一時ファイルの削除・OS 操作の完了）を確認できないときの `Internal` の識別子（SSOT）。
pub const RELEASE_PENDING: &str = "video_transcode release pending";

/// 元動画の長さ（フレーム数 / フレームレート = 2 秒）。
pub const fn source_duration() -> Duration {
    Duration::from_millis(SOURCE_FRAMES as u64 * 1000 / SOURCE_FPS as u64)
}

/// 動画変換の契約（モジュール doc の「契約」節）。
pub trait VideoTranscode: Send + Sync {
    /// 実行時に元動画を作り、小さい解像度の MP4 へ変換して出力のバイト列を返す。
    /// `remaining` は期限までの残り時間。
    fn transcode(&self, remaining: Duration) -> Result<Vec<u8>, DeviceError>;
}

/// 対象外 OS（Linux 等）・未実装 OS のフォールバック。ダミーの成功実装へは倒さない。
pub struct UnsupportedVideoTranscode;

impl VideoTranscode for UnsupportedVideoTranscode {
    fn transcode(&self, _: Duration) -> Result<Vec<u8>, DeviceError> {
        Err(DeviceError::Unsupported("video_transcode"))
    }
}

/// 元動画の `index` 枚目を BGRA（行間 `stride` バイト）で `dst` へ描く。フレームごとに位置が変わる
/// 縦縞で、同一フレームの間引きで長さが縮まないようにする。`stride < width*4`・`dst` 不足・
/// 寸法 0 のときは書かずに `false`（panic しない）。
pub fn fill_source_frame_bgra(
    index: u32,
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut [u8],
) -> bool {
    let (w, h) = (width as usize, height as usize);
    let Some(row) = w.checked_mul(4) else {
        return false;
    };
    if w == 0 || h == 0 || stride < row {
        return false;
    }
    let Some(need) = (h - 1).checked_mul(stride).and_then(|v| v.checked_add(row)) else {
        return false;
    };
    if dst.len() < need {
        return false;
    }
    let shift = index as usize * 16;
    for y in 0..h {
        let line = &mut dst[y * stride..y * stride + row];
        for (x, px) in line.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let on = ((x + shift) / 32).is_multiple_of(2);
            px.copy_from_slice(if on {
                &[40, 160, 240, 255]
            } else {
                &[230, 230, 230, 255]
            });
        }
    }
    true
}

/// MP4 から読み取った、検証に要る最小の映像の情報。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mp4VideoInfo {
    /// `stsd` の先頭エントリの型（H.264 は `avc1`）。
    pub codec: [u8; 4],
    /// 映像の幅。
    pub width: u32,
    /// 映像の高さ。
    pub height: u32,
    /// `mdhd` の時間軸。
    pub timescale: u32,
    /// `mdhd` の長さ（`timescale` 単位）。
    pub duration: u64,
    /// `stsz` のサンプル数。
    pub sample_count: u32,
}

impl Mp4VideoInfo {
    /// 再生時間。
    pub fn length(&self) -> Duration {
        let nanos = u128::from(self.duration) * 1_000_000_000 / u128::from(self.timescale.max(1));
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
}

fn unreadable() -> DeviceError {
    DeviceError::Internal(UNREADABLE)
}

/// 共有ヘルパーのエラーを本モジュールの識別子へ写す（他機能の識別子を漏らさない）。
fn boxes(data: &[u8]) -> Result<Vec<BoxEntry<'_>>, DeviceError> {
    shared_boxes(data).map_err(|_| unreadable())
}

fn sub<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], DeviceError> {
    child(data, kind).map_err(|_| unreadable())
}

/// MP4（ISO BMFF）を解析し、映像トラックの符号形式・寸法・長さ・サンプル数を取り出す。
/// `moov` 配下の全 `trak` から `hdlr` が `vide` のものを選ぶ（音声が先でも映像を選ぶ）。
/// 範囲外の読みで panic しない。映像トラックが無い・`timescale == 0`・サンプル 0・寸法 0・`mdat` に
/// 収まらないものは `Internal(UNREADABLE)`。
pub fn parse_mp4_video(bytes: &[u8]) -> Result<Mp4VideoInfo, DeviceError> {
    let top = boxes(bytes)?;
    if !top.iter().any(|(k, _)| k == b"ftyp") {
        return Err(unreadable());
    }
    let moov = top
        .iter()
        .find(|(k, _)| k == b"moov")
        .map(|(_, b)| *b)
        .ok_or_else(unreadable)?;
    let mut video = None;
    for (kind, trak) in boxes(moov)? {
        if &kind != b"trak" {
            continue;
        }
        let mdia = sub(trak, b"mdia")?;
        let hdlr = sub(mdia, b"hdlr")?;
        if hdlr.get(8..12) == Some(b"vide".as_slice()) {
            video = Some(mdia);
            break;
        }
    }
    let mdia = video.ok_or_else(unreadable)?;
    let mdhd = sub(mdia, b"mdhd")?;
    let (timescale, duration) = match mdhd.first() {
        Some(0) => (be32(mdhd, 12), be32(mdhd, 16).map(u64::from)),
        Some(1) => (be32(mdhd, 20), be64(mdhd, 24)),
        _ => (None, None),
    };
    let (timescale, duration) = timescale.zip(duration).ok_or_else(unreadable)?;
    let stbl = sub(sub(mdia, b"minf")?, b"stbl")?;
    let stsd = sub(stbl, b"stsd")?;
    let codec: [u8; 4] = stsd
        .get(12..16)
        .and_then(|t| t.try_into().ok())
        .ok_or_else(unreadable)?;
    let width = stsd
        .get(40..42)
        .map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])))
        .ok_or_else(unreadable)?;
    let height = stsd
        .get(42..44)
        .map(|b| u32::from(u16::from_be_bytes([b[0], b[1]])))
        .ok_or_else(unreadable)?;
    let stsz = sub(stbl, b"stsz")?;
    let sample_count = be32(stsz, 8).ok_or_else(unreadable)?;
    if timescale == 0 || sample_count == 0 || width == 0 || height == 0 {
        return Err(unreadable());
    }
    check_samples_fit(stsz, sample_count, &top).map_err(|_| unreadable())?;
    Ok(Mp4VideoInfo {
        codec,
        width,
        height,
        timescale,
        duration,
        sample_count,
    })
}

/// 測った長さを元動画の長さと照合する。差が [`DURATION_TOLERANCE`] を超えたら `Internal(MISMATCH)`
/// （境界は含む）。
fn check_length(measured: Duration) -> Result<Duration, DeviceError> {
    if measured.abs_diff(source_duration()) <= DURATION_TOLERANCE {
        Ok(measured)
    } else {
        Err(DeviceError::Internal(MISMATCH))
    }
}

/// 出力を読み直して成功の条件を確かめる。空 → `EMPTY`、解析できない・映像が無い・寸法 0 →
/// `UNREADABLE`、幅か高さが元動画以上 → `NOT_SMALLER`、長さが外れている → `MISMATCH`。
pub fn check_mp4_video(bytes: &[u8]) -> Result<Mp4VideoInfo, DeviceError> {
    if bytes.is_empty() {
        return Err(DeviceError::Internal(EMPTY));
    }
    let info = parse_mp4_video(bytes)?;
    if info.width >= SOURCE_WIDTH || info.height >= SOURCE_HEIGHT {
        return Err(DeviceError::Internal(NOT_SMALLER));
    }
    check_length(info.length())?;
    Ok(info)
}

/// Web で復号した 1 フレーム（コンテナなしのため、復号結果から検証する）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedFrame {
    /// 復号フレームの幅。
    pub width: u32,
    /// 復号フレームの高さ。
    pub height: u32,
    /// 表示時刻（マイクロ秒）。
    pub timestamp_us: u64,
}

/// Web の復号結果を確かめる。チャンク 0 → `EMPTY`、復号フレーム 0 → `UNREADABLE`、寸法が元動画以上 →
/// `NOT_SMALLER`、寸法が設定値と違う → `UNREADABLE`、（最大 ts − 最小 ts + 1 フレーム）が
/// 元動画 ±[`DURATION_TOLERANCE`] の外 → `MISMATCH`。成功なら測った長さを返す。
pub fn check_decoded(
    chunks: usize,
    frames: &[DecodedFrame],
    expected_w: u32,
    expected_h: u32,
) -> Result<Duration, DeviceError> {
    if chunks == 0 {
        return Err(DeviceError::Internal(EMPTY));
    }
    if frames.is_empty() {
        return Err(unreadable());
    }
    if frames
        .iter()
        .any(|f| f.width >= SOURCE_WIDTH || f.height >= SOURCE_HEIGHT)
    {
        return Err(DeviceError::Internal(NOT_SMALLER));
    }
    if frames
        .iter()
        .any(|f| f.width != expected_w || f.height != expected_h)
    {
        return Err(unreadable());
    }
    let min = frames.iter().map(|f| f.timestamp_us).min().unwrap_or(0);
    let max = frames.iter().map(|f| f.timestamp_us).max().unwrap_or(0);
    let frame_us = 1_000_000 / u64::from(SOURCE_FPS);
    // 加算が u64 を超える時刻は復元不能な長さなので MISMATCH にする（折り返しで成功させない）
    let len = (max - min)
        .checked_add(frame_us)
        .ok_or(DeviceError::Internal(MISMATCH))?;
    check_length(Duration::from_micros(len))
}

/// テスト・フェイク実装用に、解析に要る最小の MP4 を組む（`ftyp` + `moov/trak…` + `mdat`）。
/// 実際に再生できる映像ではない（`mdat` は `samples` バイトの 0）。`with_audio_track_first` は
/// 映像の前に音声 `trak`（1 サンプル）を置く。
pub fn minimal_mp4_video(
    codec: &[u8; 4],
    width: u16,
    height: u16,
    timescale: u32,
    duration: u32,
    samples: u32,
    with_audio_track_first: bool,
) -> Vec<u8> {
    use crate::media_transcode::bx;
    let trak = |handler: &[u8; 4], entry: Vec<u8>, ts: u32, dur: u32, n: u32| {
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0; 13]);
        let mut mdhd = vec![0u8; 12];
        mdhd.extend_from_slice(&ts.to_be_bytes());
        mdhd.extend_from_slice(&dur.to_be_bytes());
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend_from_slice(&entry);
        let mut stsz = vec![0u8; 8];
        stsz[7] = 1; // 固定長 1 バイト
        stsz.extend_from_slice(&n.to_be_bytes());
        let stbl = [bx(b"stsd", &stsd), bx(b"stsz", &stsz)].concat();
        let mdia = [
            bx(b"mdhd", &mdhd),
            bx(b"hdlr", &hdlr),
            bx(b"minf", &bx(b"stbl", &stbl)),
        ]
        .concat();
        bx(b"trak", &bx(b"mdia", &mdia))
    };
    let mut visual = vec![0u8; 24];
    visual.extend_from_slice(&width.to_be_bytes());
    visual.extend_from_slice(&height.to_be_bytes());
    visual.extend_from_slice(&[0; 44]);
    let video = trak(b"vide", bx(codec, &visual), timescale, duration, samples);
    let mut moov = Vec::new();
    let mut mdat = samples as usize;
    if with_audio_track_first {
        moov.extend(trak(b"soun", bx(b"mp4a", &[0; 28]), 48_000, 96_000, 1));
        mdat += 1;
    }
    moov.extend(video);
    [
        bx(b"ftyp", b"isom\0\0\0\0isom"),
        bx(b"moov", &moov),
        bx(b"mdat", &vec![0; mdat]),
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Vec<u8> {
        minimal_mp4_video(b"avc1", 640, 360, 15_000, 30_000, 30, false)
    }

    fn at(ms: i64) -> Result<Mp4VideoInfo, DeviceError> {
        let d = (2000 + ms) as u32;
        check_mp4_video(&minimal_mp4_video(b"avc1", 640, 360, 1000, d, 30, false))
    }

    #[test]
    fn source_is_two_seconds() {
        assert_eq!(source_duration(), Duration::from_secs(2));
    }

    #[test]
    fn parses_video_fields() {
        let i = parse_mp4_video(&good()).unwrap();
        assert_eq!(&i.codec, b"avc1");
        assert_eq!((i.width, i.height, i.sample_count), (640, 360, 30));
        assert_eq!(i.length(), Duration::from_secs(2));
        assert!(check_mp4_video(&good()).is_ok());
    }

    #[test]
    fn picks_video_track_even_when_audio_comes_first() {
        let m = minimal_mp4_video(b"avc1", 640, 360, 15_000, 30_000, 30, true);
        let i = parse_mp4_video(&m).unwrap();
        assert_eq!((i.width, i.sample_count), (640, 30));
    }

    #[test]
    fn truncated_input_fails_without_panic() {
        let m = good();
        for n in 0..m.len() {
            assert!(parse_mp4_video(&m[..n]).is_err(), "len {n}");
        }
    }

    #[test]
    fn malformed_inputs_fail_with_own_identifier() {
        let mut big = good();
        big[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut small = good();
        small[0..4].copy_from_slice(&4u32.to_be_bytes());
        let m = good();
        let ftyp_len = u32::from_be_bytes(m[0..4].try_into().unwrap()) as usize;
        let mdat_at = m.len() - (8 + 30);
        let mut short = m[..mdat_at].to_vec();
        short.extend_from_slice(&9u32.to_be_bytes());
        short.extend_from_slice(b"mdat");
        short.push(0);
        // 音声のみ（hdlr を soun に書き換える）
        let mut audio_only = good();
        let at = audio_only.windows(4).position(|w| w == b"vide").unwrap();
        audio_only[at..at + 4].copy_from_slice(b"soun");
        let cases: Vec<Vec<u8>> = vec![
            big,
            small,
            m[..ftyp_len].to_vec(),
            m[..mdat_at].to_vec(),
            short,
            vec![0xff; 64],
            audio_only,
            minimal_mp4_video(b"avc1", 640, 360, 0, 1, 1, false),
            minimal_mp4_video(b"avc1", 640, 360, 1000, 1, 0, false),
            minimal_mp4_video(b"avc1", 0, 360, 1000, 2000, 30, false),
            minimal_mp4_video(b"avc1", 640, 0, 1000, 2000, 30, false),
        ];
        for c in cases {
            let r = parse_mp4_video(&c);
            assert!(matches!(r, Err(DeviceError::Internal(UNREADABLE))), "{r:?}");
        }
    }

    #[test]
    fn check_mp4_video_failure_kinds() {
        assert!(matches!(
            check_mp4_video(&[]),
            Err(DeviceError::Internal(EMPTY))
        ));
        assert!(matches!(
            check_mp4_video(b"garbage"),
            Err(DeviceError::Internal(UNREADABLE))
        ));
        for (w, h) in [(1280, 360), (640, 720), (1280, 720), (2000, 2000)] {
            let m = minimal_mp4_video(b"avc1", w, h, 15_000, 30_000, 30, false);
            assert!(
                matches!(check_mp4_video(&m), Err(DeviceError::Internal(NOT_SMALLER))),
                "{w}x{h}"
            );
        }
    }

    #[test]
    fn unknown_codec_is_not_rejected() {
        let m = minimal_mp4_video(b"hvc1", 640, 360, 15_000, 30_000, 30, false);
        assert_eq!(&check_mp4_video(&m).unwrap().codec, b"hvc1");
    }

    #[test]
    fn duration_boundary() {
        assert!(at(200).is_ok());
        assert!(at(-200).is_ok());
        assert!(matches!(at(201), Err(DeviceError::Internal(MISMATCH))));
        assert!(matches!(at(-201), Err(DeviceError::Internal(MISMATCH))));
    }

    fn frames(n: u64, step_us: u64, w: u32, h: u32) -> Vec<DecodedFrame> {
        (0..n)
            .map(|i| DecodedFrame {
                width: w,
                height: h,
                timestamp_us: i * step_us,
            })
            .collect()
    }

    #[test]
    fn check_decoded_conditions() {
        let ok = frames(30, 66_667, 640, 360);
        assert!(check_decoded(30, &ok, 640, 360).is_ok());
        assert!(matches!(
            check_decoded(0, &ok, 640, 360),
            Err(DeviceError::Internal(EMPTY))
        ));
        assert!(matches!(
            check_decoded(30, &[], 640, 360),
            Err(DeviceError::Internal(UNREADABLE))
        ));
        assert!(matches!(
            check_decoded(30, &frames(30, 66_667, 1280, 720), 640, 360),
            Err(DeviceError::Internal(NOT_SMALLER))
        ));
        assert!(matches!(
            check_decoded(30, &ok, 320, 180),
            Err(DeviceError::Internal(UNREADABLE))
        ));
        // 間引かれて短くなる
        assert!(matches!(
            check_decoded(30, &frames(15, 66_667, 640, 360), 640, 360),
            Err(DeviceError::Internal(MISMATCH))
        ));
    }

    #[test]
    fn check_decoded_boundary() {
        // 長さ = last + 66_666 us。2000 ms ± 200 ms の境界。
        let dur = |last_us: u64| {
            let f = frames(2, last_us, 640, 360);
            check_decoded(2, &f, 640, 360)
        };
        assert!(dur(2_000_000 + 200_000 - 66_666).is_ok());
        assert!(dur(2_000_000 - 200_000 - 66_666).is_ok());
        assert!(dur(2_000_000 + 200_000 - 66_665).is_err());
        assert!(dur(2_000_000 - 200_000 - 66_667).is_err());
    }

    #[test]
    fn check_decoded_rejects_timestamp_overflow() {
        // max - min + frame_us が u64 を超えても panic・折り返し成功しない
        let mut f = frames(2, 0, 640, 360);
        f[1].timestamp_us = u64::MAX - 1_000;
        assert!(matches!(
            check_decoded(2, &f, 640, 360),
            Err(DeviceError::Internal(MISMATCH))
        ));
    }

    #[test]
    fn source_frames_differ_and_respect_stride() {
        let (w, h, stride) = (64u32, 4u32, 64 * 4 + 8);
        let mut a = vec![7u8; stride * h as usize];
        let mut b = a.clone();
        assert!(fill_source_frame_bgra(0, w, h, stride, &mut a));
        assert!(fill_source_frame_bgra(1, w, h, stride, &mut b));
        assert_ne!(a, b);
        // 余白は触らない
        assert_eq!(&a[256..264], &[7u8; 8]);
    }

    #[test]
    fn source_frame_rejects_bad_buffers() {
        let mut d = vec![0u8; 100];
        assert!(!fill_source_frame_bgra(0, 64, 4, 64 * 4, &mut d));
        assert!(!fill_source_frame_bgra(0, 64, 1, 10, &mut vec![0; 4096]));
        assert!(!fill_source_frame_bgra(0, 0, 4, 0, &mut d));
        assert!(!fill_source_frame_bgra(0, 4, 0, 16, &mut d));
    }

    #[test]
    fn unsupported_never_succeeds() {
        assert!(matches!(
            UnsupportedVideoTranscode.transcode(Duration::ZERO),
            Err(DeviceError::Unsupported(_))
        ));
    }
}
