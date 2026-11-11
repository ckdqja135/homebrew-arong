mod app;
mod art;
mod backup;
mod keytap;
mod otel;
mod sprites;
mod store;
mod update;
mod voice;

use objc2::MainThreadMarker;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

fn main() {
    let Some(mtm) = MainThreadMarker::new() else { return };
    // 디버그 빌드 전용 점검: Mac 로그인 항목 등록·해제가 실제로 되는지 확인하고 끝낸다.
    #[cfg(debug_assertions)]
    if std::env::var_os("ARONG_LOGIN_TEST").is_some() {
        use objc2_service_management::SMAppService;
        // SAFETY: 메인 앱 자신의 로그인 항목을 등록했다가 바로 해제한다.
        unsafe {
            let service = SMAppService::mainAppService();
            eprintln!("status before: {:?}", service.status());
            eprintln!("register: {:?}", service.registerAndReturnError().map_err(|e| e.localizedDescription().to_string()));
            eprintln!("status after register: {:?}", service.status());
            eprintln!("unregister: {:?}", service.unregisterAndReturnError().map_err(|e| e.localizedDescription().to_string()));
            eprintln!("status after unregister: {:?}", service.status());
        }
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    // Dock 아이콘 없이 메뉴 막대에만 상주한다.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let controller = app::Controller::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*controller)));
    app.run();
}
