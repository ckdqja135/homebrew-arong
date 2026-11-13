//! AppKit 껍데기: 펫 패널, 메뉴 막대, 목줄 오버레이, 타이머.
//! 전부 메인 스레드에서만 돈다. v0.1에는 키보드·AI 사용량 집계가 없고,
//! 전역 이벤트 감시도 쓰지 않는다. 마우스는 펫 창이 받는 클릭·드래그와,
//! 산책 모드 중에만 30Hz로 읽는 커서 위치(권한 불필요)뿐이다.

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use arong_core::{ActionResult, Command, Follower, Pet, Purchase, Slot, Stage, Tuning, Vec2, WalkMeter, WalkTuning};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, Sel};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationDelegate, NSBackingStoreType, NSCellImagePosition, NSColor, NSEvent, NSFont,
    NSImage, NSImageSymbolConfiguration, NSImageView,
    NSFloatingWindowLevel, NSMenu, NSMenuDelegate, NSMenuItem, NSNormalWindowLevel, NSOpenPanel, NSPanel, NSPasteboard, NSSavePanel, NSPasteboardTypeString,
    NSResponder, NSScreen, NSScrollView, NSStatusBar, NSStatusItem, NSTextAlignment, NSTextField,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
    NSWindowCollectionBehavior, NSWindowDelegate, NSWindowOcclusionState, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_core_foundation::{CFRetained, CGAffineTransform, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath};
use objc2_foundation::{
    NSCalendar, NSCalendarUnit, NSDate, NSNotification, NSNotificationCenter, NSObject,
    NSProcessInfo, NSRunLoop, NSString, NSTimer, ns_string,
};
use objc2_quartz_core::{CALayer, CAShapeLayer, CATransaction};

use crate::sprites::{Atlas, CELL, GROUND, Motion, View};
use crate::store::{OpenOutcome, Store};

/// 제자리 동작의 초당 프레임 수.
const FPS: f64 = 9.0;
const WALK_HZ: f64 = 30.0;
/// 집에 들어가거나 나오는 데 걸리는 시간(초).
const DOORWAY_SECS: f64 = 1.1;
/// 말로 훈련하는 동안, 마지막 명령 뒤로 이만큼(초) 아무 명령이 없으면 마이크를 닫는다.
const VOICE_IDLE_SECS: f64 = 30.0;
/// 가장 깊이 들어갔을 때 줄어드는 정도와, 흐려지기 시작하는 진행도.
const DOORWAY_SHRINK: f64 = 0.68;
const DOORWAY_FADE_FROM: f64 = 0.55;
/// 이 거리(pt) 미만으로 움직이고 놓으면 드래그가 아니라 쓰다듬기.
const CLICK_SLOP: f64 = 4.0;
/// 걷기 한 사이클 동안 나아가는 거리(스프라이트 px 기준).
const WALK_CYCLE_PT: f64 = 62.0;
/// 머리 위 여유(스프라이트 px). 가장 높은 모자가 가장 키 큰 단계에서도 들어가는 높이.
const HEAD_ROOM_PX: f64 = 22.0;
/// 그림을 얹는 부위(겹치는 순서대로). 옷·신발은 스프라이트에 직접 칠한다.
const ACC_SLOTS: [Slot; 3] = [Slot::Neck, Slot::Glasses, Slot::Hat];
const ROAM_HZ: f64 = 20.0;
/// 혼자 걷는 속도(pt/s).
const ROAM_SPEED: f64 = 45.0;
/// 산책 모드에서 커서가 이만큼(초) 멈춰 있으면 자동 종료.
const WALK_STILL_LIMIT: f64 = 20.0;
const WALK_MAX_SECS: f64 = 15.0 * 60.0;

thread_local! {
    static CONTROLLER: OnceCell<Retained<Controller>> = const { OnceCell::new() };
}

fn controller() -> Option<Retained<Controller>> {
    CONTROLLER.with(|c| c.get().cloned())
}

/// 설정 화면에서 바꿀 수 있는 보상 규칙 한 줄.
struct Tunable {
    label: &'static str,
    /// 설정 저장용 키.
    key: &'static str,
    min: i64,
    max: i64,
    get: fn(&Tuning) -> i64,
    set: fn(&mut Tuning, i64),
}

fn tunables() -> [Tunable; 7] {
    [
        Tunable {
            label: "타이핑: 1 XP에 필요한 타수",
            key: "tune_typing_keys_per_xp",
            min: 10,
            max: 100_000,
            get: |t| t.typing_keys_per_xp as i64,
            set: |t, v| t.typing_keys_per_xp = v as u64,
        },
        Tunable {
            label: "타이핑: 하루 100% 보상 구간 (타)",
            key: "tune_typing_full_keys",
            min: 0,
            max: 1_000_000,
            get: |t| t.typing_full_keys as i64,
            // 50% 구간은 100% 구간의 3배 지점까지로 둔다.
            set: |t, v| {
                t.typing_full_keys = v as u64;
                t.typing_half_keys = v as u64 * 3;
            },
        },
        Tunable {
            label: "AI: 1 XP에 필요한 포인트",
            key: "tune_ai_points_per_xp",
            min: 10,
            max: 10_000_000,
            get: |t| t.ai_points_per_xp as i64,
            set: |t, v| t.ai_points_per_xp = v as u64,
        },
        Tunable {
            label: "AI: 하루 100% 보상 구간 (포인트)",
            key: "tune_ai_full_points",
            min: 0,
            max: 100_000_000,
            get: |t| t.ai_full_points as i64,
            set: |t, v| {
                t.ai_full_points = v as u64;
                t.ai_half_points = v as u64 * 3;
            },
        },
        Tunable {
            label: "산책: 걸은 거리 1,000pt당 XP",
            key: "tune_walk_xp_per_unit",
            min: 0,
            max: 100,
            get: |t| t.walk_xp_per_unit as i64,
            set: |t, v| t.walk_xp_per_unit = v as u64,
        },
        Tunable {
            label: "간식 가격 (재화)",
            key: "tune_snack_price",
            min: 0,
            max: 10_000,
            get: |t| t.snack_price,
            set: |t, v| t.snack_price = v,
        },
        Tunable {
            label: "얻은 XP 대비 재화 (%)",
            key: "tune_coin_percent",
            min: 0,
            max: 200,
            get: |t| t.coin_per_xp_permille as i64 / 10,
            set: |t, v| t.coin_per_xp_permille = v as u64 * 10,
        },
    ]
}

/// yyyymmdd → "11/11".
fn short_date(date: u32) -> String {
    format!("{}/{}", date / 100 % 100, date % 100)
}

/// 제자리에서 하는 행동. 프레임 목록으로 풀어서 재생한다.
#[derive(Clone, Copy)]
enum Act {
    /// 발 주기(true = 왼발).
    Paw(bool),
    Sit,
    SitUp,
    /// 엎드려서 잠깐 쉬었다 일어난다.
    Nap,
    /// 엎드려서 배를 보이며 구르고 일어난다.
    Roll,
    /// 앉아서 밥그릇 앞에서 먹는다.
    Eat,
    /// 엎드린 채로 머문다(방석 위, "엎드려").
    LieStay,
    /// 앉은 채로 머문다("앉아").
    SitStay,
    /// 서서 혀를 내밀고 헥헥거린다.
    Pant,
    /// 정면을 바라본다.
    Look,
}

enum Drag {
    None,
    /// 눌렀지만 아직 드래그로 판정되지 않음. 그대로 놓으면 쓰다듬기.
    Pressed { start: CGPoint },
    /// 위치 고정 중이거나 산책을 끝내는 클릭이라 무시.
    Ignored,
    /// 끌어서 창 자체를 옮긴다(산책 아님, 보상 없음).
    Moving { offset: CGPoint },
}

/// 산책 모드: 버튼을 누르고 있지 않아도 커서를 목줄로 따라간다.
struct Walk {
    follower: Follower,
    meter: WalkMeter,
    last: Instant,
    last_cursor: CGPoint,
    /// 커서가 멈춰 있던 시간(초). 오래 멈추면 산책을 끝낸다.
    still: f64,
    total: f64,
}

/// 집 드나들기: 문 앞과 문 안쪽 사이를 걸으면서 작아지고 흐려진다(또는 그 반대).
struct Doorway {
    /// 문 앞에 서 있을 때의 창 원점과, 문 안쪽 깊숙이 들어갔을 때의 창 원점.
    front: CGPoint,
    deep: CGPoint,
    /// 들어가는 중이면 true, 나오는 중이면 false.
    entering: bool,
    /// 진행도 0(문 앞) ~ 1(안쪽).
    depth: f64,
    last: Instant,
}

/// 혼자 걷기: 대기 중 가끔 좌우로 조금 걸어간다.
struct Roam {
    target: CGPoint,
    /// 집에 도착했을 때처럼, 다 가서 앉는다.
    sit_on_arrival: bool,
    last: Instant,
}

enum TileIcon {
    /// SF Symbols 이름 후보.
    Symbol(&'static [&'static str]),
    /// 코드로 그린 도트 그림.
    Image(Option<Retained<NSImage>>),
}

#[derive(Clone)]
enum TileAction {
    /// 컨트롤러의 액션 셀렉터를 그대로 부른다.
    Send(Sel),
    More,
    Shop,
    MainMenu,
    /// 훈련 화면, 명령 하나 훈련하기, 말로 명령하기.
    Training,
    Train(Command),
    Voice,
    /// 상점의 부위별 목록.
    Category(Slot),
    Item(&'static str),
    House,
}

struct TileSpec {
    icon: TileIcon,
    caption: String,
    enabled: bool,
    active: bool,
    action: TileAction,
}

/// 상점 타일에 보여 줄 아이템 그림. 옷은 지금 단계의 아롱이가 입은 모습을, 신발은 색 견본을 보여 준다.
fn item_icon(id: &str, stage: arong_core::Stage) -> TileIcon {
    if let Some(acc) = crate::art::accessory_art(id) {
        return art_icon(Some(acc.art), 2.0);
    }
    if let Some(thumb) = crate::sprites::outfit_thumb(id, stage) {
        // 타일의 그림 칸(가로 84, 세로 40pt) 안에 들어가게 맞춘다.
        let fit = (84.0 / thumb.w as f64).min(40.0 / thumb.h as f64);
        return art_icon(Some(thumb), fit);
    }
    let color = crate::art::wear_color(id);
    if arong_core::shop::item(id).is_some_and(|def| def.slot == Slot::Shoes) {
        return art_icon(color.map(crate::art::boot_icon), 2.5);
    }
    art_icon(color.map(crate::art::swatch), 2.0)
}

/// 도트 그림을 타일 아이콘으로. `pt_per_px`는 그림 1px이 차지할 포인트.
fn art_icon(art: Option<crate::art::Art>, pt_per_px: f64) -> TileIcon {
    let image = art.and_then(|a| {
        let size = CGSize::new(a.w as f64 * pt_per_px, a.h as f64 * pt_per_px);
        // 레티나에서도 도트가 또렷하도록 미리 정수배로 키워 둔다.
        let rep = a.scaled(4).to_rep()?;
        rep.setSize(size);
        let image = NSImage::initWithSize(NSImage::alloc(), size);
        image.addRepresentation(&rep);
        Some(image)
    });
    TileIcon::Image(image)
}

struct Overlay {
    window: Retained<NSWindow>,
    leash: Retained<CAShapeLayer>,
    origin: CGPoint,
}

pub struct Ivars {
    /// 보상 규칙. 설정 화면에서 바꿀 수 있다.
    tuning: RefCell<Tuning>,
    walk_tuning: WalkTuning,
    pet: RefCell<Pet>,
    store: Option<Store>,
    notice: Cell<Option<&'static str>>,
    panel: OnceCell<Retained<NSPanel>>,
    /// 좌우 반전을 담당하는 빈 컨테이너. 몸 그림과 장식이 그 자식이다.
    sprite: OnceCell<Retained<CALayer>>,
    /// 몸 그림. 프레임마다 그림이 있는 영역 크기로만 놓는다.
    body: OnceCell<Retained<CALayer>>,
    /// 걸음의 (진행 위상 0~1, 예비).
    hop: Cell<(f64, f64)>,
    status_item: OnceCell<Retained<NSStatusItem>>,
    menu: OnceCell<Retained<NSMenu>>,
    atlas: RefCell<Option<Atlas>>,
    /// 표시 크기: 0 = 작게, 1 = 중간, 2 = 크게.
    size_level: Cell<u8>,
    always_on_top: Cell<bool>,
    locked: Cell<bool>,
    hidden: Cell<bool>,
    paused: Cell<bool>,
    /// 디스플레이가 꺼졌거나 세션이 비활성: 모든 타이머를 멈춘다.
    suspended: Cell<bool>,
    motion: Cell<Motion>,
    frame: Cell<usize>,
    /// 앞으로 보여 줄 (동작, 프레임) 목록. 타이머가 하나씩 꺼내 쓴다.
    script: RefCell<VecDeque<(Motion, usize)>>,
    /// 목록이 끝나면 서지 않고 마지막 자세(엎드림)로 머문다 / 지금 그렇게 머무는 중이다.
    stay_after: Cell<bool>,
    held: Cell<bool>,
    /// 밥그릇을 보여 주는 중인지.
    bowl: Cell<bool>,
    bowl_layer: OnceCell<Retained<CALayer>>,
    bowl_size: Cell<(f64, f64)>,
    /// 집 안에 들어가 있어 보이지 않는 상태.
    inside: Cell<bool>,
    /// 집을 드나드는 중의 상태와 타이머, 그리고 그동안의 몸 크기 비율(평소 1).
    doorway: RefCell<Option<Doorway>>,
    door_timer: RefCell<Option<Retained<NSTimer>>>,
    shrink: Cell<f64>,
    facing_left: Cell<bool>,
    anim_timer: RefCell<Option<Retained<NSTimer>>>,
    idle_timer: RefCell<Option<Retained<NSTimer>>>,
    walk_timer: RefCell<Option<Retained<NSTimer>>>,
    drag: RefCell<Drag>,
    overlays: RefCell<Vec<Overlay>>,
    walk: RefCell<Option<Walk>>,
    roam: RefCell<Option<Roam>>,
    roam_timer: RefCell<Option<Retained<NSTimer>>>,
    roaming: Cell<bool>,
    /// 닫은 뒤에도 다음에 열 때까지 들고 있는다(자기 이벤트 처리 중 해제 방지).
    tiles: RefCell<Option<Retained<TilePanel>>>,
    tiles_trash: RefCell<Vec<Retained<TilePanel>>>,
    tile_actions: RefCell<Vec<TileAction>>,
    tiles_open: Cell<bool>,
    /// 방금 메뉴를 띄운 자리와, 사용자가 끌어다 놓은 자리(왼쪽 위 모서리).
    /// 끌어다 놓은 뒤에는 하위 메뉴로 넘어가도 그 자리에 띄운다. 아롱이를 다시 우클릭하면 잊는다.
    tiles_placed: Cell<CGPoint>,
    /// 지금 메뉴의 스크롤 뷰(줄이 많을 때만)와 타일 수. 같은 목록을 다시 그릴 때 스크롤 위치를 잇는 데 쓴다.
    tiles_scroll: RefCell<Option<Retained<NSScrollView>>>,
    tiles_count: Cell<usize>,
    tiles_anchor: Cell<Option<CGPoint>>,
    /// 상점 타일에 마우스를 올려 미리 입어 보는 중인 아이템(부위, id). 화면에만 보이고 저장되지 않는다.
    preview: RefCell<Option<(Slot, String)>>,
    bubble: RefCell<Option<Retained<NSWindow>>>,
    bubble_timer: RefCell<Option<Retained<NSTimer>>>,
    /// 얹는 장식 레이어. 아래부터 목 장식, 안경, 모자 순서로 겹친다.
    acc_layers: OnceCell<[Retained<CALayer>; 3]>,
    /// 착용 중인 장식의 (너비, 높이, 시점별 머리 기준 위치[3/4, 옆, 정면]) px.
    acc_shapes: Cell<[Option<(f64, f64, [(f64, f64); 3])>; 3]>,
    house: RefCell<Option<Retained<NSPanel>>>,
    /// 방석·집을 화면에서 치워 두었는지.
    house_hidden: Cell<bool>,
    /// (누른 위치, 창 원점 − 커서, 드래그로 판정됨).
    house_drag: Cell<Option<(CGPoint, CGPoint, bool)>>,
    /// 타이핑 집계: 사용자가 켰는지, 잠깐 멈췄는지, 실제로 돌고 있는 탭.
    typing_on: Cell<bool>,
    typing_paused: Cell<bool>,
    keytap: RefCell<Option<crate::keytap::KeyTap>>,
    typing_timer: RefCell<Option<Retained<NSTimer>>>,
    /// 새 버전 확인: 사용자가 켰는지(None = 아직 물어보지 않음), 주기 타이머, 찾은 새 버전.
    update_on: Cell<Option<bool>>,
    update_timer: RefCell<Option<Retained<NSTimer>>>,
    new_version: RefCell<Option<String>>,
    /// 음성 명령: 사용자가 켰는지, 지금 듣고 있는 중이면 그 리스너와 타이머, 듣기를 끝낼 시각.
    voice_on: Cell<bool>,
    listener: RefCell<Option<crate::voice::Listener>>,
    voice_timer: RefCell<Option<Retained<NSTimer>>>,
    voice_until: Cell<Option<Instant>>,
    /// AI 사용량 집계: 사용자가 켰는지, 실제로 돌고 있는 루프백 수신기.
    ai_on: Cell<bool>,
    receiver: RefCell<Option<crate::otel::Receiver>>,
    rng: Cell<u64>,
}

fn now_wall() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// 로컬 달력 기준 yyyymmdd. 일일 보상 한도에만 쓴다.
fn today() -> u32 {
    let units = NSCalendarUnit::Year | NSCalendarUnit::Month | NSCalendarUnit::Day;
    let c = NSCalendar::currentCalendar().components_fromDate(units, &NSDate::now());
    (c.year().clamp(0, 9999) * 10000 + c.month().clamp(1, 12) * 100 + c.day().clamp(1, 31)) as u32
}

fn dist(a: CGPoint, b: CGPoint) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Baby => "아기",
        Stage::Growing => "성장기",
        Stage::Adult => "성견",
    }
}

fn collection_behavior() -> NSWindowCollectionBehavior {
    // 모든 Space에 보이되, 전체 화면 앱 위에는 올라가지 않는다(FullScreenAuxiliary 미지정).
    NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::Stationary
        | NSWindowCollectionBehavior::IgnoresCycle
}

/// 창이 어느 화면의 가시 영역(메뉴 막대·Dock 제외)에도 걸치지 않게 원점을 보정한다.
fn clamp_rect(mtm: MainThreadMarker, origin: CGPoint, w: f64, h: f64) -> CGPoint {
    let center = CGPoint::new(origin.x + w / 2.0, origin.y + h / 2.0);
    let mut best: Option<(f64, CGPoint)> = None;
    for screen in NSScreen::screens(mtm).iter() {
        let v = screen.visibleFrame();
        if v.size.width < w || v.size.height < h {
            continue;
        }
        let x = origin.x.clamp(v.origin.x, v.origin.x + v.size.width - w);
        let y = origin.y.clamp(v.origin.y, v.origin.y + v.size.height - h);
        let d = dist(center, CGPoint::new(x + w / 2.0, y + h / 2.0));
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, CGPoint::new(x, y)));
        }
    }
    best.map(|(_, p)| p).unwrap_or(origin)
}

fn clamp_origin(mtm: MainThreadMarker, origin: CGPoint, size: f64) -> CGPoint {
    clamp_rect(mtm, origin, size, size)
}

/// SF Symbols 벡터 아이콘. 후보 중 이 macOS에 있는 첫 번째를 쓴다.
fn symbol(names: &[&str], point: f64) -> Option<Retained<NSImage>> {
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(point, unsafe {
        objc2_app_kit::NSFontWeightMedium
    });
    names
        .iter()
        .find_map(|name| NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str(name), None))
        .and_then(|image| image.imageWithSymbolConfiguration(&config))
}

/// 큰 수를 짧게: 1234 → 1.2k, 1234567 → 1.2M.
fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

fn gray(alpha: f64) -> CFRetained<CGColor> {
    CGColor::new_generic_rgb(0.5, 0.5, 0.5, alpha)
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArongPetView"]
    struct PetView;

    impl PetView {
        // 다른 앱이 활성 상태여도 첫 클릭을 바로 받는다(앱을 활성화하지는 않는다).
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_mouse_down();
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_mouse_dragged();
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_mouse_up();
            }
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.toggle_tiles();
            }
        }
    }
);

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArongHouseView"]
    struct HouseView;

    impl HouseView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_house_down();
            }
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_house_dragged();
            }
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if let Some(c) = controller() {
                c.on_house_up();
            }
        }
    }
);

pub struct TileIvars {
    id: usize,
    enabled: bool,
    active: bool,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArongTileView"]
    #[ivars = TileIvars]
    struct TileView;

    impl TileView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // 안의 글자 라벨이 아니라 타일 자신이 클릭을 받는다.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: CGPoint) -> Option<Retained<NSView>> {
            let f = self.frame();
            let inside = point.x >= f.origin.x
                && point.x < f.origin.x + f.size.width
                && point.y >= f.origin.y
                && point.y < f.origin.y + f.size.height;
            inside.then(|| Retained::into_super(self.retain()))
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, _event: &NSEvent) {
            self.paint(true);
            if let Some(c) = controller() {
                c.tile_hover(Some(self.ivars().id));
            }
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            self.paint(false);
            if let Some(c) = controller() {
                c.tile_hover(None);
            }
        }

        // mouseDown을 직접 받아야 mouseUp이 이 뷰로 온다(기본 구현은 상위로 넘겨 버린다).
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            self.paint(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            // 타일 밖으로 끌고 나가서 놓으면 취소.
            let p = self.convertPoint_fromView(event.locationInWindow(), None);
            let b = self.bounds();
            let inside = p.x >= 0.0 && p.y >= 0.0 && p.x < b.size.width && p.y < b.size.height;
            if inside
                && self.ivars().enabled
                && let Some(c) = controller()
            {
                c.tile_clicked(self.ivars().id);
            }
        }
    }
);

impl TileView {
    fn paint(&self, hover: bool) {
        let iv = self.ivars();
        let color = match (iv.active, hover && iv.enabled) {
            (true, false) => CGColor::new_generic_rgb(0.25, 0.60, 0.85, 0.45),
            (true, true) => CGColor::new_generic_rgb(0.25, 0.60, 0.85, 0.65),
            (false, true) => gray(0.32),
            (false, false) => gray(0.14),
        };
        if let Some(layer) = self.layer() {
            layer.setBackgroundColor(Some(&color));
        }
    }
}

define_class!(
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArongTilePanel"]
    struct TilePanel;

    impl TilePanel {
        // 바깥을 클릭하면(키 창을 잃으면) 닫히도록 키 창이 된다. 앱은 활성화하지 않는다.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        // 타일 메뉴가 열려 있는 동안 자기 창에 온 키 중 Esc(53)만 확인해 닫는다.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53
                && let Some(c) = controller()
            {
                c.close_tiles();
            }
        }

        // 타일이 아닌 곳(제목, 타일 사이)을 누르고 끌면 메뉴를 옮긴다. 타일은 자기 클릭을 직접 받는다.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.performWindowDragWithEvent(event);
        }
    }
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ArongController"]
    #[ivars = Ivars]
    pub struct Controller;

    unsafe impl NSObjectProtocol for Controller {}

    unsafe impl NSApplicationDelegate for Controller {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            self.build_ui();
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _n: &NSNotification) {
            self.end_walk(false);
            self.stop_roam();
            self.end_drag();
            self.stop_listening();
            self.flush_typing();
            self.ivars().keytap.replace(None);
            self.flush_ai();
            self.ivars().receiver.replace(None);
            self.save();
        }
    }

    unsafe impl NSMenuDelegate for Controller {
        #[unsafe(method(menuNeedsUpdate:))]
        fn menu_needs_update(&self, menu: &NSMenu) {
            self.rebuild_menu(menu);
        }
    }

    unsafe impl NSWindowDelegate for Controller {
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _n: &NSNotification) {
            self.close_tiles();
        }

        // 메뉴를 끌어서 옮기는 동안 온다.
        #[unsafe(method(windowDidMove:))]
        fn window_did_move(&self, _n: &NSNotification) {
            self.tiles_dragged();
        }
    }

    impl Controller {
        #[unsafe(method(animTick:))]
        fn anim_tick(&self, _t: &NSTimer) {
            if self.advance_script() {
                return;
            }
            let iv = self.ivars();
            if iv.stay_after.replace(false) {
                // 마지막 자세로 머문다. 타이머는 멈춘다.
                iv.held.set(true);
                Self::stop(&iv.anim_timer);
            } else {
                self.rest();
            }
        }

        #[unsafe(method(idleTick:))]
        fn idle_tick(&self, _t: &NSTimer) {
            let iv = self.ivars();
            iv.idle_timer.replace(None);
            // 닫힌 타일 패널은 여기서(타일 이벤트 처리 밖에서) 해제한다.
            iv.tiles_trash.borrow_mut().clear();
            let visible = iv
                .panel
                .get()
                .is_some_and(|p| p.occlusionState().contains(NSWindowOcclusionState::Visible));
            let busy = iv.anim_timer.borrow().is_some()
                || iv.held.get()
                || iv.inside.get()
                || iv.doorway.borrow().is_some()
                || iv.walk.borrow().is_some()
                || iv.tiles_open.get()
                || !matches!(*iv.drag.borrow(), Drag::None);
            // 저전력 모드에서는 대기 동작을 건너뛴다.
            if visible && !busy && !NSProcessInfo::processInfo().isLowPowerModeEnabled() {
                let roll = self.next_random();
                if iv.roaming.get() && !iv.locked.get() && roll % 5 < 3 {
                    self.begin_roam();
                } else {
                    let picks = [Act::Paw(true), Act::Paw(false), Act::Sit, Act::SitUp, Act::Nap, Act::Roll, Act::Pant, Act::Look];
                    self.perform(picks[((roll >> 8) % picks.len() as u64) as usize]);
                }
            }
            self.schedule_idle();
        }

        #[unsafe(method(walkTick:))]
        fn walk_tick(&self, _t: &NSTimer) {
            self.step_walk();
        }

        #[unsafe(method(doorTick:))]
        fn door_tick(&self, _t: &NSTimer) {
            self.step_doorway();
        }

        #[unsafe(method(roamTick:))]
        fn roam_tick(&self, _t: &NSTimer) {
            self.step_roam();
        }

        #[unsafe(method(toggleWalk:))]
        fn toggle_walk(&self, _s: Option<&AnyObject>) {
            if self.ivars().walk.borrow().is_some() {
                self.end_walk(true);
            } else {
                self.begin_walk();
            }
        }

        #[unsafe(method(toggleRoam:))]
        fn toggle_roam(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.roaming.set(!iv.roaming.get());
            self.save_flag("roam", iv.roaming.get());
            if !iv.roaming.get() {
                self.stop_roam();
            }
            self.bubble(if iv.roaming.get() { "혼자 걷기 켬" } else { "혼자 걷기 끔" });
        }

        // 디버그 빌드 전용: 앱이 실제로 그리는 내용을 PNG로 떠서 눈으로 확인한다.
        #[cfg(debug_assertions)]
        #[unsafe(method(snapshotTick:))]
        fn snapshot_tick(&self, _t: &NSTimer) {
            use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext};
            let Some(dir) = std::env::var_os("ARONG_SNAPSHOT_DIR").map(std::path::PathBuf::from) else { return };
            let iv = self.ivars();
            let write = |rep: &NSBitmapImageRep, name: &str| {
                let png = unsafe {
                    rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new())
                };
                if let Some(png) = png {
                    let _ = std::fs::write(dir.join(name), png.to_vec());
                }
            };
            // 1) 펫 레이어(스프라이트 + 장식)를 좌우 방향과 몇 가지 동작으로.
            if let Some(layer) = iv.panel.get().and_then(|p| p.contentView()).and_then(|v| v.layer()) {
                let size = self.size() as isize;
                let tall = (self.size() + self.head_room()) as isize;
                // (동작, 프레임(None = 가장 깊이 앉거나 엎드린 프레임), 왼쪽을 보는지, 밥그릇)
                let shots = [
                    (Motion::PawLeft, Some(0), true, false),
                    (Motion::Walk, Some(2), true, false),
                    (Motion::Walk, Some(7), false, false),
                    (Motion::PawLeft, Some(4), true, false),
                    (Motion::PawRight, Some(4), false, false),
                    (Motion::Sit, None, true, true),
                    (Motion::SitUp, None, true, false),
                    (Motion::LieDown, None, true, false),
                    (Motion::Roll, Some(4), true, false),
                    (Motion::Pant, Some(2), true, false),
                    (Motion::Look, Some(1), true, false),
                    (Motion::Look, Some(4), false, false),
                ];
                for (i, (motion, frame, left, bowl)) in shots.into_iter().enumerate() {
                    let frame = frame.unwrap_or_else(|| iv.atlas.borrow().as_ref().map_or(0, |a| a.hold(motion)));
                    iv.bowl.set(bowl);
                    iv.motion.set(motion);
                    iv.frame.set(frame);
                    iv.facing_left.set(left);
                    self.show_frame();
                    let rep = unsafe {
                        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                            NSBitmapImageRep::alloc(), std::ptr::null_mut(), size, tall, 8, 4, true, false, NSDeviceRGBColorSpace, size * 4, 32,
                        )
                    };
                    if let Some(rep) = rep
                        && let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)
                    {
                        layer.renderInContext(&context.CGContext());
                        write(&rep, &format!("pet{i}.png"));
                    }
                }
                // 미리 입어 보기: 저장된 착용 상태는 그대로 두고 화면만 바뀌는지 본다.
                for (name, preview) in [("preview_on.png", Some((Slot::Clothes, "coral_hoodie".to_string()))), ("preview_off.png", None)] {
                    self.set_preview(preview);
                    let rep = unsafe {
                        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                            NSBitmapImageRep::alloc(), std::ptr::null_mut(), size, tall, 8, 4, true, false, NSDeviceRGBColorSpace, size * 4, 32,
                        )
                    };
                    if let Some(rep) = rep
                        && let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)
                    {
                        layer.renderInContext(&context.CGContext());
                        write(&rep, name);
                    }
                    eprintln!("{name}: worn clothes in save = {:?}", iv.pet.borrow().wearing(Slot::Clothes));
                }
                self.rest();
            }
            // 1-2) 집에 들어가는 연출: 집과 아롱이를 한 장에 겹쳐 그려 단계별로 본다.
            if let (Some(panel), Some(house), Some(front), Some(door)) =
                (iv.panel.get(), iv.house.borrow().clone(), self.door_front(), self.door_point())
            {
                use objc2_core_graphics::CGContext;
                let pet_layer = panel.contentView().and_then(|v| v.layer());
                let house_layer = house.contentView().and_then(|v| v.layer());
                let hf = house.frame();
                let size = self.size();
                let feet = (CELL as f64 - GROUND) * size / CELL as f64;
                let deep = CGPoint::new(door.x - size / 2.0, door.y - feet + size * 0.04);
                // 집과 문 앞 자리를 모두 담는 캔버스.
                let (x0, y0) = (hf.origin.x.min(front.x) - 4.0, front.y.min(hf.origin.y) - 4.0);
                let (cw, ch) = ((hf.size.width.max(size) + 8.0) as isize, (hf.origin.y + hf.size.height - y0 + 8.0) as isize);
                for (i, depth) in [0.0, 0.3, 0.5, 0.7, 0.9].into_iter().enumerate() {
                    let rep = unsafe {
                        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                            NSBitmapImageRep::alloc(), std::ptr::null_mut(), cw, ch, 8, 4, true, false, NSDeviceRGBColorSpace, cw * 4, 32,
                        )
                    };
                    let (Some(rep), Some(pet_layer), Some(house_layer)) = (rep, pet_layer.as_ref(), house_layer.as_ref()) else { continue };
                    let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) else { continue };
                    let cg = context.CGContext();
                    CGContext::save_g_state(Some(&cg));
                    CGContext::translate_ctm(Some(&cg), hf.origin.x - x0, hf.origin.y - y0);
                    house_layer.renderInContext(&cg);
                    CGContext::restore_g_state(Some(&cg));
                    let at = CGPoint::new(front.x + (deep.x - front.x) * depth, front.y + (deep.y - front.y) * depth);
                    iv.shrink.set(1.0 - DOORWAY_SHRINK * depth);
                    iv.motion.set(Motion::Walk);
                    iv.frame.set(i * 2);
                    iv.facing_left.set(true);
                    self.show_frame();
                    CGContext::save_g_state(Some(&cg));
                    CGContext::translate_ctm(Some(&cg), at.x - x0, at.y - y0);
                    CGContext::set_alpha(Some(&cg), 1.0 - ((depth - DOORWAY_FADE_FROM) / (1.0 - DOORWAY_FADE_FROM)).clamp(0.0, 1.0));
                    pet_layer.renderInContext(&cg);
                    CGContext::restore_g_state(Some(&cg));
                    write(&rep, &format!("door{i}.png"));
                }
                iv.shrink.set(1.0);
                self.rest();
            }
            // 2) 타일 메뉴들.
            let panels: [(&str, &dyn Fn()); 5] = [
                ("tiles_main.png", &|| self.open_main_tiles()),
                ("tiles_shop.png", &|| self.open_shop_tiles()),
                ("tiles_shoes.png", &|| self.open_category_tiles(Slot::Shoes)),
                ("tiles_clothes.png", &|| self.open_category_tiles(Slot::Clothes)),
                ("tiles_training.png", &|| self.open_training_tiles()),
            ];
            for (name, open) in panels {
                open();
                let view = iv.tiles.borrow().as_ref().and_then(|p| p.contentView());
                if let Some(view) = view
                    && let Some(rep) = view.bitmapImageRepForCachingDisplayInRect(view.bounds())
                {
                    view.cacheDisplayInRect_toBitmapImageRep(view.bounds(), &rep);
                    write(&rep, name);
                }
                self.close_tiles();
            }
            // 3) 클릭 통과 확인 준비: 비교용 불투명 창을 하나 띄우고, 반영될 시간을 둔 뒤 잰다.
            if let Some(panel) = iv.panel.get() {
                panel.setFrameOrigin(CGPoint::new(300.0, 300.0));
                panel.orderFrontRegardless();
                let reference = unsafe {
                    NSWindow::initWithContentRect_styleMask_backing_defer(
                        self.mtm().alloc(),
                        CGRect::new(CGPoint::new(600.0, 300.0), CGSize::new(100.0, 100.0)),
                        NSWindowStyleMask::Borderless,
                        NSBackingStoreType::Buffered,
                        false,
                    )
                };
                unsafe { reference.setReleasedWhenClosed(false) };
                reference.setLevel(NSFloatingWindowLevel);
                reference.orderFrontRegardless();
                eprintln!("reference window {}", reference.windowNumber());
                std::mem::forget(reference);
                std::mem::forget(self.timer(1.5, sel!(probeTick:), false, 0.0));
            }
            eprintln!("snapshot done");
        }

        // 디버그 빌드 전용: 화면의 각 지점을 클릭하면 어느 창이 받는지 윈도 서버에 묻는다.
        #[cfg(debug_assertions)]
        #[unsafe(method(probeTick:))]
        fn probe_tick(&self, _t: &NSTimer) {
            let Some(panel) = self.ivars().panel.get() else { return };
            let f = panel.frame();
            let mine = panel.windowNumber();
            let at = |x: f64, y: f64| NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(CGPoint::new(x, y), 0, self.mtm());
            eprintln!("reference center -> {} (불투명 비교 창 한가운데)", at(650.0, 350.0));
            eprintln!("pet window {mine}, frame {:?}; 아래는 창을 9x9로 나눈 격자(#=아롱이 창이 받음, .=통과), 위쪽부터", (f.origin.x, f.origin.y, f.size.width, f.size.height));
            for row in (0..9).rev() {
                let line: String = (0..9)
                    .map(|col| {
                        let x = f.origin.x + f.size.width * (col as f64 + 0.5) / 9.0;
                        let y = f.origin.y + f.size.height * (row as f64 + 0.5) / 9.0;
                        if at(x, y) == mine { '#' } else { '.' }
                    })
                    .collect();
                eprintln!("  {line}");
            }
            eprintln!("probe done");
        }

        #[unsafe(method(typingTick:))]
        fn typing_tick(&self, _t: &NSTimer) {
            self.flush_typing();
            self.flush_ai();
            // 권한이 철회됐으면 즉시 탭을 없애고, 새로 허용됐으면 시작한다.
            self.sync_typing();
        }

        #[unsafe(method(toggleTyping:))]
        fn toggle_typing(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            if iv.typing_on.get() {
                // 완전 해제: 남은 수를 반영하고 탭을 없앤다.
                self.flush_typing();
                iv.typing_on.set(false);
                iv.typing_paused.set(false);
                self.save_flag("typing", false);
                self.save_flag("typing_paused", false);
                self.sync_typing();
                self.bubble("타이핑 집계 끔");
                return;
            }
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(ns_string!("타이핑 집계를 켤까요?"));
            alert.setInformativeText(ns_string!(
                "키를 누른 '횟수'만 세어 경험치로 바꿉니다.\n\n· 어떤 키를 눌렀는지, 입력한 글자·문장·비밀번호는 읽지도 저장하지도 않습니다.\n· 저장되는 것은 오늘의 합계 숫자 하나뿐이고, 외부로 보내지 않습니다.\n· 자동 반복과 단축키는 세지 않습니다.\n\nmacOS '입력 모니터링' 권한이 필요합니다. 이 권한 자체는 키 입력 내용을 볼 수 있는 권한이라, 허용 여부는 직접 판단해 주세요. 언제든 이 메뉴에서 끌 수 있습니다."
            ));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("켜기"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
            if alert.runModal() != 1001 {
                return;
            }
            iv.typing_on.set(true);
            self.save_flag("typing", true);
            if !crate::keytap::has_permission() {
                crate::keytap::request_permission();
            }
            self.sync_typing();
            if iv.keytap.borrow().is_some() {
                self.bubble("타이핑 집계 켬");
            } else {
                self.bubble("입력 모니터링 허용 후 아롱이를 다시 실행해 주세요");
            }
        }

        #[unsafe(method(exportData:))]
        fn export_data(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let text = self.with_pet(|p, _| crate::backup::export_json(p));
            let panel = NSSavePanel::savePanel(mtm);
            panel.setNameFieldStringValue(ns_string!("arong-save.json"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            // 1 = NSModalResponseOK. 저장 위치는 사용자가 고른 한 곳뿐이다.
            if panel.runModal() != 1 {
                return;
            }
            let Some(path) = panel.URL().and_then(|u| u.path()).map(|p| p.to_string()) else { return };
            let ok = std::fs::write(&path, text).is_ok();
            if ok {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
            self.bubble(if ok { "내보냈어요" } else { "내보내기에 실패했어요" });
        }

        #[unsafe(method(importData:))]
        fn import_data(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            if panel.runModal() != 1 {
                return;
            }
            let Some(path) = panel.URL().and_then(|u| u.path()).map(|p| p.to_string()) else { return };
            // 읽기 전에 크기부터 확인한다. 읽는 파일은 사용자가 고른 이 하나뿐이다.
            let small = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() <= crate::backup::MAX_IMPORT_BYTES);
            let parsed = if small {
                std::fs::read(&path).map_err(|_| "파일을 읽을 수 없습니다").and_then(|b| crate::backup::import_json(&b, now_wall()))
            } else {
                Err("파일이 너무 크거나 읽을 수 없습니다")
            };
            let pet = match parsed {
                Ok(pet) => pet,
                Err(reason) => {
                    let alert = NSAlert::new(mtm);
                    alert.setMessageText(ns_string!("가져올 수 없는 파일입니다"));
                    alert.setInformativeText(&NSString::from_str(reason));
                    alert.runModal();
                    return;
                }
            };
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("이 파일로 지금 육성 데이터를 덮어쓸까요?"));
            alert.setInformativeText(&NSString::from_str(&format!(
                "가져올 내용: Lv {} · 재화 {} · 아이템 {}개\n지금 데이터는 사라지며 되돌릴 수 없습니다.",
                pet.level(),
                pet.coins,
                pet.owned.len()
            )));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("덮어쓰기"));
            if alert.runModal() != 1001 {
                return;
            }
            let iv = self.ivars();
            self.end_activity();
            if let Some(store) = &iv.store {
                store.delete_pet();
            }
            iv.pet.replace(pet);
            self.with_pet(|_, _| ());
            self.save();
            self.apply_outfit();
            self.apply_house();
            self.rest();
            self.bubble("가져왔어요");
        }

        #[unsafe(method(toggleLogin:))]
        fn toggle_login(&self, _s: Option<&AnyObject>) {
            use objc2_service_management::{SMAppService, SMAppServiceStatus};
            // SAFETY: 메인 앱 자신을 Mac 로그인 항목으로 등록·해제하는 공개 API다. 사용자가 누를 때만 부른다.
            let (ok, now_on) = unsafe {
                let service = SMAppService::mainAppService();
                if service.status() == SMAppServiceStatus::Enabled {
                    (service.unregisterAndReturnError().is_ok(), false)
                } else {
                    (service.registerAndReturnError().is_ok(), true)
                }
            };
            self.bubble(match (ok, now_on) {
                (true, true) => "Mac을 켜면 자동으로 실행돼요",
                (true, false) => "자동 실행을 껐어요",
                (false, _) => "자동 실행 설정에 실패했어요",
            });
        }

        // 하위 메뉴를 여는 항목이 활성 상태로 보이게 하는 빈 액션.
        #[unsafe(method(noop:))]
        fn noop(&self, _s: Option<&AnyObject>) {}

        #[unsafe(method(resetActivity:))]
        fn reset_activity(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("활동 집계 데이터만 지울까요?"));
            alert.setInformativeText(ns_string!(
                "오늘과 지난 날짜의 타수·AI 토큰 수치, 날짜별 기록이 지워집니다.\n레벨·경험치·재화·아이템·집은 그대로 남습니다. 되돌릴 수 없습니다."
            ));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("집계 데이터 삭제"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            if alert.runModal() != 1001 {
                return;
            }
            let iv = self.ivars();
            // 아직 반영되지 않은 수치도 함께 버린다.
            crate::keytap::take_count();
            crate::otel::take_samples();
            if let Some(store) = &iv.store {
                store.delete_activity();
            }
            self.with_pet(|p, _| {
                p.day.keys = 0;
                p.day.tokens = [0; 4];
                p.day.ai_units = 0;
                p.day.xp = 0;
                p.key_carry = 0;
                p.ai_carry = 0;
            });
            self.save();
            self.bubble("집계 데이터를 지웠어요");
        }

        #[unsafe(method(editRewards:))]
        fn edit_rewards(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let iv = self.ivars();
            let items = tunables();
            let (row, label_w, field_w) = (28.0, 250.0, 90.0);
            let form = NSView::initWithFrame(
                mtm.alloc(),
                CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(label_w + field_w + 8.0, row * items.len() as f64)),
            );
            let mut fields = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let y = row * (items.len() - 1 - i) as f64;
                let label = NSTextField::labelWithString(&NSString::from_str(item.label), mtm);
                label.setFrame(CGRect::new(CGPoint::new(0.0, y + 3.0), CGSize::new(label_w, 18.0)));
                form.addSubview(&label);
                let value = (item.get)(&iv.tuning.borrow());
                let field = NSTextField::textFieldWithString(&NSString::from_str(&value.to_string()), mtm);
                field.setFrame(CGRect::new(CGPoint::new(label_w + 8.0, y), CGSize::new(field_w, 22.0)));
                field.setAlignment(NSTextAlignment::Right);
                form.addSubview(&field);
                fields.push(field);
            }
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("보상 규칙"));
            alert.setInformativeText(ns_string!(
                "숫자만 입력하세요. 범위를 벗어난 값은 바뀌지 않습니다. 하루 100% 구간을 넘으면 50%, 그 3배를 넘으면 10%만 받습니다."
            ));
            alert.setAccessoryView(Some(&form));
            alert.addButtonWithTitle(ns_string!("저장"));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("기본값으로"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            // 1000 = 저장, 1001 = 취소, 1002 = 기본값으로.
            let choice = alert.runModal();
            if choice == 1001 {
                return;
            }
            let defaults = Tuning::default();
            let mut changed = 0;
            for (item, field) in items.iter().zip(&fields) {
                let wanted = if choice == 1002 {
                    Some((item.get)(&defaults))
                } else {
                    field.stringValue().to_string().trim().replace(',', "").parse::<i64>().ok().filter(|v| (item.min..=item.max).contains(v))
                };
                let Some(value) = wanted else { continue };
                if value != (item.get)(&iv.tuning.borrow()) {
                    changed += 1;
                }
                (item.set)(&mut iv.tuning.borrow_mut(), value);
                if let Some(store) = &iv.store {
                    store.set_setting(item.key, &value.to_string());
                }
            }
            self.bubble(&if choice == 1002 { "보상 규칙을 기본값으로 되돌렸어요".to_string() } else { format!("보상 규칙 {changed}개를 바꿨어요") });
        }

        #[unsafe(method(askUpdate:))]
        fn ask_update(&self, _t: &NSTimer) {
            if self.ivars().update_on.get().is_none() {
                self.confirm_update_check();
            }
        }

        #[unsafe(method(toggleUpdate:))]
        fn toggle_update(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            if iv.update_on.get() == Some(true) {
                iv.update_on.set(Some(false));
                self.save_flag("update_check", false);
                self.sync_update();
                self.bubble("새 버전 확인 끔");
            } else {
                self.confirm_update_check();
            }
        }

        #[unsafe(method(updateTick:))]
        fn update_tick(&self, _t: &NSTimer) {
            if self.ivars().update_on.get() == Some(true) {
                crate::update::check();
                // 응답은 백그라운드에서 오므로 조금 뒤에 결과를 본다.
                std::mem::forget(self.timer(20.0, sel!(updateResult:), false, 5.0));
            }
        }

        #[unsafe(method(updateResult:))]
        fn update_result(&self, _t: &NSTimer) {
            let iv = self.ivars();
            let Some(latest) = crate::update::take_latest() else { return };
            if iv.update_on.get() == Some(true) && crate::update::is_newer(&latest, env!("CARGO_PKG_VERSION")) {
                // 아롱이가 정면을 보며 직접 알려 준다(하루 한 번 확인할 때마다).
                if iv.walk.borrow().is_none() && iv.doorway.borrow().is_none() {
                    self.stop_roam();
                    self.perform(Act::Look);
                }
                self.bubble(&format!("멍! 새 버전 {latest}이 나왔다멍! 더보기 메뉴에서 업데이트 해달라 멍!"));
                iv.new_version.replace(Some(latest));
            }
        }

        #[unsafe(method(copyUpdateCommand:))]
        fn copy_update_command(&self, _s: Option<&AnyObject>) {
            // 클립보드에 쓰기만 한다(읽지 않는다). 업데이트 자체는 사용자가 터미널에서 실행한다.
            let board = NSPasteboard::generalPasteboard();
            board.clearContents();
            board.setString_forType(ns_string!("brew upgrade --cask arong"), unsafe { NSPasteboardTypeString });
            self.bubble("업데이트 명령을 복사했어요. 터미널에 붙여 넣으세요");
        }

        #[unsafe(method(toggleVoice:))]
        fn toggle_voice(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            if iv.voice_on.get() {
                self.stop_listening();
                iv.voice_on.set(false);
                self.save_flag("voice", false);
                self.bubble("음성 명령 끔");
            } else {
                self.enable_voice();
            }
        }

        #[unsafe(method(voiceTick:))]
        fn voice_tick(&self, _t: &NSTimer) {
            let iv = self.ivars();
            let heard = iv.listener.borrow().as_ref().and_then(|l| l.heard());
            if let Some(command) = heard {
                // 알아들은 명령을 따르고, 다음 명령을 위해 새로 듣기 시작한다.
                // (인식기는 지금까지 들은 문장을 계속 이어 붙이므로, 매번 새로 열어야 같은 말이 두 번 처리되지 않는다.)
                iv.listener.replace(None);
                self.do_command(command);
                match crate::voice::Listener::start() {
                    Ok(listener) => {
                        iv.listener.replace(Some(listener));
                        iv.voice_until.set(Some(Instant::now() + std::time::Duration::from_secs_f64(VOICE_IDLE_SECS)));
                    }
                    Err(reason) => {
                        self.stop_listening();
                        self.bubble(reason);
                    }
                }
                self.refresh_training_tiles();
            } else if iv.voice_until.get().is_none_or(|until| Instant::now() >= until) {
                // 한동안 명령이 없으면 마이크를 닫는다.
                self.stop_listening();
                self.bubble("말로 훈련 끝 (마이크를 닫았어요)");
                self.refresh_training_tiles();
            }
        }

        #[unsafe(method(toggleAi:))]
        fn toggle_ai(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            if iv.ai_on.get() {
                // 완전 해제: 남은 사용량을 반영하고 수신을 닫는다.
                self.flush_ai();
                iv.ai_on.set(false);
                self.save_flag("ai", false);
                self.sync_ai();
                self.bubble("AI 사용량 집계 끔");
                return;
            }
            let alert = NSAlert::new(self.mtm());
            alert.setMessageText(ns_string!("AI 사용량 집계를 켤까요? (Claude Code)"));
            alert.setInformativeText(ns_string!(
                "Claude Code가 공식으로 내보내는 사용량 수치(토큰 수)를 이 Mac 안에서만 받아 경험치로 바꿉니다.\n\n· 받는 것: 입력·출력·캐시 토큰의 '개수'뿐입니다. 대화 내용, 코드, 로그인 정보는 받지도 읽지도 않습니다.\n· 함께 전송되는 계정 식별자는 저장하지 않고 버립니다.\n· 아롱이는 127.0.0.1(이 Mac)에서만 수신하고 외부로 아무것도 보내지 않습니다.\n· 펫을 키우려고 AI 요청을 따로 만들지 않습니다.\n\n켠 뒤 Claude Code 설정(~/.claude/settings.json)에 몇 줄을 직접 추가해야 합니다. '켜고 설정 복사'를 누르면 붙여 넣을 내용이 클립보드에 복사됩니다. 아롱이가 그 파일을 고치지는 않습니다."
            ));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("켜고 설정 복사"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
            if alert.runModal() != 1001 {
                return;
            }
            iv.ai_on.set(true);
            self.save_flag("ai", true);
            self.sync_ai();
            self.do_copy_ai_config();
            if iv.receiver.borrow().is_none() {
                self.bubble("수신 포트를 열 수 없어요 (이미 사용 중)");
            }
        }

        #[unsafe(method(copyAiConfig:))]
        fn copy_ai_config(&self, _s: Option<&AnyObject>) {
            self.do_copy_ai_config();
        }

        #[unsafe(method(toggleTypingPause:))]
        fn toggle_typing_pause(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            self.flush_typing();
            iv.typing_paused.set(!iv.typing_paused.get());
            self.save_flag("typing_paused", iv.typing_paused.get());
            self.sync_typing();
            self.bubble(if iv.typing_paused.get() { "타이핑 집계 일시정지" } else { "타이핑 집계 다시 시작" });
        }

        #[unsafe(method(bubbleTick:))]
        fn bubble_tick(&self, _t: &NSTimer) {
            let iv = self.ivars();
            iv.bubble_timer.replace(None);
            if let Some(window) = iv.bubble.take() {
                window.orderOut(None);
            }
        }

        #[unsafe(method(feed:))]
        fn feed(&self, _s: Option<&AnyObject>) {
            let r = self.with_pet(|p, t| p.feed(t));
            let message = if r.xp > 0 { format!("냠냠! +{} XP", r.xp) } else { "냠냠! 배불러요".to_string() };
            self.after_action(r, Act::Eat, message);
        }

        #[unsafe(method(snack:))]
        fn snack(&self, _s: Option<&AnyObject>) {
            let r = self.with_pet(|p, t| p.snack(t));
            if r.ok {
                self.after_action(r, Act::Paw(true), "간식 냠! 기분이 좋아졌어요".to_string());
            } else {
                self.bubble("재화가 부족해요");
            }
        }

        #[unsafe(method(stroke:))]
        fn stroke(&self, _s: Option<&AnyObject>) {
            self.do_stroke();
        }

        #[unsafe(method(toggleTop:))]
        fn toggle_top(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.always_on_top.set(!iv.always_on_top.get());
            self.save_flag("always_on_top", iv.always_on_top.get());
            self.apply_level();
        }

        #[unsafe(method(toggleLock:))]
        fn toggle_lock(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.locked.set(!iv.locked.get());
            self.save_flag("locked", iv.locked.get());
            if iv.locked.get() {
                self.stop_roam();
            }
            self.bubble(if iv.locked.get() { "위치 고정 켬" } else { "위치 고정 끔" });
        }

        #[unsafe(method(sizeSmall:))]
        fn size_small(&self, _s: Option<&AnyObject>) {
            self.set_size_level(0);
        }

        #[unsafe(method(sizeMedium:))]
        fn size_medium(&self, _s: Option<&AnyObject>) {
            self.set_size_level(1);
        }

        #[unsafe(method(sizeLarge:))]
        fn size_large(&self, _s: Option<&AnyObject>) {
            self.set_size_level(2);
        }

        // 다른 프로세스가 보낸 "사진 찍어 줘" 알림. 아롱이 그림만 앱 데이터 폴더에 저장한다.
        #[unsafe(method(photoRequested:))]
        fn photo_requested(&self, _n: &NSNotification) {
            let Some(dir) = crate::store::data_dir() else { return };
            self.save_photo(&dir.join("photo.png"));
        }

        #[unsafe(method(takePhoto:))]
        fn take_photo(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let panel = NSSavePanel::savePanel(mtm);
            panel.setNameFieldStringValue(ns_string!("arong.png"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            if panel.runModal() != 1 {
                return;
            }
            let Some(path) = panel.URL().and_then(|u| u.path()).map(|p| p.to_string()) else { return };
            let ok = self.save_photo(std::path::Path::new(&path));
            self.bubble(if ok { "찰칵! 사진을 저장했어요" } else { "사진을 저장하지 못했어요" });
        }

        #[unsafe(method(toggleHouse:))]
        fn toggle_house(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.house_hidden.set(!iv.house_hidden.get());
            self.save_flag("house_hidden", iv.house_hidden.get());
            if iv.house_hidden.get() {
                // 집 안에 있거나 드나드는 중이었다면 밖으로 나오게 한다.
                self.wake();
                self.stop_roam();
            }
            self.apply_house();
            let name = self.house_word();
            self.bubble(&if iv.house_hidden.get() { format!("{name} 숨김") } else { format!("{name} 다시 놓음") });
        }

        #[unsafe(method(toggleHidden:))]
        fn toggle_hidden(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.hidden.set(!iv.hidden.get());
            self.save_flag("hidden", iv.hidden.get());
            self.apply_visibility();
        }

        #[unsafe(method(togglePause:))]
        fn toggle_pause(&self, _s: Option<&AnyObject>) {
            let iv = self.ivars();
            iv.paused.set(!iv.paused.get());
            self.save_flag("paused", iv.paused.get());
            self.apply_visibility();
        }

        #[unsafe(method(resetData:))]
        fn reset_data(&self, _s: Option<&AnyObject>) {
            let mtm = self.mtm();
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("아롱이의 육성 데이터를 초기화할까요?"));
            alert.setInformativeText(ns_string!(
                "레벨·경험치·재화·수치가 처음 상태로 돌아갑니다. 되돌릴 수 없습니다."
            ));
            alert.addButtonWithTitle(ns_string!("취소"));
            alert.addButtonWithTitle(ns_string!("초기화"));
            #[allow(deprecated)]
            NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
            // 두 번째 버튼(초기화) = NSAlertSecondButtonReturn.
            if alert.runModal() == 1001 {
                let iv = self.ivars();
                if let Some(store) = &iv.store {
                    store.delete_pet();
                }
                let mut fresh = Pet::new(now_wall());
                fresh.advance(&iv.tuning.borrow(), now_wall(), today());
                iv.pet.replace(fresh);
                if let Some(store) = &iv.store {
                    store.delete_ai_series();
                }
                self.save();
                self.reload_atlas_if_needed();
                self.apply_outfit();
                self.apply_house();
                self.rest();
            }
        }

        #[unsafe(method(screensChanged:))]
        fn screens_changed(&self, _n: &NSNotification) {
            // 모니터 분리·배율 변경: 산책을 정산하고 화면 안으로 되돌린다.
            self.end_activity();
            self.apply_size();
        }

        #[unsafe(method(suspend:))]
        fn suspend(&self, _n: &NSNotification) {
            self.ivars().suspended.set(true);
            self.end_activity();
            self.apply_visibility();
        }

        #[unsafe(method(resume:))]
        fn resume(&self, _n: &NSNotification) {
            self.ivars().suspended.set(false);
            self.apply_visibility();
        }
    }
);

impl Controller {
    pub fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let mut tuning = Tuning::default();
        let (store, outcome) = match Store::open() {
            Some((s, o)) => (Some(s), Some(o)),
            None => (None, None),
        };
        let notice = match outcome {
            None => Some("저장소를 열 수 없어 이번 실행은 저장되지 않습니다"),
            Some(OpenOutcome::RestoredFromBackup) => Some("저장 파일 손상: 백업에서 복구했습니다"),
            Some(OpenOutcome::ResetAfterCorruption) if store.as_ref().is_some_and(|s| s.setting("launched").is_some()) => {
                Some("저장 파일 손상: 새로 시작했습니다")
            }
            _ => None,
        };
        // 사용자가 바꿔 둔 보상 규칙을 불러온다(범위를 벗어난 값은 버린다).
        if let Some(store) = &store {
            for item in tunables() {
                if let Some(v) = store.setting(item.key).and_then(|v| v.parse::<i64>().ok()).filter(|v| (item.min..=item.max).contains(v)) {
                    (item.set)(&mut tuning, v);
                }
            }
        }
        let mut pet = store.as_ref().and_then(|s| s.load_pet()).unwrap_or_else(|| Pet::new(now_wall()));
        // 앱이 꺼져 있는 동안 날짜가 넘어갔으면 끝난 날을 기록으로 남긴다.
        if let (Some(finished), Some(store)) = (pet.advance(&tuning, now_wall(), today()), &store) {
            store.archive_day(&finished);
        }
        let flag = |key: &str, default: bool| store.as_ref().map_or(default, |s| s.setting_bool(key, default));
        let roaming = flag("roam", true);
        let house_hidden = flag("house_hidden", false);
        // 크기 설정. 예전의 "크게 보기" 설정만 있으면 그것을 이어받는다.
        let size_level = store
            .as_ref()
            .and_then(|s| s.setting("size"))
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|v| *v <= 2)
            .unwrap_or(if flag("big", false) { 2 } else { 1 });
        // 기본값은 꺼짐. 사용자가 직접 켜야만 집계한다.
        let typing_on = flag("typing", false);
        let typing_paused = flag("typing_paused", false);
        let ai_on = flag("ai", false);
        // 음성 명령도 기본은 꺼짐이다.
        let voice_on = flag("voice", false);
        // 새 버전 확인은 물어본 적이 없으면 None이다. 동의하기 전에는 접속하지 않는다.
        let update_on = store.as_ref().and_then(|s| s.setting("update_check")).map(|v| v == "1");
        let ivars = Ivars {
            size_level: Cell::new(size_level),
            always_on_top: Cell::new(flag("always_on_top", true)),
            locked: Cell::new(flag("locked", false)),
            hidden: Cell::new(flag("hidden", false)),
            paused: Cell::new(flag("paused", false)),
            tuning: RefCell::new(tuning),
            walk_tuning: WalkTuning::default(),
            pet: RefCell::new(pet),
            store,
            notice: Cell::new(notice),
            panel: OnceCell::new(),
            sprite: OnceCell::new(),
            body: OnceCell::new(),
            hop: Cell::new((0.0, 0.0)),
            status_item: OnceCell::new(),
            menu: OnceCell::new(),
            atlas: RefCell::new(None),
            suspended: Cell::new(false),
            motion: Cell::new(Motion::PawLeft),
            frame: Cell::new(0),
            script: RefCell::new(VecDeque::new()),
            stay_after: Cell::new(false),
            held: Cell::new(false),
            bowl: Cell::new(false),
            bowl_layer: OnceCell::new(),
            bowl_size: Cell::new((0.0, 0.0)),
            inside: Cell::new(false),
            doorway: RefCell::new(None),
            door_timer: RefCell::new(None),
            shrink: Cell::new(1.0),
            facing_left: Cell::new(true),
            anim_timer: RefCell::new(None),
            idle_timer: RefCell::new(None),
            walk_timer: RefCell::new(None),
            drag: RefCell::new(Drag::None),
            overlays: RefCell::new(Vec::new()),
            walk: RefCell::new(None),
            roam: RefCell::new(None),
            roam_timer: RefCell::new(None),
            roaming: Cell::new(roaming),
            tiles: RefCell::new(None),
            tiles_trash: RefCell::new(Vec::new()),
            tile_actions: RefCell::new(Vec::new()),
            tiles_open: Cell::new(false),
            tiles_placed: Cell::new(CGPoint::new(0.0, 0.0)),
            tiles_scroll: RefCell::new(None),
            tiles_count: Cell::new(0),
            tiles_anchor: Cell::new(None),
            preview: RefCell::new(None),
            bubble: RefCell::new(None),
            bubble_timer: RefCell::new(None),
            acc_layers: OnceCell::new(),
            acc_shapes: Cell::new([None; 3]),
            house: RefCell::new(None),
            house_hidden: Cell::new(house_hidden),
            house_drag: Cell::new(None),
            typing_on: Cell::new(typing_on),
            typing_paused: Cell::new(typing_paused),
            keytap: RefCell::new(None),
            typing_timer: RefCell::new(None),
            update_on: Cell::new(update_on),
            update_timer: RefCell::new(None),
            new_version: RefCell::new(None),
            voice_on: Cell::new(voice_on),
            listener: RefCell::new(None),
            voice_timer: RefCell::new(None),
            voice_until: Cell::new(None),
            ai_on: Cell::new(ai_on),
            receiver: RefCell::new(None),
            rng: Cell::new(now_wall() as u64 | 1),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        CONTROLLER.with(|c| {
            let _ = c.set(this.clone());
        });
        this
    }

    // ---- 구성 ----

    fn build_ui(&self) {
        let mtm = self.mtm();
        let iv = self.ivars();
        let size = self.size();

        let saved = iv.store.as_ref().and_then(|s| Some(CGPoint::new(s.setting_f64("pos_x")?, s.setting_f64("pos_y")?)));
        let origin = saved.unwrap_or_else(|| {
            // 첫 실행: 주 화면 가시 영역의 오른쪽 아래.
            let v = NSScreen::mainScreen(mtm).map(|s| s.visibleFrame()).unwrap_or(CGRect::new(
                CGPoint::new(0.0, 0.0),
                CGSize::new(800.0, 600.0),
            ));
            CGPoint::new(v.origin.x + v.size.width - size - 80.0, v.origin.y)
        });
        let origin = clamp_origin(mtm, origin, size);
        let rect = CGRect::new(origin, CGSize::new(size, size + self.head_room()));

        // 테두리 없는 비활성 패널: 클릭해도 앱이 활성화되지 않고 키 창이 되지 않는다.
        let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            rect,
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
            NSBackingStoreType::Buffered,
            false,
        );
        // SAFETY: 닫을 때 자동 release를 끄고 Retained가 수명을 관리한다.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(false);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(collection_behavior());

        let view: Retained<PetView> = unsafe {
            msg_send![PetView::alloc(mtm), initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), rect.size)]
        };
        view.setWantsLayer(true);
        let sprite = CALayer::new();
        sprite.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size, size)));
        let body = CALayer::new();
        // 도트가 뭉개지지 않게 최근접 확대.
        body.setMagnificationFilter(unsafe { objc2_quartz_core::kCAFilterNearest });
        sprite.addSublayer(&body);
        let _ = iv.body.set(body);
        if let Some(layer) = view.layer() {
            layer.addSublayer(&sprite);
        }
        // 장식은 스프라이트의 자식 레이어라 좌우 반전을 함께 따른다.
        let acc_layers = ACC_SLOTS.map(|_| {
            let layer = CALayer::new();
            layer.setMagnificationFilter(unsafe { objc2_quartz_core::kCAFilterNearest });
            sprite.addSublayer(&layer);
            layer
        });
        let _ = iv.acc_layers.set(acc_layers);
        let bowl = CALayer::new();
        bowl.setMagnificationFilter(unsafe { objc2_quartz_core::kCAFilterNearest });
        bowl.setHidden(true);
        let bowl_art = crate::art::bowl_art();
        if let Some(image) = bowl_art.cg_image() {
            // SAFETY: CGImage는 CALayer.contents로 쓸 수 있는 CF 객체이고, 레이어가 retain한다.
            unsafe { bowl.setContents(Some(&*(Retained::as_ptr(&image) as *const AnyObject))) };
            iv.bowl_size.set((bowl_art.w as f64, bowl_art.h as f64));
        }
        sprite.addSublayer(&bowl);
        let _ = iv.bowl_layer.set(bowl);
        panel.setContentView(Some(&view));
        let _ = iv.sprite.set(sprite);
        let _ = iv.panel.set(panel);

        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        menu.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(self)));
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
        item.setMenu(Some(&menu));
        let _ = iv.menu.set(menu);
        if let Some(button) = item.button(mtm) {
            button.setImage(symbol(&["pawprint.fill"], 14.0).as_deref());
            button.setImagePosition(NSCellImagePosition::ImageLeading);
        }
        let _ = iv.status_item.set(item);
        self.set_status_title("");

        let center = NSNotificationCenter::defaultCenter();
        unsafe {
            center.addObserver_selector_name_object(
                self,
                sel!(screensChanged:),
                Some(objc2_app_kit::NSApplicationDidChangeScreenParametersNotification),
                None,
            );
            let ws = NSWorkspace::sharedWorkspace().notificationCenter();
            for name in [
                objc2_app_kit::NSWorkspaceScreensDidSleepNotification,
                objc2_app_kit::NSWorkspaceSessionDidResignActiveNotification,
                objc2_app_kit::NSWorkspaceWillSleepNotification,
            ] {
                ws.addObserver_selector_name_object(self, sel!(suspend:), Some(name), None);
            }
            for name in [
                objc2_app_kit::NSWorkspaceScreensDidWakeNotification,
                objc2_app_kit::NSWorkspaceSessionDidBecomeActiveNotification,
                objc2_app_kit::NSWorkspaceDidWakeNotification,
            ] {
                ws.addObserver_selector_name_object(self, sel!(resume:), Some(name), None);
            }
        }

        // "사진 찍어 줘" 알림을 받으면 아롱이 그림을 데이터 폴더에 저장한다(받을 때만 깨어난다).
        unsafe {
            objc2_foundation::NSDistributedNotificationCenter::defaultCenter().addObserver_selector_name_object(
                self,
                sel!(photoRequested:),
                Some(ns_string!("dev.cbsong.arong.photo")),
                None,
            );
        }
        if let Some(store) = &iv.store {
            store.set_setting("launched", "1");
        }
        self.apply_outfit();
        self.apply_level();
        self.rest();
        self.apply_house();
        self.apply_visibility();
        self.sync_typing();
        self.sync_ai();
        self.save();
        match iv.update_on.get() {
            Some(true) => self.sync_update(),
            Some(false) => {}
            // 처음 한 번만 물어본다. 테스트용 디버그 빌드에서는 창을 띄우지 않는다.
            None if !cfg!(debug_assertions) => std::mem::forget(self.timer(4.0, sel!(askUpdate:), false, 1.0)),
            None => {}
        }
        #[cfg(debug_assertions)]
        if std::env::var_os("ARONG_SNAPSHOT_DIR").is_some() {
            let timer = self.timer(1.0, sel!(snapshotTick:), false, 0.0);
            std::mem::forget(timer);
        }
    }

    /// 모자가 잘리지 않도록 창 위쪽에 두는 여유 높이. 스프라이트는 창 아래쪽에 붙는다.
    fn head_room(&self) -> f64 {
        self.size() * HEAD_ROOM_PX / CELL as f64
    }

    /// 그림 1px이 차지하는 화면 pt. 중간(1배)과 크게(1.5배)는 레티나에서 화면 픽셀에 딱 맞고,
    /// 작게(0.75배)는 딱 맞지 않아 부드럽게 줄여 그린다.
    fn scale(&self) -> f64 {
        [0.75, 1.0, 1.5][self.ivars().size_level.get().min(2) as usize]
    }

    fn set_size_level(&self, level: u8) {
        let iv = self.ivars();
        iv.size_level.set(level);
        if let Some(store) = &iv.store {
            store.set_setting("size", &level.to_string());
        }
        self.finish_doorway();
        self.apply_size();
    }

    fn size(&self) -> f64 {
        CELL as f64 * self.scale()
    }

    fn apply_level(&self) {
        let iv = self.ivars();
        if let Some(panel) = iv.panel.get() {
            panel.setLevel(if iv.always_on_top.get() { NSFloatingWindowLevel } else { NSNormalWindowLevel });
            if let Some(house) = iv.house.borrow().as_ref() {
                house.setLevel(panel.level());
            }
        }
    }

    /// 크기 변경·화면 구성 변경 뒤 창을 가시 영역 안으로 맞춘다.
    fn apply_size(&self) {
        let iv = self.ivars();
        let size = self.size();
        let Some(panel) = iv.panel.get() else { return };
        let origin = clamp_origin(self.mtm(), panel.frame().origin, size);
        panel.setFrame_display(CGRect::new(origin, CGSize::new(size, size + self.head_room())), true);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(view) = panel.contentView() {
            view.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size, size + self.head_room())));
        }
        if let Some(sprite) = iv.sprite.get() {
            sprite.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(size, size)));
        }
        CATransaction::commit();
        self.show_frame();
        self.apply_house();
        self.save_position();
    }

    /// 숨김·일시정지·화면 꺼짐 상태에 맞춰 창과 타이머를 켜고 끈다.
    fn apply_visibility(&self) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        if iv.hidden.get() {
            self.end_activity();
            panel.orderOut(None);
            if let Some(house) = iv.house.borrow().as_ref() {
                house.orderOut(None);
            }
        } else {
            if let Some(house) = iv.house.borrow().as_ref() {
                house.orderFrontRegardless();
            }
            if !iv.inside.get() {
                panel.orderFrontRegardless();
            }
        }
        let quiet = iv.hidden.get() || iv.paused.get() || iv.suspended.get();
        if quiet {
            Self::stop(&iv.idle_timer);
            if iv.hidden.get() || iv.suspended.get() {
                self.rest();
            }
        } else if iv.idle_timer.borrow().is_none() {
            self.schedule_idle();
        }
    }

    // ---- 상태·저장 ----

    fn with_pet<R>(&self, f: impl FnOnce(&mut Pet, &Tuning) -> R) -> R {
        let iv = self.ivars();
        let mut pet = iv.pet.borrow_mut();
        // 날짜가 넘어갔으면 끝난 날의 합계를 일별 기록으로 남긴다.
        if let (Some(finished), Some(store)) = (pet.advance(&iv.tuning.borrow(), now_wall(), today()), &iv.store) {
            store.archive_day(&finished);
        }
        f(&mut pet, &iv.tuning.borrow())
    }

    fn save(&self) {
        let iv = self.ivars();
        if let Some(store) = &iv.store {
            if !store.save_pet(&iv.pet.borrow()) {
                iv.notice.set(Some("저장에 실패했습니다"));
            }
        }
    }

    fn save_flag(&self, key: &str, value: bool) {
        if let Some(store) = &self.ivars().store {
            store.set_setting(key, if value { "1" } else { "0" });
        }
    }

    fn save_position(&self) {
        let iv = self.ivars();
        if let (Some(store), Some(panel)) = (&iv.store, iv.panel.get()) {
            let o = panel.frame().origin;
            store.set_setting("pos_x", &format!("{:.1}", o.x));
            store.set_setting("pos_y", &format!("{:.1}", o.y));
        }
    }

    fn after_action(&self, r: ActionResult, act: Act, mut message: String) {
        self.save();
        self.wake();
        self.reload_atlas_if_needed();
        if self.ivars().walk.borrow().is_none() {
            // 레벨이 오르면 배를 보이며 구른다.
            self.perform(if r.leveled_up { Act::Roll } else { act });
        }
        if r.leveled_up {
            message.push_str(&format!(" · Lv {}!", self.ivars().pet.borrow().level()));
        }
        self.bubble(&message);
    }

    fn do_stroke(&self) {
        let r = self.with_pet(|p, t| p.stroke(t));
        let message = if r.xp > 0 { format!("좋아! +{} XP", r.xp) } else { "좋아!".to_string() };
        self.after_action(r, Act::Roll, message);
    }

    // ---- 스프라이트 ----

    fn reload_atlas_if_needed(&self) {
        let iv = self.ivars();
        let stage = iv.pet.borrow().stage(&iv.tuning.borrow());
        let (shoes, clothes) = (self.shown(Slot::Shoes), self.shown(Slot::Clothes));
        if iv.atlas.borrow().as_ref().is_some_and(|a| a.stage == stage && a.shoes == shoes && a.clothes == clothes) {
            return;
        }
        // 이전 그림은 여기서 해제된다(한 단계 분량만 유지).
        // 그림만 바꾸고 지금 동작·프레임은 건드리지 않으므로, 옷을 입고 벗어도 애니메이션이 이어진다.
        iv.atlas.replace(Atlas::load(stage, shoes.as_deref(), clothes.as_deref()));
        // 성장 단계가 바뀌면 장식 위치도 달라진다.
        self.refresh_accessories();
        self.show_frame();
    }

    fn show_frame(&self) {
        let iv = self.ivars();
        let (Some(sprite), Some(body), atlas) = (iv.sprite.get(), iv.body.get(), iv.atlas.borrow()) else { return };
        let Some(atlas) = atlas.as_ref() else { return };
        let frame = atlas.frame(iv.motion.get(), iv.frame.get());
        let (bx, by, bw, bh) = frame.rect;
        let k = self.size() / CELL as f64;
        let cell = CELL as f64;
        CATransaction::begin();
        // 암시적 애니메이션(0.25초 교차 페이드)을 끈다.
        CATransaction::setDisableActions(true);
        // SAFETY: CGImage는 CALayer.contents로 쓸 수 있는 CF 객체이고, 레이어가 retain한다.
        unsafe { body.setContents(Some(&*(CFRetained::as_ptr(&frame.image).as_ptr() as *const AnyObject))) };
        // 화면 픽셀에 딱 맞는 그림은 최근접으로 또렷하게, 그렇지 않은 경우(걷기 그림, 작게 보기)는 선형으로 매끄럽게.
        let small = iv.size_level.get() == 0;
        body.setMagnificationFilter(unsafe {
            if frame.view == View::Side || small { objc2_quartz_core::kCAFilterLinear } else { objc2_quartz_core::kCAFilterNearest }
        });
        // 그림이 있는 영역만 덮는다(스프라이트 px → 레이어 좌표, y는 위가 +).
        body.setFrame(CGRect::new(CGPoint::new(bx * k, (cell - by - bh) * k), CGSize::new(bw * k, bh * k)));
        // 원본 스프라이트는 왼쪽을 본다. 오른쪽으로 갈 때만 뒤집는다.
        let sx = if iv.facing_left.get() { 1.0 } else { -1.0 };
        // 집을 드나들 때는 발 위치를 그대로 둔 채 몸만 작아진다(멀어지는 느낌).
        let shrink = iv.shrink.get();
        let feet = (cell - GROUND) * k;
        let lift = -(self.size() / 2.0 - feet) * (1.0 - shrink);
        sprite.setAffineTransform(CGAffineTransform { a: sx * shrink, b: 0.0, c: 0.0, d: shrink, tx: 0.0, ty: lift });
        if let Some(layers) = iv.acc_layers.get() {
            // 프레임마다 계산해 둔 머리 꼭대기를 기준으로 장식을 얹는다.
            let (ax, ay) = frame.head;
            for (layer, shape) in layers.iter().zip(iv.acc_shapes.get()) {
                // 구르는 동안처럼 머리 위치를 알 수 없는 프레임에서는 장식을 잠깐 숨긴다.
                layer.setHidden(!frame.wearable);
                layer.setMagnificationFilter(unsafe { if small { objc2_quartz_core::kCAFilterLinear } else { objc2_quartz_core::kCAFilterNearest } });
                if let Some((w, h, offsets)) = shape {
                    let (dx, dy) = offsets[match frame.view {
                        View::Quarter => 0,
                        View::Side => 1,
                        View::Front => 2,
                    }];
                    layer.setFrame(CGRect::new(
                        CGPoint::new((ax + dx - w / 2.0) * k, (cell - (ay + dy + h)) * k),
                        CGSize::new(w * k, h * k),
                    ));
                }
            }
        }
        if let Some(bowl) = iv.bowl_layer.get() {
            // 밥그릇은 얼굴 앞(왼쪽) 바닥에 놓는다.
            let (w, h) = iv.bowl_size.get();
            bowl.setHidden(!iv.bowl.get());
            bowl.setFrame(CGRect::new(CGPoint::new(2.0 * k, (cell - GROUND) * k), CGSize::new(w * k, h * k)));
        }
        CATransaction::commit();
    }

    /// 행동 하나를 프레임 목록으로 풀어 재생한다. 타이머는 재생 중에만 돈다.
    fn perform(&self, act: Act) {
        let iv = self.ivars();
        let mut list: Vec<(Motion, usize)> = Vec::new();
        {
            let atlas = iv.atlas.borrow();
            let Some(a) = atlas.as_ref() else { return };
            // 전체 / 서 있다가 가장 깊은 자세까지 / 거기서 다시 서기까지 / 그 자세로 잠깐 머물기.
            let all = |m: Motion| (0..a.len(m)).map(move |i| (m, i));
            let down = |m: Motion| (0..=a.hold(m)).map(move |i| (m, i));
            let up = |m: Motion| (a.hold(m)..a.len(m)).map(move |i| (m, i));
            let pause = |m: Motion, ticks: usize| std::iter::repeat_n((m, a.hold(m)), ticks);
            match act {
                Act::Paw(left) => list.extend(all(if left { Motion::PawLeft } else { Motion::PawRight })),
                Act::Sit | Act::SitUp | Act::Eat => {
                    let m = if matches!(act, Act::SitUp) { Motion::SitUp } else { Motion::Sit };
                    list.extend(down(m));
                    list.extend(pause(m, if matches!(act, Act::Eat) { 22 } else { 12 }));
                    list.extend(up(m));
                }
                Act::Nap => {
                    list.extend(down(Motion::LieDown));
                    list.extend(pause(Motion::LieDown, 36));
                    list.extend(up(Motion::LieDown));
                }
                Act::Roll => {
                    list.extend(down(Motion::LieDown));
                    list.extend(all(Motion::Roll));
                    list.extend(up(Motion::LieDown));
                }
                Act::LieStay => list.extend(down(Motion::LieDown)),
                Act::SitStay => list.extend(down(Motion::Sit)),
                Act::Pant => {
                    for _ in 0..4 {
                        list.extend(all(Motion::Pant));
                    }
                }
                Act::Look => {
                    // 원본이 느린 동작이라 프레임마다 두 번씩 보여 준다.
                    for _ in 0..2 {
                        list.extend(all(Motion::Look).flat_map(|f| [f, f]));
                    }
                }
            }
        }
        iv.held.set(false);
        iv.stay_after.set(matches!(act, Act::LieStay | Act::SitStay));
        iv.bowl.set(matches!(act, Act::Eat));
        iv.script.replace(list.into());
        self.advance_script();
        if iv.anim_timer.borrow().is_none() {
            let timer = self.timer(1.0 / FPS, sel!(animTick:), true, 0.02);
            iv.anim_timer.replace(Some(timer));
        }
    }

    /// 목록에서 다음 프레임을 꺼내 보여 준다. 더 없으면 `false`.
    fn advance_script(&self) -> bool {
        let iv = self.ivars();
        let next = iv.script.borrow_mut().pop_front();
        match next {
            Some((motion, frame)) => {
                iv.motion.set(motion);
                iv.frame.set(frame);
                self.show_frame();
                true
            }
            None => false,
        }
    }

    /// 정지 화면: 프레임 타이머를 끄고 서 있는 그림 한 장만 띄워 둔다.
    fn rest(&self) {
        let iv = self.ivars();
        Self::stop(&iv.anim_timer);
        iv.script.borrow_mut().clear();
        iv.stay_after.set(false);
        iv.held.set(false);
        iv.bowl.set(false);
        iv.motion.set(Motion::PawLeft);
        iv.frame.set(0);
        iv.hop.set((0.0, 0.0));
        self.show_frame();
    }

    /// 집 안에 있거나 엎드려 머무는 중이면 깨워서 서 있는 상태로 돌린다.
    fn wake(&self) {
        let iv = self.ivars();
        // 드나드는 중이었다면 그 연출은 건너뛴다.
        if let Some(door) = iv.doorway.take() {
            Self::stop(&iv.door_timer);
            iv.inside.set(true);
            if let Some(panel) = iv.panel.get() {
                panel.setFrameOrigin(door.front);
            }
        }
        iv.shrink.set(1.0);
        if let Some(panel) = iv.panel.get() {
            panel.setAlphaValue(1.0);
            if iv.inside.replace(false) && !iv.hidden.get() {
                panel.orderFrontRegardless();
                self.rest();
            }
        }
        if iv.held.get() {
            self.rest();
        }
    }

    /// 이동 중의 걸음: 걷기 프레임을 시간이 아니라 "이동한 거리"에 맞춰 넘긴다.
    /// 그래서 천천히 가면 천천히, 빨리 끌면 빨리 발을 구르고, 발이 미끄러져 보이지 않는다.
    fn stride(&self, moved_pt: f64, running: bool) {
        let iv = self.ivars();
        Self::stop(&iv.anim_timer);
        iv.script.borrow_mut().clear();
        iv.stay_after.set(false);
        iv.held.set(false);
        iv.bowl.set(false);
        iv.motion.set(Motion::Walk);
        let k = self.size() / CELL as f64;
        // 걷기 한 사이클(12프레임) 동안 나아가는 거리. 달릴 때는 보폭을 넓게 본다.
        let cycle = if running { WALK_CYCLE_PT * 1.5 } else { WALK_CYCLE_PT } * k;
        let phase = (iv.hop.get().0 + moved_pt / cycle).fract();
        iv.hop.set((phase, 0.0));
        let count = iv.atlas.borrow().as_ref().map_or(1, |a| a.len(Motion::Walk));
        iv.frame.set(((phase * count as f64) as usize).min(count - 1));
        self.show_frame();
    }

    // ---- 타이머 ----

    fn timer(&self, interval: f64, selector: Sel, repeats: bool, tolerance: f64) -> Retained<NSTimer> {
        // SAFETY: 대상은 프로세스 수명 동안 살아 있는 컨트롤러이고, 셀렉터는 위에서 정의한
        // `fn(&self, &NSTimer)` 메서드다.
        let timer = unsafe {
            NSTimer::timerWithTimeInterval_target_selector_userInfo_repeats(interval, self, selector, None, repeats)
        };
        timer.setTolerance(tolerance);
        // 메뉴 추적·드래그 중에도 돌도록 common 모드에 건다.
        unsafe { NSRunLoop::mainRunLoop().addTimer_forMode(&timer, objc2_foundation::NSRunLoopCommonModes) };
        timer
    }

    fn stop(slot: &RefCell<Option<Retained<NSTimer>>>) {
        if let Some(timer) = slot.borrow_mut().take() {
            timer.invalidate();
        }
    }

    fn next_random(&self) -> u64 {
        let iv = self.ivars();
        let mut x = iv.rng.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        iv.rng.set(x);
        x
    }

    fn schedule_idle(&self) {
        let iv = self.ivars();
        if iv.hidden.get() || iv.paused.get() || iv.suspended.get() {
            return;
        }
        // 10~30초에 한 번, 넉넉한 허용 오차로 다른 깨우기와 합쳐지게 한다.
        let wait = 10.0 + (self.next_random() % 20) as f64;
        let timer = self.timer(wait, sel!(idleTick:), false, 5.0);
        iv.idle_timer.replace(Some(timer));
    }

    fn set_status_title(&self, title: &str) {
        let iv = self.ivars();
        if let Some(button) = iv.status_item.get().and_then(|i| i.button(self.mtm())) {
            button.setTitle(&NSString::from_str(title));
        }
    }

    // ---- 메뉴 ----

    fn rebuild_menu(&self, menu: &NSMenu) {
        // 메뉴에 보이는 오늘 수치가 최신이 되도록 먼저 반영한다.
        self.flush_typing();
        self.flush_ai();
        let mtm = self.mtm();
        let iv = self.ivars();
        let (level, in_level, stage, coins, satiety, mood, affinity) = self.with_pet(|p, t| {
            let (level, in_level) = arong_core::pet::level_of(p.xp);
            (level, in_level, p.stage(t), p.coins, p.satiety, p.mood, p.affinity)
        });
        menu.removeAllItems();
        let add = |title: &str, action: Option<Sel>, checked: bool| {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(title),
                    action,
                    ns_string!(""),
                )
            };
            match action {
                Some(_) => unsafe { item.setTarget(Some(self)) },
                None => item.setEnabled(false),
            }
            item.setState(if checked { 1 } else { 0 });
            menu.addItem(&item);
            item
        };
        add(&format!("아롱이 · Lv {level} ({})", stage_name(stage)), None, false);
        add(
            &format!("XP {in_level}/{} · 재화 {coins}", arong_core::pet::xp_to_next(level)),
            None,
            false,
        );
        add(&format!("포만감 {satiety:.0} · 기분 {mood:.0} · 친밀도 {affinity}"), None, false);
        if let Some(notice) = iv.notice.get() {
            add(notice, None, false);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add("밥 주기", Some(sel!(feed:)), false);
        let snack = add(&format!("간식 주기 (재화 {})", iv.tuning.borrow().snack_price), Some(sel!(snack:)), false);
        snack.setEnabled(coins >= iv.tuning.borrow().snack_price);
        add("쓰다듬기", Some(sel!(stroke:)), false);
        add("산책 모드", Some(sel!(toggleWalk:)), iv.walk.borrow().is_some());
        add("우클릭: 타일 메뉴 · 끌기: 위치 이동", None, false);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        add("항상 위에 표시", Some(sel!(toggleTop:)), iv.always_on_top.get());
        add("위치 고정 (끌기·혼자 걷기 잠금)", Some(sel!(toggleLock:)), iv.locked.get());
        add("혼자 걷기 (좌우로 돌아다님)", Some(sel!(toggleRoam:)), iv.roaming.get());
        if iv.pet.borrow().house_level > 0 {
            // 가진 것이 방석이면 "방석 숨기기", 집이면 "집 숨기기"로 보여 준다.
            add(&format!("{} 숨기기", self.house_word()), Some(sel!(toggleHouse:)), iv.house_hidden.get());
        }
        let level = iv.size_level.get();
        add("크기: 작게", Some(sel!(sizeSmall:)), level == 0);
        add("크기: 중간", Some(sel!(sizeMedium:)), level == 1);
        add("크기: 크게", Some(sel!(sizeLarge:)), level == 2);
        add("숨기기", Some(sel!(toggleHidden:)), iv.hidden.get());
        add("일시정지 (대기 동작 끔)", Some(sel!(togglePause:)), iv.paused.get());
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let keys = iv.pet.borrow().day.keys;
        add(
            &format!("타이핑 집계: {} · 오늘 {keys}타", self.typing_status()),
            Some(sel!(toggleTyping:)),
            iv.typing_on.get(),
        );
        if iv.typing_on.get() {
            add("타이핑 집계 일시정지", Some(sel!(toggleTypingPause:)), iv.typing_paused.get());
        }
        let tokens = iv.pet.borrow().day.tokens;
        add(
            &format!("AI 사용량 집계 (Claude Code): {}", self.ai_status()),
            Some(sel!(toggleAi:)),
            iv.ai_on.get(),
        );
        if iv.ai_on.get() {
            add(
                &format!(
                    "  오늘 실측: 출력 {} · 입력 {} · 캐시 읽기 {} · 캐시 쓰기 {}",
                    compact(tokens[1]),
                    compact(tokens[0]),
                    compact(tokens[2]),
                    compact(tokens[3])
                ),
                None,
                false,
            );
            add("  Claude Code 설정 다시 복사", Some(sel!(copyAiConfig:)), false);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        // SAFETY: 상태 조회만 한다.
        let login_on = unsafe {
            objc2_service_management::SMAppService::mainAppService().status()
                == objc2_service_management::SMAppServiceStatus::Enabled
        };
        add("음성 명령 (훈련에서 누를 때만 마이크 사용)", Some(sel!(toggleVoice:)), iv.voice_on.get());
        add("Mac 켤 때 자동 실행", Some(sel!(toggleLogin:)), login_on);
        add("새 버전 확인 (하루 한 번 GitHub 접속)", Some(sel!(toggleUpdate:)), iv.update_on.get() == Some(true));
        if let Some(version) = iv.new_version.borrow().as_ref() {
            add(&format!("  새 버전 {version} 있음 · 업데이트 명령 복사"), Some(sel!(copyUpdateCommand:)), false);
        }
        add("사진 찍기…", Some(sel!(takePhoto:)), false);
        add("육성 데이터 내보내기…", Some(sel!(exportData:)), false);
        add("육성 데이터 가져오기…", Some(sel!(importData:)), false);
        add("보상 규칙…", Some(sel!(editRewards:)), false);
        // 최근 기록: 오늘과 지난 7일의 합계만 보여 준다.
        let history = NSMenu::new(mtm);
        history.setAutoenablesItems(false);
        let line = |d: &arong_core::DayCounters, name: String| {
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &NSString::from_str(&format!(
                        "{name}  XP {} · {}타 · 출력 토큰 {} · 산책 {}회",
                        d.xp,
                        d.keys,
                        compact(d.tokens[1]),
                        d.walks
                    )),
                    None,
                    ns_string!(""),
                )
            };
            item.setEnabled(false);
            history.addItem(&item);
        };
        line(&iv.pet.borrow().day, "오늘".to_string());
        for day in iv.store.as_ref().map(|s| s.recent_days(7)).unwrap_or_default() {
            line(&day, short_date(day.date));
        }
        let history_item = add("최근 기록", Some(sel!(noop:)), false);
        history_item.setSubmenu(Some(&history));
        add("활동 집계 데이터만 삭제…", Some(sel!(resetActivity:)), false);
        add("육성 데이터 초기화…", Some(sel!(resetData:)), false);
        let quit = add("아롱이 종료", Some(sel!(terminate:)), false);
        unsafe { quit.setTarget(Some(&NSApplication::sharedApplication(mtm))) };
    }

    // ---- 마우스: 쓰다듬기 / 이동 ----

    fn on_mouse_down(&self) {
        let iv = self.ivars();
        self.close_tiles();
        iv.tiles_trash.borrow_mut().clear();
        self.finish_doorway();
        self.stop_roam();
        if iv.walk.borrow().is_some() {
            // 산책 중 아롱이를 클릭하면 산책을 끝낸다.
            self.end_walk(true);
            iv.drag.replace(Drag::Ignored);
        } else {
            iv.drag.replace(Drag::Pressed { start: NSEvent::mouseLocation() });
        }
    }

    fn on_mouse_dragged(&self) {
        let iv = self.ivars();
        let cursor = NSEvent::mouseLocation();
        let Some(panel) = iv.panel.get() else { return };
        let pressed = match &*iv.drag.borrow() {
            Drag::Pressed { start } if dist(*start, cursor) >= CLICK_SLOP => Some(*start),
            _ => None,
        };
        if let Some(start) = pressed {
            let origin = panel.frame().origin;
            iv.drag.replace(if iv.locked.get() {
                Drag::Ignored
            } else {
                Drag::Moving { offset: CGPoint::new(origin.x - start.x, origin.y - start.y) }
            });
        }
        let moving = match &*iv.drag.borrow() {
            Drag::Moving { offset } => Some(*offset),
            _ => None,
        };
        if let Some(offset) = moving {
            let target = CGPoint::new(cursor.x + offset.x, cursor.y + offset.y);
            panel.setFrameOrigin(clamp_origin(self.mtm(), target, self.size()));
        }
    }

    fn on_mouse_up(&self) {
        let was_click = matches!(*self.ivars().drag.borrow(), Drag::Pressed { .. });
        self.end_drag();
        if was_click {
            self.do_stroke();
        }
    }

    fn end_drag(&self) {
        if let Drag::Moving { .. } = self.ivars().drag.replace(Drag::None) {
            self.save_position();
        }
    }

    /// 화면 꺼짐·숨김·모니터 변경 등: 진행 중인 움직임을 모두 안전하게 끝낸다.
    fn end_activity(&self) {
        self.close_tiles();
        self.stop_listening();
        self.finish_doorway();
        self.end_walk(true);
        self.stop_roam();
        self.end_drag();
    }

    // ---- 타일 메뉴 ----

    fn toggle_tiles(&self) {
        if self.ivars().tiles_open.get() {
            self.close_tiles();
        } else {
            self.ivars().tiles_anchor.set(None);
            self.open_main_tiles();
        }
    }

    /// 메뉴를 끌어서 옮겼으면 그 자리를 기억한다.
    fn tiles_dragged(&self) {
        let iv = self.ivars();
        if !iv.tiles_open.get() {
            return;
        }
        let Some(panel) = iv.tiles.borrow().clone() else { return };
        let f = panel.frame();
        let placed = iv.tiles_placed.get();
        if (f.origin.x - placed.x).abs() > 1.0 || (f.origin.y - placed.y).abs() > 1.0 {
            iv.tiles_anchor.set(Some(CGPoint::new(f.origin.x, f.origin.y + f.size.height)));
            iv.tiles_placed.set(f.origin);
        }
    }

    fn close_tiles(&self) {
        let iv = self.ivars();
        if !iv.tiles_open.replace(false) {
            return;
        }
        self.set_preview(None);
        // 해제는 나중에 한다. 지금은 타일의 이벤트 처리 중일 수 있다.
        let panel = iv.tiles.borrow().clone();
        if let Some(panel) = panel {
            panel.orderOut(None);
        }
    }

    /// 타일에 마우스가 올라오거나 나갔다. 아직 안 가진 꾸미기 아이템이면 화면의 아롱이에게 미리 입혀 본다.
    fn tile_hover(&self, id: Option<usize>) {
        let iv = self.ivars();
        let item = id.and_then(|id| match iv.tile_actions.borrow().get(id) {
            Some(TileAction::Item(item)) => arong_core::shop::item(item),
            _ => None,
        });
        let preview = item.filter(|def| !iv.pet.borrow().owned.contains(def.id)).map(|def| (def.slot, def.id.to_string()));
        self.set_preview(preview);
    }

    fn set_preview(&self, preview: Option<(Slot, String)>) {
        let iv = self.ivars();
        if *iv.preview.borrow() == preview {
            return;
        }
        iv.preview.replace(preview);
        self.apply_outfit();
    }

    /// 화면에 보여 줄 착용 아이템: 미리 입어 보는 중이면 그것을, 아니면 실제로 입은 것을.
    fn shown(&self, slot: Slot) -> Option<String> {
        let iv = self.ivars();
        if let Some((_, id)) = iv.preview.borrow().as_ref().filter(|(s, _)| *s == slot) {
            return Some(id.clone());
        }
        iv.pet.borrow().wearing(slot).map(str::to_string)
    }

    fn open_main_tiles(&self) {
        let iv = self.ivars();
        let (level, in_level, stage, coins, satiety, mood) = self.with_pet(|p, t| {
            let (level, in_level) = arong_core::pet::level_of(p.xp);
            (level, in_level, p.stage(t), p.coins, p.satiety, p.mood)
        });
        let walking = iv.walk.borrow().is_some();
        let tile = |symbols: &'static [&'static str], caption: &str, enabled: bool, active: bool, action: TileAction| TileSpec {
            icon: TileIcon::Symbol(symbols),
            caption: caption.to_string(),
            enabled,
            active,
            action,
        };
        let tiles = vec![
            tile(&["fork.knife"], "밥 주기", true, false, TileAction::Send(sel!(feed:))),
            tile(
                &["birthday.cake.fill", "gift.fill"],
                &format!("간식 ({})", iv.tuning.borrow().snack_price),
                coins >= iv.tuning.borrow().snack_price,
                false,
                TileAction::Send(sel!(snack:)),
            ),
            tile(&["heart.fill"], "쓰다듬기", true, false, TileAction::Send(sel!(stroke:))),
            tile(&["figure.walk"], if walking { "산책 끝내기" } else { "산책" }, true, walking, TileAction::Send(sel!(toggleWalk:))),
            tile(&["bag.fill", "cart.fill"], "상점", true, false, TileAction::Shop),
            tile(&["pawprint.fill"], "혼자 걷기", true, iv.roaming.get(), TileAction::Send(sel!(toggleRoam:))),
            tile(&["pin.fill"], "위치 고정", true, iv.locked.get(), TileAction::Send(sel!(toggleLock:))),
            tile(&["graduationcap.fill", "star.fill"], "훈련", true, false, TileAction::Training),
            tile(&["ellipsis.circle.fill", "ellipsis.circle"], "더보기", true, false, TileAction::More),
        ];
        self.show_tiles(
            &format!("아롱이 · Lv {level} ({}) · 재화 {coins}", stage_name(stage)),
            &format!("XP {in_level}/{} · 포만감 {satiety:.0} · 기분 {mood:.0}", arong_core::pet::xp_to_next(level)),
            tiles,
        );
    }

    fn open_shop_tiles(&self) {
        let (coins, worn, house_level, stage) = self.with_pet(|p, t| (p.coins, p.worn.clone(), p.house_level, p.stage(t)));
        let mut tiles = Vec::new();
        for slot in Slot::ALL {
            // 옷은 옷 입은 그림이 있는 것만 판다. 하나도 없으면 칸을 내놓지 않는다(가진 옷과 저장 데이터는 그대로 둔다).
            if slot == Slot::Clothes && !arong_core::ITEMS.iter().any(|d| d.slot == slot && crate::sprites::has_outfit(d.id)) {
                continue;
            }
            let first = arong_core::ITEMS.iter().find(|d| d.slot == slot).map(|d| d.id);
            let icon = match slot {
                Slot::Clothes => TileIcon::Symbol(&["tshirt.fill", "hanger"]),
                Slot::Shoes => item_icon(first.unwrap_or(""), stage),
                _ => item_icon(first.unwrap_or(""), stage),
            };
            let caption = match worn.get(&slot).and_then(|id| arong_core::shop::item(id)) {
                Some(def) => format!("{} · {}", slot.name(), def.name),
                None => slot.name().to_string(),
            };
            tiles.push(TileSpec { icon, caption, enabled: true, active: false, action: TileAction::Category(slot) });
        }
        let next = house_level + 1;
        let (caption, enabled, shown) = match arong_core::shop::HOUSE_PRICES.get(house_level as usize) {
            Some(price) => (
                format!("{} · {}", arong_core::HOUSE_NAMES[next as usize], price),
                coins >= *price,
                next,
            ),
            None => ("집 · 최고 단계".to_string(), false, house_level),
        };
        tiles.push(TileSpec {
            icon: art_icon(crate::art::house_art(shown), 0.6),
            caption,
            enabled,
            active: false,
            action: TileAction::House,
        });
        tiles.push(TileSpec {
            icon: TileIcon::Symbol(&["arrow.uturn.backward.circle.fill", "arrow.uturn.backward"]),
            caption: "돌아가기".to_string(),
            enabled: true,
            active: false,
            action: TileAction::MainMenu,
        });
        self.show_tiles(&format!("상점 · 재화 {coins}"), "부위를 골라 구경하세요", tiles);
    }

    fn open_category_tiles(&self, slot: Slot) {
        let (coins, owned, worn, stage) =
            self.with_pet(|p, t| (p.coins, p.owned.clone(), p.wearing(slot).map(str::to_string), p.stage(t)));
        // 목록이 길면 스크롤해야 하므로, 돌아가기는 늘 보이는 맨 앞에 둔다.
        let mut tiles = Vec::new();
        tiles.push(TileSpec {
            icon: TileIcon::Symbol(&["arrow.uturn.backward.circle.fill", "arrow.uturn.backward"]),
            caption: "돌아가기".to_string(),
            enabled: true,
            active: false,
            action: TileAction::Shop,
        });
        // 옷은 옷 입은 그림이 있는 것만 보여 준다.
        let shown = |d: &&arong_core::shop::ItemDef| d.slot == slot && (slot != Slot::Clothes || crate::sprites::has_outfit(d.id));
        for def in arong_core::ITEMS.iter().filter(shown) {
            let has = owned.contains(def.id);
            let wearing = worn.as_deref() == Some(def.id);
            let caption = if wearing {
                format!("{} · 착용 중", def.name)
            } else if has {
                format!("{} · 보유", def.name)
            } else {
                format!("{} · {}", def.name, def.price)
            };
            tiles.push(TileSpec {
                icon: item_icon(def.id, stage),
                caption,
                enabled: has || coins >= def.price,
                active: wearing,
                action: TileAction::Item(def.id),
            });
        }
        self.show_tiles(&format!("{} · 재화 {coins}", slot.name()), "올려 두면 미리 입어 보기 · 누르면 구매, 입고 벗기", tiles);
    }

    fn show_tiles(&self, title: &str, subtitle: &str, tiles: Vec<TileSpec>) {
        const TILE_W: f64 = 92.0;
        const TILE_H: f64 = 76.0;
        const GAP: f64 = 8.0;
        const HEADER: f64 = 46.0;

        let mtm = self.mtm();
        let iv = self.ivars();
        let Some(pet_panel) = iv.panel.get() else { return };
        self.stop_roam();

        // 타일이 많아도(옷처럼) 창을 키우지 않는다. 이 줄 수를 넘으면 창 안에서 스크롤한다.
        const MAX_ROWS: usize = 4;
        let cols: usize = 3;
        let count = tiles.len();
        let rows = count.div_ceil(cols);
        let shown_rows = rows.min(MAX_ROWS);
        let width = cols as f64 * TILE_W + (cols as f64 + 1.0) * GAP;
        let grid_height = rows as f64 * TILE_H + (rows as f64 + 1.0) * GAP;
        let shown_height = shown_rows as f64 * TILE_H + (shown_rows as f64 + 1.0) * GAP;
        let height = HEADER + shown_height;

        // 아롱이 바로 위(자리가 없으면 아래)에 띄운다.
        let pf = pet_panel.frame();
        let above = CGPoint::new(pf.origin.x + pf.size.width / 2.0 - width / 2.0, pf.origin.y + pf.size.height + 6.0);
        // 위, 아래, 오른쪽, 왼쪽 순서로 아롱이를 가리지 않는 자리를 찾는다(미리 입어 보기가 보여야 한다).
        let middle = pf.origin.y + pf.size.height / 2.0 - height / 2.0;
        let spots = [
            above,
            CGPoint::new(above.x, pf.origin.y - height - 6.0),
            CGPoint::new(pf.origin.x + pf.size.width + 6.0, middle),
            CGPoint::new(pf.origin.x - width - 6.0, middle),
        ];
        let covers = |o: CGPoint| {
            o.x < pf.origin.x + pf.size.width && o.x + width > pf.origin.x && o.y < pf.origin.y + pf.size.height && o.y + height > pf.origin.y
        };
        let origin = spots
            .into_iter()
            .map(|spot| clamp_rect(mtm, spot, width, height))
            .find(|o| !covers(*o))
            .unwrap_or_else(|| clamp_rect(mtm, above, width, height));
        // 사용자가 끌어다 놓은 자리가 있으면 그 자리(왼쪽 위 모서리 기준)에 띄운다.
        let origin = match iv.tiles_anchor.get() {
            Some(anchor) => clamp_rect(mtm, CGPoint::new(anchor.x, anchor.y - height), width, height),
            None => origin,
        };
        iv.tiles_placed.set(origin);
        let rect = CGRect::new(origin, CGSize::new(width, height));
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), rect.size);

        let panel: Retained<TilePanel> = unsafe {
            msg_send![
                TilePanel::alloc(mtm),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                backing: NSBackingStoreType::Buffered,
                defer: false,
            ]
        };
        // SAFETY: close를 부르지 않고 Retained가 수명을 관리한다.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setOpaque(false);
        panel.setBackgroundColor(Some(&NSColor::clearColor()));
        panel.setHasShadow(true);
        panel.setHidesOnDeactivate(false);
        panel.setCollectionBehavior(collection_behavior());
        panel.setLevel(NSFloatingWindowLevel + 1);
        panel.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(self)));

        let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), bounds);
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        effect.setWantsLayer(true);
        if let Some(layer) = effect.layer() {
            layer.setCornerRadius(18.0);
            layer.setMasksToBounds(true);
        }

        let label = |text: &str, size: f64, bold: bool, frame: CGRect| {
            let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            let font = if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) };
            field.setFont(Some(&font));
            field.setAlignment(NSTextAlignment::Center);
            field.setFrame(frame);
            field
        };

        let title = label(title, 13.0, true, CGRect::new(CGPoint::new(GAP, height - 26.0), CGSize::new(width - 2.0 * GAP, 18.0)));
        effect.addSubview(&title);
        let stats = label(subtitle, 11.0, false, CGRect::new(CGPoint::new(GAP, height - 44.0), CGSize::new(width - 2.0 * GAP, 16.0)));
        stats.setTextColor(Some(&NSColor::secondaryLabelColor()));
        effect.addSubview(&stats);

        // 타일을 담는 판. 줄이 많으면 이 판을 스크롤 뷰에 넣는다.
        let grid = NSView::initWithFrame(mtm.alloc(), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, grid_height)));
        let mut actions = Vec::with_capacity(tiles.len());
        for (id, spec) in tiles.into_iter().enumerate() {
            let (col, row) = (id % cols, id / cols);
            let frame = CGRect::new(
                CGPoint::new(
                    GAP + col as f64 * (TILE_W + GAP),
                    grid_height - GAP - (row as f64 + 1.0) * TILE_H - row as f64 * GAP,
                ),
                CGSize::new(TILE_W, TILE_H),
            );
            let tile = TileView::alloc(mtm).set_ivars(TileIvars { id, enabled: spec.enabled, active: spec.active });
            let tile: Retained<TileView> = unsafe { msg_send![super(tile), initWithFrame: frame] };
            tile.setWantsLayer(true);
            if let Some(layer) = tile.layer() {
                layer.setCornerRadius(14.0);
            }
            tile.paint(false);
            // SAFETY: 소유자는 이 타일이고, 타일이 mouseEntered:/mouseExited:를 구현한다.
            let tracking = unsafe {
                NSTrackingArea::initWithRect_options_owner_userInfo(
                    NSTrackingArea::alloc(),
                    CGRect::new(CGPoint::new(0.0, 0.0), frame.size),
                    NSTrackingAreaOptions::MouseEnteredAndExited
                        | NSTrackingAreaOptions::ActiveAlways
                        | NSTrackingAreaOptions::InVisibleRect,
                    Some(&tile),
                    None,
                )
            };
            tile.addTrackingArea(&tracking);

            let icon = NSImageView::new(mtm);
            icon.setFrame(CGRect::new(CGPoint::new(0.0, 28.0), CGSize::new(TILE_W, 40.0)));
            match &spec.icon {
                TileIcon::Symbol(names) => {
                    icon.setImage(symbol(names, 24.0).as_deref());
                    let tint = if spec.enabled { NSColor::labelColor() } else { NSColor::secondaryLabelColor() };
                    icon.setContentTintColor(Some(&tint));
                }
                TileIcon::Image(image) => {
                    icon.setImage(image.as_deref());
                    if !spec.enabled {
                        icon.setAlphaValue(0.6);
                    }
                }
            }
            let caption = label(&spec.caption, 10.5, false, CGRect::new(CGPoint::new(1.0, 8.0), CGSize::new(TILE_W - 2.0, 15.0)));
            if !spec.enabled {
                caption.setTextColor(Some(&NSColor::secondaryLabelColor()));
            }
            tile.addSubview(&icon);
            tile.addSubview(&caption);
            grid.addSubview(&tile);
            actions.push(spec.action);
        }

        // 같은 목록을 다시 그릴 때(구매·착용 직후)는 보던 스크롤 위치를 유지한다.
        let kept = iv.tiles_scroll.borrow().as_ref().filter(|_| iv.tiles_count.get() == count).map(|old| old.contentView().bounds().origin.y);
        iv.tiles_count.set(count);
        if rows > shown_rows {
            let scroll = NSScrollView::initWithFrame(mtm.alloc(), CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width, shown_height)));
            scroll.setDrawsBackground(false);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            scroll.setDocumentView(Some(&grid));
            // 처음에는 맨 위(첫 줄)를 보여 준다.
            let y = kept.unwrap_or(grid_height - shown_height).clamp(0.0, grid_height - shown_height);
            grid.scrollPoint(CGPoint::new(0.0, y));
            effect.addSubview(&scroll);
            iv.tiles_scroll.replace(Some(scroll));
        } else {
            effect.addSubview(&grid);
            iv.tiles_scroll.replace(None);
        }
        panel.setContentView(Some(&effect));
        // 이전 패널은 지금 그 패널의 타일 이벤트 처리 중일 수 있어 바로 해제하지 않는다.
        if let Some(old) = iv.tiles.replace(Some(panel.clone())) {
            old.orderOut(None);
            iv.tiles_trash.borrow_mut().push(old);
        }
        iv.tile_actions.replace(actions);
        iv.tiles_open.set(true);
        panel.makeKeyAndOrderFront(None);
    }

    fn tile_clicked(&self, id: usize) {
        let Some(action) = self.ivars().tile_actions.borrow().get(id).cloned() else { return };
        self.close_tiles();
        match action {
            TileAction::Send(action) => {
                // SAFETY: 타일에 넣는 셀렉터는 모두 이 클래스에 정의된 `fn(&self, Option<&AnyObject>)` 액션이다.
                let _: *mut AnyObject =
                    unsafe { msg_send![self, performSelector: action, withObject: std::ptr::null::<AnyObject>()] };
            }
            TileAction::More => {
                // 나머지 설정은 메뉴 막대와 같은 메뉴를 아롱이 옆에 띄운다.
                let iv = self.ivars();
                if let (Some(menu), Some(view)) = (iv.menu.get(), iv.panel.get().and_then(|p| p.contentView())) {
                    menu.popUpMenuPositioningItem_atLocation_inView(None, CGPoint::new(0.0, self.size()), Some(&view));
                }
            }
            TileAction::Shop => self.open_shop_tiles(),
            TileAction::Training => self.open_training_tiles(),
            TileAction::Train(command) => {
                self.do_command(command);
                self.open_training_tiles();
            }
            TileAction::Voice => {
                self.start_listening();
                self.open_training_tiles();
            }
            TileAction::MainMenu => self.open_main_tiles(),
            TileAction::Category(slot) => self.open_category_tiles(slot),
            TileAction::Item(id) => {
                let Some(def) = arong_core::shop::item(id) else { return };
                let message = self.with_pet(|p, _| {
                    if !p.owned.contains(id) {
                        match p.buy(id) {
                            Purchase::Bought => {
                                p.wear(id);
                                format!("{} 구매!", def.name)
                            }
                            _ => "재화가 부족해요".to_string(),
                        }
                    } else if p.wearing(def.slot) == Some(id) {
                        p.take_off(def.slot);
                        format!("{} 벗음", def.name)
                    } else {
                        p.wear(id);
                        format!("{} 착용", def.name)
                    }
                });
                self.save();
                self.apply_outfit();
                self.bubble(&message);
                self.open_category_tiles(def.slot);
            }
            TileAction::House => {
                let message = self.with_pet(|p, _| match p.upgrade_house() {
                    Purchase::Bought => format!("집: {}", arong_core::HOUSE_NAMES[p.house_level as usize]),
                    Purchase::MaxLevel => "이미 최고 단계예요".to_string(),
                    _ => "재화가 부족해요".to_string(),
                });
                self.save();
                self.apply_house();
                self.bubble(&message);
                self.open_shop_tiles();
            }
        }
    }

    // ---- 타이핑 집계 ----

    fn typing_status(&self) -> &'static str {
        let iv = self.ivars();
        if !iv.typing_on.get() {
            "꺼짐"
        } else if iv.typing_paused.get() {
            "일시정지"
        } else if iv.keytap.borrow().is_some() {
            "집계 중"
        } else {
            "권한 필요"
        }
    }

    /// 설정·권한 상태에 맞춰 탭과 반영 타이머를 켜고 끈다.
    fn sync_typing(&self) {
        let iv = self.ivars();
        let wanted = iv.typing_on.get() && !iv.typing_paused.get();
        let allowed = wanted && crate::keytap::has_permission();
        let running = iv.keytap.borrow().is_some();
        if running && !allowed {
            // 끔·일시정지·권한 철회: 탭을 즉시 없앤다(Drop이 감시를 해제한다).
            iv.keytap.replace(None);
            crate::keytap::take_count();
        } else if !running && allowed {
            iv.keytap.replace(crate::keytap::KeyTap::start());
        }
        // 타이핑과 AI 사용량은 같은 타이머로 모아서 반영한다.
        if wanted || iv.ai_on.get() {
            if iv.typing_timer.borrow().is_none() {
                // 1분에 한 번만 모아서 반영한다. 키 입력마다 화면·디스크를 건드리지 않는다.
                iv.typing_timer.replace(Some(self.timer(60.0, sel!(typingTick:), true, 20.0)));
            }
        } else {
            Self::stop(&iv.typing_timer);
        }
        // 메뉴 막대의 점: 지금 실제로 집계 중이라는 표시.
        self.set_status_title(if iv.keytap.borrow().is_some() { "•" } else { "" });
    }

    /// 쌓인 타건 수를 경험치로 반영한다.
    fn flush_typing(&self) {
        let iv = self.ivars();
        if iv.keytap.borrow().is_none() {
            return;
        }
        let keys = crate::keytap::take_count();
        if keys == 0 {
            return;
        }
        let r = self.with_pet(|p, t| p.grant_typing(t, keys));
        self.save();
        self.reload_atlas_if_needed();
        if r.leveled_up {
            self.bubble(&format!("Lv {}!", iv.pet.borrow().level()));
        }
    }

    // ---- 훈련 ----

    fn open_training_tiles(&self) {
        let iv = self.ivars();
        let (level, affinity, rows) = self.with_pet(|p, _| {
            (p.level(), p.affinity, Command::ALL.map(|c| (c, p.locked(c), p.mastery(c))))
        });
        let mut tiles = Vec::new();
        for (command, locked, mastery) in rows {
            let symbols: &'static [&'static str] = match command {
                Command::Sit => &["arrow.down.to.line.compact", "arrow.down.to.line"],
                Command::PawLeft => &["hand.point.left.fill"],
                Command::PawRight => &["hand.point.right.fill"],
                Command::LieDown => &["bed.double.fill"],
                Command::StandUp => &["arrow.up.to.line.compact", "arrow.up.to.line"],
            };
            let (icon, caption) = match locked {
                Some((need_level, need_affinity)) => {
                    (TileIcon::Symbol(&["lock.fill"]), format!("{} · Lv{need_level} 친밀{need_affinity}", command.name()))
                }
                None => (TileIcon::Symbol(symbols), format!("{} · {mastery}%", command.name())),
            };
            tiles.push(TileSpec { icon, caption, enabled: locked.is_none(), active: false, action: TileAction::Train(command) });
        }
        let listening = iv.listener.borrow().is_some();
        tiles.push(TileSpec {
            icon: TileIcon::Symbol(&["mic.fill"]),
            caption: if listening { "듣는 중 · 끝내기" } else { "말로 훈련" }.to_string(),
            enabled: true,
            active: listening,
            action: TileAction::Voice,
        });
        tiles.push(TileSpec {
            icon: TileIcon::Symbol(&["arrow.uturn.backward.circle.fill", "arrow.uturn.backward"]),
            caption: "돌아가기".to_string(),
            enabled: true,
            active: false,
            action: TileAction::MainMenu,
        });
        self.show_tiles(&format!("훈련 · Lv {level} · 친밀도 {affinity}"), "눌러서 시키기 · % 는 숙련도(높을수록 잘 따라 해요)", tiles);
    }

    /// 훈련 화면이 열려 있으면 숙련도와 "듣는 중" 표시를 새로 그린다.
    fn refresh_training_tiles(&self) {
        if self.ivars().tiles_open.get() {
            self.open_training_tiles();
        }
    }

    /// 명령 하나를 시킨다(타일로든 말로든 여기로 온다).
    fn do_command(&self, command: Command) {
        let iv = self.ivars();
        self.wake_from_house();
        if let Some((level, affinity)) = iv.pet.borrow().locked(command) {
            self.bubble(&format!("{}는 Lv {level} · 친밀도 {affinity}부터 배울 수 있어요", command.name()));
            return;
        }
        if iv.walk.borrow().is_some() || iv.roam.borrow().is_some() || iv.doorway.borrow().is_some() {
            self.bubble("지금은 걷는 중이에요");
            return;
        }
        // "일어서"는 앉거나 엎드려 있을 때만 의미가 있다.
        if command == Command::StandUp && !iv.held.get() {
            self.bubble("이미 서 있어요");
            return;
        }
        let roll = (self.next_random() % 100) as u8;
        let Some(result) = self.with_pet(|p, t| p.train(t, command, roll)) else { return };
        self.save();
        self.reload_atlas_if_needed();
        let name = command.name();
        if result.success {
            match command {
                Command::Sit => self.perform(Act::SitStay),
                Command::LieDown => self.perform(Act::LieStay),
                Command::PawLeft => self.perform(Act::Paw(true)),
                Command::PawRight => self.perform(Act::Paw(false)),
                Command::StandUp => self.get_up(),
            }
            let xp = if result.reward.xp > 0 { format!(" · +{} XP", result.reward.xp) } else { String::new() };
            self.bubble(&format!("{name}! 잘했어 · 숙련 {}%{xp}", result.mastery));
        } else {
            // 못 알아들으면 갸웃하며 바라본다. "일어서"를 못 알아들으면 그대로 있는다.
            if !iv.held.get() {
                self.perform(Act::Look);
            }
            self.bubble(&format!("{name}…? 갸웃 · 숙련 {}%", result.mastery));
        }
    }

    /// 앉거나 엎드려 머무는 중이면 일어서는 동작을 재생한다.
    fn get_up(&self) {
        let iv = self.ivars();
        if !iv.held.get() {
            return;
        }
        let motion = iv.motion.get();
        let list: Vec<(Motion, usize)> = match iv.atlas.borrow().as_ref() {
            Some(a) => (a.hold(motion)..a.len(motion)).map(|i| (motion, i)).collect(),
            None => return,
        };
        iv.held.set(false);
        iv.stay_after.set(false);
        iv.script.replace(list.into());
        self.advance_script();
        if iv.anim_timer.borrow().is_none() {
            iv.anim_timer.replace(Some(self.timer(1.0 / FPS, sel!(animTick:), true, 0.02)));
        }
    }

    /// 집 안에 있으면 문 앞으로 나오게 한다(앉거나 엎드린 자세는 그대로 둔다).
    fn wake_from_house(&self) {
        if self.ivars().inside.get() || self.ivars().doorway.borrow().is_some() {
            self.wake();
        }
    }

    // ---- 음성 명령 ----

    /// 무엇을 듣는지 설명하고 동의를 받은 뒤 macOS 권한을 요청한다.
    fn enable_voice(&self) {
        let iv = self.ivars();
        let mtm = self.mtm();
        if !crate::voice::supported() {
            let alert = NSAlert::new(mtm);
            alert.setMessageText(ns_string!("이 Mac에서는 음성 명령을 쓸 수 없어요"));
            alert.setInformativeText(ns_string!(
                "아롱이는 소리를 외부로 보내지 않기 위해 '기기 안 음성 인식'만 씁니다. 이 Mac은 한국어 기기 안 인식을 지원하지 않거나, 앱이 정식 번들로 실행되지 않았습니다. 훈련은 타일을 눌러서 그대로 할 수 있어요."
            ));
            alert.runModal();
            return;
        }
        let alert = NSAlert::new(mtm);
        alert.setMessageText(ns_string!("말로 훈련할까요? (마이크 사용)"));
        alert.setInformativeText(ns_string!(
            "훈련 화면에서 '말로 훈련'을 누르면 마이크로 듣기 시작해서, 앉아·왼손·오른손·엎드려·일어서 중 어느 말인지 알아냅니다. 다시 누르거나 30초 동안 명령이 없으면 마이크를 닫습니다.\n\n· 평소에는 마이크를 열지 않습니다. '말로 훈련'을 켠 동안뿐입니다.\n· 소리는 이 Mac 안에서만 인식하고 외부로 보내지 않습니다.\n· 소리와 인식된 문장은 저장하지 않습니다. 다섯 명령 중 어느 것인지만 씁니다.\n\n계속하면 macOS가 '음성 인식'과 '마이크' 권한을 차례로 묻습니다. 더보기 메뉴에서 언제든 끌 수 있습니다."
        ));
        alert.addButtonWithTitle(ns_string!("취소"));
        alert.addButtonWithTitle(ns_string!("켜기"));
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        if alert.runModal() != 1001 {
            return;
        }
        iv.voice_on.set(true);
        self.save_flag("voice", true);
        if crate::voice::permission() == crate::voice::Permission::NotAsked {
            crate::voice::request_permission();
            self.bubble("권한을 허용한 뒤 '말로 훈련'을 눌러 주세요");
        } else {
            self.bubble("음성 명령 켬 · 훈련에서 '말로 훈련'을 눌러 주세요");
        }
    }

    /// "말로 훈련": 누르면 듣기 시작해서 명령을 계속 받고, 다시 누르거나 한동안 말이 없으면 끝낸다.
    /// 꺼져 있으면 먼저 동의를 받는다.
    fn start_listening(&self) {
        let iv = self.ivars();
        if !iv.voice_on.get() {
            self.enable_voice();
            return;
        }
        if iv.listener.borrow().is_some() {
            self.stop_listening();
            self.bubble("말로 훈련 끝 (마이크를 닫았어요)");
            return;
        }
        match crate::voice::permission() {
            crate::voice::Permission::NotAsked => {
                crate::voice::request_permission();
                self.bubble("권한을 허용한 뒤 다시 눌러 주세요");
                return;
            }
            crate::voice::Permission::Denied => {
                self.bubble("시스템 설정에서 아롱이의 음성 인식을 허용해 주세요");
                return;
            }
            crate::voice::Permission::Allowed => {}
        }
        match crate::voice::Listener::start() {
            Ok(listener) => {
                iv.listener.replace(Some(listener));
                iv.voice_until.set(Some(Instant::now() + std::time::Duration::from_secs_f64(VOICE_IDLE_SECS)));
                Self::stop(&iv.voice_timer);
                iv.voice_timer.replace(Some(self.timer(0.15, sel!(voiceTick:), true, 0.05)));
                self.bubble("말해 보세요: 앉아 / 왼손 / 오른손 / 엎드려 / 일어서 (끝내려면 다시 누르기)");
            }
            Err(reason) => self.bubble(reason),
        }
    }

    /// 듣기를 끝낸다. 리스너를 버리는 순간 마이크가 닫힌다.
    fn stop_listening(&self) {
        let iv = self.ivars();
        Self::stop(&iv.voice_timer);
        iv.voice_until.set(None);
        iv.listener.replace(None);
    }

    // ---- 새 버전 확인 ----

    /// 무엇에 접속하는지 설명하고 동의를 받는다. 거절해도 다시 묻지 않는다(메뉴에서 언제든 켤 수 있다).
    fn confirm_update_check(&self) {
        let iv = self.ivars();
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(ns_string!("새 버전이 나오면 알려 드릴까요?"));
        alert.setInformativeText(ns_string!(
            "켜면 하루에 한 번쯤 GitHub에 있는 아롱이의 공개 파일 하나를 읽어 버전 숫자만 확인합니다.\n\n· 아롱이가 스스로 인터넷에 접속하는 것은 이것 하나뿐입니다.\n· 육성 데이터·타수·토큰 수 등 어떤 정보도 보내지 않습니다. 다만 접속 기록(IP 주소)은 GitHub 쪽에 남습니다.\n· 자동으로 내려받거나 설치하지 않습니다. 새 버전이 있으면 알려만 드리고, 업데이트는 brew로 직접 합니다.\n\n더보기 메뉴에서 언제든 끌 수 있습니다."
        ));
        alert.addButtonWithTitle(ns_string!("괜찮아요"));
        alert.addButtonWithTitle(ns_string!("알려 주기"));
        #[allow(deprecated)]
        NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        let on = alert.runModal() == 1001;
        iv.update_on.set(Some(on));
        self.save_flag("update_check", on);
        self.sync_update();
        if on {
            self.bubble("새 버전이 나오면 알려 드릴게요");
        }
    }

    /// 켜져 있으면 곧 한 번 확인하고 그 뒤로 하루에 한 번 확인한다. 꺼지면 타이머를 없앤다.
    fn sync_update(&self) {
        let iv = self.ivars();
        if iv.update_on.get() == Some(true) {
            if iv.update_timer.borrow().is_none() {
                std::mem::forget(self.timer(8.0, sel!(updateTick:), false, 3.0));
                iv.update_timer.replace(Some(self.timer(24.0 * 3600.0, sel!(updateTick:), true, 3600.0)));
            }
        } else {
            Self::stop(&iv.update_timer);
            iv.new_version.replace(None);
        }
    }

    // ---- AI 사용량 집계 ----

    fn ai_status(&self) -> &'static str {
        let iv = self.ivars();
        if !iv.ai_on.get() {
            "꺼짐"
        } else if iv.receiver.borrow().is_none() {
            "포트 사용 불가"
        } else if now_wall() - crate::otel::last_received() < 600 {
            "집계 중"
        } else {
            "수신 대기"
        }
    }

    /// 수신 인증용 토큰. 처음 켤 때 한 번 만들어 설정에 둔다.
    fn ai_token(&self) -> Option<String> {
        let store = self.ivars().store.as_ref()?;
        match store.setting("ai_token").filter(|t| t.len() == 32) {
            Some(token) => Some(token),
            None => {
                let token = crate::otel::new_token()?;
                store.set_setting("ai_token", &token);
                Some(token)
            }
        }
    }

    /// Claude Code 설정에 붙여 넣을 내용을 클립보드에 복사한다. 설정 파일을 직접 고치지는 않는다.
    fn do_copy_ai_config(&self) {
        let Some(token) = self.ai_token() else { return };
        let text = format!(
            "\"env\": {{\n  \"CLAUDE_CODE_ENABLE_TELEMETRY\": \"1\",\n  \"OTEL_METRICS_EXPORTER\": \"otlp\",\n  \"OTEL_EXPORTER_OTLP_METRICS_PROTOCOL\": \"http/json\",\n  \"OTEL_EXPORTER_OTLP_METRICS_ENDPOINT\": \"http://127.0.0.1:{}/v1/metrics\",\n  \"OTEL_EXPORTER_OTLP_METRICS_HEADERS\": \"Authorization=Bearer {token}\",\n  \"OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE\": \"cumulative\"\n}}",
            crate::otel::DEFAULT_PORT
        );
        // 클립보드에 쓰기만 한다(읽지 않는다).
        let board = NSPasteboard::generalPasteboard();
        board.clearContents();
        board.setString_forType(&NSString::from_str(&text), unsafe { NSPasteboardTypeString });
        self.bubble("Claude Code 설정을 복사했어요");
    }

    fn sync_ai(&self) {
        let iv = self.ivars();
        let running = iv.receiver.borrow().is_some();
        if iv.ai_on.get() && !running {
            let receiver = self.ai_token().and_then(|t| crate::otel::Receiver::start(crate::otel::DEFAULT_PORT, t));
            iv.receiver.replace(receiver);
        } else if !iv.ai_on.get() && running {
            // 끄면 수신 소켓을 즉시 닫고 대기 중인 표본도 버린다.
            iv.receiver.replace(None);
            crate::otel::take_samples();
        }
        self.sync_typing();
    }

    /// 받은 사용량을 경험치로 반영한다. 증가분 계산과 저장은 한 트랜잭션이다.
    fn flush_ai(&self) {
        let iv = self.ivars();
        if iv.receiver.borrow().is_none() {
            return;
        }
        let samples = crate::otel::take_samples();
        if samples.is_empty() {
            return;
        }
        let Some(store) = &iv.store else { return };
        let before = iv.pet.borrow().level();
        let ok = store.commit_ai(&samples, now_wall(), |deltas| {
            self.with_pet(|p, t| {
                p.grant_ai(t, deltas);
                p.clone()
            })
        });
        if !ok {
            iv.notice.set(Some("저장에 실패했습니다"));
        }
        self.reload_atlas_if_needed();
        let after = iv.pet.borrow().level();
        if after > before {
            self.bubble(&format!("Lv {after}!"));
        }
    }

    // ---- 말풍선 ----

    /// 아롱이 위에 짧은 안내를 잠깐 띄운다. 마우스 이벤트는 통과시킨다.
    fn bubble(&self, text: &str) {
        let mtm = self.mtm();
        let iv = self.ivars();
        let Some(pet_panel) = iv.panel.get() else { return };
        if iv.hidden.get() {
            return;
        }
        let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
        label.setFont(Some(&NSFont::boldSystemFontOfSize(12.0)));
        label.setTextColor(Some(&NSColor::whiteColor()));
        label.sizeToFit();
        let text_size = label.frame().size;
        let (w, h) = (text_size.width + 20.0, text_size.height + 10.0);
        let pf = pet_panel.frame();
        let origin = clamp_rect(mtm, CGPoint::new(pf.origin.x + pf.size.width / 2.0 - w / 2.0, pf.origin.y + pf.size.height - 8.0), w, h);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                mtm.alloc(),
                CGRect::new(origin, CGSize::new(w, h)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        // SAFETY: close를 부르지 않고 Retained가 수명을 관리한다.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setCollectionBehavior(collection_behavior());
        window.setLevel(NSFloatingWindowLevel + 2);
        if let Some(view) = window.contentView() {
            view.setWantsLayer(true);
            if let Some(layer) = view.layer() {
                layer.setCornerRadius(h / 2.0);
                layer.setBackgroundColor(Some(&CGColor::new_generic_rgb(0.12, 0.12, 0.15, 0.92)));
            }
            label.setFrameOrigin(CGPoint::new(10.0, 5.0));
            view.addSubview(&label);
        }
        window.orderFrontRegardless();
        if let Some(old) = iv.bubble.replace(Some(window)) {
            old.orderOut(None);
        }
        Self::stop(&iv.bubble_timer);
        iv.bubble_timer.replace(Some(self.timer(2.2, sel!(bubbleTick:), false, 0.3)));
    }

    // ---- 꾸미기 ----

    /// 착용 상태가 바뀌었을 때: 얹는 장식을 다시 만들고, 옷·신발이 바뀌었으면 스프라이트를 다시 칠한다.
    fn apply_outfit(&self) {
        self.refresh_accessories();
        self.reload_atlas_if_needed();
        self.show_frame();
    }

    /// 모자·안경·목 장식 레이어의 그림과 위치 정보를 현재 착용 상태·성장 단계에 맞춘다.
    fn refresh_accessories(&self) {
        let iv = self.ivars();
        let Some(layers) = iv.acc_layers.get() else { return };
        let stage = crate::art::stage_index(iv.pet.borrow().stage(&iv.tuning.borrow()));
        let worn = ACC_SLOTS.map(|slot| self.shown(slot));
        let mut shapes = [None; 3];
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for (i, layer) in layers.iter().enumerate() {
            let acc = worn[i].as_deref().and_then(crate::art::accessory_art);
            match acc.as_ref().and_then(|a| a.art.cg_image().map(|image| (a, image))) {
                Some((a, image)) => {
                    // SAFETY: CGImage는 CALayer.contents로 쓸 수 있는 CF 객체이고, 레이어가 retain한다.
                    unsafe { layer.setContents(Some(&*(Retained::as_ptr(&image) as *const AnyObject))) };
                    let at = |o: (i32, i32)| (o.0 as f64, o.1 as f64);
                    shapes[i] = Some((a.art.w as f64, a.art.h as f64, [at(a.offset[stage]), at(a.side_offset[stage]), at(a.front_offset[stage])]));
                }
                // 벗으면 레이어 내용을 비운다.
                None => unsafe { layer.setContents(None) },
            }
        }
        CATransaction::commit();
        iv.acc_shapes.set(shapes);
    }

    // ---- 집 ----

    /// 지금 화면에 보이는 아롱이를 PNG로 저장한다(배경 투명).
    /// 화면을 캡처하는 것이 아니라 앱이 가진 그림 레이어만 다시 그리므로, 다른 창이나 바탕화면은 찍히지 않는다.
    fn save_photo(&self, path: &std::path::Path) -> bool {
        use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSDeviceRGBColorSpace, NSGraphicsContext};
        use objc2_core_graphics::CGContext;
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return false };
        // 아롱이만 찍는다. 방석·집은 넣지 않는다.
        let mut parts: Vec<(CGRect, Retained<CALayer>)> = Vec::new();
        if !iv.inside.get() && !iv.hidden.get() {
            if let Some(layer) = panel.contentView().and_then(|v| v.layer()) {
                parts.push((panel.frame(), layer));
            }
        }
        if parts.is_empty() {
            return false;
        }
        let pad = 6.0;
        let x0 = parts.iter().map(|(f, _)| f.origin.x).fold(f64::MAX, f64::min) - pad;
        let y0 = parts.iter().map(|(f, _)| f.origin.y).fold(f64::MAX, f64::min) - pad;
        let x1 = parts.iter().map(|(f, _)| f.origin.x + f.size.width).fold(f64::MIN, f64::max) + pad;
        let y1 = parts.iter().map(|(f, _)| f.origin.y + f.size.height).fold(f64::MIN, f64::max) + pad;
        // 2배로 그려서 또렷하게 남긴다.
        let k = 2.0;
        let (w, h) = (((x1 - x0) * k) as isize, ((y1 - y0) * k) as isize);
        if w <= 0 || h <= 0 || w > 4000 || h > 4000 {
            return false;
        }
        // SAFETY: planes에 NULL을 주면 rep가 버퍼를 직접 할당하고 소유한다.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(), std::ptr::null_mut(), w, h, 8, 4, true, false, NSDeviceRGBColorSpace, w * 4, 32,
            )
        };
        let Some(rep) = rep else { return false };
        let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) else { return false };
        let cg = context.CGContext();
        CGContext::scale_ctm(Some(&cg), k, k);
        for (frame, layer) in &parts {
            CGContext::save_g_state(Some(&cg));
            CGContext::translate_ctm(Some(&cg), frame.origin.x - x0, frame.origin.y - y0);
            layer.renderInContext(&cg);
            CGContext::restore_g_state(Some(&cg));
        }
        let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &objc2_foundation::NSDictionary::new()) };
        let Some(png) = png else { return false };
        let ok = std::fs::write(path, png.to_vec()).is_ok();
        if ok {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        ok
    }

    /// 지금 가진 것을 부르는 말: 1단계는 방석, 그 위로는 집.
    fn house_word(&self) -> &'static str {
        if self.ivars().pet.borrow().house_level <= 1 { "방석" } else { "집" }
    }

    fn house_size(&self) -> CGSize {
        // 집은 아롱이 크기 설정을 따라간다(중간일 때 가로 128pt).
        let k = 2.0 * self.scale();
        CGSize::new(crate::art::HOUSE_W as f64 * k, crate::art::HOUSE_H as f64 * k)
    }

    /// 집 단계·크기·표시 여부에 맞춰 집 창을 만들거나 고치거나 없앤다.
    fn apply_house(&self) {
        let mtm = self.mtm();
        let iv = self.ivars();
        let Some(pet_panel) = iv.panel.get() else { return };
        // 집을 숨겨 두었으면 집이 없는 것처럼 다룬다(산 집과 단계는 그대로 남는다).
        let level = if iv.house_hidden.get() { 0 } else { iv.pet.borrow().house_level };
        let Some(image) = crate::art::house_art(level).and_then(|a| a.cg_image()) else {
            if let Some(old) = iv.house.take() {
                old.orderOut(None);
            }
            return;
        };
        let size = self.house_size();
        let existing = iv.house.borrow().clone();
        let window = match existing {
            Some(window) => window,
            None => {
                let pf = pet_panel.frame();
                let saved = iv.store.as_ref().and_then(|s| Some(CGPoint::new(s.setting_f64("house_x")?, s.setting_f64("house_y")?)));
                let origin = saved.unwrap_or(CGPoint::new(pf.origin.x - size.width - 12.0, pf.origin.y));
                let window = NSPanel::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    CGRect::new(origin, size),
                    NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
                    NSBackingStoreType::Buffered,
                    false,
                );
                // SAFETY: close를 부르지 않고 Retained가 수명을 관리한다.
                unsafe { window.setReleasedWhenClosed(false) };
                window.setOpaque(false);
                window.setBackgroundColor(Some(&NSColor::clearColor()));
                window.setHasShadow(false);
                window.setHidesOnDeactivate(false);
                window.setCollectionBehavior(collection_behavior());
                let view: Retained<HouseView> =
                    unsafe { msg_send![HouseView::alloc(mtm), initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), size)] };
                view.setWantsLayer(true);
                window.setContentView(Some(&view));
                iv.house.replace(Some(window.clone()));
                window
            }
        };
        let origin = clamp_rect(mtm, window.frame().origin, size.width, size.height);
        window.setFrame_display(CGRect::new(origin, size), true);
        window.setLevel(pet_panel.level());
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if let Some(layer) = window.contentView().and_then(|v| v.layer()) {
            layer.setMagnificationFilter(unsafe { objc2_quartz_core::kCAFilterNearest });
            // SAFETY: CGImage는 CALayer.contents로 쓸 수 있는 CF 객체이고, 레이어가 retain한다.
            unsafe { layer.setContents(Some(&*(Retained::as_ptr(&image) as *const AnyObject))) };
        }
        CATransaction::commit();
        if iv.hidden.get() {
            window.orderOut(None);
        } else {
            window.orderFrontRegardless();
            // 아롱이가 집 앞에 서도록 항상 집보다 위에 둔다.
            pet_panel.orderFrontRegardless();
        }
    }

    fn save_house_position(&self) {
        let iv = self.ivars();
        if let (Some(store), Some(house)) = (&iv.store, iv.house.borrow().as_ref()) {
            let o = house.frame().origin;
            store.set_setting("house_x", &format!("{:.1}", o.x));
            store.set_setting("house_y", &format!("{:.1}", o.y));
        }
    }

    fn on_house_down(&self) {
        let iv = self.ivars();
        let Some(house) = iv.house.borrow().clone() else { return };
        let cursor = NSEvent::mouseLocation();
        let o = house.frame().origin;
        iv.house_drag.set(Some((cursor, CGPoint::new(o.x - cursor.x, o.y - cursor.y), false)));
    }

    fn on_house_dragged(&self) {
        let iv = self.ivars();
        let Some((start, offset, moved)) = iv.house_drag.get() else { return };
        let Some(house) = iv.house.borrow().clone() else { return };
        let cursor = NSEvent::mouseLocation();
        if iv.locked.get() || (!moved && dist(start, cursor) < CLICK_SLOP) {
            return;
        }
        iv.house_drag.set(Some((start, offset, true)));
        let size = house.frame().size;
        house.setFrameOrigin(clamp_rect(self.mtm(), CGPoint::new(cursor.x + offset.x, cursor.y + offset.y), size.width, size.height));
    }

    fn on_house_up(&self) {
        match self.ivars().house_drag.take() {
            Some((_, _, true)) => self.save_house_position(),
            // 집을 그냥 클릭하면 아롱이가 집으로 걸어가고, 집 안에 있으면 나온다.
            Some(_) if self.ivars().inside.get() => self.begin_doorway(false),
            Some(_) => self.go_home(),
            None => {}
        }
    }

    /// 집 앞에 도착했을 때: 방석이면 그 위에 엎드려 쉬고, 집이면 안으로 들어간다.
    fn arrive_home(&self) {
        let iv = self.ivars();
        if iv.pet.borrow().house_level >= 2 {
            self.begin_doorway(true);
        } else {
            self.perform(Act::LieStay);
        }
    }

    /// 집 창에서 문 아래 가운데가 있는 화면 위치.
    fn door_point(&self) -> Option<CGPoint> {
        let house = self.ivars().house.borrow().clone()?;
        let hf = house.frame();
        let k = hf.size.width / crate::art::HOUSE_W as f64;
        let (x, from_bottom) = crate::art::HOUSE_DOOR;
        Some(CGPoint::new(hf.origin.x + x * k, hf.origin.y + from_bottom * k))
    }

    /// 문 앞에 서 있을 때의 창 원점: 발이 문 바로 앞 조금 아래에 온다.
    fn door_front(&self) -> Option<CGPoint> {
        let door = self.door_point()?;
        let size = self.size();
        let feet = (CELL as f64 - GROUND) * size / CELL as f64;
        Some(CGPoint::new(door.x - size / 2.0, door.y - feet - size * 0.10))
    }

    /// 집으로 들어가거나(entering) 집에서 나오는 연출을 시작한다.
    fn begin_doorway(&self, entering: bool) {
        let iv = self.ivars();
        let (Some(panel), Some(door), Some(front)) = (iv.panel.get(), self.door_point(), self.door_front()) else { return };
        let size = self.size();
        let feet = (CELL as f64 - GROUND) * size / CELL as f64;
        // 가장 깊이 들어갔을 때는 발이 문턱 조금 위에 온다.
        let deep = CGPoint::new(door.x - size / 2.0, door.y - feet + size * 0.04);
        self.stop_roam();
        iv.inside.set(false);
        iv.doorway.replace(Some(Doorway { front, deep, entering, depth: if entering { 0.0 } else { 1.0 }, last: Instant::now() }));
        if !entering {
            panel.setFrameOrigin(deep);
            panel.setAlphaValue(0.0);
            if !iv.hidden.get() {
                panel.orderFrontRegardless();
            }
        }
        Self::stop(&iv.door_timer);
        iv.door_timer.replace(Some(self.timer(1.0 / WALK_HZ, sel!(doorTick:), true, 0.005)));
        self.step_doorway();
    }

    fn step_doorway(&self) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        let (origin, depth, moved, done) = {
            let mut door = iv.doorway.borrow_mut();
            let Some(door) = door.as_mut() else { return };
            let now = Instant::now();
            let dt = now.duration_since(door.last).as_secs_f64().min(0.1);
            door.last = now;
            let step = dt / DOORWAY_SECS;
            door.depth = if door.entering { (door.depth + step).min(1.0) } else { (door.depth - step).max(0.0) };
            let d = door.depth;
            let origin = CGPoint::new(door.front.x + (door.deep.x - door.front.x) * d, door.front.y + (door.deep.y - door.front.y) * d);
            let done = if door.entering { d >= 1.0 } else { d <= 0.0 };
            (origin, d, dist(panel.frame().origin, origin).max(self.size() * step * 0.6), done)
        };
        // 들어갈수록 작아지고, 절반을 넘으면 문 안의 어둠 속으로 흐려진다.
        iv.shrink.set(1.0 - DOORWAY_SHRINK * depth);
        panel.setAlphaValue(1.0 - ((depth - DOORWAY_FADE_FROM) / (1.0 - DOORWAY_FADE_FROM)).clamp(0.0, 1.0));
        panel.setFrameOrigin(origin);
        self.stride(moved, false);
        if done {
            self.finish_doorway();
        }
    }

    /// 드나들기를 끝낸다(중간에 끊겨도 호출된다): 들어가는 중이었으면 집 안, 나오는 중이었으면 문 앞.
    fn finish_doorway(&self) {
        let iv = self.ivars();
        let Some(door) = iv.doorway.take() else { return };
        Self::stop(&iv.door_timer);
        iv.shrink.set(1.0);
        let Some(panel) = iv.panel.get() else { return };
        panel.setFrameOrigin(door.front);
        if door.entering {
            iv.inside.set(true);
            panel.orderOut(None);
            panel.setAlphaValue(1.0);
            self.rest();
        } else {
            panel.setAlphaValue(1.0);
            self.rest();
            self.save_position();
        }
    }

    fn go_home(&self) {
        let iv = self.ivars();
        let Some(house) = iv.house.borrow().clone() else { return };
        if iv.locked.get() || iv.walk.borrow().is_some() || iv.hidden.get() || iv.doorway.borrow().is_some() {
            return;
        }
        // 집이면 문 앞, 방석이면 방석 위(가운데, 살짝 아래)로 간다.
        let hf = house.frame();
        let size = self.size();
        let spot = if iv.pet.borrow().house_level >= 2 {
            self.door_front()
        } else {
            Some(CGPoint::new(hf.origin.x + hf.size.width / 2.0 - size / 2.0, hf.origin.y - size * 0.02))
        };
        let Some(spot) = spot else { return };
        self.begin_roam_to(clamp_origin(self.mtm(), spot, size), true);
    }

    // ---- 혼자 걷기 ----

    fn begin_roam(&self) {
        const MIN_TRIP: f64 = 30.0;
        let Some(panel) = self.ivars().panel.get() else { return };
        let origin = panel.frame().origin;
        let roll = self.next_random();
        let span = 80.0 + (roll % 240) as f64;
        let first = if roll & 0x100 == 0 { span } else { -span };
        // 화면 끝에 막혀 있으면 반대쪽으로 간다.
        let target_x = [first, -first]
            .into_iter()
            .map(|dx| clamp_origin(self.mtm(), CGPoint::new(origin.x + dx, origin.y), self.size()).x)
            .find(|x| (x - origin.x).abs() >= MIN_TRIP);
        if let Some(x) = target_x {
            self.begin_roam_to(CGPoint::new(x, origin.y), false);
        }
    }

    fn begin_roam_to(&self, target: CGPoint, sit_on_arrival: bool) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        let origin = panel.frame().origin;
        if (target.x - origin.x).abs() > 0.5 {
            iv.facing_left.set(target.x < origin.x);
        }
        iv.roam.replace(Some(Roam { target, sit_on_arrival, last: Instant::now() }));
        Self::stop(&iv.roam_timer);
        iv.roam_timer.replace(Some(self.timer(1.0 / ROAM_HZ, sel!(roamTick:), true, 0.01)));
    }

    fn step_roam(&self) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        let origin = panel.frame().origin;
        let (next, sit) = {
            let mut roam = iv.roam.borrow_mut();
            let Some(roam) = roam.as_mut() else { return };
            let now = Instant::now();
            let dt = now.duration_since(roam.last).as_secs_f64().min(0.1);
            roam.last = now;
            let left = dist(origin, roam.target);
            let step = ROAM_SPEED * dt;
            if left <= step {
                (None, roam.sit_on_arrival)
            } else {
                let k = step / left;
                (Some(CGPoint::new(origin.x + (roam.target.x - origin.x) * k, origin.y + (roam.target.y - origin.y) * k)), false)
            }
        };
        match next {
            Some(p) => {
                self.stride(dist(origin, p), false);
                panel.setFrameOrigin(p);
            }
            None => {
                self.stop_roam();
                if sit {
                    self.arrive_home();
                }
            }
        }
    }

    fn stop_roam(&self) {
        let iv = self.ivars();
        if iv.roam.take().is_some() {
            Self::stop(&iv.roam_timer);
            self.rest();
            self.save_position();
        }
    }

    // ---- 산책 모드 ----

    fn begin_walk(&self) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        if iv.hidden.get() || iv.suspended.get() {
            return;
        }
        let mtm = self.mtm();
        self.stop_roam();
        self.wake();
        let f = panel.frame();
        let pos = Vec2::new(f.origin.x + f.size.width / 2.0, f.origin.y + self.size() / 2.0);
        iv.walk.replace(Some(Walk {
            follower: Follower::new(pos),
            meter: WalkMeter::new(pos),
            last: Instant::now(),
            last_cursor: NSEvent::mouseLocation(),
            still: 0.0,
            total: 0.0,
        }));

        // 목줄 오버레이: 화면마다 하나, 마우스 이벤트를 전부 통과시킨다. 산책 중에만 존재한다.
        let mut overlays = Vec::new();
        for screen in NSScreen::screens(mtm).iter() {
            let frame = screen.frame();
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    mtm.alloc(),
                    frame,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            // SAFETY: close를 부르지 않고 Retained가 수명을 관리한다.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setOpaque(false);
            window.setBackgroundColor(Some(&NSColor::clearColor()));
            window.setHasShadow(false);
            window.setIgnoresMouseEvents(true);
            window.setCollectionBehavior(collection_behavior());
            window.setLevel(panel.level());
            let leash = CAShapeLayer::new();
            let color: CFRetained<CGColor> = CGColor::new_generic_rgb(0.16, 0.50, 0.66, 1.0);
            leash.setStrokeColor(Some(&color));
            leash.setFillColor(None);
            leash.setLineWidth(3.0);
            leash.setLineCap(unsafe { objc2_quartz_core::kCALineCapRound });
            if let Some(view) = window.contentView() {
                view.setWantsLayer(true);
                if let Some(layer) = view.layer() {
                    leash.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), frame.size));
                    layer.addSublayer(&leash);
                }
            }
            window.orderFrontRegardless();
            overlays.push(Overlay { window, leash, origin: frame.origin });
        }
        iv.overlays.replace(overlays);
        panel.orderFrontRegardless();

        Self::stop(&iv.walk_timer);
        iv.walk_timer.replace(Some(self.timer(1.0 / WALK_HZ, sel!(walkTick:), true, 0.005)));
        self.bubble("산책 시작! 아롱이를 클릭하면 끝");
        self.step_walk();
    }

    fn step_walk(&self) {
        let iv = self.ivars();
        let Some(panel) = iv.panel.get() else { return };
        let cursor = NSEvent::mouseLocation();
        let size = self.size();
        let mtm = self.mtm();
        let (pos, moved, speed, expired) = {
            let mut walk = iv.walk.borrow_mut();
            let Some(walk) = walk.as_mut() else { return };
            let now = Instant::now();
            let dt = now.duration_since(walk.last).as_secs_f64();
            walk.last = now;
            let before = walk.follower.pos;
            let wanted = walk.follower.step(&iv.walk_tuning, Vec2::new(cursor.x, cursor.y), dt);
            // 화면 경계·Dock·메뉴 막대 밖으로는 나가지 않는다. 막히면 그만큼 거리도 쌓이지 않는다.
            let origin = clamp_origin(mtm, CGPoint::new(wanted.x - size / 2.0, wanted.y - size / 2.0), size);
            walk.follower.pos = Vec2::new(origin.x + size / 2.0, origin.y + size / 2.0);
            walk.meter.tick(&iv.walk_tuning, walk.follower.pos, dt);
            let moved = Vec2::new(walk.follower.pos.x - before.x, walk.follower.pos.y - before.y);
            let capped = dt.min(iv.walk_tuning.max_dt);
            let speed = if capped > 0.0 { before.dist(walk.follower.pos) / capped } else { 0.0 };
            // 커서가 오래 멈춰 있으면(자리를 비움) 고빈도 갱신을 계속 돌리지 않고 끝낸다.
            if dist(walk.last_cursor, cursor) > 3.0 {
                walk.last_cursor = cursor;
                walk.still = 0.0;
            } else {
                walk.still += capped;
            }
            walk.total += capped;
            let expired = walk.still >= WALK_STILL_LIMIT || walk.total >= WALK_MAX_SECS;
            (walk.follower.pos, moved, speed, expired)
        };
        if expired {
            self.end_walk(true);
            return;
        }

        if moved.x.abs() > 0.3 {
            iv.facing_left.set(moved.x < 0.0);
        }
        if speed < 5.0 {
            // 멈춰 있을 때는 제자리에서 헥헥.
            // 서 있는 동안은 계속 헥헥거린다(한 번 끝나면 다음 틱에 다시 시작).
            if iv.anim_timer.borrow().is_none() {
                self.perform(Act::Pant);
            }
        } else {
            self.stride((moved.x * moved.x + moved.y * moved.y).sqrt(), speed >= iv.walk_tuning.run_speed);
        }
        panel.setFrameOrigin(CGPoint::new(pos.x - size / 2.0, pos.y - size / 2.0));

        // 목줄: 목 → 커서. 여유 길이만큼 아래로 처지는 2차 곡선 하나.
        // 목줄은 지금 프레임의 목 위치(머리 기준점 + 단계별 목 오프셋)에 붙는다.
        let (cx, cy) = {
            let pet = iv.pet.borrow();
            let stage = crate::art::stage_index(pet.stage(&iv.tuning.borrow()));
            let atlas = iv.atlas.borrow();
            let frame = atlas.as_ref().map(|a| a.frame(iv.motion.get(), iv.frame.get()));
            let (ax, ay) = frame.map_or((CELL as f64 / 2.0, CELL as f64 / 4.0), |f| f.head);
            let (dx, dy) = if frame.is_some_and(|f| f.view == View::Side) { crate::art::NECK_SIDE_OFFSET[stage] } else { crate::art::NECK_OFFSET[stage] };
            ((ax + dx as f64) / CELL as f64, (ay + dy as f64 + 4.0) / CELL as f64)
        };
        let nx = if iv.facing_left.get() { cx } else { 1.0 - cx };
        let neck = CGPoint::new(pos.x - size / 2.0 + nx * size, pos.y + size / 2.0 - cy * size + iv.hop.get().1);
        let slack = (iv.walk_tuning.leash_len - dist(neck, cursor)).max(0.0);
        let control = CGPoint::new((neck.x + cursor.x) / 2.0, (neck.y + cursor.y) / 2.0 - slack * 0.6);
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        for overlay in iv.overlays.borrow().iter() {
            let (ox, oy) = (overlay.origin.x, overlay.origin.y);
            let path = CGMutablePath::new();
            // SAFETY: 방금 만든 유효한 경로이고 변환 행렬은 NULL(없음)을 허용한다.
            unsafe {
                CGMutablePath::move_to_point(Some(&path), std::ptr::null(), neck.x - ox, neck.y - oy);
                CGMutablePath::add_quad_curve_to_point(
                    Some(&path),
                    std::ptr::null(),
                    control.x - ox,
                    control.y - oy,
                    cursor.x - ox,
                    cursor.y - oy,
                );
            }
            overlay.leash.setPath(Some(&path));
        }
        CATransaction::commit();
    }

    /// 산책 종료: 고빈도 타이머와 오버레이를 즉시 없애고 걸은 거리로 정산한다.
    fn end_walk(&self, reward: bool) {
        let iv = self.ivars();
        let Some(walk) = iv.walk.take() else { return };
        Self::stop(&iv.walk_timer);
        for overlay in iv.overlays.take() {
            overlay.window.orderOut(None);
        }
        self.rest();
        self.save_position();
        if reward {
            let walked = walk.meter.finish(walk.follower.pos);
            let r = self.with_pet(|p, t| p.finish_walk(t, walked));
            let message = if r.xp > 0 { format!("산책 끝! +{} XP", r.xp) } else { "산책 끝!".to_string() };
            self.after_action(r, Act::Pant, message);
        }
    }
}
