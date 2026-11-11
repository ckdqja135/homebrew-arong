//! 로컬 SQLite 저장. 펫 상태 한 행과 설정 key-value만 둔다.
//! 개별 이벤트(클릭·이동 경로 등)는 저장하지 않는다.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use std::collections::{BTreeMap, BTreeSet};

use arong_core::{Command, DayCounters, MAX_HOUSE_LEVEL, Pet, Slot, shop};
use rusqlite::{Connection, OptionalExtension, params};

use crate::otel::Sample;

const SCHEMA_VERSION: i64 = 7;
const DB_NAME: &str = "arong.sqlite";
const BACKUP_NAME: &str = "arong.sqlite.bak";

pub struct Store {
    conn: Connection,
}

/// 열 때 일어난 일. 메뉴에 한 줄로 알려 주기 위한 정보.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpenOutcome {
    Ok,
    RestoredFromBackup,
    ResetAfterCorruption,
}

pub fn data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/dev.cbsong.arong"))
}

fn restrict(path: &Path, mode: u32) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

fn open_checked(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    let check: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if check != "ok" {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > SCHEMA_VERSION {
        // 더 새 버전이 쓴 파일은 건드리지 않는다.
        return Err(rusqlite::Error::InvalidQuery);
    }
    migrate(&conn, version)?;
    Ok(conn)
}

fn migrate(conn: &Connection, from: i64) -> rusqlite::Result<()> {
    if from < 1 {
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS pet (
                 id INTEGER PRIMARY KEY CHECK (id = 1),
                 xp INTEGER NOT NULL, coins INTEGER NOT NULL, coin_carry INTEGER NOT NULL,
                 satiety REAL NOT NULL, mood REAL NOT NULL, affinity INTEGER NOT NULL,
                 last_wall INTEGER NOT NULL,
                 day_date INTEGER NOT NULL, day_feeds INTEGER NOT NULL,
                 day_pets INTEGER NOT NULL, day_walks INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             PRAGMA user_version = 1;
             COMMIT;",
        )?;
    }
    if from < 2 {
        // 상점: 쓰고 있는 모자, 집 단계, 보관함.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE pet ADD COLUMN hat TEXT;
             ALTER TABLE pet ADD COLUMN house_level INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE IF NOT EXISTS inventory (item_id TEXT PRIMARY KEY);
             PRAGMA user_version = 2;
             COMMIT;",
        )?;
    }
    if from < 3 {
        // 타이핑 집계: 오늘의 타건 수와 환산 이월분만 둔다(개별 입력 기록 없음).
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE pet ADD COLUMN day_keys INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN key_carry INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 3;
             COMMIT;",
        )?;
    }
    if from < 4 {
        // AI 사용량: 오늘의 종류별 합계와, 중복 보상을 막기 위한 시계열별 마지막 값만 둔다.
        // ai_series.key는 속성 조합의 해시라 세션 ID·계정 정보 원문은 저장되지 않는다.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE pet ADD COLUMN day_tok_in INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN day_tok_out INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN day_tok_cr INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN day_tok_cc INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN day_ai_units INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE pet ADD COLUMN ai_carry INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE IF NOT EXISTS ai_series (
                 key INTEGER PRIMARY KEY, last_value INTEGER NOT NULL,
                 last_time INTEGER NOT NULL, updated INTEGER NOT NULL
             );
             PRAGMA user_version = 4;
             COMMIT;",
        )?;
    }
    if from < 5 {
        // 착용 부위가 모자 하나에서 여러 개로 늘었다. 기존 모자는 그대로 옮긴다.
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS worn (slot TEXT PRIMARY KEY, item_id TEXT NOT NULL);
             INSERT OR IGNORE INTO worn (slot, item_id) SELECT 'hat', hat FROM pet WHERE hat IS NOT NULL;
             PRAGMA user_version = 5;
             COMMIT;",
        )?;
    }
    if from < 6 {
        // 날짜별 기록: 하루가 끝날 때 그날의 합계 한 줄만 남긴다(개별 입력·요청 기록 없음).
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE pet ADD COLUMN day_xp INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE IF NOT EXISTS daily (
                 date INTEGER PRIMARY KEY, xp INTEGER NOT NULL, keys INTEGER NOT NULL,
                 tok_in INTEGER NOT NULL, tok_out INTEGER NOT NULL, tok_cr INTEGER NOT NULL, tok_cc INTEGER NOT NULL,
                 feeds INTEGER NOT NULL, pets INTEGER NOT NULL, walks INTEGER NOT NULL
             );
             PRAGMA user_version = 6;
             COMMIT;",
        )?;
    }
    if from < 7 {
        // 훈련: 명령별 숙련도와 오늘의 보상 횟수.
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE pet ADD COLUMN day_trains INTEGER NOT NULL DEFAULT 0;
             CREATE TABLE IF NOT EXISTS skills (command TEXT PRIMARY KEY, mastery INTEGER NOT NULL);
             PRAGMA user_version = 7;
             COMMIT;",
        )?;
    }
    Ok(())
}

impl Store {
    pub fn open() -> Option<(Store, OpenOutcome)> {
        let dir = data_dir()?;
        fs::create_dir_all(&dir).ok()?;
        restrict(&dir, 0o700);
        let db = dir.join(DB_NAME);
        let bak = dir.join(BACKUP_NAME);

        let (conn, outcome) = match open_checked(&db) {
            Ok(conn) => (conn, OpenOutcome::Ok),
            Err(_) => {
                // 손상된 파일은 지우지 않고 옆에 보존한다.
                if db.exists() {
                    let _ = fs::rename(&db, dir.join("arong.sqlite.corrupt"));
                }
                if bak.exists() && fs::copy(&bak, &db).is_ok() {
                    match open_checked(&db) {
                        Ok(conn) => (conn, OpenOutcome::RestoredFromBackup),
                        Err(_) => {
                            let _ = fs::remove_file(&db);
                            (open_checked(&db).ok()?, OpenOutcome::ResetAfterCorruption)
                        }
                    }
                } else {
                    (open_checked(&db).ok()?, OpenOutcome::ResetAfterCorruption)
                }
            }
        };
        restrict(&db, 0o600);
        let store = Store { conn };
        if outcome == OpenOutcome::Ok {
            store.backup(&dir);
        }
        Some((store, outcome))
    }

    /// 정상으로 열린 직후에만 백업을 갱신한다(실행당 1회).
    fn backup(&self, dir: &Path) {
        let tmp = dir.join("arong.sqlite.bak.tmp");
        let _ = fs::remove_file(&tmp);
        let Some(tmp_str) = tmp.to_str() else { return };
        if self.conn.execute("VACUUM INTO ?1", params![tmp_str]).is_ok() {
            restrict(&tmp, 0o600);
            let _ = fs::rename(&tmp, dir.join(BACKUP_NAME));
        }
    }

    /// 저장된 값은 믿지 않고 범위를 다시 맞춘다.
    pub fn load_pet(&self) -> Option<Pet> {
        // 카탈로그에 없는 id는 버린다.
        let owned: BTreeSet<String> = self
            .conn
            .prepare("SELECT item_id FROM inventory")
            .and_then(|mut st| st.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|id| shop::item(id).is_some())
            .collect();
        let skills: BTreeMap<Command, u8> = self
            .conn
            .prepare("SELECT command, mastery FROM skills")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?.collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(key, mastery)| Some((Command::from_key(&key)?, mastery.clamp(0, 100) as u8)))
            .collect();
        // 착용 정보도 믿지 않는다: 가진 아이템이고 부위가 맞을 때만 받아들인다.
        let worn: BTreeMap<Slot, String> = self
            .conn
            .prepare("SELECT slot, item_id FROM worn")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(slot, id)| {
                let slot = Slot::from_key(&slot)?;
                (owned.contains(&id) && shop::item(&id)?.slot == slot).then_some((slot, id))
            })
            .collect();
        self.conn
            .query_row(
                "SELECT xp, coins, coin_carry, satiety, mood, affinity, last_wall,
                        day_date, day_feeds, day_pets, day_walks, hat, house_level, day_keys, key_carry,
                        day_tok_in, day_tok_out, day_tok_cr, day_tok_cc, day_ai_units, ai_carry, day_xp, day_trains
                 FROM pet WHERE id = 1",
                [],
                |r| {
                    let stat = |v: f64| if v.is_finite() { v.clamp(0.0, 100.0) } else { 50.0 };
                    Ok(Pet {
                        xp: r.get::<_, i64>(0)?.clamp(0, 10_000_000) as u64,
                        coins: r.get::<_, i64>(1)?.clamp(0, 100_000_000),
                        coin_carry: r.get::<_, i64>(2)?.clamp(0, 999) as u64,
                        satiety: stat(r.get(3)?),
                        mood: stat(r.get(4)?),
                        affinity: r.get::<_, i64>(5)?.max(0) as u64,
                        last_wall: r.get(6)?,
                        day: DayCounters {
                            date: r.get::<_, i64>(7)?.clamp(0, 9999_12_31) as u32,
                            feeds: r.get::<_, i64>(8)?.clamp(0, 1_000_000) as u32,
                            pets: r.get::<_, i64>(9)?.clamp(0, 1_000_000) as u32,
                            walks: r.get::<_, i64>(10)?.clamp(0, 1_000_000) as u32,
                            keys: r.get::<_, i64>(13)?.clamp(0, 100_000_000) as u64,
                            tokens: [
                                r.get::<_, i64>(15)?.max(0) as u64,
                                r.get::<_, i64>(16)?.max(0) as u64,
                                r.get::<_, i64>(17)?.max(0) as u64,
                                r.get::<_, i64>(18)?.max(0) as u64,
                            ],
                            ai_units: r.get::<_, i64>(19)?.max(0) as u64,
                            xp: r.get::<_, i64>(21)?.max(0) as u64,
                            trains: r.get::<_, i64>(22)?.clamp(0, 1_000_000) as u32,
                        },
                        worn: worn.clone(),
                        house_level: r.get::<_, i64>(12)?.clamp(0, MAX_HOUSE_LEVEL as i64) as u32,
                        owned: owned.clone(),
                        skills: skills.clone(),
                        key_carry: r.get::<_, i64>(14)?.clamp(0, 9_999) as u64,
                        ai_carry: r.get::<_, i64>(20)?.clamp(0, 9_999_999) as u64,
                    })
                },
            )
            .optional()
            .ok()
            .flatten()
    }

    fn write_pet(conn: &Connection, p: &Pet) -> rusqlite::Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO pet
             (id, xp, coins, coin_carry, satiety, mood, affinity, last_wall,
              day_date, day_feeds, day_pets, day_walks, hat, house_level, day_keys, key_carry,
              day_tok_in, day_tok_out, day_tok_cr, day_tok_cc, day_ai_units, ai_carry, day_xp, day_trains)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)",
            params![
                p.xp as i64,
                p.coins,
                p.coin_carry as i64,
                p.satiety,
                p.mood,
                p.affinity as i64,
                p.last_wall,
                p.day.date,
                p.day.feeds,
                p.day.pets,
                p.day.walks,
                p.wearing(Slot::Hat),
                p.house_level,
                p.day.keys as i64,
                p.key_carry as i64,
                p.day.tokens[0] as i64,
                p.day.tokens[1] as i64,
                p.day.tokens[2] as i64,
                p.day.tokens[3] as i64,
                p.day.ai_units as i64,
                p.ai_carry as i64,
                p.day.xp as i64,
                p.day.trains
            ],
        )?;
        for id in &p.owned {
            conn.execute("INSERT OR IGNORE INTO inventory (item_id) VALUES (?1)", params![id])?;
        }
        for (command, mastery) in &p.skills {
            conn.execute("INSERT OR REPLACE INTO skills (command, mastery) VALUES (?1, ?2)", params![command.key(), mastery])?;
        }
        conn.execute("DELETE FROM worn", [])?;
        for (slot, id) in &p.worn {
            conn.execute("INSERT INTO worn (slot, item_id) VALUES (?1, ?2)", params![slot.key(), id])?;
        }
        Ok(())
    }

    /// 한 번의 행동 결과(XP·재화·수치·일일 카운터·보관함)를 한 트랜잭션으로 쓴다.
    /// 구매 시 재화 차감과 아이템 추가가 함께 반영되거나 함께 버려진다.
    pub fn save_pet(&self, p: &Pet) -> bool {
        let write = || -> rusqlite::Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            Self::write_pet(&tx, p)?;
            tx.commit()
        };
        write().is_ok()
    }

    /// 사용량 표본을 "아직 보상하지 않은 증가분"으로 바꾸고, 그 보상이 반영된 펫 상태와
    /// 시계열별 마지막 값을 한 트랜잭션으로 저장한다. 그래서 재시작·재전송·중복 수신이 있어도
    /// 같은 토큰이 두 번 보상되지 않는다. `apply`는 증가분을 받아 보상을 반영한 펫을 돌려준다.
    pub fn commit_ai(&self, samples: &[Sample], now: i64, apply: impl FnOnce([u64; 4]) -> Pet) -> bool {
        let run = || -> rusqlite::Result<()> {
            let tx = self.conn.unchecked_transaction()?;
            let mut deltas = [0u64; 4];
            for s in samples.iter().filter(|s| s.kind < 4) {
                let (last_value, last_time): (i64, i64) = tx
                    .query_row("SELECT last_value, last_time FROM ai_series WHERE key = ?1", params![s.key], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?
                    .unwrap_or((0, 0));
                // 이미 본 시각이거나 더 오래된 보고는 재전송으로 보고 버린다.
                if s.time <= last_time {
                    continue;
                }
                let value = s.value.min(i64::MAX as u64) as i64;
                let (delta, new_value) = if s.cumulative {
                    // 누적값: 지난번보다 늘어난 만큼만. 줄어들었으면 보상 없이 기준만 낮춘다.
                    ((value - last_value).max(0), value)
                } else {
                    (value, last_value)
                };
                tx.execute(
                    "INSERT OR REPLACE INTO ai_series (key, last_value, last_time, updated) VALUES (?1, ?2, ?3, ?4)",
                    params![s.key, new_value, s.time, now],
                )?;
                deltas[s.kind] = deltas[s.kind].saturating_add(delta as u64);
            }
            // 끝난 세션의 기록은 일주일 뒤 지운다.
            tx.execute("DELETE FROM ai_series WHERE updated < ?1", params![now - 7 * 86_400])?;
            let pet = apply(deltas);
            Self::write_pet(&tx, &pet)?;
            tx.commit()
        };
        run().is_ok()
    }

    /// 끝난 하루의 합계를 일별 기록에 남기고, 최근 90일만 유지한다.
    pub fn archive_day(&self, d: &DayCounters) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO daily (date, xp, keys, tok_in, tok_out, tok_cr, tok_cc, feeds, pets, walks)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                d.date,
                d.xp as i64,
                d.keys as i64,
                d.tokens[0] as i64,
                d.tokens[1] as i64,
                d.tokens[2] as i64,
                d.tokens[3] as i64,
                d.feeds,
                d.pets,
                d.walks
            ],
        );
        let _ = self.conn.execute(
            "DELETE FROM daily WHERE date NOT IN (SELECT date FROM daily ORDER BY date DESC LIMIT 90)",
            [],
        );
    }

    /// 최근 일별 기록(최신순).
    pub fn recent_days(&self, limit: u32) -> Vec<DayCounters> {
        self.conn
            .prepare("SELECT date, xp, keys, tok_in, tok_out, tok_cr, tok_cc, feeds, pets, walks FROM daily ORDER BY date DESC LIMIT ?1")
            .and_then(|mut st| {
                st.query_map(params![limit], |r| {
                    let n = |i: usize| -> rusqlite::Result<u64> { Ok(r.get::<_, i64>(i)?.max(0) as u64) };
                    Ok(DayCounters {
                        date: n(0)? as u32,
                        xp: n(1)?,
                        keys: n(2)?,
                        tokens: [n(3)?, n(4)?, n(5)?, n(6)?],
                        feeds: n(7)? as u32,
                        pets: n(8)? as u32,
                        walks: n(9)? as u32,
                        ai_units: 0,
                        trains: 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })
            .unwrap_or_default()
    }

    /// 활동 집계 기록(일별 기록과 AI 시계열 체크포인트)만 지운다. 육성 상태는 건드리지 않는다.
    pub fn delete_activity(&self) {
        let _ = self.conn.execute_batch("BEGIN; DELETE FROM daily; DELETE FROM ai_series; COMMIT;");
    }

    /// AI 사용량 관련 기록(시계열 체크포인트)을 지운다.
    pub fn delete_ai_series(&self) {
        let _ = self.conn.execute("DELETE FROM ai_series", []);
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        );
    }

    pub fn setting_f64(&self, key: &str) -> Option<f64> {
        self.setting(key)?.parse::<f64>().ok().filter(|v| v.is_finite())
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        match self.setting(key).as_deref() {
            Some("1") => true,
            Some("0") => false,
            _ => default,
        }
    }

    /// 육성 데이터 초기화. 설정(위치·크기)은 남긴다.
    pub fn delete_pet(&self) {
        let _ = self.conn.execute_batch("BEGIN; DELETE FROM pet; DELETE FROM inventory; DELETE FROM worn; DELETE FROM skills; DELETE FROM ai_series; DELETE FROM daily; COMMIT;");
    }
}
