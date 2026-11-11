//! 상점: 꾸미기 아이템과 집 업그레이드. 재화는 현금 결제가 없는 로컬 게임 내 재화다.
//! 가격은 조정 가능한 초기값.

use crate::pet::Pet;

/// 착용 부위. 부위마다 하나씩만 착용한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    Hat,
    Glasses,
    Neck,
    Clothes,
    Shoes,
}

impl Slot {
    pub const ALL: [Slot; 5] = [Slot::Hat, Slot::Glasses, Slot::Neck, Slot::Clothes, Slot::Shoes];

    pub fn name(self) -> &'static str {
        match self {
            Slot::Hat => "모자",
            Slot::Glasses => "안경",
            Slot::Neck => "목 장식",
            Slot::Clothes => "옷",
            Slot::Shoes => "신발",
        }
    }

    /// 저장용 키.
    pub fn key(self) -> &'static str {
        match self {
            Slot::Hat => "hat",
            Slot::Glasses => "glasses",
            Slot::Neck => "neck",
            Slot::Clothes => "clothes",
            Slot::Shoes => "shoes",
        }
    }

    pub fn from_key(key: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|s| s.key() == key)
    }
}

pub struct ItemDef {
    pub id: &'static str,
    pub name: &'static str,
    pub price: i64,
    pub slot: Slot,
}

const fn def(id: &'static str, name: &'static str, price: i64, slot: Slot) -> ItemDef {
    ItemDef { id, name, price, slot }
}

pub const ITEMS: [ItemDef; 16] = [
    def("party", "파티 고깔", 60, Slot::Hat),
    def("beanie", "빨간 비니", 150, Slot::Hat),
    def("straw", "밀짚모자", 200, Slot::Hat),
    def("crown", "왕관", 500, Slot::Hat),
    def("round_glasses", "동글 안경", 120, Slot::Glasses),
    def("sunglasses", "선글라스", 180, Slot::Glasses),
    def("heart_glasses", "하트 안경", 250, Slot::Glasses),
    def("ribbon", "리본", 80, Slot::Neck),
    def("scarf", "목도리", 140, Slot::Neck),
    def("bowtie", "나비넥타이", 160, Slot::Neck),
    def("brown_wear", "갈색 옷", 200, Slot::Clothes),
    def("red_wear", "빨간 옷", 300, Slot::Clothes),
    def("blue_wear", "파란 옷", 300, Slot::Clothes),
    def("green_wear", "초록 옷", 300, Slot::Clothes),
    def("red_boots", "빨간 장화", 180, Slot::Shoes),
    def("yellow_boots", "노란 장화", 180, Slot::Shoes),
];

/// 집 단계 이름. 0은 집 없음.
pub const HOUSE_NAMES: [&str; 4] = ["없음", "방석", "나무 집", "파란 지붕 집"];
/// `HOUSE_PRICES[n]`은 n단계에서 n+1단계로 올리는 가격.
pub const HOUSE_PRICES: [i64; 3] = [500, 1500, 4000];
pub const MAX_HOUSE_LEVEL: u32 = HOUSE_PRICES.len() as u32;

pub fn item(id: &str) -> Option<&'static ItemDef> {
    ITEMS.iter().find(|i| i.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purchase {
    Bought,
    AlreadyOwned,
    NotEnoughCoins,
    UnknownItem,
    MaxLevel,
}

impl Pet {
    /// 재화 차감과 보관함 추가를 한 번에 한다. 이미 가진 아이템은 다시 사지지 않는다.
    pub fn buy(&mut self, id: &str) -> Purchase {
        let Some(def) = item(id) else { return Purchase::UnknownItem };
        if self.owned.contains(def.id) {
            return Purchase::AlreadyOwned;
        }
        if self.coins < def.price {
            return Purchase::NotEnoughCoins;
        }
        self.coins -= def.price;
        self.owned.insert(def.id.to_string());
        Purchase::Bought
    }

    /// 가진 아이템만 착용할 수 있다. 같은 부위의 기존 아이템은 자동으로 벗는다.
    pub fn wear(&mut self, id: &str) -> bool {
        match item(id) {
            Some(def) if self.owned.contains(def.id) => {
                self.worn.insert(def.slot, def.id.to_string());
                true
            }
            _ => false,
        }
    }

    pub fn take_off(&mut self, slot: Slot) {
        self.worn.remove(&slot);
    }

    pub fn wearing(&self, slot: Slot) -> Option<&str> {
        self.worn.get(&slot).map(String::as_str)
    }

    pub fn next_house_price(&self) -> Option<i64> {
        HOUSE_PRICES.get(self.house_level as usize).copied()
    }

    pub fn upgrade_house(&mut self) -> Purchase {
        let Some(price) = self.next_house_price() else { return Purchase::MaxLevel };
        if self.coins < price {
            return Purchase::NotEnoughCoins;
        }
        self.coins -= price;
        self.house_level += 1;
        Purchase::Bought
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_are_unique_and_slot_keys_round_trip() {
        for (i, a) in ITEMS.iter().enumerate() {
            assert!(ITEMS.iter().skip(i + 1).all(|b| b.id != a.id));
        }
        for slot in Slot::ALL {
            assert_eq!(Slot::from_key(slot.key()), Some(slot));
        }
    }

    #[test]
    fn buying_twice_charges_once() {
        let mut p = Pet::new(0);
        p.coins = 1000;
        assert_eq!(p.buy("beanie"), Purchase::Bought);
        assert_eq!(p.buy("beanie"), Purchase::AlreadyOwned);
        assert_eq!(p.coins, 850);
        assert_eq!(p.buy("nope"), Purchase::UnknownItem);
    }

    #[test]
    fn cannot_buy_without_coins_or_wear_unowned() {
        let mut p = Pet::new(0);
        p.coins = 59;
        assert_eq!(p.buy("party"), Purchase::NotEnoughCoins);
        assert_eq!(p.coins, 59);
        assert!(!p.wear("party"));
        p.coins = 60;
        assert_eq!(p.buy("party"), Purchase::Bought);
        assert!(p.wear("party"));
        assert_eq!(p.wearing(Slot::Hat), Some("party"));
        p.take_off(Slot::Hat);
        assert_eq!(p.wearing(Slot::Hat), None);
    }

    #[test]
    fn one_item_per_slot_and_slots_are_independent() {
        let mut p = Pet::new(0);
        p.coins = 10_000;
        for id in ["party", "crown", "sunglasses", "scarf"] {
            assert_eq!(p.buy(id), Purchase::Bought);
            assert!(p.wear(id));
        }
        assert_eq!(p.wearing(Slot::Hat), Some("crown"));
        assert_eq!(p.wearing(Slot::Glasses), Some("sunglasses"));
        assert_eq!(p.wearing(Slot::Neck), Some("scarf"));
        assert_eq!(p.wearing(Slot::Shoes), None);
    }

    #[test]
    fn house_upgrades_in_order_until_max() {
        let mut p = Pet::new(0);
        p.coins = 499;
        assert_eq!(p.upgrade_house(), Purchase::NotEnoughCoins);
        p.coins = 500 + 1500 + 4000;
        for level in 1..=MAX_HOUSE_LEVEL {
            assert_eq!(p.upgrade_house(), Purchase::Bought);
            assert_eq!(p.house_level, level);
        }
        assert_eq!(p.coins, 0);
        assert_eq!(p.upgrade_house(), Purchase::MaxLevel);
    }
}
