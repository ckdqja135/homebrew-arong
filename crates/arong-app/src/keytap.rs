//! 타건 수 집계용 읽기 전용 이벤트 탭. 이 파일이 키 입력을 다루는 유일한 곳이다.
//!
//! 지키는 것:
//! - keyDown "발생 횟수"만 센다. 키 코드·문자·입력 순서·시각은 읽지도 저장하지도 않는다.
//! - 콜백은 원자적 카운터를 1 올리는 일만 한다(파일·화면·로그 없음).
//! - 자동 반복과 Cmd/Ctrl 단축키는 세지 않는다.
//! - 읽기 전용(ListenOnly) 탭이라 입력을 바꾸거나 막을 수 없다.
//! - 사용자가 켰을 때만 만들고, 끄거나 권한이 사라지면 탭을 없앤다.
//! - Secure Input(비밀번호 입력 등)으로 전달되지 않는 입력을 우회하지 않는다.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use objc2_core_foundation::{CFMachPort, CFRetained, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventTapProxy, CGEventType, CGPreflightListenEventAccess, CGRequestListenEventAccess,
};

static COUNT: AtomicU64 = AtomicU64::new(0);
/// 탭이 시스템에 의해 꺼졌을 때 콜백에서 다시 켜기 위한 포인터. 탭 스레드가 사는 동안만 유효하다.
static PORT: AtomicPtr<CFMachPort> = AtomicPtr::new(std::ptr::null_mut());

/// 입력 모니터링 권한이 있는지 조용히 확인한다(창을 띄우지 않는다).
pub fn has_permission() -> bool {
    CGPreflightListenEventAccess()
}

/// 시스템 권한 요청 창을 띄운다. 사용자가 집계를 켠 직후에만 부른다.
pub fn request_permission() -> bool {
    CGRequestListenEventAccess()
}

/// 지난번 이후 쌓인 타건 수를 가져오고 0으로 되돌린다.
pub fn take_count() -> u64 {
    COUNT.swap(0, Ordering::Relaxed)
}

unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    _info: *mut c_void,
) -> *mut CGEvent {
    if kind == CGEventType::KeyDown {
        // SAFETY: 콜백이 도는 동안 시스템이 유효한 이벤트를 넘겨준다.
        let e = unsafe { event.as_ref() };
        let repeat = CGEvent::integer_value_field(Some(e), CGEventField::KeyboardEventAutorepeat) != 0;
        let shortcut = CGEvent::flags(Some(e)).intersects(CGEventFlags::MaskCommand | CGEventFlags::MaskControl);
        if !repeat && !shortcut {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
    } else if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        let port = PORT.load(Ordering::Acquire);
        // SAFETY: PORT는 탭 스레드가 포트를 무효화하기 전에 null로 되돌린다.
        if let Some(port) = unsafe { port.as_ref() } {
            CGEvent::tap_enable(port, true);
        }
    }
    event.as_ptr()
}

struct SendRunLoop(CFRetained<CFRunLoop>);
// SAFETY: CFRunLoop는 스레드 안전하며, 다른 스레드에서 CFRunLoopStop을 부르는 용도로만 쓴다.
unsafe impl Send for SendRunLoop {}

pub struct KeyTap {
    run_loop: SendRunLoop,
    thread: Option<JoinHandle<()>>,
}

impl KeyTap {
    /// 전용 스레드에 탭을 만든다. 권한이 없으면 `None`.
    pub fn start() -> Option<KeyTap> {
        if !has_permission() {
            return None;
        }
        let (tx, rx) = mpsc::channel::<Option<SendRunLoop>>();
        let thread = std::thread::Builder::new()
            .name("arong-keytap".into())
            .spawn(move || {
                let mask = 1u64 << CGEventType::KeyDown.0;
                // SAFETY: 콜백은 위의 `on_event`이고 user_info는 쓰지 않는다.
                let port = unsafe {
                    CGEvent::tap_create(
                        CGEventTapLocation::SessionEventTap,
                        CGEventTapPlacement::HeadInsertEventTap,
                        CGEventTapOptions::ListenOnly,
                        mask,
                        Some(on_event),
                        std::ptr::null_mut(),
                    )
                };
                let ready = port.as_ref().and_then(|port| {
                    let source = CFMachPort::new_run_loop_source(None, Some(port), 0)?;
                    let run_loop = CFRunLoop::current()?;
                    // SAFETY: 시스템이 제공하는 상수 모드 이름이다.
                    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
                    Some(run_loop)
                });
                let (Some(port), Some(run_loop)) = (port, ready) else {
                    let _ = tx.send(None);
                    return;
                };
                PORT.store(CFRetained::as_ptr(&port).as_ptr(), Ordering::Release);
                let _ = tx.send(Some(SendRunLoop(run_loop)));
                CFRunLoop::run();
                // 감시 해제: 포인터를 먼저 지우고 포트를 무효화한다.
                PORT.store(std::ptr::null_mut(), Ordering::Release);
                CGEvent::tap_enable(&port, false);
                port.invalidate();
            })
            .ok()?;
        match rx.recv() {
            Ok(Some(run_loop)) => Some(KeyTap { run_loop, thread: Some(thread) }),
            _ => {
                let _ = thread.join();
                None
            }
        }
    }
}

impl Drop for KeyTap {
    fn drop(&mut self) {
        self.run_loop.0.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
