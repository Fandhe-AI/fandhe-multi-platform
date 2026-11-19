//! `run --os web` の配信（127.0.0.1 の静的ファイル配信）。外部プロセス（python 等）に頼らないのは、
//! 親が終了したときに子が残ってポートを握り続けるのを避けるため（cargo-fandhe と一緒に終わる）。
//!
//! 守る範囲: GET だけ・配信ディレクトリ配下の通常ファイルだけ（`..`・絶対パス・ドライブ文字・symlink は拒否）。
//! 守らない範囲: 同時接続の性能・Range・キャッシュ制御（開発用の確認配信で、127.0.0.1 の外へは開かない）。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

/// `dir` を 127.0.0.1 の空きポートで配る。URL を標準出力に出し、プロセスが終わるまで戻らない。
pub(super) fn serve(dir: &Path) -> Result<(), String> {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("serve の段を起動できません（待ち受けに失敗）: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("serve の段を起動できません: {e}"))?
        .port();
    println!("http://127.0.0.1:{port}/ で配信します（Ctrl-C で停止）");
    loop {
        match listener.accept() {
            Ok((mut conn, _)) => {
                let dir = dir.to_path_buf();
                // 1 接続ずつ別スレッド。ブラウザの先行接続（何も送らない接続）が他を止めないようにする。
                std::thread::spawn(move || {
                    let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
                    let _ = handle(&mut conn, &dir, port);
                });
            }
            // fd 枯渇などで失敗が続いても空回りしない。
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// 1 リクエストを読んで 1 レスポンスを返す。`Host` が `127.0.0.1:<port>`・`localhost:<port>` 以外なら 400
/// （DNS rebinding で外部ページから読まれるのを防ぐ）。
fn handle(conn: &mut (impl Read + Write), dir: &Path, port: u16) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    // ヘッダの終わりまで（上限 8 KiB）。本文は読まない。
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 8192 {
        let n = conn.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split(' ');
    let (method, target) = (first.next().unwrap_or(""), first.next().unwrap_or(""));
    let host_ok = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.eq_ignore_ascii_case("host"))
        .is_some_and(|(_, v)| {
            let v = v.trim();
            v == format!("127.0.0.1:{port}") || v == format!("localhost:{port}")
        });
    if !host_ok {
        return respond(conn, "400 Bad Request", "text/plain", b"400");
    }
    if method != "GET" {
        return respond(conn, "405 Method Not Allowed", "text/plain", b"405");
    }
    match resolve(dir, target) {
        Err(()) => respond(conn, "400 Bad Request", "text/plain", b"400"),
        Ok(path) => match std::fs::read(&path) {
            Ok(body) => respond(conn, "200 OK", content_type(&path), &body),
            Err(_) => respond(conn, "404 Not Found", "text/plain", b"404"),
        },
    }
}

fn respond(conn: &mut impl Write, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        conn,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    conn.write_all(body)
}

/// リクエストの対象（`/a/b.js?x#y`）を `dir` 配下の通常ファイルのパスにする。
/// 配信してはいけないパス（`..`・絶対パス・ドライブ文字・バックスラッシュ・NUL・symlink 経由・通常ファイルでない）は `Err`。
/// `/` と末尾 `/` は `index.html`。存在しないだけのパスは `Ok`（読めずに 404 になる）。
pub(super) fn resolve(dir: &Path, target: &str) -> Result<PathBuf, ()> {
    let path = target.split(['?', '#']).next().unwrap_or_default();
    let mut rel = percent_decode(path).ok_or(())?;
    if rel.contains(['\\', ':', '\0']) {
        return Err(());
    }
    rel = rel.trim_start_matches('/').to_string();
    if rel.is_empty() || rel.ends_with('/') {
        rel.push_str("index.html");
    }
    // 先に全要素を検査する（途中で存在しない要素があっても、後ろの `..` を見逃さない）。
    let segs: Vec<_> = Path::new(&rel)
        .components()
        .map(|c| match c {
            Component::Normal(seg) => Ok(seg),
            _ => Err(()),
        })
        .collect::<Result<_, _>>()?;
    let mut full = dir.to_path_buf();
    for seg in segs {
        full.push(seg);
        match std::fs::symlink_metadata(&full) {
            Ok(md) if md.file_type().is_symlink() => return Err(()),
            Ok(_) => {}
            Err(_) => return Ok(full),
        }
    }
    if full.is_file() {
        Ok(full)
    } else {
        Err(())
    }
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 拡張子から Content-Type。`.wasm` は `application/wasm`（ストリーミングコンパイルに必要）。
pub(super) fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("txt") => "text/plain; charset=utf-8",
        Some("otf") => "font/otf",
        Some("ttf") => "font/ttf",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の一時ディレクトリ。破棄時に消す。
    struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl AsRef<Path> for Tmp {
        fn as_ref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(name: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("fandhe-serve-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).expect("作成");
        std::fs::write(d.join("index.html"), "x").expect("書き込み");
        std::fs::write(d.join("sub/a.js"), "x").expect("書き込み");
        Tmp(d)
    }

    /// 入力を読ませ、書き出された応答を返す入出力。
    struct Pipe(std::io::Cursor<Vec<u8>>, Vec<u8>);

    impl Read for Pipe {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }

    impl Write for Pipe {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.1.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 1 リクエストを処理して応答の全文を返す。
    fn request(dir: &Path, raw: &str) -> String {
        let mut p = Pipe(std::io::Cursor::new(raw.as_bytes().to_vec()), Vec::new());
        handle(&mut p, dir, 4000).expect("処理");
        String::from_utf8_lossy(&p.1).into_owned()
    }

    #[test]
    fn handler_status_codes() {
        let d = tmp("http");
        let get = |t: &str, host: &str| format!("GET {t} HTTP/1.1\r\nHost: {host}\r\n\r\n");
        let status = |r: String| r.lines().next().unwrap_or_default().to_string();
        assert_eq!(
            status(request(&d, &get("/", "127.0.0.1:4000"))),
            "HTTP/1.1 200 OK"
        );
        assert_eq!(
            status(request(&d, &get("/sub/a.js", "localhost:4000"))),
            "HTTP/1.1 200 OK"
        );
        assert_eq!(
            status(request(&d, &get("/nope", "127.0.0.1:4000"))),
            "HTTP/1.1 404 Not Found"
        );
        assert_eq!(
            status(request(&d, &get("/../x", "127.0.0.1:4000"))),
            "HTTP/1.1 400 Bad Request"
        );
        assert_eq!(
            status(request(
                &d,
                "POST / HTTP/1.1\r\nhost: 127.0.0.1:4000\r\n\r\n"
            )),
            "HTTP/1.1 405 Method Not Allowed"
        );
        // Host が違う（DNS rebinding）・無い・ポート違いは本文を返さず 400。
        for host in [
            "evil.example:4000",
            "127.0.0.1:4001",
            "127.0.0.1",
            "localhost",
        ] {
            let r = request(&d, &get("/", host));
            assert_eq!(status(r.clone()), "HTTP/1.1 400 Bad Request", "{host}");
            assert!(!r.contains("\r\n\r\nx"), "{host}");
        }
        assert_eq!(
            status(request(&d, "GET / HTTP/1.1\r\n\r\n")),
            "HTTP/1.1 400 Bad Request"
        );
        let ok = request(&d, &get("/", "127.0.0.1:4000"));
        assert!(ok.contains("Content-Type: text/html; charset=utf-8"));
        assert!(ok.ends_with("\r\n\r\nx"));
    }

    #[test]
    fn resolves_index_and_files() {
        let d = tmp("ok");
        assert_eq!(resolve(&d, "/"), Ok(d.join("index.html")));
        assert_eq!(resolve(&d, "/?q=1"), Ok(d.join("index.html")));
        assert_eq!(resolve(&d, "/sub/a.js#h"), Ok(d.join("sub").join("a.js")));
        assert_eq!(resolve(&d, "/sub/%61.js"), Ok(d.join("sub").join("a.js")));
        // 存在しないパスは Ok（読めず 404）。ディレクトリそのものは拒否。
        assert!(resolve(&d, "/nope.js").is_ok());
        assert_eq!(resolve(&d, "/sub"), Err(()));
    }

    #[test]
    fn rejects_traversal_absolute_and_drive_paths() {
        let d = tmp("bad");
        for t in [
            "/../Cargo.toml",
            "/sub/../../x",
            "/%2e%2e/x",
            "/%2E%2E%2Fx",
            "/sub/..%2f..%2fx",
            "//etc/passwd/..",
            "/C:/Windows/win.ini",
            "/a%5Cb",
            "/a%00b",
            "/%zz",
            "/%ff",
            "/%2",
        ] {
            assert_eq!(resolve(&d, t), Err(()), "{t}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let d = tmp("link");
        let outside = tmp("outside");
        std::os::unix::fs::symlink(outside.join("index.html"), d.join("l.html")).expect("link");
        std::os::unix::fs::symlink(&outside, d.join("ldir")).expect("link");
        assert_eq!(resolve(&d, "/l.html"), Err(()));
        assert_eq!(resolve(&d, "/ldir/index.html"), Err(()));
    }

    #[test]
    fn content_types() {
        for (f, t) in [
            ("a.html", "text/html; charset=utf-8"),
            ("a.js", "text/javascript; charset=utf-8"),
            ("a.wasm", "application/wasm"),
            ("a.css", "text/css; charset=utf-8"),
            ("a.json", "application/json"),
            ("a.webmanifest", "application/manifest+json"),
            ("a.png", "image/png"),
            ("a.svg", "image/svg+xml"),
            ("a.ico", "image/x-icon"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.webp", "image/webp"),
            ("a.gif", "image/gif"),
            ("a.txt", "text/plain; charset=utf-8"),
            ("a.otf", "font/otf"),
            ("a.ttf", "font/ttf"),
            ("a.woff2", "font/woff2"),
            ("a.bin", "application/octet-stream"),
            ("noext", "application/octet-stream"),
        ] {
            assert_eq!(content_type(Path::new(f)), t, "{f}");
        }
    }
}
