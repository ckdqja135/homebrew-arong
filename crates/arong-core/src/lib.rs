//! 아롱이 육성·경제·산책 규칙. UI·FFI·시계에 의존하지 않는 순수 로직.
//! 시간은 전부 호출자가 주입한다(벽시계 초, 로컬 날짜, 경과 dt).

pub mod ai;
pub mod pet;
pub mod shop;
pub mod training;
pub mod walk;

pub use training::{Command, Heard, TrainResult};
pub use shop::{HOUSE_NAMES, ITEMS, MAX_HOUSE_LEVEL, Purchase, Slot};
pub use pet::{ActionResult, DayCounters, Pet, Stage, Tuning};
pub use walk::{Follower, Vec2, WalkMeter, WalkTuning};
