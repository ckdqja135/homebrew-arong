//! 목줄 산책의 추종 규칙과 거리 집계. 좌표는 전역 포인트(pt) 단위.
//! 물리 엔진 없이 "가까우면 멈추고, 멀면 속도 제한을 두고 따라간다"만 구현한다.

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn dist(self, o: Vec2) -> f64 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
}

#[derive(Debug, Clone)]
pub struct WalkTuning {
    /// 이 거리 안에서는 멈춰 있는다.
    pub dead_zone: f64,
    /// 목줄이 팽팽해지는 길이(표시용 처짐 계산에 사용).
    pub leash_len: f64,
    /// 거리 초과분에 곱하는 추종 계수(1/s).
    pub gain: f64,
    pub max_speed: f64,
    /// 이 속도 이상이면 달리기 모션.
    pub run_speed: f64,
    /// 한 틱에 허용하는 최대 dt. 프레임이 밀려도 순간이동하지 않게 한다.
    pub max_dt: f64,
    /// 거리 표본 간격(초). 제자리 흔들기는 표본 사이 직선 거리로 상쇄된다.
    pub sample_secs: f64,
}

impl Default for WalkTuning {
    fn default() -> Self {
        Self {
            dead_zone: 60.0,
            leash_len: 140.0,
            gain: 4.0,
            max_speed: 280.0,
            run_speed: 160.0,
            max_dt: 0.1,
            sample_secs: 1.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Follower {
    pub pos: Vec2,
    /// 직전 틱의 속도(pt/s). 모션 선택에 쓴다.
    pub speed: f64,
}

impl Follower {
    pub fn new(pos: Vec2) -> Self {
        Self { pos, speed: 0.0 }
    }

    /// 커서를 향해 한 틱 이동하고 새 위치를 돌려준다.
    pub fn step(&mut self, t: &WalkTuning, cursor: Vec2, dt: f64) -> Vec2 {
        let dt = if dt.is_finite() { dt.clamp(0.0, t.max_dt) } else { 0.0 };
        let d = self.pos.dist(cursor);
        if !d.is_finite() || d <= t.dead_zone || dt == 0.0 {
            self.speed = 0.0;
            return self.pos;
        }
        let over = d - t.dead_zone;
        let speed = (t.gain * over).min(t.max_speed);
        // 데드존 안쪽까지 넘어 들어가지 않는다.
        let step = (speed * dt).min(over);
        self.pos.x += (cursor.x - self.pos.x) / d * step;
        self.pos.y += (cursor.y - self.pos.y) / d * step;
        self.speed = step / dt;
        self.pos
    }
}

/// 아롱이가 실제로 이동한 거리 집계. 경로는 보관하지 않고 직전 표본 한 점만 둔다.
#[derive(Debug, Clone)]
pub struct WalkMeter {
    last_sample: Vec2,
    since_sample: f64,
    pub distance: f64,
    pub duration: f64,
}

impl WalkMeter {
    pub fn new(start: Vec2) -> Self {
        Self { last_sample: start, since_sample: 0.0, distance: 0.0, duration: 0.0 }
    }

    pub fn tick(&mut self, t: &WalkTuning, pos: Vec2, dt: f64) {
        let dt = if dt.is_finite() { dt.clamp(0.0, t.max_dt) } else { 0.0 };
        self.duration += dt;
        self.since_sample += dt;
        if self.since_sample >= t.sample_secs {
            self.distance += self.last_sample.dist(pos);
            self.last_sample = pos;
            self.since_sample = 0.0;
        }
    }

    /// 산책 종료 시 마지막 구간을 더한 총 거리.
    pub fn finish(mut self, pos: Vec2) -> f64 {
        self.distance += self.last_sample.dist(pos);
        self.distance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stays_inside_dead_zone() {
        let t = WalkTuning::default();
        let mut f = Follower::new(Vec2::new(0.0, 0.0));
        f.step(&t, Vec2::new(59.0, 0.0), 1.0 / 30.0);
        assert_eq!(f.pos, Vec2::new(0.0, 0.0));
        assert_eq!(f.speed, 0.0);
    }

    #[test]
    fn far_cursor_never_teleports() {
        let t = WalkTuning::default();
        let mut f = Follower::new(Vec2::new(0.0, 0.0));
        // 커서가 5000pt 밖으로 튀고 프레임이 2초 밀려도 한 틱 이동량은 제한된다.
        f.step(&t, Vec2::new(5000.0, 0.0), 2.0);
        assert!(f.pos.x <= t.max_speed * t.max_dt + 1e-9);
        assert!(f.speed <= t.max_speed + 1e-9);
    }

    #[test]
    fn converges_to_dead_zone_edge_without_overshoot() {
        let t = WalkTuning::default();
        let mut f = Follower::new(Vec2::new(0.0, 0.0));
        let cursor = Vec2::new(300.0, 400.0);
        for _ in 0..600 {
            f.step(&t, cursor, 1.0 / 30.0);
            assert!(f.pos.dist(cursor) >= t.dead_zone - 1e-6);
        }
        assert!((f.pos.dist(cursor) - t.dead_zone).abs() < 1.0);
    }

    #[test]
    fn shaking_in_place_earns_almost_nothing() {
        let t = WalkTuning::default();
        let mut f = Follower::new(Vec2::new(0.0, 0.0));
        let mut m = WalkMeter::new(f.pos);
        let dt = 1.0 / 30.0;
        // 10초 동안 좌우 200pt를 초당 5번 흔든다.
        for i in 0..300 {
            let x = if (i / 3) % 2 == 0 { 200.0 } else { -200.0 };
            let p = f.step(&t, Vec2::new(x, 0.0), dt);
            m.tick(&t, p, dt);
        }
        let shaken = m.finish(f.pos);
        assert!(shaken < 300.0, "shaken = {shaken}");

        // 같은 시간 한 방향으로 걸으면 훨씬 많이 걷는다.
        let mut f = Follower::new(Vec2::new(0.0, 0.0));
        let mut m = WalkMeter::new(f.pos);
        for i in 0..300 {
            let p = f.step(&t, Vec2::new(200.0 + i as f64 * 8.0, 0.0), dt);
            m.tick(&t, p, dt);
        }
        assert!(m.finish(f.pos) > 2000.0);
    }

    #[test]
    fn bad_inputs_are_ignored() {
        let t = WalkTuning::default();
        let mut f = Follower::new(Vec2::new(1.0, 2.0));
        f.step(&t, Vec2::new(f64::NAN, 0.0), 0.03);
        f.step(&t, Vec2::new(500.0, 0.0), f64::NAN);
        f.step(&t, Vec2::new(500.0, 0.0), -1.0);
        assert_eq!(f.pos, Vec2::new(1.0, 2.0));
    }
}
