//! 육성 데이터 내보내기·가져오기(JSON).
//!
//! 내보내는 것은 펫의 진행 상태뿐이다: 경험치, 재화, 수치, 집 단계, 가진 아이템, 착용 정보.
//! 타이핑·AI 사용량 수치, 수신 토큰, 창 위치 같은 설정은 넣지 않는다.
//! 가져올 때는 파일을 믿지 않는다: 크기·형식·버전·값 범위를 검사하고, 파일 안의 어떤 것도 실행하지 않는다.

use std::collections::{BTreeMap, BTreeSet};

use arong_core::{MAX_HOUSE_LEVEL, Pet, Slot, shop};
use serde_json::{Value, json};

const FORMAT: &str = "arong-save";
const VERSION: i64 = 1;
/// 가져오기 파일 크기 상한. 정상 파일은 1KB 안팎이다.
pub const MAX_IMPORT_BYTES: u64 = 64 * 1024;

pub fn export_json(p: &Pet) -> String {
    let worn: BTreeMap<&str, &str> = p.worn.iter().map(|(slot, id)| (slot.key(), id.as_str())).collect();
    let value = json!({
        "format": FORMAT,
        "version": VERSION,
        "pet": {
            "xp": p.xp,
            "coins": p.coins,
            "satiety": p.satiety.round(),
            "mood": p.mood.round(),
            "affinity": p.affinity,
            "house_level": p.house_level,
            "owned": p.owned,
            "worn": worn,
        },
    });
    serde_json::to_string_pretty(&value).unwrap_or_default()
}

/// 검증을 통과하면 새 펫 상태를 돌려준다. 실패하면 사용자에게 보여 줄 이유를 돌려준다.
pub fn import_json(bytes: &[u8], now_wall: i64) -> Result<Pet, &'static str> {
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        return Err("파일이 너무 큽니다");
    }
    let root: Value = serde_json::from_slice(bytes).map_err(|_| "아롱이 저장 파일이 아닙니다")?;
    if root.get("format").and_then(Value::as_str) != Some(FORMAT) {
        return Err("아롱이 저장 파일이 아닙니다");
    }
    match root.get("version").and_then(Value::as_i64) {
        Some(VERSION) => {}
        Some(v) if v > VERSION => return Err("더 새 버전에서 만든 파일입니다"),
        _ => return Err("지원하지 않는 버전의 파일입니다"),
    }
    let pet = root.get("pet").ok_or("파일 내용이 올바르지 않습니다")?;
    let int = |key: &str, max: i64| -> Result<i64, &'static str> {
        match pet.get(key).and_then(Value::as_i64) {
            Some(v) if (0..=max).contains(&v) => Ok(v),
            _ => Err("값이 허용 범위를 벗어났습니다"),
        }
    };
    let stat = |key: &str| -> Result<f64, &'static str> {
        match pet.get(key).and_then(Value::as_f64) {
            Some(v) if (0.0..=100.0).contains(&v) => Ok(v),
            _ => Err("값이 허용 범위를 벗어났습니다"),
        }
    };
    let mut out = Pet::new(now_wall);
    out.xp = int("xp", 10_000_000)? as u64;
    out.coins = int("coins", 100_000_000)?;
    out.satiety = stat("satiety")?;
    out.mood = stat("mood")?;
    out.affinity = int("affinity", 1_000_000_000)? as u64;
    out.house_level = int("house_level", MAX_HOUSE_LEVEL as i64)? as u32;

    let owned = pet.get("owned").and_then(Value::as_array).ok_or("파일 내용이 올바르지 않습니다")?;
    if owned.len() > shop::ITEMS.len() {
        return Err("파일 내용이 올바르지 않습니다");
    }
    let mut set = BTreeSet::new();
    for id in owned {
        // 카탈로그에 없는 아이템은 받지 않는다.
        let def = id.as_str().and_then(shop::item).ok_or("알 수 없는 아이템이 들어 있습니다")?;
        set.insert(def.id.to_string());
    }
    out.owned = set;

    let worn = pet.get("worn").and_then(Value::as_object).ok_or("파일 내용이 올바르지 않습니다")?;
    for (slot, id) in worn {
        let slot = Slot::from_key(slot).ok_or("파일 내용이 올바르지 않습니다")?;
        let def = id.as_str().and_then(shop::item).ok_or("알 수 없는 아이템이 들어 있습니다")?;
        // 가지고 있고 부위가 맞는 것만 착용할 수 있다.
        if def.slot != slot || !out.wear(def.id) {
            return Err("착용 정보가 올바르지 않습니다");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Pet {
        let mut p = Pet::new(100);
        p.xp = 1234;
        p.coins = 10_000;
        p.house_level = 2;
        p.buy("crown");
        p.buy("scarf");
        p.wear("crown");
        p.wear("scarf");
        p.day.keys = 4321;
        p.day.tokens = [1, 2, 3, 4];
        p
    }

    #[test]
    fn round_trips_progress_but_not_activity_counts() {
        let p = sample();
        let text = export_json(&p);
        assert!(!text.contains("4321") && !text.contains("tokens") && !text.contains("keys"));
        let back = import_json(text.as_bytes(), 999).unwrap();
        assert_eq!((back.xp, back.coins, back.house_level), (p.xp, p.coins, p.house_level));
        assert_eq!(back.owned, p.owned);
        assert_eq!(back.worn, p.worn);
        assert_eq!(back.day.keys, 0);
        assert_eq!(back.last_wall, 999);
    }

    #[test]
    fn rejects_wrong_format_version_and_size() {
        assert!(import_json(b"hello", 0).is_err());
        assert!(import_json(br#"{"format":"other","version":1,"pet":{}}"#, 0).is_err());
        let newer = export_json(&sample()).replace("\"version\": 1", "\"version\": 2");
        assert_eq!(import_json(newer.as_bytes(), 0), Err("더 새 버전에서 만든 파일입니다"));
        assert!(import_json(&vec![b' '; MAX_IMPORT_BYTES as usize + 1], 0).is_err());
    }

    #[test]
    fn rejects_out_of_range_values_and_unknown_or_unowned_items() {
        let good = export_json(&sample());
        for (from, to) in [
            ("\"coins\": 9360", "\"coins\": -5"),
            ("\"xp\": 1234", "\"xp\": 99999999999"),
            ("\"house_level\": 2", "\"house_level\": 9"),
            ("\"mood\": 70.0", "\"mood\": 1e9"),
            ("\"scarf\"\n", "\"rm -rf\"\n"),
            ("\"hat\": \"crown\"", "\"hat\": \"party\""),
            ("\"hat\": \"crown\"", "\"hat\": \"scarf\""),
        ] {
            let bad = good.replacen(from, to, 1);
            assert_ne!(bad, good, "fixture should contain {from}");
            assert!(import_json(bad.as_bytes(), 0).is_err(), "{to} should be rejected");
        }
    }
}
