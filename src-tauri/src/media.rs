//! Local media server for the editor's <video>/<audio>/<img> elements.
//!
//! Webviews differ in how well they stream media from custom URL schemes (WebKitGTK
//! refuses), so the app serves local files over plain HTTP on 127.0.0.1 instead:
//! loopback only, a random per-launch token in every URL, Range support for seeking,
//! and CORS headers so frames can be read back from the canvas.
//!
//! URL form: http://127.0.0.1:<port>/<token>/<percent-encoded absolute path>

use std::collections::hash_map::RandomState;
use std::fs::File;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub struct MediaServer {
    pub port: u16,
    pub token: String,
}

fn random_token() -> String {
    // RandomState is seeded from the OS RNG.
    (0..2)
        .map(|_| {
            let mut h = RandomState::new().build_hasher();
            h.write_u64(std::process::id() as u64);
            format!("{:016x}", h.finish())
        })
        .collect()
}

pub fn start() -> io::Result<MediaServer> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = random_token();
    let tk = token.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tk = tk.clone();
            std::thread::spawn(move || {
                let _ = handle(stream, &tk);
            });
        }
    });
    Ok(MediaServer { port, token })
}

fn mime(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        _ => "application/octet-stream",
    }
}

const CORS: &str = "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: Range\r\nAccess-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n";

fn simple(out: &mut TcpStream, code: &str, extra: &str) -> io::Result<()> {
    write!(out, "HTTP/1.1 {code}\r\n{CORS}{extra}Content-Length: 0\r\n\r\n")
}

fn handle(stream: TcpStream, token: &str) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(60)))?;
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        // request line + headers (keep-alive: several requests per connection)
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let target = parts.next().unwrap_or("").to_string();
        let mut range: Option<String> = None;
        let mut close = false;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h)? == 0 {
                return Ok(());
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let k = k.trim().to_ascii_lowercase();
                if k == "range" {
                    range = Some(v.trim().to_string());
                } else if k == "connection" && v.trim().eq_ignore_ascii_case("close") {
                    close = true;
                }
            }
        }
        if method == "OPTIONS" {
            simple(&mut out, "204 No Content", "")?;
            continue;
        }
        let path_part = target.split('?').next().unwrap_or("");
        let rest = path_part.trim_start_matches('/');
        let Some((tk, enc)) = rest.split_once('/') else {
            simple(&mut out, "404 Not Found", "")?;
            continue;
        };
        if tk != token {
            simple(&mut out, "403 Forbidden", "")?;
            return Ok(());
        }
        let path = percent_encoding::percent_decode_str(enc).decode_utf8_lossy().to_string();
        let Ok(mut f) = File::open(&path) else {
            simple(&mut out, "404 Not Found", "")?;
            continue;
        };
        let len = f.metadata()?.len();
        let head = method == "HEAD";
        let ctype = mime(&path);

        let parsed = range.as_deref().and_then(|r| r.strip_prefix("bytes=")).map(|r| {
            let r = r.split(',').next().unwrap_or("");
            let (a, b) = r.split_once('-').unwrap_or((r, ""));
            if a.is_empty() {
                let n: u64 = b.parse().unwrap_or(0).min(len);
                (len.saturating_sub(n), len.saturating_sub(1))
            } else {
                let s: u64 = a.parse().unwrap_or(0);
                let e: u64 = if b.is_empty() { len.saturating_sub(1) } else { b.parse::<u64>().unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1)) };
                (s, e)
            }
        });
        let conn = if close { "Connection: close\r\n" } else { "Connection: keep-alive\r\n" };
        match parsed {
            Some((s, e)) if len == 0 || s >= len || s > e => {
                simple(&mut out, "416 Range Not Satisfiable", &format!("Content-Range: bytes */{len}\r\n"))?;
            }
            Some((s, e)) => {
                let n = e - s + 1;
                write!(out, "HTTP/1.1 206 Partial Content\r\n{CORS}{conn}Content-Type: {ctype}\r\nAccept-Ranges: bytes\r\nContent-Range: bytes {s}-{e}/{len}\r\nContent-Length: {n}\r\n\r\n")?;
                if !head {
                    f.seek(SeekFrom::Start(s))?;
                    io::copy(&mut (&mut f).take(n), &mut out)?;
                }
            }
            None => {
                write!(out, "HTTP/1.1 200 OK\r\n{CORS}{conn}Content-Type: {ctype}\r\nAccept-Ranges: bytes\r\nContent-Length: {len}\r\n\r\n")?;
                if !head {
                    io::copy(&mut f, &mut out)?;
                }
            }
        }
        out.flush()?;
        if close {
            return Ok(());
        }
    }
}
