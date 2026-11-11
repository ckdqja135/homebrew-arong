//! Claude Code 사용량 어댑터: OpenTelemetry 메트릭(OTLP/HTTP JSON)을 루프백으로 받는다.
//!
//! 지키는 것:
//! - 사용자가 켰을 때만 127.0.0.1의 한 포트에서 `POST /v1/metrics`만 받는다. 외부로 보내는 통신은 없다.
//! - `claude_code.token.usage` 메트릭의 숫자만 쓴다. 요청에 함께 오는 계정 이메일·계정 ID·세션 ID 등은
//!   시계열을 구분하는 64비트 해시로만 쓰이고, 원문은 저장·로그·표시하지 않는다.
//! - 앱이 만든 임의 토큰이 헤더에 없으면 거절한다. 크기·빈도·형식을 검증한다.
//! - 인증정보·대화 내용·파일을 읽지 않는다. Claude Code 설정도 건드리지 않는다(사용자가 직접 넣는다).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use arong_core::ai;
use serde_json::Value;

pub const DEFAULT_PORT: u16 = 47318;
const METRIC: &str = "claude_code.token.usage";
const MAX_HEADER: usize = 8 * 1024;
const MAX_BODY: usize = 256 * 1024;
const MAX_POINTS: usize = 2_000;
const MAX_QUEUE: usize = 4_096;
const MAX_REQUESTS_PER_SEC: u32 = 20;
/// 데이터 포인트 하나가 가질 수 있는 최대 토큰 수(이보다 크면 버린다).
const MAX_VALUE: f64 = 1e12;

/// 공통 사용량 이벤트. 실제 측정값만 담는다.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// 시계열(세션·모델·종류 등 속성 조합 + 시작 시각)의 해시.
    pub key: i64,
    /// `arong_core::ai`의 종류 인덱스.
    pub kind: usize,
    pub value: u64,
    /// 측정 시각(unix 나노초). 재전송·중복 판별에 쓴다.
    pub time: i64,
    /// true면 시작 이후 누적값, false면 직전 보고 이후 증가분.
    pub cumulative: bool,
}

static QUEUE: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
/// 마지막으로 유효한 사용량을 받은 시각(unix 초). 0이면 아직 없음.
static LAST_OK: AtomicI64 = AtomicI64::new(0);

pub fn take_samples() -> Vec<Sample> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

pub fn last_received() -> i64 {
    LAST_OK.load(Ordering::Relaxed)
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.as_i64(),
        _ => None,
    }
}

/// OTLP JSON 본문에서 토큰 사용량 포인트만 뽑는다. 형식이 다르면 `None`.
pub fn parse_metrics(body: &[u8]) -> Option<Vec<Sample>> {
    let root: Value = serde_json::from_slice(body).ok()?;
    let mut out = Vec::new();
    for resource in root.get("resourceMetrics")?.as_array()? {
        for scope in resource.get("scopeMetrics").and_then(Value::as_array).into_iter().flatten() {
            for metric in scope.get("metrics").and_then(Value::as_array).into_iter().flatten() {
                if metric.get("name").and_then(Value::as_str) != Some(METRIC) {
                    continue;
                }
                let Some(sum) = metric.get("sum") else { continue };
                let cumulative = match sum.get("aggregationTemporality") {
                    Some(Value::Number(n)) => n.as_i64() == Some(2),
                    Some(Value::String(s)) => s == "AGGREGATION_TEMPORALITY_CUMULATIVE",
                    _ => false,
                };
                for point in sum.get("dataPoints").and_then(Value::as_array).into_iter().flatten() {
                    if out.len() >= MAX_POINTS {
                        return Some(out);
                    }
                    if let Some(sample) = parse_point(point, cumulative) {
                        out.push(sample);
                    }
                }
            }
        }
    }
    Some(out)
}

fn parse_point(point: &Value, cumulative: bool) -> Option<Sample> {
    let attributes = point.get("attributes")?.as_array()?;
    let mut kind = None;
    // 속성 순서가 바뀌어도 같은 키가 나오도록 정렬해서 해시한다. 원문은 여기서 버려진다.
    let mut pairs: Vec<(String, String)> = Vec::with_capacity(attributes.len());
    for attribute in attributes.iter().take(64) {
        let name = attribute.get("key")?.as_str()?;
        let value = attribute.get("value")?;
        if name == "type" {
            kind = match value.get("stringValue").and_then(Value::as_str) {
                Some("input") => Some(ai::INPUT),
                Some("output") => Some(ai::OUTPUT),
                Some("cacheRead") => Some(ai::CACHE_READ),
                Some("cacheCreation") => Some(ai::CACHE_CREATION),
                _ => None,
            };
        }
        pairs.push((name.to_string(), value.to_string()));
    }
    pairs.sort();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (name, value) in &pairs {
        fnv1a(&mut hash, name.as_bytes());
        fnv1a(&mut hash, b"=");
        fnv1a(&mut hash, value.as_bytes());
        fnv1a(&mut hash, b";");
    }
    // 프로세스가 다시 시작하면 누적값이 0부터 다시 오므로 시작 시각도 키에 넣는다.
    let start = point.get("startTimeUnixNano").and_then(as_i64).unwrap_or(0);
    fnv1a(&mut hash, &start.to_le_bytes());

    let value = match (point.get("asInt"), point.get("asDouble")) {
        (Some(v), _) => as_i64(v)? as f64,
        (None, Some(v)) => v.as_f64()?,
        _ => return None,
    };
    if !value.is_finite() || !(0.0..=MAX_VALUE).contains(&value) {
        return None;
    }
    Some(Sample {
        key: hash as i64,
        kind: kind?,
        value: value as u64,
        time: point.get("timeUnixNano").and_then(as_i64)?,
        cumulative,
    })
}

pub struct Receiver {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Receiver {
    /// 루프백 전용 수신을 시작한다. 포트를 쓸 수 없으면 `None`.
    pub fn start(port: u16, token: String) -> Option<Receiver> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).ok()?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("arong-otel".into())
            .spawn(move || {
                let mut window = Instant::now();
                let mut in_window = 0u32;
                // accept에서 잠들어 있다가 연결이 올 때만 깬다(폴링 없음).
                for stream in listener.incoming() {
                    if flag.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(mut stream) = stream else { continue };
                    if window.elapsed() >= Duration::from_secs(1) {
                        window = Instant::now();
                        in_window = 0;
                    }
                    in_window += 1;
                    let status = if in_window > MAX_REQUESTS_PER_SEC {
                        429
                    } else {
                        handle(&mut stream, &token, port).unwrap_or(400)
                    };
                    respond(&mut stream, status);
                }
            })
            .ok()?;
        Some(Receiver { port, stop, thread: Some(thread) })
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // accept를 깨우기 위한 자기 자신으로의 연결.
        let _ = TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.port).into(),
            Duration::from_millis(500),
        );
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn respond(stream: &mut TcpStream, status: u16) {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        _ => "Bad Request",
    };
    // 오류 응답에 받은 내용을 되돌려 쓰지 않는다.
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
    );
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 요청 하나를 검증하고 사용량을 큐에 넣는다. 반환값은 HTTP 상태 코드.
fn handle(stream: &mut TcpStream, token: &str, port: u16) -> Option<u16> {
    if !stream.peer_addr().ok()?.ip().is_loopback() {
        return Some(401);
    }
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok()?;

    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEADER {
            return Some(413);
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..header_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request = lines.next()?.split(' ');
    let (method, path) = (request.next()?, request.next()?);
    if method != "POST" {
        return Some(405);
    }
    if path != "/v1/metrics" {
        return Some(404);
    }
    let (mut length, mut authorized, mut host_ok, mut json, mut encoded) = (None, false, false, false, false);
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match name.to_ascii_lowercase().as_str() {
            "content-length" => length = value.parse::<usize>().ok(),
            "authorization" => {
                authorized = value.strip_prefix("Bearer ").is_some_and(|t| constant_time_eq(t.as_bytes(), token.as_bytes()))
            }
            // 브라우저를 통한 DNS 리바인딩 요청을 걸러낸다.
            "host" => host_ok = value == format!("127.0.0.1:{port}") || value == format!("localhost:{port}"),
            "content-type" => json = value.to_ascii_lowercase().starts_with("application/json"),
            "content-encoding" => encoded = !value.eq_ignore_ascii_case("identity"),
            _ => {}
        }
    }
    if !authorized || !host_ok {
        return Some(401);
    }
    if !json || encoded {
        return Some(415);
    }
    let Some(length) = length else { return Some(411) };
    if length > MAX_BODY {
        return Some(413);
    }
    let mut body = buf[header_end..].to_vec();
    if body.len() > length {
        return Some(400);
    }
    while body.len() < length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > length {
            return Some(400);
        }
    }
    let samples = parse_metrics(&body)?;
    if !samples.is_empty() {
        let mut queue = QUEUE.lock().ok()?;
        let room = MAX_QUEUE.saturating_sub(queue.len());
        queue.extend(samples.into_iter().take(room));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        LAST_OK.store(now, Ordering::Relaxed);
    }
    Some(200)
}

/// 수신 인증용 임의 토큰(16바이트, 16진수). 사용자의 어떤 자격 증명과도 무관하다.
pub fn new_token() -> Option<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut bytes).ok()?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(temporality: i64, points: &[(&str, &str, f64, &str)]) -> Vec<u8> {
        let points: Vec<Value> = points
            .iter()
            .map(|(session, kind, value, time)| {
                serde_json::json!({
                    "startTimeUnixNano": "1000",
                    "timeUnixNano": time,
                    "asDouble": value,
                    "attributes": [
                        {"key": "user.email", "value": {"stringValue": "someone@example.com"}},
                        {"key": "session.id", "value": {"stringValue": session}},
                        {"key": "model", "value": {"stringValue": "claude-x"}},
                        {"key": "type", "value": {"stringValue": kind}},
                    ],
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({
            "resourceMetrics": [{"scopeMetrics": [{"metrics": [
                {"name": "claude_code.cost.usage", "sum": {"dataPoints": [{"asDouble": 1.5}]}},
                {"name": "claude_code.token.usage", "sum": {"aggregationTemporality": temporality, "dataPoints": points}},
            ]}]}],
        }))
        .unwrap()
    }

    #[test]
    fn parses_only_token_usage_points() {
        let body = payload(2, &[("s1", "input", 2.0, "2000"), ("s1", "output", 4.0, "2000"), ("s1", "cacheRead", 11383.0, "2000")]);
        let samples = parse_metrics(&body).unwrap();
        assert_eq!(samples.len(), 3);
        assert!(samples.iter().all(|s| s.cumulative && s.time == 2000));
        assert_eq!(samples[1].kind, ai::OUTPUT);
        assert_eq!(samples[2].value, 11383);
        // 종류가 다르면 다른 시계열.
        assert_ne!(samples[0].key, samples[1].key);
    }

    #[test]
    fn same_series_keeps_its_key_and_sessions_differ() {
        let a = parse_metrics(&payload(2, &[("s1", "output", 4.0, "2000")])).unwrap();
        let b = parse_metrics(&payload(2, &[("s1", "output", 9.0, "3000")])).unwrap();
        let c = parse_metrics(&payload(2, &[("s2", "output", 9.0, "3000")])).unwrap();
        assert_eq!(a[0].key, b[0].key);
        assert_ne!(a[0].key, c[0].key);
    }

    #[test]
    fn rejects_bad_shapes_and_values() {
        assert!(parse_metrics(b"not json").is_none());
        assert!(parse_metrics(b"{}").is_none());
        let body = payload(1, &[("s1", "mystery", 4.0, "2000"), ("s1", "output", -1.0, "2000"), ("s1", "output", 1e300, "2000")]);
        assert!(parse_metrics(&body).unwrap().is_empty());
        let delta = parse_metrics(&payload(1, &[("s1", "output", 4.0, "2000")])).unwrap();
        assert!(!delta[0].cumulative);
    }
}
