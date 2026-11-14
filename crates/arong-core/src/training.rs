//! 훈련: 친해지고 자라면 명령이 하나씩 열리고, 반복할수록 잘 따라 한다.
//! 수치는 조정 가능한 초기값이다.

use crate::pet::{ActionResult, Pet, Tuning};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Command {
    Sit,
    PawLeft,
    PawRight,
    LieDown,
    StandUp,
}

impl Command {
    pub const ALL: [Command; 5] = [Command::Sit, Command::PawLeft, Command::PawRight, Command::LieDown, Command::StandUp];

    /// 사용자가 부르는 말.
    pub fn name(self) -> &'static str {
        match self {
            Command::Sit => "앉아",
            Command::PawLeft => "왼손",
            Command::PawRight => "오른손",
            Command::LieDown => "엎드려",
            Command::StandUp => "일어서",
        }
    }

    /// 저장용 키.
    pub fn key(self) -> &'static str {
        match self {
            Command::Sit => "sit",
            Command::PawLeft => "paw_left",
            Command::PawRight => "paw_right",
            Command::LieDown => "lie_down",
            Command::StandUp => "stand_up",
        }
    }

    pub fn from_key(key: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.key() == key)
    }

    /// 이 명령을 배울 수 있는 최소 (레벨, 친밀도).
    pub fn requirement(self) -> (u32, u64) {
        match self {
            Command::Sit => (3, 20),
            // 앉아를 배우면 일어서도 함께 배운다.
            Command::StandUp => (3, 20),
            Command::PawLeft => (5, 50),
            Command::PawRight => (7, 90),
            Command::LieDown => (10, 150),
        }
    }

    /// 음성 인식 결과에서 명령을 찾는다. 띄어쓰기·문장부호는 무시하고, 흔한 오인식 표기도 받아 준다.
    /// 인식된 문장 자체는 어디에도 남기지 않고, 어느 명령인지만 돌려준다.
    pub fn from_speech(text: &str) -> Option<Command> {
        let said: String = text.chars().filter(|c| c.is_alphanumeric()).collect();
        // 더 긴 표현부터 본다("오른손" 안에 다른 명령이 들어 있지 않게).
        let table: [(&[&str], Command); 5] = [
            (&["오른손", "오른발", "오른쪽손", "오른쪽"], Command::PawRight),
            (&["왼손", "왼발", "왼쪽손", "왼쪽", "웬손", "인손"], Command::PawLeft),
            (&["엎드려", "업드려", "엎드려라", "엎드리", "업드리"], Command::LieDown),
            (&["일어서", "일어나", "이러서", "일어섯"], Command::StandUp),
            (&["앉아", "안자", "앉어", "안저", "앉자"], Command::Sit),
        ];
        // 한 문장에 여러 명령이 섞여 있으면 가장 마지막에 말한 것을 따른다.
        table
            .iter()
            .filter_map(|(words, command)| words.iter().filter_map(|w| said.rfind(w)).max().map(|at| (at, *command)))
            .max_by_key(|(at, _)| *at)
            .map(|(_, command)| command)
    }
}

/// 음성에서 알아들은 것: 훈련 명령이거나, 이름을 부른 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
    /// 명령만 말했다.
    Command(Command),
    /// 이름을 부르면서 명령했다("아롱아 앉아").
    Called(Command),
    /// "아롱아" 하고 부르기만 했다.
    Name,
}

impl Heard {
    /// 인식된 문장 자체는 어디에도 남기지 않는다.
    pub fn from_speech(text: &str) -> Option<Heard> {
        let command = Command::from_speech(text);
        let said: String = text.chars().filter(|c| c.is_alphanumeric()).collect();
        // 흔한 오인식 표기도 받아 준다.
        let names = ["아롱", "아룡", "아농", "알옹", "아론", "아롬", "아랑아", "아렁아", "어롱", "아롱", "야롱", "아옹아", "아놓아"];
        let named = names.iter().any(|name| said.contains(name));
        match (command, named) {
            (Some(command), true) => Some(Heard::Called(command)),
            (Some(command), false) => Some(Heard::Command(command)),
            (None, true) => Some(Heard::Name),
            (None, false) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrainResult {
    pub success: bool,
    /// 훈련 뒤의 숙련도(0~100).
    pub mastery: u8,
    pub reward: ActionResult,
}

impl Pet {
    /// 아직 못 배우는 명령이면 필요한 (레벨, 친밀도)를 돌려준다.
    pub fn locked(&self, command: Command) -> Option<(u32, u64)> {
        let (level, affinity) = command.requirement();
        (self.level() < level || self.affinity < affinity).then_some((level, affinity))
    }

    pub fn mastery(&self, command: Command) -> u8 {
        self.skills.get(&command).copied().unwrap_or(0)
    }

    /// 훈련 한 번. `roll`은 0~99의 난수(호출자가 넣는다). 숙련도가 높을수록 성공한다.
    /// 성공 확률 = 40% + 숙련도 × 0.6. 성공하면 숙련도가 많이, 실패해도 조금 오른다.
    pub fn train(&mut self, t: &Tuning, command: Command, roll: u8) -> Option<TrainResult> {
        if self.locked(command).is_some() {
            return None;
        }
        let mastery = self.mastery(command);
        let chance = 40 + mastery as u32 * 60 / 100;
        let success = (roll as u32) < chance;
        let gained = if success { t.train_mastery_success } else { t.train_mastery_fail };
        let mastery = mastery.saturating_add(gained).min(100);
        self.skills.insert(command, mastery);
        // 보상은 성공했을 때만, 하루 정해진 횟수까지만 준다.
        let xp = if success && self.day.trains < t.train_xp_per_day { t.train_xp } else { 0 };
        if success {
            self.day.trains += 1;
            self.mood = (self.mood + t.train_mood).min(100.0);
        }
        Some(TrainResult { success, mastery, reward: self.grant_xp(t, xp) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> (Pet, Tuning) {
        let t = Tuning::default();
        let mut p = Pet::new(0);
        p.advance(&t, 0, 2026_11_11);
        p.xp = 100_000;
        p.affinity = 1_000;
        (p, t)
    }

    #[test]
    fn commands_unlock_by_level_and_affinity() {
        let t = Tuning::default();
        let mut p = Pet::new(0);
        assert_eq!(p.locked(Command::Sit), Some((3, 20)));
        assert!(p.train(&t, Command::Sit, 0).is_none());
        p.xp = crate::pet::xp_to_next(0) + crate::pet::xp_to_next(1) + crate::pet::xp_to_next(2);
        assert_eq!(p.level(), 3);
        // 레벨만 되고 친밀도가 모자라면 아직 잠겨 있다.
        assert!(p.locked(Command::Sit).is_some());
        p.affinity = 20;
        assert_eq!(p.locked(Command::Sit), None);
        assert_eq!(p.locked(Command::StandUp), None);
        assert_eq!(p.locked(Command::PawLeft), Some((5, 50)));
        assert_eq!(p.locked(Command::LieDown), Some((10, 150)));
    }

    #[test]
    fn mastery_raises_success_chance_and_caps_at_100() {
        let (mut p, t) = ready();
        // 숙련도 0: 40% 미만의 주사위만 성공.
        assert!(p.train(&t, Command::Sit, 39).unwrap().success);
        let (mut q, _) = ready();
        assert!(!q.train(&t, Command::Sit, 40).unwrap().success);
        // 실패해도 조금은 는다.
        assert_eq!(q.mastery(Command::Sit), t.train_mastery_fail);
        for _ in 0..100 {
            p.train(&t, Command::Sit, 0);
        }
        assert_eq!(p.mastery(Command::Sit), 100);
        // 숙련도 100이면 항상 성공.
        assert!(p.train(&t, Command::Sit, 99).unwrap().success);
        // 다른 명령의 숙련도는 따로 쌓인다.
        assert_eq!(p.mastery(Command::PawLeft), 0);
    }

    #[test]
    fn training_xp_is_limited_per_day() {
        let (mut p, t) = ready();
        let before = p.xp;
        for _ in 0..(t.train_xp_per_day + 10) {
            p.train(&t, Command::Sit, 0);
        }
        assert_eq!(p.xp - before, t.train_xp * t.train_xp_per_day as u64);
        // 실패는 보상도 횟수도 없다.
        let (mut q, _) = ready();
        let r = q.train(&t, Command::Sit, 99).unwrap();
        assert_eq!((r.success, r.reward.xp, q.day.trains), (false, 0, 0));
    }

    #[test]
    fn speech_is_matched_loosely_but_only_to_known_commands() {
        assert_eq!(Command::from_speech("앉아!"), Some(Command::Sit));
        assert_eq!(Command::from_speech("아롱아 앉아"), Some(Command::Sit));
        assert_eq!(Command::from_speech("왼 손"), Some(Command::PawLeft));
        assert_eq!(Command::from_speech("오른손!"), Some(Command::PawRight));
        assert_eq!(Command::from_speech("업드려"), Some(Command::LieDown));
        assert_eq!(Command::from_speech("일어나"), Some(Command::StandUp));
        // 여러 개를 말하면 마지막 것.
        assert_eq!(Command::from_speech("앉아 아니 엎드려"), Some(Command::LieDown));
        assert_eq!(Command::from_speech("안녕 오늘 날씨 좋다"), None);
        assert_eq!(Command::from_speech(""), None);
        // 이름만 부르면 이름, 명령이 같이 있으면 명령.
        assert_eq!(Heard::from_speech("아롱아~"), Some(Heard::Name));
        assert_eq!(Heard::from_speech("아롱이!"), Some(Heard::Name));
        assert_eq!(Heard::from_speech("아론아"), Some(Heard::Name));
        assert_eq!(Heard::from_speech("아랑아"), Some(Heard::Name));
        assert_eq!(Heard::from_speech("아롱아 앉아"), Some(Heard::Called(Command::Sit)));
        assert_eq!(Heard::from_speech("앉아"), Some(Heard::Command(Command::Sit)));
        assert_eq!(Heard::from_speech("오늘 날씨 좋다"), None);
        for command in Command::ALL {
            assert_eq!(Command::from_speech(command.name()), Some(command));
            assert_eq!(Command::from_key(command.key()), Some(command));
        }
    }
}
