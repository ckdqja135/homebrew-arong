//! 스프라이트. PNG는 바이너리에 포함되어 있어 실행 중 파일을 읽지 않는다.
//! 성장 단계나 신발이 바뀔 때 한 번만 디코딩하고, 프레임은 원본을 참조하는 부분 이미지로 잘라 둔다.
//!
//! 동작마다 가로로 이어 붙인 시트가 하나씩 있다(한 칸 128px, 6~12프레임, 모두 왼쪽을 본다).
//! 프레임마다 "스프라이트 칸 안에서 어디에 얼마만 한 크기로 놓일지"와 머리 위치를 미리 계산해 둔다.

use arong_core::Stage;
use objc2::rc::Retained;
use objc2_app_kit::NSBitmapImageRep;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGImage;
use objc2_foundation::NSData;

use crate::art::{self, Art};

/// 스프라이트 한 칸의 크기(px). 창 크기와 장식 위치의 기준이다.
pub const CELL: usize = 128;
/// 발이 닿는 줄(위에서부터 px).
pub const GROUND: f64 = 121.0;
/// 걷기(옆모습) 그림은 서 있는 그림보다 작게 그려져 있어, 몸집이 비슷해 보이도록 키운다(아기, 성장기, 성견).
const WALK_SCALE: [f64; 3] = [1.0, 1.08, 1.12];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    /// 옆모습 걷기. 이동한 거리에 맞춰 프레임을 넘긴다.
    Walk = 0,
    /// 서 있다가 왼발을 내민다. 0번 프레임이 기본 서 있는 자세다.
    PawLeft = 1,
    PawRight = 2,
    /// 서기 → 엎드리기(가운데 프레임) → 서기.
    LieDown = 3,
    /// 서기 → 앉기(가운데 프레임) → 서기.
    Sit = 4,
    /// 앉았다가 일어서는 다른 그림. `Sit`과 같은 흐름으로 쓴다.
    SitUp = 5,
    /// 엎드린 채 배를 보이며 구른다.
    Roll = 6,
    /// 서서 혀를 내밀고 헥헥거린다.
    Pant = 7,
    /// 정면을 바라본다(반짝이 포함).
    Look = 8,
}

pub const MOTIONS: [Motion; 9] = [
    Motion::Walk,
    Motion::PawLeft,
    Motion::PawRight,
    Motion::LieDown,
    Motion::Sit,
    Motion::SitUp,
    Motion::Roll,
    Motion::Pant,
    Motion::Look,
];

macro_rules! sheets {
    ($stage:expr) => {
        [
            include_bytes!(concat!("../../../assets/", $stage, "_walk.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_paw_left.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_paw_right.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_lie_down.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_sit.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_sit_up.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_roll.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_pant.png")).as_slice(),
            include_bytes!(concat!("../../../assets/", $stage, "_look.png")).as_slice(),
        ]
    };
}

/// `MOTIONS` 순서의 시트. 단계 순서는 아기, 성장기, 성견.
static SHEETS: [[&[u8]; 9]; 3] = [sheets!("baby"), sheets!("growing"), sheets!("adult")];

macro_rules! outfit {
    ($id:literal, $index:literal, $stage:literal) => {
        ($id, $index, sheets!(concat!("outfits/", $id, "/", $stage)))
    };
}

/// 옷 입은 그림: (옷 id, 단계 번호, `MOTIONS` 순서의 시트). 옷만 있는 레이어가 아니라 옷을 입은 아롱이 전체 그림이라,
/// 입으면 몸 그림을 통째로 바꿔 쓴다. 기본 그림과 프레임 수·위치·윤곽이 같다(scripts/build_outfit.py가 확인한다).
/// 여기 없는 단계는 기본 그림을 그대로 쓴다.
static OUTFITS: [(&str, usize, [&[u8]; 9]); 27] = [
    outfit!("blue_knit", 0, "baby"),
    outfit!("blue_knit", 1, "growing"),
    outfit!("blue_knit", 2, "adult"),
    outfit!("coral_hoodie", 0, "baby"),
    outfit!("coral_hoodie", 1, "growing"),
    outfit!("coral_hoodie", 2, "adult"),
    outfit!("teal_hanbok", 0, "baby"),
    outfit!("teal_hanbok", 1, "growing"),
    outfit!("teal_hanbok", 2, "adult"),
    outfit!("pink_bunny", 0, "baby"),
    outfit!("pink_bunny", 1, "growing"),
    outfit!("pink_bunny", 2, "adult"),
    outfit!("yellow_bear", 0, "baby"),
    outfit!("yellow_bear", 1, "growing"),
    outfit!("yellow_bear", 2, "adult"),
    outfit!("green_dino", 0, "baby"),
    outfit!("green_dino", 1, "growing"),
    outfit!("green_dino", 2, "adult"),
    outfit!("camel_trench", 0, "baby"),
    outfit!("camel_trench", 1, "growing"),
    outfit!("camel_trench", 2, "adult"),
    outfit!("lavender_cape", 0, "baby"),
    outfit!("lavender_cape", 1, "growing"),
    outfit!("lavender_cape", 2, "adult"),
    outfit!("sky_puffer", 0, "baby"),
    outfit!("sky_puffer", 1, "growing"),
    outfit!("sky_puffer", 2, "adult"),
];

/// 색만 다른 옷: (옷 id, 그림을 빌려 쓰는 옷 id, 옷 색으로 볼 색상 폭(도), 가장 밝은 밝기, 가장 낮은 채도).
/// 그림을 따로 두지 않고, 읽을 때 옷 색만 `art::wear_color`로 바꾼다.
static OUTFIT_COLORS: [(&str, &str, f64, f64, f64); 26] = [
    ("red_knit", "blue_knit", 30.0, 1.0, 0.3),
    ("yellow_knit", "blue_knit", 30.0, 1.0, 0.3),
    ("pink_knit", "blue_knit", 30.0, 1.0, 0.3),
    ("green_knit", "blue_knit", 30.0, 1.0, 0.3),
    ("sky_hoodie", "coral_hoodie", 30.0, 1.0, 0.3),
    ("yellow_hoodie", "coral_hoodie", 30.0, 1.0, 0.3),
    ("mint_hoodie", "coral_hoodie", 30.0, 1.0, 0.3),
    ("purple_hoodie", "coral_hoodie", 30.0, 1.0, 0.3),
    // 한복은 저고리의 청록색만 바꾼다. 색동 소매의 파랑·하늘색 줄은 건드리지 않도록 색상 폭과 밝기를 좁게 둔다.
    ("pink_hanbok", "teal_hanbok", 14.0, 0.7, 0.3),
    ("yellow_hanbok", "teal_hanbok", 14.0, 0.7, 0.3),
    ("yellow_dino", "green_dino", 30.0, 1.0, 0.3),
    ("white_dino", "green_dino", 30.0, 1.0, 0.3),
    ("pink_dino", "green_dino", 30.0, 1.0, 0.3),
    ("red_dino", "green_dino", 30.0, 1.0, 0.3),
    ("red_trench", "camel_trench", 30.0, 1.0, 0.25),
    ("pink_trench", "camel_trench", 30.0, 1.0, 0.25),
    ("sky_trench", "camel_trench", 30.0, 1.0, 0.25),
    ("olive_trench", "camel_trench", 30.0, 1.0, 0.25),
    // 망토와 패딩은 파스텔이라 흐린 픽셀까지 옷 색으로 본다. 크림색 곰 무늬·줄무늬는 색상이 달라 그대로 남는다.
    ("pink_cape", "lavender_cape", 30.0, 1.0, 0.1),
    ("sky_cape", "lavender_cape", 30.0, 1.0, 0.1),
    ("yellow_cape", "lavender_cape", 30.0, 1.0, 0.1),
    ("mint_cape", "lavender_cape", 30.0, 1.0, 0.1),
    ("pink_puffer", "sky_puffer", 25.0, 1.0, 0.15),
    ("yellow_puffer", "sky_puffer", 25.0, 1.0, 0.15),
    ("red_puffer", "sky_puffer", 25.0, 1.0, 0.15),
    ("purple_puffer", "sky_puffer", 25.0, 1.0, 0.15),
];

/// (그림을 가진 옷 id, 바꿔 칠하는 방법).
fn outfit_source(id: &str) -> (&str, Option<art::Tint>) {
    match OUTFIT_COLORS.iter().find(|(variant, ..)| *variant == id) {
        Some(&(_, source, spread, brightest, min_sat)) => {
            let tint = art::wear_color(source)
                .zip(art::wear_color(id))
                .map(|(from, to)| art::Tint { from, to, spread, brightest, min_sat });
            (source, tint)
        }
        None => (id, None),
    }
}

/// 이 옷의 그림이 하나라도 있는지. 그림이 없는 옷은 상점에 내놓지 않는다.
pub fn has_outfit(id: &str) -> bool {
    let (source, _) = outfit_source(id);
    OUTFITS.iter().any(|(outfit, ..)| *outfit == source)
}

/// 이 단계·동작의 옷 입은 시트와, 바꿔 칠할 색(색만 다른 옷일 때).
fn outfit_sheet(id: &str, stage: Stage, motion: Motion) -> Option<(&'static [u8], Option<art::Tint>)> {
    let stage = art::stage_index(stage);
    let (source, tint) = outfit_source(id);
    OUTFITS.iter().find(|(outfit, s, _)| *outfit == source && *s == stage).map(|(.., sheets)| (sheets[motion as usize], tint))
}

/// 상점에 보여 줄 옷 그림: 이 단계의 서 있는 프레임에서 옷이 있는 부분만 잘라 낸다(색만 다른 옷은 색을 바꿔서).
pub fn outfit_thumb(id: &str, stage: Stage) -> Option<Art> {
    const PAD: usize = 5;
    let (bytes, tint) = outfit_sheet(id, stage, Motion::PawLeft)?;
    let decode = |bytes: &[u8]| -> Option<Art> {
        let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(bytes))?;
        Art::from_rep(&rep)
    };
    let bare = decode(SHEETS[art::stage_index(stage)][Motion::PawLeft as usize])?;
    let mut dressed = decode(bytes)?;
    if dressed.w != bare.w || dressed.h != bare.h {
        return None;
    }
    if let Some(tint) = tint {
        art::tint_garment(&mut dressed, &bare, &tint);
    }
    // 서 있는 프레임(0번 칸)에서 옷 없는 그림과 달라진 영역.
    let (mut x0, mut y0, mut x1, mut y1) = (CELL, CELL, 0, 0);
    for y in 0..CELL {
        for x in 0..CELL {
            if dressed.px[y * dressed.w + x] != bare.px[y * bare.w + x] {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x0 > x1 {
        return None;
    }
    let (x0, y0) = (x0.saturating_sub(PAD), y0.saturating_sub(PAD));
    let (x1, y1) = ((x1 + PAD).min(CELL - 1), (y1 + PAD).min(CELL - 1));
    let mut thumb = Art::new(x1 - x0 + 1, y1 - y0 + 1);
    for y in y0..=y1 {
        for x in x0..=x1 {
            thumb.px[(y - y0) * thumb.w + (x - x0)] = dressed.px[y * dressed.w + x];
        }
    }
    Some(thumb)
}

/// 시트마다 "서 있는 자세"가 몇 번째 프레임인지. 에셋마다 시작 자세가 달라서(서서 시작하는 것과
/// 앉아서 시작하는 것이 섞여 있다) 여기서 맞춘다. 앱에서는 항상 서 있는 자세가 0번이 되도록 돌려 쓴다.
fn standing_frame(stage: Stage, motion: Motion) -> usize {
    match (stage, motion) {
        (Stage::Baby, Motion::SitUp) => 4,
        (Stage::Adult, Motion::Sit | Motion::SitUp) => 4,
        _ => 0,
    }
}

/// 그림의 시점.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// 왼쪽을 보는 3/4 정면(서기, 발 주기, 앉기 등 대부분).
    Quarter,
    /// 옆모습(걷기).
    Side,
    /// 정면(정면 보기).
    Front,
}

pub struct Frame {
    pub image: CFRetained<CGImage>,
    /// 스프라이트 칸(좌상단 원점)에서 그림이 놓일 자리: x, y, 너비, 높이.
    pub rect: (f64, f64, f64, f64),
    /// 같은 좌표계에서 머리 꼭대기 중심. 장식을 얹는 기준점이다.
    pub head: (f64, f64),
    /// 어느 쪽에서 본 그림인지. 장식 위치가 달라진다.
    pub view: View,
    /// 머리 위치를 믿을 수 있어 장식을 얹어도 되는지(구르는 동안은 얹지 않는다).
    pub wearable: bool,
}

pub struct Atlas {
    pub stage: Stage,
    /// 이 그림에 칠해 넣은 신발 아이템 id.
    pub shoes: Option<String>,
    /// 입고 있는 옷 아이템 id(그림이 없는 동작은 기본 그림이다).
    pub clothes: Option<String>,
    motions: Vec<Vec<Frame>>,
    // 부분 이미지들이 참조하는 원본을 살려 둔다.
    _sources: Vec<Retained<CGImage>>,
}

/// 시트 한 장을 디코딩하고, 신발을 칠한 뒤, 그림이 있는 영역만 잘라 프레임으로 만든다.
/// `dressed`는 같은 동작의 옷 입은 시트다. 있으면 그리는 그림만 그것으로 바꾸고, 머리 기준점은 기본 그림에서 구한다
/// (옷 색에 따라 기준점이 흔들리지 않게).
fn build(
    bytes: &[u8],
    dressed: Option<(&[u8], Option<art::Tint>)>,
    stage: Stage,
    motion: Motion,
    shoes: Option<&str>,
) -> Option<(Vec<Frame>, Retained<CGImage>)> {
    let rep = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(bytes))?;
    let mut source = rep.CGImage()?;
    let (width, height) = (CGImage::width(Some(&source)), CGImage::height(Some(&source)));
    if height != CELL || width == 0 || width % CELL != 0 || width / CELL > 16 {
        return None;
    }
    let count = width / CELL;
    let side = motion == Motion::Walk;
    // 픽셀 사본은 기준점·경계 계산과 신발 칠하기에만 쓰고 바로 버린다.
    let mut pixels = Art::from_rep(&rep)?;
    let anchors: Vec<(i32, i32)> = (0..count).map(|col| art::head_anchor(&pixels, col, 0, CELL, side)).collect();
    // 옷 입은 시트는 기본 시트와 크기가 같을 때만 쓴다. 아니면 기본 그림으로 둔다.
    let tint = dressed.and_then(|(_, tint)| tint);
    let dressed = dressed
        .and_then(|(bytes, _)| NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(bytes)))
        .and_then(|rep| Some((Art::from_rep(&rep)?, rep.CGImage()?)))
        .filter(|(art, _)| art.w == width && art.h == height);
    if let Some((mut art, image)) = dressed {
        source = match tint {
            Some(tint) => {
                art::tint_garment(&mut art, &pixels, &tint);
                art.cg_image()?
            }
            None => image,
        };
        pixels = art;
    }
    if let Some(color) = shoes.and_then(art::wear_color) {
        art::paint_shoes(&mut pixels, CELL, color);
        source = pixels.cg_image()?;
    }
    // 가로 가운데와 발이 닿는 줄을 기준으로 놓는다(걷기만 비율이 다르다).
    let scale = if side { WALK_SCALE[art::stage_index(stage)] } else { 1.0 };
    let place = |x: f64, y: f64| (CELL as f64 / 2.0 + (x - CELL as f64 / 2.0) * scale, GROUND + (y - GROUND) * scale);
    let mut frames = Vec::with_capacity(count);
    for col in 0..count {
        // 프레임은 그림이 실제로 있는 영역만 잘라 둔다. 창에서 클릭을 받는 범위가
        // 레이어 크기를 따르기 때문에, 빈 여백까지 레이어로 덮으면 뒤의 앱 클릭을 막는다.
        let (x, y, w, h) = pixels.opaque_bounds(col * CELL, 0, CELL, CELL).unwrap_or((0, 0, CELL, CELL));
        let crop = CGRect::new(CGPoint::new((col * CELL + x) as f64, y as f64), CGSize::new(w as f64, h as f64));
        let (rx, ry) = place(x as f64, y as f64);
        let (ax, ay) = anchors[col];
        frames.push(Frame {
            image: CGImage::with_image_in_rect(Some(&source), crop)?,
            rect: (rx, ry, w as f64 * scale, h as f64 * scale),
            head: place(ax as f64, ay as f64),
            view: match motion {
                Motion::Walk => View::Side,
                Motion::Look => View::Front,
                _ => View::Quarter,
            },
            wearable: motion != Motion::Roll,
        });
    }
    // 서 있는 자세가 0번이 되도록 돌린다.
    let start = standing_frame(stage, motion) % count;
    frames.rotate_left(start);
    Some((frames, source))
}

impl Atlas {
    /// `shoes`가 있으면 디코딩한 그림에 한 번 칠해 넣는다(프레임마다 다시 칠하지 않는다).
    /// `clothes`가 있고 그 옷의 그림이 있는 동작은 옷 입은 그림으로 바꿔 쓴다.
    pub fn load(stage: Stage, shoes: Option<&str>, clothes: Option<&str>) -> Option<Atlas> {
        let sheets = &SHEETS[art::stage_index(stage)];
        let mut motions = Vec::with_capacity(MOTIONS.len());
        let mut sources = Vec::with_capacity(MOTIONS.len());
        for motion in MOTIONS {
            let dressed = clothes.and_then(|id| outfit_sheet(id, stage, motion));
            let (frames, source) = build(sheets[motion as usize], dressed, stage, motion, shoes)?;
            motions.push(frames);
            sources.push(source);
        }
        Some(Atlas { stage, shoes: shoes.map(str::to_string), clothes: clothes.map(str::to_string), motions, _sources: sources })
    }

    /// 이 동작의 프레임 수.
    pub fn len(&self, motion: Motion) -> usize {
        self.motions[motion as usize].len()
    }

    /// 앉거나 엎드린 자세가 가장 깊은 가운데 프레임.
    pub fn hold(&self, motion: Motion) -> usize {
        self.len(motion) / 2
    }

    pub fn frame(&self, motion: Motion, index: usize) -> &Frame {
        let frames = &self.motions[motion as usize];
        &frames[index % frames.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outfit_frames_sit_exactly_where_the_bare_frames_do() {
        for (id, stage, _) in OUTFITS {
            let stage = [Stage::Baby, Stage::Growing, Stage::Adult][stage];
            let bare = Atlas::load(stage, None, None).expect("bare atlas");
            let worn = Atlas::load(stage, None, Some(id)).expect("dressed atlas");
            for motion in MOTIONS {
                assert_eq!(bare.len(motion), worn.len(motion), "{id} {motion:?}");
                for index in 0..bare.len(motion) {
                    let (a, b) = (bare.frame(motion, index), worn.frame(motion, index));
                    // 머리 기준점은 그대로이고, 그림 자리는 가장자리 1~2px 안에서만 다르다.
                    assert_eq!(a.head, b.head, "{id} {motion:?} {index}");
                    let (ra, rb) = (a.rect, b.rect);
                    let near = |x: f64, y: f64| (x - y).abs() <= 3.0;
                    assert!(near(ra.0, rb.0) && near(ra.1, rb.1) && near(ra.2, rb.2) && near(ra.3, rb.3), "{id} {motion:?} {index} {ra:?} {rb:?}");
                    assert!(near(ra.1 + ra.3, rb.1 + rb.3));
                }
            }
        }
        // 상점 그림은 옷이 있는 부분만 담는다.
        for stage in [Stage::Baby, Stage::Growing, Stage::Adult] {
            let thumb = outfit_thumb("pink_hanbok", stage).expect("thumb");
            assert!(thumb.w > 20 && thumb.w < CELL && thumb.h > 15 && thumb.h < CELL, "{stage:?} {}x{}", thumb.w, thumb.h);
        }
        assert!(outfit_thumb("red_wear", Stage::Adult).is_none());
        assert!(has_outfit("blue_knit") && has_outfit("pink_knit") && has_outfit("mint_hoodie") && !has_outfit("red_wear"));
        // 색만 다른 옷도 같은 자리에 놓인다.
        let (blue, pink) = (Atlas::load(Stage::Adult, None, Some("blue_knit")).unwrap(), Atlas::load(Stage::Adult, None, Some("pink_knit")).unwrap());
        for motion in MOTIONS {
            for index in 0..blue.len(motion) {
                assert_eq!(blue.frame(motion, index).rect, pink.frame(motion, index).rect);
            }
        }
    }

    #[test]
    fn all_stages_decode_with_tight_frames_on_the_ground() {
        for stage in [Stage::Baby, Stage::Growing, Stage::Adult] {
            let atlas = Atlas::load(stage, Some("yellow_boots"), None).expect("atlas should decode");
            assert_eq!(atlas.len(Motion::Walk), 12);
            for motion in MOTIONS {
                assert!((6..=12).contains(&atlas.len(motion)), "{stage:?} {motion:?}");
                for index in 0..atlas.len(motion) {
                    let frame = atlas.frame(motion, index);
                    let (x, y, w, h) = frame.rect;
                    let cell = CELL as f64;
                    // 그림은 스프라이트 칸 안에 있고, 칸 전체보다 작게 잘린다.
                    assert!(x >= -1.0 && y >= 0.0 && x + w <= cell + 1.0 && y + h <= cell + 1.0, "{stage:?} {motion:?} {index} {:?}", frame.rect);
                    assert!(w > 20.0 && h > 20.0 && w * h < cell * cell);
                    // 발(또는 배)이 바닥 줄 근처에 닿는다.
                    assert!((y + h - GROUND).abs() <= 7.0, "{stage:?} {motion:?} {index} bottom {}", y + h);
                    assert_eq!(frame.view == View::Side, motion == Motion::Walk);
                    assert_eq!(frame.wearable, motion != Motion::Roll);
                }
            }
            // 기본 자세(왼발 주기 0번)와 앉기·엎드리기의 0번은 서 있는 자세라 키가 비슷하고,
            // 엎드린 가운데 프레임은 그보다 낮다.
            let stand = atlas.frame(Motion::PawLeft, 0).rect.3;
            for motion in [Motion::LieDown, Motion::Sit, Motion::SitUp] {
                let first = atlas.frame(motion, 0).rect.3;
                assert!((first - stand).abs() <= stand * 0.15, "{stage:?} {motion:?} first {first} stand {stand}");
            }
            let lying = atlas.frame(Motion::LieDown, atlas.hold(Motion::LieDown)).rect.3;
            assert!(lying < stand * 0.85, "{stage:?} lying {lying} stand {stand}");
        }
    }
}
