use std::collections::{BTreeMap, BTreeSet};

use crate::shop::Slot;

/// 조정 가능한 초기값. 확정값이 아니라 써 보면서 바꾸는 숫자들이다.
#[derive(Debug, Clone)]
pub struct Tuning {
    pub satiety_decay_per_hour: f64,
    pub satiety_floor: f64,
    pub mood_decay_per_hour: f64,
    pub mood_floor: f64,
    /// 앱 종료·잠자기 등 미접속 구간에 적용하는 감소의 상한(초).
    pub max_decay_secs: i64,
    pub feed_satiety: f64,
    pub feed_xp: u64,
    pub feed_xp_per_day: u32,
    pub snack_price: i64,
    pub snack_satiety: f64,
    pub snack_mood: f64,
    pub pet_mood: f64,
    pub pet_xp: u64,
    pub pet_xp_per_day: u32,
    /// 아롱이가 실제로 걸은 거리(pt) 대비 보상.
    pub walk_pt_per_unit: f64,
    pub walk_xp_per_unit: u64,
    pub walk_xp_cap_per_walk: u64,
    pub walk_mood: f64,
    /// 하루에 이 횟수까지는 100%, 이후는 `walk_extra_rate`.
    pub walk_full_per_day: u32,
    pub walk_extra_rate: f64,
    /// 얻은 XP 중 재화로 함께 쌓이는 비율(천분율).
    pub coin_per_xp_permille: u64,
    pub level_up_coins: i64,
    pub growing_level: u32,
    pub adult_level: u32,
    /// 포만감·기분이 낮을 때 활동 보상에 곱하는 배율.
    pub low_stat_threshold: f64,
    pub low_stat_rate: f64,
    /// 타이핑: 하루 이 타수까지 100%, `typing_half_keys`까지 50%, 그 뒤는 10%.
    pub typing_full_keys: u64,
    pub typing_half_keys: u64,
    /// 100% 구간에서 1 XP에 필요한 타수.
    pub typing_keys_per_xp: u64,
    /// AI: 하루 이 포인트까지 100%, `ai_half_points`까지 50%, 그 뒤는 10%.
    pub ai_full_points: u64,
    pub ai_half_points: u64,
    pub ai_points_per_xp: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            satiety_decay_per_hour: 4.0,
            satiety_floor: 20.0,
            mood_decay_per_hour: 2.0,
            mood_floor: 30.0,
            max_decay_secs: 12 * 3600,
            feed_satiety: 35.0,
            feed_xp: 5,
            feed_xp_per_day: 3,
            snack_price: 5,
            snack_satiety: 10.0,
            snack_mood: 15.0,
            pet_mood: 5.0,
            pet_xp: 2,
            pet_xp_per_day: 10,
            walk_pt_per_unit: 1000.0,
            walk_xp_per_unit: 5,
            walk_xp_cap_per_walk: 30,
            walk_mood: 20.0,
            walk_full_per_day: 3,
            walk_extra_rate: 0.2,
            coin_per_xp_permille: 300,
            level_up_coins: 20,
            growing_level: 8,
            adult_level: 20,
            low_stat_threshold: 30.0,
            low_stat_rate: 0.8,
            typing_full_keys: 5_000,
            typing_half_keys: 15_000,
            typing_keys_per_xp: 100,
            ai_full_points: 100_000,
            ai_half_points: 300_000,
            ai_points_per_xp: 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Baby,
    Growing,
    Adult,
}

/// 로컬 날짜별 보상 횟수. 날짜가 앞으로 넘어갈 때만 초기화된다.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayCounters {
    /// yyyymmdd. 0이면 아직 기록 없음.
    pub date: u32,
    pub feeds: u32,
    pub pets: u32,
    pub walks: u32,
    /// 오늘 집계된 타건 수(횟수만, 내용 없음).
    pub keys: u64,
    /// 오늘 측정된 AI 토큰 수(입력, 출력, 캐시 읽기, 캐시 쓰기).
    pub tokens: [u64; 4],
    /// 오늘 쌓인 AI 활동 포인트 × 100.
    pub ai_units: u64,
    /// 오늘 얻은 경험치(모든 경로 합계).
    pub xp: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pet {
    pub xp: u64,
    pub coins: i64,
    /// XP→재화 환산의 소수점 이월분(천분율 단위).
    pub coin_carry: u64,
    pub satiety: f64,
    pub mood: f64,
    pub affinity: u64,
    /// 포만감·기분이 마지막으로 계산된 벽시계 시각(unix 초).
    pub last_wall: i64,
    pub day: DayCounters,
    /// 구매한 아이템 id.
    pub owned: BTreeSet<String>,
    /// 부위별로 착용 중인 아이템 id.
    pub worn: BTreeMap<Slot, String>,
    /// 0 = 집 없음.
    pub house_level: u32,
    /// 타건 → XP 환산의 이월분(가중 타수 × 10).
    pub key_carry: u64,
    /// AI 포인트 → XP 환산의 이월분.
    pub ai_carry: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ActionResult {
    pub ok: bool,
    pub xp: u64,
    pub coins: i64,
    pub leveled_up: bool,
    pub stage_changed: bool,
}

/// 레벨 n → n+1에 필요한 XP.
pub fn xp_to_next(level: u32) -> u64 {
    50 + 25 * level as u64
}

/// 누적 XP → (레벨, 현재 레벨에서 쌓은 XP).
pub fn level_of(mut xp: u64) -> (u32, u64) {
    let mut level = 0;
    loop {
        let need = xp_to_next(level);
        if xp < need {
            return (level, xp);
        }
        xp -= need;
        level += 1;
    }
}

impl Pet {
    pub fn new(now_wall: i64) -> Self {
        Self {
            xp: 0,
            coins: 0,
            coin_carry: 0,
            satiety: 70.0,
            mood: 70.0,
            affinity: 0,
            last_wall: now_wall,
            day: DayCounters::default(),
            owned: BTreeSet::new(),
            worn: BTreeMap::new(),
            house_level: 0,
            key_carry: 0,
            ai_carry: 0,
        }
    }

    pub fn level(&self) -> u32 {
        level_of(self.xp).0
    }

    pub fn stage(&self, t: &Tuning) -> Stage {
        let level = self.level();
        if level >= t.adult_level {
            Stage::Adult
        } else if level >= t.growing_level {
            Stage::Growing
        } else {
            Stage::Baby
        }
    }

    /// 경과 시간만큼 포만감·기분을 줄이고 날짜 카운터를 넘긴다.
    /// 매초 타이머 없이, 상태를 읽거나 바꾸기 직전에만 호출하면 된다.
    /// 시계가 뒤로 가면 감소 없이 기준 시각만 다시 잡고, 날짜는 되돌리지 않는다.
    /// 날짜가 넘어갔으면 끝난 날의 집계를 돌려준다(일별 기록으로 남기는 용도).
    pub fn advance(&mut self, t: &Tuning, now_wall: i64, today: u32) -> Option<DayCounters> {
        let elapsed = (now_wall - self.last_wall).clamp(0, t.max_decay_secs);
        let hours = elapsed as f64 / 3600.0;
        // 이미 하한 아래라면 더 내리지 않는다.
        let satiety_floor = t.satiety_floor.min(self.satiety);
        let mood_floor = t.mood_floor.min(self.mood);
        self.satiety = (self.satiety - t.satiety_decay_per_hour * hours).max(satiety_floor);
        self.mood = (self.mood - t.mood_decay_per_hour * hours).max(mood_floor);
        self.last_wall = now_wall;
        if today > self.day.date {
            let finished = std::mem::replace(&mut self.day, DayCounters { date: today, ..DayCounters::default() });
            // 아직 기록이 없던 첫날(date 0)은 남길 것이 없다.
            return (finished.date != 0).then_some(finished);
        }
        None
    }

    pub(crate) fn activity_rate(&self, t: &Tuning) -> f64 {
        if self.satiety < t.low_stat_threshold || self.mood < t.low_stat_threshold {
            t.low_stat_rate
        } else {
            1.0
        }
    }

    pub(crate) fn grant_xp(&mut self, t: &Tuning, xp: u64) -> ActionResult {
        let before_level = self.level();
        let before_stage = self.stage(t);
        self.xp = self.xp.saturating_add(xp);
        self.day.xp = self.day.xp.saturating_add(xp);
        let carry = self.coin_carry + xp * t.coin_per_xp_permille;
        let mut coins = (carry / 1000) as i64;
        self.coin_carry = carry % 1000;
        let gained_levels = self.level() - before_level;
        coins += gained_levels as i64 * t.level_up_coins;
        self.coins = self.coins.saturating_add(coins);
        ActionResult {
            ok: true,
            xp,
            coins,
            leveled_up: gained_levels > 0,
            stage_changed: self.stage(t) != before_stage,
        }
    }

    /// 기본 사료는 무료·무제한. XP만 하루 횟수 제한이 있다.
    pub fn feed(&mut self, t: &Tuning) -> ActionResult {
        self.satiety = (self.satiety + t.feed_satiety).min(100.0);
        let xp = if self.day.feeds < t.feed_xp_per_day { t.feed_xp } else { 0 };
        self.day.feeds += 1;
        self.grant_xp(t, xp)
    }

    pub fn snack(&mut self, t: &Tuning) -> ActionResult {
        if self.coins < t.snack_price {
            return ActionResult::default();
        }
        self.coins -= t.snack_price;
        self.satiety = (self.satiety + t.snack_satiety).min(100.0);
        self.mood = (self.mood + t.snack_mood).min(100.0);
        let mut r = self.grant_xp(t, 0);
        r.coins -= t.snack_price;
        r
    }

    pub fn stroke(&mut self, t: &Tuning) -> ActionResult {
        self.mood = (self.mood + t.pet_mood).min(100.0);
        self.affinity += 1;
        let xp = if self.day.pets < t.pet_xp_per_day { t.pet_xp } else { 0 };
        self.day.pets += 1;
        self.grant_xp(t, xp)
    }

    /// 타건 "횟수"를 XP로 바꾼다. 많이 칠수록 덜 주는 체감 구조라 과한 사용을 부추기지 않는다.
    pub fn grant_typing(&mut self, t: &Tuning, keys: u64) -> ActionResult {
        // 한 번에 들어올 수 있는 양을 제한한다(비정상 입력 방어).
        let mut left = keys.min(1_000_000);
        let mut pos = self.day.keys;
        let mut units = 0u64;
        for (limit, weight) in [(t.typing_full_keys, 10), (t.typing_half_keys, 5), (u64::MAX, 1)] {
            if pos < limit && left > 0 {
                let n = left.min(limit - pos);
                units += n * weight;
                pos += n;
                left -= n;
            }
        }
        self.day.keys = pos;
        if self.activity_rate(t) < 1.0 {
            units = units * 8 / 10;
        }
        let per_xp = t.typing_keys_per_xp.max(1) * 10;
        let total = self.key_carry + units;
        self.key_carry = total % per_xp;
        self.grant_xp(t, total / per_xp)
    }

    /// `walked_pt`는 커서가 아니라 아롱이가 실제로 이동한 거리.
    pub fn finish_walk(&mut self, t: &Tuning, walked_pt: f64) -> ActionResult {
        let units = if walked_pt.is_finite() { (walked_pt / t.walk_pt_per_unit).max(0.0) } else { 0.0 };
        let mut xp = (units * t.walk_xp_per_unit as f64).min(t.walk_xp_cap_per_walk as f64);
        if xp < 1.0 {
            // 보상이 없는 짧은 끌기는 하루 산책 횟수로 세지 않는다.
            return ActionResult { ok: true, ..ActionResult::default() };
        }
        if self.day.walks >= t.walk_full_per_day {
            xp *= t.walk_extra_rate;
        }
        xp *= self.activity_rate(t);
        self.day.walks += 1;
        self.mood = (self.mood + t.walk_mood).min(100.0);
        self.affinity += 2;
        self.grant_xp(t, xp.floor() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY1: u32 = 2026_11_11;

    fn pet() -> (Pet, Tuning) {
        let t = Tuning::default();
        let mut p = Pet::new(1_000_000);
        p.advance(&t, 1_000_000, DAY1);
        (p, t)
    }

    #[test]
    fn level_curve_matches_design() {
        assert_eq!(level_of(0), (0, 0));
        assert_eq!(level_of(49), (0, 49));
        assert_eq!(level_of(50), (1, 0));
        assert_eq!(level_of(1100).0, 8);
        assert_eq!(level_of(1099).0, 7);
        assert_eq!(level_of(5750).0, 20);
    }

    #[test]
    fn decay_is_capped_and_floored() {
        let (mut p, t) = pet();
        p.satiety = 100.0;
        p.mood = 100.0;
        // 3일 방치해도 12시간분만 감소한다.
        p.advance(&t, 1_000_000 + 3 * 86_400, DAY1 + 3);
        assert_eq!(p.satiety, 52.0);
        assert_eq!(p.mood, 76.0);
        for i in 0..20 {
            p.advance(&t, p.last_wall + 86_400, DAY1 + 4 + i);
        }
        assert_eq!(p.satiety, t.satiety_floor);
        assert_eq!(p.mood, t.mood_floor);
    }

    #[test]
    fn clock_going_backwards_does_not_decay_or_reopen_day() {
        let (mut p, t) = pet();
        for _ in 0..3 {
            p.feed(&t);
        }
        let (satiety, xp) = (p.satiety, p.xp);
        p.advance(&t, 1_000_000 - 86_400, DAY1 - 1);
        assert_eq!(p.satiety, satiety);
        assert_eq!(p.day.date, DAY1);
        // 날짜를 되돌려도 밥 XP 한도가 다시 열리지 않는다.
        assert_eq!(p.feed(&t).xp, 0);
        assert_eq!(p.xp, xp);
    }

    #[test]
    fn day_rollover_hands_back_the_finished_day() {
        let (mut p, t) = pet();
        p.feed(&t);
        p.grant_typing(&t, 250);
        // 같은 날에는 돌려줄 것이 없다.
        assert_eq!(p.advance(&t, p.last_wall + 60, DAY1), None);
        let finished = p.advance(&t, p.last_wall + 60, DAY1 + 1).expect("finished day");
        assert_eq!((finished.date, finished.feeds, finished.keys, finished.xp), (DAY1, 1, 250, 7));
        assert_eq!(p.day, DayCounters { date: DAY1 + 1, ..DayCounters::default() });
        // 시계가 되돌아가도 기록을 다시 만들지 않는다.
        assert_eq!(p.advance(&t, p.last_wall, DAY1), None);
    }

    #[test]
    fn day_rollover_resets_counters() {
        let (mut p, t) = pet();
        for _ in 0..5 {
            p.feed(&t);
        }
        assert_eq!(p.xp, 15);
        p.advance(&t, p.last_wall + 60, DAY1 + 1);
        assert_eq!(p.feed(&t).xp, 5);
    }

    #[test]
    fn snack_needs_coins_and_never_goes_negative() {
        let (mut p, t) = pet();
        assert!(!p.snack(&t).ok);
        p.coins = 7;
        assert!(p.snack(&t).ok);
        assert_eq!(p.coins, 2);
        assert!(!p.snack(&t).ok);
        assert_eq!(p.coins, 2);
    }

    #[test]
    fn coins_carry_fractions() {
        let (mut p, t) = pet();
        // 2 XP × 0.3 = 0.6 재화씩: 10번이면 6.
        for _ in 0..10 {
            p.stroke(&t);
        }
        assert_eq!(p.xp, 20);
        assert_eq!(p.coins, 6);
    }

    #[test]
    fn walk_reward_is_capped_and_diminishes() {
        let (mut p, t) = pet();
        assert_eq!(p.finish_walk(&t, 50.0).xp, 0);
        assert_eq!(p.day.walks, 0);
        assert_eq!(p.finish_walk(&t, 2000.0).xp, 10);
        assert_eq!(p.finish_walk(&t, 1e9).xp, 30);
        assert_eq!(p.finish_walk(&t, f64::NAN).xp, 0);
        assert_eq!(p.finish_walk(&t, 6000.0).xp, 30);
        // 네 번째부터는 20%.
        assert_eq!(p.finish_walk(&t, 6000.0).xp, 6);
    }

    #[test]
    fn typing_reward_diminishes_and_carries() {
        let (mut p, t) = pet();
        assert_eq!(p.grant_typing(&t, 99).xp, 0);
        assert_eq!(p.grant_typing(&t, 1).xp, 1);
        // 5,000타까지 100%: 누적 50 XP.
        p.grant_typing(&t, 4_900);
        assert_eq!(p.xp, 50);
        // 다음 10,000타는 50%: +50 XP.
        p.grant_typing(&t, 10_000);
        assert_eq!(p.xp, 100);
        // 그 뒤는 10%: 10,000타에 +10 XP.
        p.grant_typing(&t, 10_000);
        assert_eq!(p.xp, 110);
        assert_eq!(p.day.keys, 25_000);
        // 다음 날에는 다시 100% 구간부터.
        p.advance(&t, p.last_wall + 60, DAY1 + 1);
        assert_eq!(p.grant_typing(&t, 1_000).xp, 10);
        // 비정상적으로 큰 값은 잘린다.
        p.grant_typing(&t, u64::MAX);
        assert!(p.day.keys <= 1_001_000);
    }

    #[test]
    fn level_up_pays_bonus_and_reports_stage_change() {
        let (mut p, t) = pet();
        p.xp = 1099;
        let r = p.stroke(&t);
        assert!(r.leveled_up && r.stage_changed);
        assert_eq!(p.stage(&t), Stage::Growing);
        assert_eq!(r.coins, 20);
    }
}
