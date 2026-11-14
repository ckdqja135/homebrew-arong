//! 코드로 그린 도트 그림(모자·집)과 픽셀 유틸리티.
//! 별도 에셋 파일이 생기기 전까지 쓰는 임시 그림이며, 외부 파일을 읽지 않는다.

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_app_kit::{NSBitmapImageRep, NSDeviceRGBColorSpace};
use objc2_core_graphics::CGImage;

use arong_core::Stage;


pub type Rgba = [u8; 4];

const CLEAR: Rgba = [0, 0, 0, 0];
const OUTLINE: Rgba = [38, 28, 30, 255];

#[derive(Clone)]
pub struct Art {
    pub w: usize,
    pub h: usize,
    pub px: Vec<Rgba>,
}

impl Art {
    pub fn new(w: usize, h: usize) -> Art {
        Art { w, h, px: vec![CLEAR; w * h] }
    }

    pub fn get(&self, x: i32, y: i32) -> Rgba {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return CLEAR;
        }
        self.px[y as usize * self.w + x as usize]
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgba) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            self.px[y as usize * self.w + x as usize] = c;
        }
    }

    /// 왼쪽·위 포함, 오른쪽·아래 제외.
    pub fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgba) {
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, c);
            }
        }
    }

    /// 글자 격자를 그림으로. 가장자리에 외곽선 자리 1px을 둔다.
    fn from_ascii(rows: &[&str], palette: &[(char, Rgba)], outline: bool) -> Art {
        let w = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let mut art = Art::new(w + 2, rows.len() + 2);
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if let Some((_, c)) = palette.iter().find(|(k, _)| *k == ch) {
                    art.set(x as i32 + 1, y as i32 + 1, *c);
                }
            }
        }
        if outline {
            art.outline();
        }
        art
    }

    /// 불투명 픽셀에 맞닿은 투명 픽셀을 어두운 외곽선으로 채운다.
    fn outline(&mut self) {
        let src = self.clone();
        for y in 0..self.h as i32 {
            for x in 0..self.w as i32 {
                let near = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| src.get(x + dx, y + dy)[3] > 0);
                if src.get(x, y)[3] == 0 && near {
                    self.set(x, y, OUTLINE);
                }
            }
        }
    }



    /// 주어진 영역에서 불투명 픽셀을 감싸는 최소 사각형(영역 기준 x, y, 너비, 높이). 비어 있으면 `None`.
    pub fn opaque_bounds(&self, x0: usize, y0: usize, w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..h {
            for x in 0..w {
                if self.get((x0 + x) as i32, (y0 + y) as i32)[3] > 127 {
                    min_x = min_x.min(x);
                    min_y = min_y.min(y);
                    max_x = max_x.max(x);
                    max_y = max_y.max(y);
                }
            }
        }
        (min_x <= max_x).then(|| (min_x, min_y, max_x - min_x + 1, max_y - min_y + 1))
    }

    /// 정수배 최근접 확대(미리보기용).
    pub fn scaled(&self, k: usize) -> Art {
        let mut out = Art::new(self.w * k, self.h * k);
        for y in 0..out.h {
            for x in 0..out.w {
                out.px[y * out.w + x] = self.px[(y / k) * self.w + x / k];
            }
        }
        out
    }

    /// 디코딩된 비트맵(8비트 RGBA)을 픽셀 배열로 복사한다.
    pub fn from_rep(rep: &NSBitmapImageRep) -> Option<Art> {
        let (w, h) = (rep.pixelsWide() as usize, rep.pixelsHigh() as usize);
        let (stride, bpp) = (rep.bytesPerRow() as usize, rep.bitsPerPixel() as usize);
        let data = rep.bitmapData();
        if data.is_null() || bpp != 32 || rep.isPlanar() || stride < w * 4 || w == 0 || h == 0 {
            return None;
        }
        // SAFETY: 비평면 32bpp 비트맵의 버퍼는 stride × h 바이트이고 rep가 살아 있는 동안 유효하다.
        let bytes = unsafe { std::slice::from_raw_parts(data, stride * h) };
        let mut art = Art::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let i = y * stride + x * 4;
                art.px[y * w + x] = [bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]];
            }
        }
        Some(art)
    }

    pub fn to_rep(&self) -> Option<Retained<NSBitmapImageRep>> {
        if self.w == 0 || self.h == 0 {
            return None;
        }
        // SAFETY: planes에 NULL을 주면 rep가 버퍼를 직접 할당하고 소유한다.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                self.w as isize,
                self.h as isize,
                8,
                4,
                true,
                false,
                NSDeviceRGBColorSpace,
                (self.w * 4) as isize,
                32,
            )
        }?;
        let data = rep.bitmapData();
        let stride = rep.bytesPerRow() as usize;
        if data.is_null() || stride < self.w * 4 {
            return None;
        }
        // SAFETY: 위에서 요청한 크기(stride × h)의 버퍼다. 알파는 0 또는 255만 쓰므로
        // 프리멀티플라이 여부와 무관하게 값이 같다.
        let bytes = unsafe { std::slice::from_raw_parts_mut(data, stride * self.h) };
        for y in 0..self.h {
            for x in 0..self.w {
                let c = self.px[y * self.w + x];
                let c = if c[3] > 127 { [c[0], c[1], c[2], 255] } else { CLEAR };
                bytes[y * stride + x * 4..y * stride + x * 4 + 4].copy_from_slice(&c);
            }
        }
        Some(rep)
    }

    pub fn cg_image(&self) -> Option<Retained<CGImage>> {
        self.to_rep()?.CGImage()
    }
}

/// 프레임 안에서 머리 꼭대기 위치(셀 좌상단 기준 px). 모자를 얹는 기준점이다.
/// 몸이 검은색이라는 점을 이용해, 하트·반짝이 같은 효과 픽셀은 건너뛴다.
pub fn head_anchor(atlas: &Art, col: usize, row: usize, cell: usize, side: bool) -> (i32, i32) {
    let dark = |x: usize, y: usize| {
        let c = atlas.get((col * cell + x) as i32, (row * cell + y) as i32);
        c[3] > 128 && c[0].max(c[1]).max(c[2]) < 70
    };
    // 옆모습은 머리가 왼쪽 절반에 있다. 꼬리가 머리보다 높이 솟아 있어도 속지 않게 왼쪽만 본다.
    let limit = if side {
        let xs: Vec<usize> = (0..cell).filter(|&x| (0..cell).any(|y| dark(x, y))).collect();
        match (xs.first(), xs.last()) {
            (Some(&lo), Some(&hi)) => lo + (hi - lo) / 2,
            _ => cell,
        }
    } else {
        cell
    };
    // 반짝이 같은 작은 어두운 조각에 속지 않도록, 두 줄 연속으로 충분히 넓은 곳을 머리 꼭대기로 본다.
    let wide = |y: usize| (0..limit).filter(|&x| dark(x, y)).count() >= 4;
    let Some(top) = (0..cell - 1).find(|&y| wide(y) && wide(y + 1)) else {
        return (cell as i32 / 2, cell as i32 / 4);
    };
    // 귀 끝부터 몇 줄 아래까지의 가로 중심 = 머리 중심.
    let (mut sum, mut n) = (0usize, 0usize);
    for y in top..(top + 10).min(cell) {
        for x in (0..limit).filter(|&x| dark(x, y)) {
            sum += x;
            n += 1;
        }
    }
    ((sum as f64 / n.max(1) as f64).round() as i32, top as i32)
}

/// 머리 기준점에 얹는 장식(모자·안경·목 장식).
pub struct Accessory {
    pub art: Art,
    /// 단계별(아기, 성장기, 성견) 위치: 머리 꼭대기 중심에서 본 (그림 가로 중심 dx, 그림 위쪽 dy) px.
    pub offset: [(i32, i32); 3],
    /// 옆모습(걷기) 프레임에서의 위치.
    pub side_offset: [(i32, i32); 3],
    /// 정면(정면 보기) 프레임에서의 위치.
    pub front_offset: [(i32, i32); 3],
}

pub fn stage_index(stage: Stage) -> usize {
    match stage {
        Stage::Baby => 0,
        Stage::Growing => 1,
        Stage::Adult => 2,
    }
}

const W: Rgba = [245, 240, 232, 255];
const G: Rgba = [205, 198, 190, 255];
const R: Rgba = [214, 62, 62, 255];
const RL: Rgba = [240, 110, 100, 255];
const P: Rgba = [142, 98, 214, 255];
const B: Rgba = [255, 214, 92, 255];
const Y: Rgba = [250, 204, 48, 255];
const YD: Rgba = [214, 160, 30, 255];
const C: Rgba = [78, 170, 230, 255];
const CD: Rgba = [44, 110, 180, 255];
const S: Rgba = [232, 200, 130, 255];
const SD: Rgba = [198, 160, 92, 255];
const LENS: Rgba = [176, 214, 240, 255];
const NAVY: Rgba = [36, 44, 70, 255];
const PINK: Rgba = [244, 110, 160, 255];
const PINKL: Rgba = [255, 180, 206, 255];
const PD: Rgba = [104, 68, 170, 255];
const GR: Rgba = [84, 170, 96, 255];
const GRL: Rgba = [146, 210, 124, 255];
const O: Rgba = [244, 150, 50, 255];
const OL: Rgba = [255, 200, 120, 255];

pub fn accessory_art(id: &str) -> Option<Accessory> {
    // 모자는 머리 꼭대기보다 `sink`px 눌러 씌운다.
    let hat = |rows: &[&str], palette: &[(char, Rgba)], sink: i32| {
        let art = Art::from_ascii(rows, palette, true).scaled(ACC_SCALE);
        let dy = sink * ACC_SCALE as i32 - art.h as i32;
        Some(Accessory { art, offset: [(0, dy); 3], side_offset: [(0, dy); 3], front_offset: [(0, dy); 3] })
    };
    let glasses = |rows: &[&str], palette: &[(char, Rgba)]| {
        Some(Accessory { art: Art::from_ascii(rows, palette, false).scaled(ACC_SCALE), offset: GLASSES_OFFSET, side_offset: GLASSES_SIDE_OFFSET, front_offset: GLASSES_FRONT_OFFSET })
    };
    let neck = |rows: &[&str], palette: &[(char, Rgba)]| {
        Some(Accessory { art: Art::from_ascii(rows, palette, true).scaled(ACC_SCALE), offset: NECK_OFFSET, side_offset: NECK_SIDE_OFFSET, front_offset: NECK_FRONT_OFFSET })
    };
    match id {
        "party" => hat(
            &[
                ".....yy.....",
                ".....yy.....",
                ".....pp.....",
                "....pppp....",
                "....pbpp....",
                "...ppppbp...",
                "...pbpppp...",
                "..ppppbppp..",
                "..pbppppbp..",
                ".ppppbppppp.",
                ".pbppppbppp.",
                "pppppbpppppp",
                "wwwwwwwwwwww",
            ],
            &[('y', Y), ('p', P), ('b', B), ('w', W)],
            3,
        ),
        "beanie" => hat(
            &[
                "......wwww......",
                ".....wwwwww.....",
                "......wwww......",
                "....rrrrrrrr....",
                "..rrrrrrrrrrrr..",
                ".rrRRrrrrrrrrrr.",
                ".rRrrrrrrrrrrrr.",
                "rrrrrrrrrrrrrrrr",
                "wwwwwwwwwwwwwwww",
                "wgwgwgwgwgwgwgwg",
            ],
            &[('w', W), ('g', G), ('r', R), ('R', RL)],
            5,
        ),
        "straw" => hat(
            &[
                ".......ssssssss.......",
                "......ssSsssssss......",
                "......sSssssssss......",
                "......rrrrrrrrrr......",
                "......rrrrrrrrrr......",
                "..ssssssssssssssssss..",
                "ssssSSssssssssssssssss",
                "..dddddddddddddddddd..",
            ],
            &[('s', S), ('S', W), ('d', SD), ('r', R)],
            5,
        ),
        "crown" => hat(
            &[
                "y.....yy.....y",
                "yy...yyyy...yy",
                "yyy.yyyyyy.yyy",
                "yyyyyyyyyyyyyy",
                "yryyyyccyyyyry",
                "yyyyyyyyyyyyyy",
                "dddddddddddddd",
            ],
            &[('y', Y), ('d', YD), ('r', R), ('c', C)],
            3,
        ),
        "round_glasses" => glasses(
            &[
                ".ggg...ggg.",
                "gllwg.gllwg",
                "glllggglllg",
                "glllg.glllg",
                ".ggg...ggg.",
            ],
            &[('g', Y), ('l', LENS), ('w', W)],
        ),
        "sunglasses" => glasses(
            &[
                "wwwww.wwwww",
                "wnnhnwnnhnw",
                "wnnnnwnnnnw",
                ".wwww.wwww.",
            ],
            &[('w', W), ('n', NAVY), ('h', C)],
        ),
        "heart_glasses" => glasses(
            &[
                ".p.p...p.p.",
                "pPppp.pPppp",
                "ppppppppppp",
                ".ppp...ppp.",
                "..p.....p..",
            ],
            &[('p', PINK), ('P', PINKL)],
        ),
        "ribbon" => neck(
            &[
                "rr.....rr",
                "rRr.y.rRr",
                "rrrryrrrr",
                "rrr.y.rrr",
                "rr.....rr",
            ],
            &[('r', R), ('R', RL), ('y', Y)],
        ),
        "bowtie" => neck(
            &[
                "cc.....cc",
                "cCc.w.cdc",
                "ccccwcccc",
                "cdc.w.cCc",
                "cc.....cc",
            ],
            &[('c', C), ('C', W), ('d', CD), ('w', W)],
        ),
        "scarf" => neck(
            &[
                "rrwrrwrrwrrwrr",
                "rwrrwrrwrrwrrw",
                ".........rrw..",
                ".........rwr..",
                ".........w.w..",
            ],
            &[('r', R), ('w', W)],
        ),
        "headbow" => hat(
            &[
                "pp........pp",
                "pPpp....ppPp",
                "pppppqqppppp",
                "pPpppqqppPpp",
                "ppp......ppp",
            ],
            &[('p', PINK), ('P', PINKL), ('q', R)],
            3,
        ),
        "flower_crown" => hat(
            &[
                "..pp....ww....pp..",
                ".pypp..wyww..pypp.",
                "gppppggwwwwggppppg",
                ".gGggGggGggGggGgg.",
            ],
            &[('p', PINK), ('y', Y), ('w', W), ('g', GR), ('G', GRL)],
            3,
        ),
        "cap" => hat(
            &[
                "........cccccc....",
                "......ccCccccccc..",
                ".....ccCccccccccc.",
                ".....cccccwwcccccc",
                ".....ccccccccccccc",
                "dddddddddddddddddd",
            ],
            &[('c', C), ('C', W), ('d', CD), ('w', W)],
            4,
        ),
        "chef" => hat(
            &[
                "..www.wwww.www..",
                ".wwwwwwwwwwwwww.",
                "wwwwwwwwwwwwwwww",
                "wwwwwwwwwwwwwwww",
                ".wwgwwwgwwwgwww.",
                "..wwwwwwwwwwww..",
                "..wgwgwgwgwgww..",
                "..wwwwwwwwwwww..",
                "..gggggggggggg..",
            ],
            &[('w', W), ('g', G)],
            3,
        ),
        "santa" => hat(
            &[
                "...........ww...",
                "..........wwww..",
                ".........rrww...",
                ".......rrrrr....",
                ".....rrrrrrrr...",
                "....rrRrrrrrrr..",
                "...rrRrrrrrrrr..",
                "..rrrrrrrrrrrr..",
                "wwwwwwwwwwwwwwww",
                "wgwwgwwgwwgwwgww",
            ],
            &[('w', W), ('g', G), ('r', R), ('R', RL)],
            5,
        ),
        "chick" => hat(
            &[
                "...yyyy...",
                "..yyyyyy..",
                "ooynyyyy..",
                "..yyyyyyy.",
                ".yyyYYyyyy",
                ".yyyyyyyy.",
                "..yyyyyy..",
            ],
            &[('y', Y), ('Y', YD), ('o', O), ('n', NAVY)],
            1,
        ),
        "wizard" => hat(
            &[
                ".........pp.......",
                "........ppp.......",
                "........pppp......",
                ".......ppypp......",
                ".......ppppp......",
                "......pppppyp.....",
                "......ppppppp.....",
                ".....pyppppppp....",
                ".....yyyyyyyyy....",
                "...ppppppppppppp..",
                "dddddddddddddddddd",
            ],
            &[('p', P), ('d', PD), ('y', Y)],
            4,
        ),
        "movie_glasses" => glasses(
            &[
                "wwwwwwwwwww",
                "wrrrrwccccw",
                "wrrrrwccccw",
                "wwwww.wwwww",
            ],
            &[('w', W), ('r', R), ('c', C)],
        ),
        "goggles" => glasses(
            &[
                ".nnnnnnnnn.",
                "nooOoooooon",
                "noooooooOon",
                ".nnnnnnnnn.",
            ],
            &[('n', NAVY), ('o', O), ('O', OL)],
        ),
        "star_glasses" => glasses(
            &[
                "..y.....y..",
                ".yyy...yyy.",
                "yyYyyyyyYyy",
                ".yyy...yyy.",
                ".y.y...y.y.",
            ],
            &[('y', Y), ('Y', W)],
        ),
        "bell" => neck(
            &[
                "rrrrrrrrrr",
                "rrrryyrrrr",
                "...yYyy...",
                "...yyyy...",
                "....nn....",
            ],
            &[('r', R), ('y', Y), ('Y', W), ('n', YD)],
        ),
        "bandana" => neck(
            &[
                "rrrrrrrrrrrr",
                ".rrwrrrrwrr.",
                "..rrrrwrrr..",
                "...rrrrrr...",
                "....rwrr....",
                ".....rr.....",
            ],
            &[('r', R), ('w', W)],
        ),
        "necktie" => neck(
            &[
                "wwwwwwww",
                "..wnnw..",
                "...nn...",
                "..nnnn..",
                "..ncnn..",
                "..nnnn..",
                "...nn...",
            ],
            &[('w', W), ('n', CD), ('c', C)],
        ),
        "pearls" => neck(
            &[
                "wgwgwgwgwgw",
                "....wpw....",
                ".....w.....",
            ],
            &[('w', W), ('g', G), ('p', PINKL)],
        ),
        "medal" => neck(
            &[
                "c.......c",
                ".c.....c.",
                "..c...c..",
                "...yyy...",
                "..yyYyy..",
                "..yydyy..",
                "...yyy...",
            ],
            &[('c', C), ('y', Y), ('Y', W), ('d', YD)],
        ),
        "lei" => neck(
            &[
                ".pp.ww.pp.ww.pp.",
                "pyypwyywpyypwyyw",
                ".pp.ww.pp.ww.pp.",
            ],
            &[('p', PINK), ('w', W), ('y', Y)],
        ),
        "bib" => neck(
            &[
                ".pp......pp.",
                "pwwp....pwwp",
                "pwwwppppwwwp",
                "pwwwwwwwwwwp",
                "pwwwrwrwwwwp",
                "pwwwrrrwwwwp",
                ".pwwwrwwwwp.",
                "..pwwwwwwp..",
                "...pppppp...",
            ],
            &[('p', PINK), ('w', W), ('r', R)],
        ),
        "sailor" => neck(
            &[
                "wwwwwwwwwwwwww",
                "wnnnnwwwwnnnnw",
                "wnnnnnwwnnnnnw",
                ".wnnnnrrnnnnw.",
                "..wwwwrrwwww..",
                "......rr......",
                ".....rrrr.....",
            ],
            &[('w', W), ('n', CD), ('r', R)],
        ),
        "capelet" => neck(
            &[
                "....rryyrr....",
                "..rrrryyrrrr..",
                ".rrRrrrrrrrrr.",
                "rrRrrrrrrrrrrr",
                "rrrrrrrrrrrrrr",
                "wwnwwwnwwwnwww",
                "wwwwnwwwnwwwww",
            ],
            &[('r', R), ('R', RL), ('y', Y), ('w', W), ('n', NAVY)],
        ),
        _ => None,
    }
}

/// 얼굴이 살짝 왼쪽을 보고 있어서 눈은 머리 중심보다 왼쪽에 있다.
const GLASSES_OFFSET: [(i32, i32); 3] = [(-6, 22), (-6, 26), (-8, 30)];
pub const NECK_OFFSET: [(i32, i32); 3] = [(-4, 44), (-4, 48), (-6, 54)];
/// 정면 보기에서는 얼굴이 가운데에 온다(성장기는 살짝 왼쪽을 본 채로 그려져 있다).
const GLASSES_FRONT_OFFSET: [(i32, i32); 3] = [(0, 24), (-4, 26), (0, 30)];
const NECK_FRONT_OFFSET: [(i32, i32); 3] = [(0, 46), (-3, 48), (0, 54)];
/// 옆모습에서는 눈이 머리 앞쪽(왼쪽)에, 목이 머리 바로 아래 뒤쪽에 있다.
const GLASSES_SIDE_OFFSET: [(i32, i32); 3] = [(-6, 18), (-4, 26), (-9, 31)];
pub const NECK_SIDE_OFFSET: [(i32, i32); 3] = [(-3, 37), (2, 43), (-4, 53)];
/// 장식 그림 1칸이 차지하는 스프라이트 픽셀 수.
const ACC_SCALE: usize = 2;

fn hsv(c: Rgba) -> (f64, f64, f64) {
    let (r, g, b) = (c[0] as f64 / 255.0, c[1] as f64 / 255.0, c[2] as f64 / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

/// 색만 다른 옷을 만들 때 바꿔 칠하는 방법.
#[derive(Clone, Copy)]
pub struct Tint {
    /// 그림에 그려진 원래 옷 색과 새 색.
    pub from: Rgba,
    pub to: Rgba,
    /// 원래 옷 색에서 색상이 몇 도까지 벗어난 픽셀을 옷 색으로 볼지. 여러 색이 섞인 옷은 좁게 준다.
    pub spread: f64,
    /// 이보다 밝은 픽셀은 옷 색으로 보지 않는다(같은 계열의 밝은 줄무늬를 남길 때 쓴다). 1.0이면 제한 없음.
    pub brightest: f64,
    /// 이보다 흐린(채도가 낮은) 픽셀은 옷 색으로 보지 않는다. 파스텔 옷은 낮게 준다.
    pub min_sat: f64,
}

fn from_hsv(h: f64, s: f64, v: f64) -> Rgba {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |k: f64| ((k + v - c) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b), 255]
}

/// 색만 다른 옷: 옷 그림에서 원래 옷 색(`from`) 계열의 픽셀만 `color` 계열로 바꾼다(명암은 유지한다).
/// 옷 픽셀은 "옷 없는 그림과 달라진, 원래 옷 색과 색상이 가까운 픽셀"로 찾으므로, 털·흰 줄무늬·끈·효과 픽셀은 건드리지 않는다.
pub fn tint_garment(dressed: &mut Art, bare: &Art, tint: &Tint) {
    let (base_h, base_s, base_v) = hsv(tint.from);
    let (to_h, to_s, to_v) = hsv(tint.to);
    for (pixel, under) in dressed.px.iter_mut().zip(&bare.px) {
        let (h, s, v) = hsv(*pixel);
        let apart = (h - base_h).rem_euclid(360.0);
        if pixel[3] > 128 && pixel != under && s > tint.min_sat && apart.min(360.0 - apart) <= tint.spread && v <= tint.brightest {
            *pixel = from_hsv(to_h, (s * to_s / base_s).min(1.0), (v * to_v / base_v).min(1.0));
        }
    }
}

fn shade(c: Rgba, k: f64) -> Rgba {
    let f = |v: u8| (v as f64 * k).round().clamp(0.0, 255.0) as u8;
    [f(c[0]), f(c[1]), f(c[2]), 255]
}

/// 옷·신발의 기준 색.
pub fn wear_color(id: &str) -> Option<Rgba> {
    Some(match id {
        "red_wear" | "red_boots" => [220, 64, 64, 255],
        "blue_wear" => [70, 124, 214, 255],
        "green_wear" => [72, 168, 96, 255],
        "brown_wear" => [196, 112, 66, 255],
        "blue_knit" => [52, 132, 222, 255],
        "red_knit" => [222, 62, 62, 255],
        "yellow_knit" => [246, 196, 50, 255],
        "pink_knit" => [246, 124, 172, 255],
        "green_knit" => [66, 172, 98, 255],
        "coral_hoodie" => [240, 96, 72, 255],
        "sky_hoodie" => [96, 180, 236, 255],
        "yellow_hoodie" => [246, 196, 50, 255],
        "mint_hoodie" => [84, 204, 168, 255],
        "purple_hoodie" => [150, 104, 220, 255],
        "teal_hanbok" => [28, 160, 164, 255],
        "pink_hanbok" => [240, 118, 168, 255],
        "yellow_hanbok" => [244, 194, 54, 255],
        "pink_bunny" => [240, 110, 150, 255],
        "yellow_bear" => [246, 190, 40, 255],
        "green_dino" => [72, 172, 60, 255],
        "yellow_dino" => [246, 200, 56, 255],
        "white_dino" => [236, 236, 230, 255],
        "pink_dino" => [246, 128, 176, 255],
        "red_dino" => [222, 62, 62, 255],
        "camel_trench" => [164, 127, 89, 255],
        "red_trench" => [206, 70, 66, 255],
        "pink_trench" => [236, 138, 170, 255],
        "sky_trench" => [112, 176, 226, 255],
        "olive_trench" => [128, 156, 78, 255],
        "lavender_cape" => [187, 142, 203, 255],
        "pink_cape" => [246, 150, 184, 255],
        "sky_cape" => [130, 196, 240, 255],
        "yellow_cape" => [248, 214, 100, 255],
        "mint_cape" => [120, 214, 180, 255],
        "sky_puffer" => [120, 202, 218, 255],
        "pink_puffer" => [246, 140, 180, 255],
        "yellow_puffer" => [248, 206, 70, 255],
        "red_puffer" => [226, 76, 70, 255],
        "purple_puffer" => [164, 124, 226, 255],
        "yellow_boots" => [246, 200, 52, 255],
        "blue_boots" => [70, 124, 214, 255],
        "pink_boots" => [244, 120, 168, 255],
        "purple_boots" => [142, 98, 214, 255],
        _ => return None,
    })
}

/// 밥 먹을 때 발 앞에 놓는 밥그릇.
pub fn bowl_art() -> Art {
    Art::from_ascii(
        &[
            "...ffffffff...",
            "..ffFffffFff..",
            "bbbbbbbbbbbbbb",
            "bBBbbbbbbbbbbb",
            ".bbbbbbbbbbbb.",
            "..dddddddddd..",
        ],
        &[('f', [176, 116, 62, 255]), ('F', [214, 160, 96, 255]), ('b', C), ('B', W), ('d', CD)],
        true,
    )
    .scaled(ACC_SCALE)
}

/// 상점에 보여 줄 색 견본.
pub fn swatch(color: Rgba) -> Art {
    let mut art = Art::new(14, 14);
    art.rect(2, 2, 12, 12, color);
    art.rect(3, 3, 11, 5, shade(color, 1.25));
    art.rect(2, 10, 12, 12, shade(color, 0.7));
    art.outline();
    art
}

/// 몸통 픽셀(검은 털·회색·크림색)인지. 간식·하트 같은 선명한 효과 픽셀은 제외한다.
fn is_body(c: Rgba) -> bool {
    let (_, s, v) = hsv(c);
    c[3] > 128 && (v < 0.3 || s < 0.45)
}

/// 장화의 높이(px)와 밑창 색.
const BOOT_H: i32 = 10;
/// 바닥에서 이만큼(px) 떠 있는 발까지 장화를 신긴다.
const FOOT_LIFT: i32 = 8;
const SOLE: Rgba = [58, 46, 48, 255];

/// 신발: 프레임마다 바닥에 닿은 발(좁은 기둥)에 장화를 신긴다.
/// 그림을 따로 얹지 않고 원본 다리 모양을 따라가므로 걷는 동작과 어긋나지 않는다.
/// 발 모양 그대로 칠하면 "발을 색칠한" 것처럼 보이므로, 장화답게 목 높이를 일자로 맞추고
/// 접힌 목단·광택·평평한 밑창을 넣는다. 공중에 든 발은 찾지 못해 신기지 않는다.
pub fn paint_shoes(atlas: &mut Art, cell: usize, color: Rgba) {
    for row in 0..atlas.h / cell {
        for col in 0..atlas.w / cell {
            let (ox, oy) = ((col * cell) as i32, (row * cell) as i32);
            let lowest: Vec<Option<i32>> = (0..cell as i32)
                .map(|x| (0..cell as i32).rev().find(|&y| is_body(atlas.get(ox + x, oy + y))))
                .collect();
            let Some(ground) = lowest.iter().flatten().copied().max() else { continue };
            // 발은 바닥에 닿아 있고 발끝이 밝은색(크림·회색)이다. 검은 털 덩어리(배·꼬리)는 제외한다.
            // 걷느라 살짝 든 발도 포함하도록 바닥에서 몇 px 떠 있는 것까지 본다.
            let mut foot: Vec<bool> = (0..cell)
                .map(|x| {
                    lowest[x].is_some_and(|y| {
                        y >= ground - FOOT_LIFT && (y - 3..=y).any(|yy| hsv(atlas.get(ox + x as i32, oy + yy)).2 > 0.36)
                    })
                })
                .collect();
            // 발가락 사이의 어두운 틈(한두 칸)으로 발 하나가 둘로 갈라지지 않게 메운다.
            for x in 1..cell.saturating_sub(2) {
                if !foot[x] && foot[x - 1] && (foot[x + 1] || foot[x + 2]) && lowest[x].is_some_and(|y| y >= ground - FOOT_LIFT) {
                    foot[x] = true;
                }
            }
            // 발 구간을 모은다. 나란히 붙은 두 발(아기처럼 다리가 짧을 때)은 반으로 나눈다.
            let mut runs: Vec<(i32, i32)> = Vec::new();
            let mut x = 0;
            while x < cell {
                if !foot[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < cell && foot[x] {
                    x += 1;
                }
                let (start, end) = (start as i32, x as i32);
                match end - start {
                    // 너무 좁은 조각은 발이 아니다.
                    0..=2 => {}
                    3..=20 => runs.push((start, end)),
                    21..=32 => {
                        let mid = (start + end) / 2;
                        runs.push((start, mid));
                        runs.push((mid + 1, end));
                    }
                    // 넓게 바닥에 닿은 부분은 발이 아니라 앉은 엉덩이·배다.
                    _ => {}
                }
            }
            for (run_start, run_end) in runs {
                let (mut start, mut end) = (run_start, run_end);
                // 이 발의 바닥 줄과 장화 목 높이(발 전체에서 일자).
                let sole = (start..end).filter_map(|fx| lowest[fx as usize]).max().unwrap_or(ground);
                // 밝은 발끝만 덮으면 발 위에 상자를 붙인 것처럼 보인다. 발 가장자리의 어두운 윤곽까지 넓혀 발 전체를 감싼다.
                // 윤곽은 얇으므로, 몇 칸 밖이 바로 빈 곳일 때만 넓힌다(옆 다리나 앉은 몸통으로 번지지 않게).
                let solid = |fx: i32| (sole - 4..=sole - 1).any(|y| atlas.get(ox + fx, oy + y)[3] > 128);
                let open = |fx: i32, dir: i32| (1..=3).any(|d| atlas.get(ox + fx + dir * d, oy + sole - 2)[3] <= 128);
                for _ in 0..3 {
                    if start > 0 && !foot[(start - 1) as usize] && solid(start - 1) && open(start - 1, -1) {
                        start -= 1;
                    }
                    if end < cell as i32 && !foot[end as usize] && solid(end) && open(end, 1) {
                        end += 1;
                    }
                }

                // 작은 발에는 낮은 장화를 신긴다.
                let height = ((end - start) * 3 / 4).clamp(7, BOOT_H);
                let top = sole - height + 1;
                for fx in start..end {
                    let bottom = lowest[fx as usize].unwrap_or(sole).min(sole);
                    for y in top..=sole {
                        // 목단 모서리는 한 칸 깎아 둥글게 한다.
                        if y == top && (fx == start || fx == end - 1) {
                            continue;
                        }
                        let inside = atlas.get(ox + fx, oy + y)[3] > 128;
                        // 발끝 아래의 둥근 빈틈은 밑창으로 메워 바닥을 평평하게 한다.
                        if !inside && y <= bottom {
                            continue;
                        }
                        let c = if y >= sole {
                            SOLE
                        } else if y == top {
                            shade(color, 1.35)
                        } else if y == top + 1 {
                            shade(color, 1.15)
                        } else if y == top + 2 {
                            shade(color, 0.7)
                        } else if fx == start || fx == end - 1 {
                            shade(color, 0.55)
                        } else if (fx == start + 2 || fx == start + 3) && (y == sole - 3 || y == sole - 4) {
                            shade(color, 1.4)
                        } else {
                            color
                        };
                        atlas.set(ox + fx, oy + y, c);
                    }
                }
            }
        }
    }
}

/// 상점에 보여 줄 장화 그림.
pub fn boot_icon(color: Rgba) -> Art {
    Art::from_ascii(
        &[
            "...LLLLL....",
            "...lllll....",
            "...ddddd....",
            "...ccccc....",
            "...cHccc....",
            "...cHccc....",
            "...cccccc...",
            "...cccccccc.",
            "..cccccccHcc",
            "..cccccccccc",
            "..ssssssssss",
        ],
        &[('L', shade(color, 1.35)), ('l', shade(color, 1.15)), ('d', shade(color, 0.7)), ('c', color), ('H', shade(color, 1.4)), ('s', SOLE)],
        true,
    )
}

/// 옷: 몸통 털 위에 줄무늬 조끼를 덧칠한다. 옷 입은 프레임 에셋이 생기기 전까지의 임시 방식이다.
pub const HOUSE_W: usize = 64;
pub const HOUSE_H: usize = 56;
/// 집 그림에서 문 아래 가운데의 위치: (왼쪽에서 px, 아래에서 px). 2·3단계 집에 해당한다.
pub const HOUSE_DOOR: (f64, f64) = (32.0, 2.0);

/// 집 단계별 그림. 1 = 방석, 2 = 나무 집, 3 = 파란 지붕 집.
pub fn house_art(level: u32) -> Option<Art> {
    let mut a = Art::new(HOUSE_W, HOUSE_H);
    match level {
        1 => {
            // 방석: 둥근 쿠션.
            a.rect(10, 45, 54, 54, [196, 70, 74, 255]);
            a.rect(8, 47, 56, 52, [196, 70, 74, 255]);
            a.rect(12, 45, 52, 47, [232, 116, 112, 255]);
            a.rect(16, 48, 48, 51, [166, 52, 60, 255]);
            a.rect(20, 49, 44, 50, [150, 44, 54, 255]);
        }
        2 | 3 => {
            let fancy = level == 3;
            let (roof, roof_dark) =
                if fancy { ([72, 118, 198, 255], [50, 88, 160, 255]) } else { ([196, 66, 56, 255], [156, 46, 42, 255]) };
            let wall = if fancy { [240, 226, 198, 255] } else { [186, 130, 78, 255] };
            let plank = if fancy { [216, 198, 166, 255] } else { [156, 104, 58, 255] };
            if fancy {
                // 굴뚝은 지붕보다 먼저 그려 지붕에 가려지게 한다.
                a.rect(44, 6, 51, 20, [164, 88, 66, 255]);
                a.rect(43, 5, 52, 8, [120, 62, 50, 255]);
            }
            a.rect(12, 26, 52, 54, wall);
            for y in (31..54).step_by(6) {
                a.rect(12, y, 52, y + 1, plank);
            }
            for y in 6..=26 {
                let half = (y - 6) * 26 / 20 + 2;
                a.rect(32 - half, y, 32 + half, y + 1, if y >= 25 { roof_dark } else { roof });
            }
            // 아치형 문.
            let door = [62, 42, 34, 255];
            a.rect(24, 38, 40, 54, door);
            a.rect(25, 37, 39, 38, door);
            a.rect(27, 36, 37, 37, door);
            // 문패.
            a.rect(26, 29, 38, 34, [246, 214, 96, 255]);
            a.rect(28, 31, 36, 32, [150, 104, 40, 255]);
            if fancy {
                a.rect(15, 38, 21, 45, [150, 204, 236, 255]);
                a.rect(18, 38, 19, 45, [250, 250, 250, 255]);
                a.rect(15, 41, 21, 42, [250, 250, 250, 255]);
                // 화단.
                for (x, c) in [(44, [240, 120, 160, 255]), (48, [250, 210, 80, 255])] {
                    a.rect(x, 50, x + 1, 54, [84, 160, 84, 255]);
                    a.rect(x - 1, 47, x + 2, 50, c);
                }
            }
        }
        _ => return None,
    }
    a.outline();
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arong_core::Slot;

    #[test]
    fn every_catalog_item_has_art() {
        for def in arong_core::ITEMS.iter() {
            match def.slot {
                Slot::Hat | Slot::Glasses | Slot::Neck => {
                    assert!(accessory_art(def.id).expect(def.id).art.cg_image().is_some());
                }
                Slot::Clothes | Slot::Shoes => assert!(wear_color(def.id).is_some(), "{}", def.id),
            }
        }
        for level in 1..=arong_core::MAX_HOUSE_LEVEL {
            assert!(house_art(level).expect("house art").cg_image().is_some());
        }
        assert!(house_art(0).is_none());
        assert!(bowl_art().cg_image().is_some());
    }

    #[test]
    fn opaque_bounds_wrap_only_drawn_pixels() {
        let mut art = Art::new(10, 10);
        assert_eq!(art.opaque_bounds(0, 0, 10, 10), None);
        art.rect(3, 4, 6, 9, [1, 2, 3, 255]);
        assert_eq!(art.opaque_bounds(0, 0, 10, 10), Some((3, 4, 3, 5)));
        assert_eq!(art.opaque_bounds(5, 5, 5, 5), Some((0, 0, 1, 4)));
    }
}
