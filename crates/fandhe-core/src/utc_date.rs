//! UTC の暦日計算（端末依存ゼロの純関数）。画面と OS 実装の双方から呼ばれ、
//! 日付計算を 1 か所にまとめる（`tsv` と同じ位置づけの横断部品）。

/// 1970-01-01 からの経過日数 → `(year, month, day)` へ変換する純関数（Howard
/// Hinnant の civil_from_days アルゴリズム）。時刻ライブラリは追加しない方針
/// （`ponytail:` 現地時刻が必要になったら tz 対応を足す、本画面は UTC 固定で足りる）。
/// 画面（`screens::background_task::format_utc`）と OS 実装（Windows の
/// `ITrigger` 境界文字列）の両方が使う、日付計算の唯一の置き場。
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
