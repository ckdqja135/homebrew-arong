//! Claude Code 사용량 어댑터: 이 Mac에 남는 Claude Code 대화 기록 파일에서 토큰 수만 뽑는다.
//!
//! 지키는 것:
//! - 사용자가 따로 동의하고 켰을 때만 읽는다. 기본은 꺼짐이고, 끄면 바로 멈춘다.
//! - 읽는 곳은 `~/.claude/projects` 아래의 `.jsonl`뿐이다. 심볼릭 링크는 따라가지 않고,
//!   다른 폴더(설정, 인증 파일, 다른 앱의 데이터)는 열지 않는다.
//! - 기록 파일에는 대화 본문과 코드가 함께 들어 있다. 여기서는 답변 줄의 토큰 수 네 가지와
//!   시각, 중복 판별용 메시지 ID만 꺼내 쓰고, 나머지는 줄을 해석한 직후 버린다.
//!   본문·경로·세션 ID는 저장·로그·표시하지 않는다. 메시지 ID도 64비트 해시로만 남는다.
//! - 파일을 고치지 않는다. 외부로 보내는 통신은 없다.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use arong_core::ai;
use serde_json::Value;

/// `projects/<프로젝트>/<세션>/subagents/<파일>`까지만 내려간다.
const MAX_DEPTH: usize = 4;
const MAX_FILES: usize = 20_000;
/// 이보다 긴 줄은 해석하지 않고 넘긴다(답변 줄은 보통 훨씬 짧다).
const MAX_LINE: usize = 8 * 1024 * 1024;
const MAX_QUEUE: usize = 200_000;
/// 메시지 하나가 가질 수 있는 종류별 최대 토큰 수(이보다 크면 버린다).
const MAX_VALUE: u64 = 1_000_000_000;
/// 중복 판별 기록(`ai_series`)은 일주일 보관된다. 그보다 오래된 줄은 다시 세지 않도록 버린다.
const WINDOW_SECS: i64 = 6 * 86_400;

/// 공통 사용량 이벤트. 실제 측정값만 담는다.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    /// 메시지 ID·요청 ID·토큰 종류의 해시.
    pub key: i64,
    /// `arong_core::ai`의 종류 인덱스.
    pub kind: usize,
    /// 그 메시지에서 지금까지 확인된 토큰 수(같은 메시지가 여러 줄·여러 파일에 다시 나온다).
    pub value: u64,
    /// 읽은 시각(unix 나노초).
    pub time: i64,
}

static QUEUE: Mutex<Vec<Sample>> = Mutex::new(Vec::new());
/// 파일별로 어디까지 읽었는지. 앱이 떠 있는 동안만 기억한다.
static OFFSETS: Mutex<Option<HashMap<PathBuf, u64>>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);
/// 마지막으로 새 사용량을 찾은 시각(unix 초). 0이면 아직 없음.
static LAST_OK: AtomicI64 = AtomicI64::new(0);
/// 마지막으로 끝까지 훑은 읽기의 시작 시각(unix 초). 0이면 아직 없음.
static LAST_SCAN: AtomicI64 = AtomicI64::new(0);

pub fn take_samples() -> Vec<Sample> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

pub fn last_received() -> i64 {
    LAST_OK.load(Ordering::Relaxed)
}

pub fn last_scan() -> i64 {
    LAST_SCAN.load(Ordering::Relaxed)
}

/// 끌 때: 읽은 위치와 아직 반영하지 않은 표본을 모두 버린다.
pub fn reset() {
    if let Ok(mut offsets) = OFFSETS.lock() {
        *offsets = None;
    }
    take_samples();
    LAST_OK.store(0, Ordering::Relaxed);
    LAST_SCAN.store(0, Ordering::Relaxed);
}

fn root() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".claude/projects"))
}

/// Claude Code 기록 폴더가 이 Mac에 있는지.
pub fn available() -> bool {
    root().is_some_and(|r| r.is_dir())
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 새로 늘어난 부분을 별도 스레드에서 읽는다. 이미 읽는 중이면 아무것도 하지 않는다.
/// `since`(unix 초) 이전의 사용량은 세지 않는다. 처음 보는 파일은 `fresh` 이후에 바뀐 것만 연다.
pub fn scan_async(since: i64, fresh: i64) {
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new().name("arong-usage".into()).spawn(move || {
        let started = now_secs();
        if let Some(root) = root() {
            scan(&root, since.max(started - WINDOW_SECS), fresh, started);
        }
        LAST_SCAN.store(started, Ordering::Relaxed);
        BUSY.store(false, Ordering::Release);
    });
    if spawned.is_err() {
        BUSY.store(false, Ordering::Release);
    }
}

fn scan(root: &Path, since: i64, fresh: i64, now: i64) {
    let mut files = Vec::new();
    collect(root, 0, &mut files);
    let Ok(mut guard) = OFFSETS.lock() else { return };
    let offsets = guard.get_or_insert_with(HashMap::new);
    let mut found: HashMap<i64, (usize, u64)> = HashMap::new();
    for (path, len, modified) in &files {
        let start = match offsets.get(path) {
            // 파일이 줄어들었으면(교체됨) 처음부터 다시 본다. 중복은 메시지 ID로 걸러진다.
            Some(&at) if *len < at => 0,
            Some(&at) => at,
            None if *modified >= fresh => 0,
            None => *len,
        };
        let end = if *len > start { read_from(path, start, since, &mut found).unwrap_or(start) } else { start };
        offsets.insert(path.clone(), end);
    }
    // 지워진 파일의 위치 기록은 버린다.
    if offsets.len() > files.len() {
        let alive: std::collections::HashSet<&PathBuf> = files.iter().map(|f| &f.0).collect();
        offsets.retain(|p, _| alive.contains(p));
    }
    drop(guard);
    if found.is_empty() {
        return;
    }
    let time = now.saturating_mul(1_000_000_000);
    if let Ok(mut queue) = QUEUE.lock() {
        let room = MAX_QUEUE.saturating_sub(queue.len());
        queue.extend(found.into_iter().take(room).map(|(key, (kind, value))| Sample { key, kind, value, time }));
        LAST_OK.store(now, Ordering::Relaxed);
    }
}

/// `.jsonl` 파일의 (경로, 크기, 수정 시각)을 모은다. 링크는 따라가지 않는다.
fn collect(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64, i64)>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            if depth + 1 < MAX_DEPTH {
                collect(&path, depth + 1, out);
            }
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            let Ok(meta) = entry.metadata() else { continue };
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push((path, meta.len(), modified));
        }
    }
}

/// `start`부터 완성된 줄만 읽고, 다음에 이어 읽을 위치를 돌려준다.
fn read_from(path: &Path, start: u64, since: i64, found: &mut HashMap<i64, (usize, u64)>) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut at = start;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = reader.read_until(b'\n', &mut line).ok()?;
        // 아직 쓰는 중인 마지막 줄은 다음에 다시 읽는다.
        if n == 0 || line.last() != Some(&b'\n') {
            return Some(at);
        }
        at += n as u64;
        if line.len() <= MAX_LINE {
            parse_line(&line, since, found);
        }
    }
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// 답변 줄 하나에서 토큰 수만 꺼낸다. 같은 메시지는 종류별로 가장 큰 값만 남긴다.
fn parse_line(line: &[u8], since: i64, found: &mut HashMap<i64, (usize, u64)>) {
    // 사용량이 없는 줄(사용자 입력, 도구 결과 등)은 해석하지 않는다.
    if !contains(line, b"\"usage\"") || !contains(line, b"\"assistant\"") {
        return;
    }
    let Ok(root) = serde_json::from_slice::<Value>(line) else { return };
    if root.get("type").and_then(Value::as_str) != Some("assistant") {
        return;
    }
    let Some(time) = root.get("timestamp").and_then(Value::as_str).and_then(parse_time) else { return };
    if time < since {
        return;
    }
    let Some(message) = root.get("message") else { return };
    let Some(id) = message.get("id").and_then(Value::as_str) else { return };
    let Some(usage) = message.get("usage") else { return };
    let mut base = 0xcbf2_9ce4_8422_2325u64;
    fnv1a(&mut base, id.as_bytes());
    fnv1a(&mut base, b";");
    fnv1a(&mut base, root.get("requestId").and_then(Value::as_str).unwrap_or("").as_bytes());
    for (kind, name) in [
        (ai::INPUT, "input_tokens"),
        (ai::OUTPUT, "output_tokens"),
        (ai::CACHE_READ, "cache_read_input_tokens"),
        (ai::CACHE_CREATION, "cache_creation_input_tokens"),
    ] {
        let Some(value) = usage.get(name).and_then(Value::as_u64) else { continue };
        if value == 0 || value > MAX_VALUE {
            continue;
        }
        let mut hash = base;
        fnv1a(&mut hash, &[b'#', kind as u8]);
        let slot = found.entry(hash as i64).or_insert((kind, 0));
        slot.1 = slot.1.max(value);
    }
}

/// `2026-11-16T03:12:45.123Z` → unix 초. 형식이 다르면 `None`.
fn parse_time(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' || !s.ends_with('Z') {
        return None;
    }
    let num = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, min, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || h > 23 || min > 59 || sec > 60 {
        return None;
    }
    // 그레고리력 날짜 → 1970-01-01부터의 일수.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146_097 + doe - 719_468) * 86_400 + h * 3600 + min * 60 + sec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(id: &str, time: &str, output: u64) -> String {
        serde_json::json!({
            "type": "assistant",
            "timestamp": time,
            "requestId": "req_1",
            "sessionId": "private-session",
            "cwd": "/private/path",
            "message": {
                "id": id,
                "content": [{"type": "text", "text": "private words"}],
                "usage": {"input_tokens": 3, "output_tokens": output, "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 0},
            },
        })
        .to_string()
    }

    fn total(found: &HashMap<i64, (usize, u64)>, kind: usize) -> u64 {
        found.values().filter(|v| v.0 == kind).map(|v| v.1).sum()
    }

    #[test]
    fn parses_time() {
        assert_eq!(parse_time("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_time("2026-11-16T03:12:45.123Z"), Some(1_794_798_765));
        assert_eq!(parse_time("2024-02-29T23:59:59Z"), Some(1_709_251_199));
        assert_eq!(parse_time("yesterday"), None);
        assert_eq!(parse_time("2026-13-16T03:12:45.123Z"), None);
    }

    #[test]
    fn takes_only_token_counts_from_assistant_lines() {
        let mut found = HashMap::new();
        parse_line(line("msg_1", "2026-11-16T03:12:45.123Z", 40).as_bytes(), 0, &mut found);
        // 값이 0인 종류는 남기지 않는다.
        assert_eq!(found.len(), 3);
        assert_eq!(total(&found, ai::INPUT), 3);
        assert_eq!(total(&found, ai::OUTPUT), 40);
        assert_eq!(total(&found, ai::CACHE_READ), 1000);
        let user = r#"{"type":"user","timestamp":"2026-11-16T03:12:45.123Z","message":{"content":"\"usage\" \"assistant\""}}"#;
        parse_line(user.as_bytes(), 0, &mut found);
        parse_line(b"not json \"usage\" \"assistant\"", 0, &mut found);
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn repeated_message_keeps_the_largest_value() {
        let mut found = HashMap::new();
        // 스트리밍 중간 줄과 마지막 줄, 그리고 다른 파일에 복사된 줄.
        for output in [5, 40, 40, 5] {
            parse_line(line("msg_1", "2026-11-16T03:12:45.123Z", output).as_bytes(), 0, &mut found);
        }
        assert_eq!(total(&found, ai::OUTPUT), 40);
        parse_line(line("msg_2", "2026-11-16T03:12:50.000Z", 7).as_bytes(), 0, &mut found);
        assert_eq!(total(&found, ai::OUTPUT), 47);
    }

    #[test]
    fn skips_usage_before_the_start_time() {
        let mut found = HashMap::new();
        let since = parse_time("2026-11-16T00:00:00.000Z").unwrap();
        parse_line(line("msg_old", "2026-11-15T23:59:59.000Z", 40).as_bytes(), since, &mut found);
        assert!(found.is_empty());
        parse_line(line("msg_new", "2026-11-16T00:00:00.000Z", 40).as_bytes(), since, &mut found);
        assert_eq!(total(&found, ai::OUTPUT), 40);
    }

    #[test]
    fn reads_only_complete_lines_and_resumes() {
        let dir = std::env::temp_dir().join(format!("arong-usage-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("proj/session/subagents")).unwrap();
        let path = dir.join("proj/session/subagents/agent-a.jsonl");
        let first = line("msg_1", "2026-11-16T03:12:45.123Z", 40);
        let second = line("msg_2", "2026-11-16T03:12:50.000Z", 7);
        let mut file = File::create(&path).unwrap();
        // 두 번째 줄은 아직 쓰는 중이다.
        write!(file, "{first}\n{}", &second[..20]).unwrap();
        let mut found = HashMap::new();
        let at = read_from(&path, 0, 0, &mut found).unwrap();
        assert_eq!(at, first.len() as u64 + 1);
        assert_eq!(total(&found, ai::OUTPUT), 40);
        writeln!(file, "{}", &second[20..]).unwrap();
        let mut found = HashMap::new();
        read_from(&path, at, 0, &mut found).unwrap();
        assert_eq!(total(&found, ai::OUTPUT), 7);

        let mut files = Vec::new();
        fs::write(dir.join("proj/notes.txt"), "x").unwrap();
        collect(&dir, 0, &mut files);
        assert_eq!(files.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }
}
