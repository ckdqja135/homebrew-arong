//! 음성 명령. 이 파일이 마이크와 음성 인식을 다루는 유일한 곳이다.
//!
//! 지키는 것:
//! - 사용자가 음성 명령을 켜고, 훈련 화면에서 "말로 훈련"을 누른 동안만 마이크를 연다
//!   (다시 누르거나 30초 동안 명령이 없으면 닫는다).
//!   평소에는 마이크를 열지 않는다(`Listener`가 살아 있는 동안만 듣는다).
//! - 기기 안에서만 인식한다(`requiresOnDeviceRecognition`). 기기 안 인식을 지원하지 않는 Mac에서는 아예 쓰지 않는다.
//!   그래서 소리가 외부 서버로 나가지 않는다.
//! - 소리도, 인식된 문장도 저장하거나 기록하지 않는다. 다섯 가지 명령 중 어느 것인지, 또는 이름("아롱아")을
//!   불렀는지만 밖으로 내보낸다.
//! - 권한(마이크, 음성 인식)은 macOS가 사용자에게 직접 묻는다. 거부되면 듣지 않는다.

use std::panic::AssertUnwindSafe;
use std::ptr::NonNull;
use std::sync::Mutex;

use arong_core::{Command, Heard};
use block2::RcBlock;
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_avf_audio::{AVAudioEngine, AVAudioInputNode, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::{NSArray, NSBundle, NSError, NSLocale, NSString, ns_string};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognizer,
    SFSpeechRecognizerAuthorizationStatus,
};

/// 인식 결과로 찾은 명령(또는 이름 부르기). 메인 스레드가 가져간다.
static HEARD: Mutex<Option<Heard>> = Mutex::new(None);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    /// 아직 물어보지 않았다.
    NotAsked,
    Allowed,
    Denied,
}

/// 앱 번들에 마이크·음성 인식 사용 설명이 들어 있는지. 없으면 권한을 요청하는 순간 macOS가 앱을 종료시키므로 쓰지 않는다.
fn bundle_declares_usage() -> bool {
    let bundle = NSBundle::mainBundle();
    [ns_string!("NSMicrophoneUsageDescription"), ns_string!("NSSpeechRecognitionUsageDescription")]
        .iter()
        .all(|key| bundle.objectForInfoDictionaryKey(key).is_some())
}

fn recognizer() -> Option<Retained<SFSpeechRecognizer>> {
    let locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), ns_string!("ko-KR"));
    // SAFETY: 공개 초기화 메서드. 지원하지 않는 언어면 nil을 돌려준다.
    unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) }
}

/// 이 Mac에서 한국어를 기기 안에서 인식할 수 있는지.
pub fn supported() -> bool {
    // SAFETY: 상태 조회만 한다.
    bundle_declares_usage() && recognizer().is_some_and(|r| unsafe { r.supportsOnDeviceRecognition() })
}

pub fn permission() -> Permission {
    if !bundle_declares_usage() {
        return Permission::Denied;
    }
    // SAFETY: 상태 조회만 한다(창을 띄우지 않는다).
    match unsafe { SFSpeechRecognizer::authorizationStatus() } {
        SFSpeechRecognizerAuthorizationStatus::Authorized => Permission::Allowed,
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => Permission::NotAsked,
        _ => Permission::Denied,
    }
}

/// macOS의 음성 인식 권한 창을 띄운다. 사용자가 음성 명령을 켠 직후에만 부른다.
pub fn request_permission() {
    if !bundle_declares_usage() {
        return;
    }
    // 답은 `permission()`으로 다시 확인하므로 여기서는 받기만 한다.
    let done = RcBlock::new(|_status: SFSpeechRecognizerAuthorizationStatus| {});
    // SAFETY: 블록은 선언된 인자 형식 그대로다.
    unsafe { SFSpeechRecognizer::requestAuthorization(&done) };
}

/// 지금 쓸 수 있는 마이크(입력 장치)가 있는지. 장치 구성만 묻고 마이크를 열지 않는다.
/// 음성 인식 권한을 받은 뒤에만 부른다.
pub fn has_input() -> bool {
    if !bundle_declares_usage() || permission() != Permission::Allowed {
        return false;
    }
    // 입력 장치가 없으면 AVFoundation이 예외를 던질 수 있어서 잡아서 "없음"으로 본다.
    objc2::exception::catch(AssertUnwindSafe(|| unsafe {
        let engine = AVAudioEngine::new();
        let format = engine.inputNode().inputFormatForBus(0);
        format.channelCount() > 0 && format.sampleRate() > 0.0
    }))
    .unwrap_or(false)
}

/// 듣는 동안에만 존재한다. 없어지면 마이크와 인식을 즉시 닫는다.
pub struct Listener {
    engine: Retained<AVAudioEngine>,
    input: Retained<AVAudioInputNode>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
}

impl Listener {
    /// 마이크를 열고 인식을 시작한다. 실패하면 사용자에게 보여 줄 이유를 돌려준다.
    pub fn start() -> Result<Listener, &'static str> {
        if !bundle_declares_usage() {
            return Err("이 실행 방식에서는 음성을 쓸 수 없어요");
        }
        if permission() != Permission::Allowed {
            return Err("음성 인식 권한이 필요해요");
        }
        let recognizer = recognizer().ok_or("한국어 음성 인식을 쓸 수 없어요")?;
        // SAFETY: 상태 조회.
        if !unsafe { recognizer.supportsOnDeviceRecognition() && recognizer.isAvailable() } {
            return Err("이 Mac은 기기 안 음성 인식을 지원하지 않아요");
        }
        if let Ok(mut heard) = HEARD.lock() {
            *heard = None;
        }
        // SAFETY: 공개 API를 문서대로 쓴다. 요청은 반드시 기기 안 인식으로 제한한다.
        let request = unsafe {
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setRequiresOnDeviceRecognition(true);
            request.setShouldReportPartialResults(true);
            let mut words: Vec<Retained<NSString>> = Command::ALL.iter().map(|c| NSString::from_str(c.name())).collect();
            words.push(NSString::from_str("아롱아"));
            request.setContextualStrings(&NSArray::from_retained_slice(&words));
            request
        };
        let on_result = RcBlock::new(|result: *mut SFSpeechRecognitionResult, _error: *mut NSError| {
            // SAFETY: 콜백이 도는 동안 유효하거나 null이다.
            let Some(result) = (unsafe { result.as_ref() }) else { return };
            // 문장은 여기서 명령으로 바꾼 뒤 버린다(저장·기록하지 않는다).
            let text = unsafe { result.bestTranscription().formattedString() }.to_string();
            if let (Some(found), Ok(mut heard)) = (Heard::from_speech(&text), HEARD.lock()) {
                *heard = Some(found);
            }
        });
        // SAFETY: 블록은 선언된 인자 형식 그대로이고, 작업이 살아 있는 동안 프레임워크가 붙잡는다.
        let task = unsafe { recognizer.recognitionTaskWithRequest_resultHandler(&request, &on_result) };

        let engine = unsafe { AVAudioEngine::new() };
        let input = unsafe { engine.inputNode() };
        let feed = request.clone();
        let on_audio = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
            // SAFETY: 콜백이 도는 동안 버퍼는 유효하다. 소리는 인식 요청에만 넘기고 어디에도 남기지 않는다.
            unsafe { feed.appendAudioPCMBuffer(buffer.as_ref()) };
        });
        // 마이크가 없거나 권한이 없으면 AVFoundation이 예외를 던질 수 있어서 잡아서 실패로 돌린다.
        let started = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            let format = input.outputFormatForBus(0);
            input.installTapOnBus_bufferSize_format_block(0, 1024, Some(&format), RcBlock::as_ptr(&on_audio));
            engine.prepare();
            engine.startAndReturnError().is_ok()
        }));
        let listener = Listener { engine, input, request, task };
        match started {
            Ok(true) => Ok(listener),
            // 실패하면 `listener`가 여기서 버려지면서 열었던 것을 닫는다.
            _ => Err("마이크를 열 수 없어요 (권한을 확인해 주세요)"),
        }
    }

    /// 지금까지 알아들은 것이 있으면 가져간다.
    pub fn heard(&self) -> Option<Heard> {
        HEARD.lock().ok()?.take()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // 감시 해제: 마이크 탭을 떼고 엔진을 멈춘 뒤 인식을 취소한다.
        let _ = objc2::exception::catch(AssertUnwindSafe(|| unsafe {
            self.input.removeTapOnBus(0);
            self.engine.stop();
            self.request.endAudio();
            self.task.cancel();
        }));
        if let Ok(mut heard) = HEARD.lock() {
            *heard = None;
        }
    }
}
