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
    ($stage:literal) => {
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
    motions: Vec<Vec<Frame>>,
    // 부분 이미지들이 참조하는 원본을 살려 둔다.
    _sources: Vec<Retained<CGImage>>,
}

/// 시트 한 장을 디코딩하고, 신발을 칠한 뒤, 그림이 있는 영역만 잘라 프레임으로 만든다.
fn build(bytes: &[u8], stage: Stage, motion: Motion, shoes: Option<&str>) -> Option<(Vec<Frame>, Retained<CGImage>)> {
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
    pub fn load(stage: Stage, shoes: Option<&str>) -> Option<Atlas> {
        let sheets = &SHEETS[art::stage_index(stage)];
        let mut motions = Vec::with_capacity(MOTIONS.len());
        let mut sources = Vec::with_capacity(MOTIONS.len());
        for motion in MOTIONS {
            let (frames, source) = build(sheets[motion as usize], stage, motion, shoes)?;
            motions.push(frames);
            sources.push(source);
        }
        Some(Atlas { stage, shoes: shoes.map(str::to_string), motions, _sources: sources })
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
    fn all_stages_decode_with_tight_frames_on_the_ground() {
        for stage in [Stage::Baby, Stage::Growing, Stage::Adult] {
            let atlas = Atlas::load(stage, Some("yellow_boots")).expect("atlas should decode");
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
