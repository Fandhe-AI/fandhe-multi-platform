//! base64url（パディングなし）の符号化・復号（RFC 4648 §5）。
//!
//! パスキー本体（契約・OS 実装・effect ドライバ）は機能 crate `fandhe-cap-passkey-signin` へ移した（Issue #1188）。
//! 本モジュールには OS に触れない base64url だけを残す。`digital_credentials`・`web_auth_session` が PKCE や
//! challenge の符号化に使い、機能 crate も同じものを再公開する（`http.rs` と同じ形。機能 crate は本 crate へ依存し、
//! 本 crate から機能 crate へは依存しない）。

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// base64url（パディングなし）へ符号化する。
pub fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |a, (i, b)| a | (*b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

/// base64url（パディングなし）を復号する。`=`・不正文字・不正な長さ・正規形でない入力は `None`。
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = B64.iter().position(|x| x == c)? as u32;
            n |= v << (18 - 6 * i);
        }
        for i in 0..chunk.len() - 1 {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    // 余りビットが 0 でない入力は正規形でないので拒否する。
    (base64url_encode(&out) == s).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_rfc4648_vectors_round_trip() {
        for (raw, enc) in [
            ("", ""),
            ("f", "Zg"),
            ("fo", "Zm8"),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg"),
            ("fooba", "Zm9vYmE"),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64url_encode(raw.as_bytes()), enc);
            assert_eq!(base64url_decode(enc).unwrap(), raw.as_bytes());
        }
        assert_eq!(base64url_encode(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn base64url_rejects_padding_bad_chars_and_noncanonical() {
        for s in ["Zg==", "Zm9v+", "Zm9v/", "Z", "Zm 9", "Zh"] {
            assert!(base64url_decode(s).is_none(), "{s}");
        }
    }
}
