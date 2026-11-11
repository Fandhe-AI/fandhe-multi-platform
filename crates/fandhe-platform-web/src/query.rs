//! URL クエリの復号（全ターゲットの純関数）。
//!
//! `?url=` / `?share_text=` のような外部入力を機能が受け口（`UrlInbox` / `ShareInbox`）へ渡す前に、
//! bytes を壊さず取り出す。厳密な UTF-8 検査は受け口の責務。

/// クエリ文字列から `key` の値を、置換なしのパーセントデコードで元のバイト列として取り出す。
/// 厳密な UTF-8 検査は受け口（`receive_bytes`）に任せ、ここでは bytes を壊さない。
pub fn query_param_bytes(search: &str, key: &str) -> Option<Vec<u8>> {
    let q = search.strip_prefix('?').unwrap_or(search);
    let raw = q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })?;
    let b = raw.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() && hex(b[i + 1]).is_some() && hex(b[i + 2]).is_some() => {
                out.push(hex(b[i + 1])? << 4 | hex(b[i + 2])?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_keeps_invalid_utf8_bytes() {
        let v = query_param_bytes("?screen=x&url=web%2Bfandhe%3A%FF", "url").unwrap();
        assert_eq!(v, b"web+fandhe:\xFF");
        assert!(std::str::from_utf8(&v).is_err());
        assert_eq!(query_param_bytes("?a=1", "url"), None);
        assert_eq!(query_param_bytes("?url=a%2", "url").unwrap(), b"a%2");
        assert_eq!(query_param_bytes("?url=a%41", "url").unwrap(), b"aA");
    }
}
