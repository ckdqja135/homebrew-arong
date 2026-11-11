//! AI 도구 사용량(토큰 수) → 경험치. 도구마다 다른 형식은 앱 쪽 어댑터가
//! 아래 네 종류의 "증가분"으로 바꿔서 넘긴다. 대화 내용은 여기까지 오지 않는다.

use crate::pet::{ActionResult, Pet, Tuning};

/// 토큰 종류. 서로 겹치지 않는다(입력에는 캐시 읽기·쓰기가 포함되지 않는다).
pub const INPUT: usize = 0;
pub const OUTPUT: usize = 1;
pub const CACHE_READ: usize = 2;
pub const CACHE_CREATION: usize = 3;
pub const KINDS: usize = 4;

/// 토큰 1개당 활동 포인트 × 100. 캐시 읽기는 양이 수백 배라 아주 낮게 친다.
const WEIGHT_X100: [u64; KINDS] = [25, 100, 1, 25];
/// 한 번에 반영하는 종류별 상한(비정상 입력 방어).
const MAX_PER_GRANT: u64 = 50_000_000;

impl Pet {
    /// 실제로 측정된 토큰 증가분만 넘겨야 한다(추정값 금지).
    pub fn grant_ai(&mut self, t: &Tuning, tokens: [u64; KINDS]) -> ActionResult {
        let mut left = 0u64;
        for kind in 0..KINDS {
            let n = tokens[kind].min(MAX_PER_GRANT);
            self.day.tokens[kind] = self.day.tokens[kind].saturating_add(n);
            left += n * WEIGHT_X100[kind];
        }
        // 하루 포인트가 많아질수록 덜 준다: AI를 많이 써야만 크는 구조를 피한다.
        let mut pos = self.day.ai_units;
        let mut eff = 0u64;
        for (limit, weight) in [(t.ai_full_points * 100, 10), (t.ai_half_points * 100, 5), (u64::MAX, 1)] {
            if pos < limit && left > 0 {
                let n = left.min(limit - pos);
                eff += n * weight;
                pos += n;
                left -= n;
            }
        }
        self.day.ai_units = pos;
        if self.activity_rate(t) < 1.0 {
            eff = eff * 8 / 10;
        }
        let per_xp = t.ai_points_per_xp.max(1) * 1000;
        let total = self.ai_carry + eff;
        self.ai_carry = total % per_xp;
        self.grant_xp(t, total / per_xp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pet() -> (Pet, Tuning) {
        let t = Tuning::default();
        let mut p = Pet::new(1_000);
        p.advance(&t, 1_000, 2026_11_11);
        (p, t)
    }

    #[test]
    fn kinds_are_weighted_separately() {
        let (mut p, t) = pet();
        // 출력 1,000 = 1,000포인트 = 1 XP.
        assert_eq!(p.grant_ai(&t, [0, 1_000, 0, 0]).xp, 1);
        // 입력 4,000 × 0.25 = 1,000포인트.
        assert_eq!(p.grant_ai(&t, [4_000, 0, 0, 0]).xp, 1);
        // 캐시 읽기 100,000 × 0.01 = 1,000포인트.
        assert_eq!(p.grant_ai(&t, [0, 0, 100_000, 0]).xp, 1);
        assert_eq!(p.day.tokens, [4_000, 1_000, 100_000, 0]);
    }

    #[test]
    fn daily_reward_diminishes() {
        let (mut p, t) = pet();
        // 10만 포인트까지 100%.
        assert_eq!(p.grant_ai(&t, [0, 100_000, 0, 0]).xp, 100);
        // 다음 20만 포인트는 50%.
        assert_eq!(p.grant_ai(&t, [0, 200_000, 0, 0]).xp, 100);
        // 그 뒤는 10%.
        assert_eq!(p.grant_ai(&t, [0, 100_000, 0, 0]).xp, 10);
    }

    #[test]
    fn small_amounts_carry_over() {
        let (mut p, t) = pet();
        for _ in 0..9 {
            assert_eq!(p.grant_ai(&t, [0, 100, 0, 0]).xp, 0);
        }
        assert_eq!(p.grant_ai(&t, [0, 100, 0, 0]).xp, 1);
    }

    #[test]
    fn absurd_values_are_capped() {
        let (mut p, t) = pet();
        p.grant_ai(&t, [u64::MAX; KINDS]);
        assert!(p.day.tokens.iter().all(|&n| n == MAX_PER_GRANT));
    }
}
