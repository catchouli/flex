//! A local relay between mpv and YouTube's video servers.
//!
//! YouTube throttles a request for a whole stream to roughly 4 Mbit/s, about what 1080p
//! needs, so playback stalls every few seconds on some networks. Requests for pieces of
//! up to ~10 MB come at full speed (yt-dlp and YouTube's own player fetch that way). mpv
//! only knows how to ask for "from here to the end", so it asks this relay on 127.0.0.1
//! instead, which fetches from YouTube in pieces and passes the bytes on. Seeking works:
//! each seek is a new request for a different range.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Upstream piece size. YouTube serves up to 10 MiB per request at full speed.
pub const CHUNK: u64 = 10 << 20;
const USER_AGENT: &str = concat!("flex/", env!("CARGO_PKG_VERSION"));

#[derive(Clone)]
struct Upstream {
    url: String,
    /// Total size, once known.
    size: Option<u64>,
}

pub struct Proxy {
    port: u16,
    streams: Arc<Mutex<HashMap<u64, Upstream>>>,
    next: AtomicU64,
}

/// The relay, started on first use.
pub fn global() -> Result<&'static Proxy> {
    static PROXY: OnceLock<Result<Proxy, String>> = OnceLock::new();
    PROXY.get_or_init(|| Proxy::start().map_err(|e| format!("{e:#}"))).as_ref().map_err(|e| anyhow::anyhow!("{e}"))
}

impl Proxy {
    pub fn start() -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("starting the YouTube relay")?;
        let port = listener.local_addr()?.port();
        let streams: Arc<Mutex<HashMap<u64, Upstream>>> = Arc::default();
        let shared = Arc::clone(&streams);
        std::thread::Builder::new().name("youtube-relay".into()).spawn(move || {
            for conn in listener.incoming().flatten() {
                let streams = Arc::clone(&shared);
                std::thread::spawn(move || {
                    if let Err(e) = serve(conn, &streams) {
                        log::warn!("youtube relay: {e:#}");
                    }
                });
            }
        })?;
        log::info!("youtube relay on 127.0.0.1:{port}");
        Ok(Self { port, streams, next: AtomicU64::new(1) })
    }

    /// A local URL that relays `url` (whose size is `size`, if known).
    pub fn add(&self, url: &str, size: Option<u64>) -> String {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.streams.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Upstream { url: url.to_owned(), size });
        format!("http://127.0.0.1:{}/{id}", self.port)
    }
}

/// A parsed request: method, path, and the Range header's (start, end inclusive).
#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    range: Option<(u64, Option<u64>)>,
}

fn read_request(reader: &mut impl BufRead) -> Result<Request> {
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let mut parts = first.split_whitespace();
    let (Some(method), Some(path)) = (parts.next(), parts.next()) else { bail!("bad request line {first:?}") };
    let mut range = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("range")
        {
            range = parse_range(value.trim());
        }
    }
    Ok(Request { method: method.to_owned(), path: path.to_owned(), range })
}

/// "bytes=100-" or "bytes=100-199" (one range; that's all mpv asks for).
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let (start, end) = value.strip_prefix("bytes=")?.split_once('-')?;
    let start = start.trim().parse().ok()?;
    let end = end.trim();
    Some((start, if end.is_empty() { None } else { Some(end.parse().ok()?) }))
}

/// The total in "bytes 0-99/12345".
fn total_from_content_range(value: &str) -> Option<u64> {
    value.rsplit_once('/')?.1.trim().parse().ok()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Asks for `start..=end` upstream; returns the response once it's a good one.
fn fetch(agent: &ureq::Agent, url: &str, start: u64, end: u64) -> Result<ureq::http::Response<ureq::Body>> {
    let response = agent.get(url).header("User-Agent", USER_AGENT).header("Range", &format!("bytes={start}-{end}")).call()?;
    let status = response.status().as_u16();
    if status != 206 && status != 200 {
        bail!("YouTube answered {status} for bytes {start}-{end}");
    }
    Ok(response)
}

fn serve(conn: TcpStream, streams: &Mutex<HashMap<u64, Upstream>>) -> Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(30)))?;
    let mut reader = BufReader::new(conn.try_clone()?);
    let request = read_request(&mut reader)?;
    let mut out = conn;
    let id: Option<u64> = request.path.trim_start_matches('/').parse().ok();
    let Some(mut upstream) = id.and_then(|id| streams.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned()) else {
        out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
        return Ok(());
    };
    let agent = agent();
    // The size is needed for the headers; one byte from upstream tells it.
    let size = match upstream.size {
        Some(size) => size,
        None => {
            let response = match fetch(&agent, &upstream.url, 0, 0) {
                Ok(r) => r,
                Err(e) => {
                    out.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    return Err(e);
                }
            };
            let total = response.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(total_from_content_range);
            let size = total.context("YouTube didn't say how big the stream is")?;
            upstream.size = Some(size);
            if let Some(id) = id {
                streams.lock().unwrap_or_else(|e| e.into_inner()).insert(id, upstream.clone());
            }
            size
        }
    };
    let (start, end) = match request.range {
        Some((start, end)) => (start, end.unwrap_or(size.saturating_sub(1)).min(size.saturating_sub(1))),
        None => (0, size.saturating_sub(1)),
    };
    if start >= size {
        let head = format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{size}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        out.write_all(head.as_bytes())?;
        return Ok(());
    }
    let length = end - start + 1;
    let head = if request.range.is_some() {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\n\
             Content-Range: bytes {start}-{end}/{size}\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nAccept-Ranges: bytes\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n")
    };
    if request.method.eq_ignore_ascii_case("HEAD") {
        out.write_all(head.as_bytes())?;
        return Ok(());
    }
    // The reply only goes out once YouTube is sending, so a failure at the start is
    // retried before mpv sees anything, and reported to it as an error if it lasts.
    let mut head = Some(head);
    let mut pos = start;
    let mut buf = vec![0; 256 * 1024];
    // Upstream hiccups (a failed request, a connection dropped mid-piece) are retried
    // from the exact byte reached, so mpv never notices.
    let mut failures = 0;
    while pos <= end {
        let piece_end = (pos + CHUNK - 1).min(end);
        let before = pos;
        let result = (|| -> Result<()> {
            let mut response = fetch(&agent, &upstream.url, pos, piece_end)?;
            let mut body = response.body_mut().as_reader();
            loop {
                let n = body.read(&mut buf)?;
                if n == 0 {
                    return Ok(());
                }
                // mpv closing the connection (a seek, or stopping) ends this response.
                let to_mpv = |out: &mut TcpStream, bytes: &[u8]| out.write_all(bytes).map_err(|e| anyhow::anyhow!(Downstream(e)));
                if let Some(head) = head.take() {
                    to_mpv(&mut out, head.as_bytes())?;
                }
                to_mpv(&mut out, &buf[..n])?;
                pos += n as u64;
            }
        })();
        match result {
            Err(e) if e.is::<Downstream>() => return Ok(()),
            Ok(()) if pos > before => failures = 0,
            other => {
                failures += 1;
                let why = other.err().map_or_else(|| "nothing sent".to_owned(), |e| format!("{e:#}"));
                if failures > MAX_FAILURES {
                    if head.is_some() {
                        let _ = out.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                    bail!("giving up at byte {pos}: {why}");
                }
                log::info!("youtube relay: retrying from byte {pos} ({why})");
                std::thread::sleep(Duration::from_millis(300 * failures));
            }
        }
    }
    Ok(())
}

/// Retries in a row (without progress) before giving up on a stream.
const MAX_FAILURES: u64 = 4;

/// Writing to mpv failed: it went away, which is normal (seeks, stopping).
#[derive(Debug)]
struct Downstream(std::io::Error);

impl std::fmt::Display for Downstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mpv went away: {}", self.0)
    }
}

impl std::error::Error for Downstream {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=100-199"), Some((100, Some(199))));
        assert_eq!(parse_range("items=1-2"), None);
        assert_eq!(total_from_content_range("bytes 0-0/168736189"), Some(168_736_189));
        let mut req = std::io::Cursor::new(b"GET /7 HTTP/1.1\r\nHost: x\r\nRange: bytes=5-\r\n\r\n".to_vec());
        assert_eq!(read_request(&mut req).unwrap(), Request { method: "GET".into(), path: "/7".into(), range: Some((5, None)) });
    }

    /// A fake YouTube: serves `data` with Range support, refusing pieces over CHUNK (as
    /// a stand-in for throttling), and records the ranges it was asked for. With `flaky`,
    /// its first full piece is cut off halfway.
    /// The (start, end) ranges a fake upstream was asked for.
    type Asked = Arc<Mutex<Vec<(u64, u64)>>>;

    fn fake_upstream(data: Arc<Vec<u8>>, flaky: bool) -> (String, Asked) {
        let cut_next = Arc::new(std::sync::atomic::AtomicBool::new(flaky));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let url = format!("http://127.0.0.1:{}/videoplayback", listener.local_addr().unwrap().port());
        let asked: Asked = Arc::default();
        let log = Arc::clone(&asked);
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (data, log, cut_next) = (Arc::clone(&data), Arc::clone(&log), Arc::clone(&cut_next));
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(conn.try_clone().unwrap());
                    let req = read_request(&mut reader).unwrap();
                    let (start, end) = req.range.map_or((0, data.len() as u64 - 1), |(s, e)| (s, e.unwrap_or(data.len() as u64 - 1)));
                    log.lock().unwrap().push((start, end));
                    let mut out = conn;
                    if end - start + 1 > CHUNK {
                        let _ = out.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n");
                        return;
                    }
                    let body = &data[start as usize..=end as usize];
                    let cut = body.len() > 1 && cut_next.swap(false, Ordering::Relaxed);
                    let head = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len(),
                        body.len()
                    );
                    let sent = if cut { &body[..body.len() / 2] } else { body };
                    let _ = out.write_all(head.as_bytes()).and_then(|()| out.write_all(sent));
                });
            }
        });
        (url, asked)
    }

    #[test]
    fn relays_in_pieces_and_seeks() {
        let data: Arc<Vec<u8>> = Arc::new((0..(25u64 << 20) + 12_345).map(|i| (i * 7 % 251) as u8).collect());
        let (upstream, asked) = fake_upstream(Arc::clone(&data), false);
        let proxy = Proxy::start().unwrap();
        // Size unknown: the relay asks upstream for it.
        let local = proxy.add(&upstream, None);
        let client: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();

        // The whole stream, as mpv first asks for it.
        let mut response = client.get(&local).header("Range", "bytes=0-").call().unwrap();
        assert_eq!(response.status().as_u16(), 206);
        let range = response.headers().get("content-range").unwrap().to_str().unwrap().to_owned();
        assert_eq!(range, format!("bytes 0-{}/{}", data.len() - 1, data.len()));
        let body = response.body_mut().with_config().limit(u64::MAX).read_to_vec().unwrap();
        assert!(body == *data, "bytes differ");
        assert!(asked.lock().unwrap().iter().all(|(s, e)| e - s < CHUNK), "every upstream request is one piece");

        // A seek: from the middle of a piece to the end.
        let from = (12u64 << 20) + 99;
        let mut response = client.get(&local).header("Range", &format!("bytes={from}-")).call().unwrap();
        let body = response.body_mut().with_config().limit(u64::MAX).read_to_vec().unwrap();
        assert!(body == data[from as usize..], "seeked bytes differ");

        // Past the end, and an unknown stream.
        let past = client.get(&local).header("Range", &format!("bytes={}-", data.len())).call().unwrap();
        assert_eq!(past.status().as_u16(), 416);
        let unknown = format!("{}/999999", local.rsplit_once('/').unwrap().0);
        assert_eq!(client.get(&unknown).call().unwrap().status().as_u16(), 404);
    }

    #[test]
    fn resumes_when_youtube_drops_a_piece() {
        let data: Arc<Vec<u8>> = Arc::new((0..(15u64 << 20)).map(|i| (i * 13 % 251) as u8).collect());
        let (upstream, asked) = fake_upstream(Arc::clone(&data), true);
        let proxy = Proxy::start().unwrap();
        let local = proxy.add(&upstream, Some(data.len() as u64));
        let mut response = ureq::get(&local).header("Range", "bytes=0-").call().unwrap();
        let body = response.body_mut().with_config().limit(u64::MAX).read_to_vec().unwrap();
        assert!(body == *data, "bytes differ after resuming");
        // The cut piece was asked for again from where it stopped (half of the first piece).
        let half = CHUNK / 2;
        assert!(asked.lock().unwrap().iter().any(|(s, _)| *s == half), "{:?}", asked.lock().unwrap());
    }

    #[test]
    fn reports_youtube_failing_at_the_start() {
        // Nothing listens here: every upstream request fails.
        let dead = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let url = format!("http://127.0.0.1:{}/videoplayback", dead.local_addr().unwrap().port());
        drop(dead);
        let proxy = Proxy::start().unwrap();
        let local = proxy.add(&url, Some(1000));
        let client: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let response = client.get(&local).header("Range", "bytes=0-").call().unwrap();
        assert_eq!(response.status().as_u16(), 502, "an error, not an empty 206");
    }

    /// Real network: relays a real stream and checks the speed.
    /// `cargo test -p flex-core relays_youtube_fast -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn relays_youtube_fast() {
        let tools = super::super::Tools::find().expect("yt-dlp");
        let video = tools.resolve("aqz-KE-bpKQ", 1080, false).unwrap();
        let local = global().unwrap().add(&video.video_url, video.video_size);
        let started = std::time::Instant::now();
        let response = ureq::get(&local).header("Range", "bytes=0-").call().unwrap();
        let mut reader = response.into_body().into_reader().take(30 << 20);
        let n = std::io::copy(&mut reader, &mut std::io::sink()).unwrap();
        let rate = n as f64 / started.elapsed().as_secs_f64() / 1024.0;
        eprintln!("relayed {} MB at {rate:.0} KB/s", n >> 20);
        assert!(rate > 1500.0, "{rate:.0} KB/s is about the throttled speed");
    }
}
