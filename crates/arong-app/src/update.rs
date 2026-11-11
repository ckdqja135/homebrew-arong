//! 새 버전 확인. 이 앱이 스스로 외부에 접속하는 유일한 곳이다.
//!
//! 지키는 것:
//! - 사용자가 켰을 때만, 하루에 한 번쯤, 공개 저장소의 cask 파일 하나를 읽는다.
//! - 보내는 것은 그 파일을 달라는 요청뿐이다. 펫 상태·타수·토큰 수 등 어떤 데이터도 붙이지 않는다.
//!   (접속 자체로 GitHub 쪽에 IP 주소가 남는 것은 피할 수 없다.)
//! - 쿠키·캐시·자격 증명을 쓰지 않는 일회용 세션으로 요청한다.
//! - 받은 내용에서는 버전 숫자만 뽑고, 내려받거나 실행하지 않는다. 업데이트는 사용자가 brew로 직접 한다.

use std::sync::Mutex;

use block2::RcBlock;
use objc2_foundation::{NSData, NSError, NSString, NSURL, NSURLResponse, NSURLSession, NSURLSessionConfiguration};

const CASK_URL: &str = "https://raw.githubusercontent.com/ckdqja135/homebrew-arong/main/Casks/arong.rb";
/// 이보다 큰 응답은 읽지 않는다. 정상 파일은 2KB 안팎이다.
const MAX_BYTES: usize = 64 * 1024;

/// 마지막 확인에서 읽어 온 최신 버전. 메인 스레드가 나중에 가져간다.
static LATEST: Mutex<Option<String>> = Mutex::new(None);

pub fn take_latest() -> Option<String> {
    LATEST.lock().ok()?.take()
}

/// cask 파일 본문에서 `version "1.2.3"` 줄의 값을 뽑는다. 숫자와 점만 허용한다.
pub fn parse_cask_version(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|l| l.starts_with("version \""))?;
    let value = line.strip_prefix("version \"")?.split('"').next()?;
    let ok = !value.is_empty() && value.len() <= 20 && value.split('.').all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()));
    ok.then(|| value.to_string())
}

/// `latest`가 `current`보다 높은 버전인지(점으로 나눈 숫자를 앞에서부터 비교).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u32> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() };
    let (a, b) = (parts(latest), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// 확인 요청을 보낸다. 결과는 백그라운드에서 `LATEST`에 들어간다(실패하면 아무 일도 없다).
pub fn check() {
    let Some(url) = NSURL::URLWithString(&NSString::from_str(CASK_URL)) else { return };
    let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
    config.setTimeoutIntervalForRequest(15.0);
    let session = NSURLSession::sessionWithConfiguration(&config);
    let done = RcBlock::new(|data: *mut NSData, _response: *mut NSURLResponse, error: *mut NSError| {
        // SAFETY: 완료 콜백이 도는 동안 시스템이 넘겨준 포인터는 유효하거나 null이다.
        let Some(data) = (unsafe { data.as_ref() }) else { return };
        if !error.is_null() || data.len() > MAX_BYTES {
            return;
        }
        let bytes = data.to_vec();
        let Ok(text) = std::str::from_utf8(&bytes) else { return };
        if let (Some(version), Ok(mut slot)) = (parse_cask_version(text), LATEST.lock()) {
            *slot = Some(version);
        }
    });
    // SAFETY: 블록은 위에서 선언한 인자 형식 그대로이고, 세션이 완료까지 붙잡는다.
    let task = unsafe { session.dataTaskWithURL_completionHandler(&url, &done) };
    task.resume();
    // 요청이 끝나면 세션을 정리하게 한다.
    session.finishTasksAndInvalidate();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_version_from_cask_text_only_when_well_formed() {
        let cask = "cask \"arong\" do\n  version \"0.2.1\"\n  sha256 \"abc\"\nend\n";
        assert_eq!(parse_cask_version(cask).as_deref(), Some("0.2.1"));
        assert_eq!(parse_cask_version("version \"1\"").as_deref(), Some("1"));
        for bad in ["", "no version here", "version \"\"", "version \"1.2.3-beta\"", "version \"$(rm -rf)\"", "version \"1..2\"", "version \"1234567.1\""] {
            assert_eq!(parse_cask_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.1.0.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
    }
}
