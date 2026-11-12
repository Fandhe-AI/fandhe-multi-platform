//! MP4 box 読みの共有部品（音声変換の機能 crate `fandhe-cap-media-transcode` と、動画変換 [`crate::video_transcode`] が使う）。
//!
//! 音声変換の本体（契約・OS 実装・effect ドライバ）は機能 crate へ移した（Issue #1188）。ここには OS に触れない
//! box の走査と、動画変換の読み直しでも共用する検査だけを残す（`http.rs` と同じ形。機能 crate は本 crate へ
//! 依存し、本 crate から機能 crate へは依存しない）。失敗は理由を持たない [`Unreadable`] で、呼び出し側が
//! 自機能のエラー種別へ写す。

use std::time::Duration;

/// 出力の長さと入力の長さの許容差（AAC のプライミング・パディング分）。音声・動画の変換が共用する。
pub const DURATION_TOLERANCE: Duration = Duration::from_millis(200);

/// box を読めない（header 不足・size 不整合・必要な box が無い・サンプル表が合わない）。理由は持たない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable;

/// box の `(型, 本体)`。
pub type BoxEntry<'a> = ([u8; 4], &'a [u8]);

fn unreadable() -> Unreadable {
    Unreadable
}

pub fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

pub fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// 並んだ box を `(型, 本体)` の列にする。size が 1 なら 64bit の largesize、0 なら末尾まで。
/// header 長未満・残りを超える size は失敗（panic しない）。
pub fn boxes(mut data: &[u8]) -> Result<Vec<BoxEntry<'_>>, Unreadable> {
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

pub fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<&'a [u8], Unreadable> {
    boxes(data)?
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, body)| body)
        .ok_or_else(unreadable)
}

/// `stsz` が示すサンプルのデータ量が `mdat`（トップレベルの合計）に収まることを確かめる。
/// 音声データ本体が無い・参照範囲より小さい出力は成功にしない。動画の読み直しも共用する。
pub fn check_samples_fit(
    stsz: &[u8],
    sample_count: u32,
    top: &[BoxEntry<'_>],
) -> Result<(), Unreadable> {
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

/// box を 1 つ組む（`size` + 型 + 本体）。テスト・フェイク用の MP4 組み立てが共用する。
pub fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(kind);
    v.extend_from_slice(body);
    v
}
