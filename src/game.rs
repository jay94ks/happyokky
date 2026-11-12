use std::sync::Arc;

use crate::CAT_NAME;

const MINUTE: u64 = 60_000;
const DOZE_AFTER: u64 = 5 * MINUTE;
const SLEEP_AFTER: u64 = 6 * MINUTE;
pub const WAKE_CLICKS: u8 = 3;
pub const NAME_LIMIT: usize = 24;

pub fn valid_name(input: &str) -> Option<&str> {
    let name = input.trim();
    (!name.is_empty() && !name.chars().any(char::is_control) && name.chars().count() <= NAME_LIMIT)
        .then_some(name)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Awake,
    Dozing,
    Asleep,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Care {
    Feed,
    Pet,
    Play,
}

impl Care {
    pub const ALL: [Self; 3] = [Self::Feed, Self::Pet, Self::Play];

    fn slot(self) -> usize {
        match self {
            Self::Feed => 0,
            Self::Pet => 1,
            Self::Play => 2,
        }
    }

    fn cooldown(self) -> u64 {
        match self {
            Self::Feed => 60_000,
            Self::Pet => 10_000,
            Self::Play => 30_000,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Feed => "밥 주기",
            Self::Pet => "쓰다듬기",
            Self::Play => "공 던지기",
        }
    }

    pub fn feedback(self) -> &'static str {
        match self {
            Self::Feed => "냠냠, 잘 먹었어요!",
            Self::Pet => "기분 좋게 골골거려요.",
            Self::Play => "데굴데굴 굴러가는 공을 쫓아요!",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stats {
    fullness: u16,
    mood: u16,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            fullness: 820,
            mood: 860,
        }
    }
}

impl Stats {
    pub fn percentages(self) -> [u16; 2] {
        [self.fullness, self.mood].map(|value| (value + 5) / 10)
    }

    pub fn care(&mut self, care: Care) {
        match care {
            Care::Feed => {
                self.fullness = (self.fullness + 280).min(1000);
                self.mood = (self.mood + 35).min(1000);
            }
            Care::Pet => self.mood = (self.mood + 180).min(1000),
            Care::Play => {
                self.mood = (self.mood + 240).min(1000);
                self.fullness = self.fullness.saturating_sub(30);
            }
        }
    }

    fn elapse_minute(&mut self, mode: Mode) {
        self.fullness = self.fullness.saturating_sub(5);
        if mode != Mode::Asleep {
            self.mood = self.mood.saturating_sub(4);
        }
    }

    pub fn encode(self) -> String {
        format!("2 {} {}\n", self.fullness, self.mood)
    }

    pub fn decode(data: &str) -> Option<Self> {
        let mut fields = data.split_whitespace();
        let version = fields.next()?;
        let fullness = fields.next()?.parse().ok()?;
        let mood = fields.next()?.parse().ok()?;
        match version {
            "1" => {
                fields.next()?;
            }
            "2" | "3" => {}
            _ => return None,
        }
        (fields.next().is_none() && fullness <= 1000 && mood <= 1000)
            .then_some(Self { fullness, mood })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedPet {
    pub stats: Stats,
    pub name: Arc<str>,
}

impl Default for SavedPet {
    fn default() -> Self {
        Self {
            stats: Stats::default(),
            name: CAT_NAME.into(),
        }
    }
}

impl SavedPet {
    pub fn encode(&self) -> String {
        if self.name.as_ref() == CAT_NAME {
            return self.stats.encode();
        }
        format!(
            "3 {} {}\n{}\n",
            self.stats.fullness, self.stats.mood, self.name
        )
    }

    pub fn decode(data: &str) -> Option<Self> {
        if data.starts_with("3 ") {
            let (values, name) = data.split_once('\n')?;
            return Some(Self {
                stats: Stats::decode(values)?,
                name: valid_name(name).unwrap_or(CAT_NAME).into(),
            });
        }
        Some(Self {
            stats: Stats::decode(data)?,
            ..Self::default()
        })
    }
}

pub struct Game {
    pub stats: Stats,
    mode: Mode,
    wake_clicks: u8,
    care_ready_at: [u64; 3],
    previous_tick: u64,
    last_minute: u64,
    activity_hunger: f32,
}

impl Game {
    pub fn new(now: u64, stats: Stats) -> Self {
        Self {
            stats,
            mode: Mode::Awake,
            wake_clicks: 0,
            care_ready_at: [0; 3],
            previous_tick: now,
            last_minute: now,
            activity_hunger: 0.0,
        }
    }

    pub fn tick(&mut self, now: u64, idle: u64) {
        if now - self.previous_tick >= DOZE_AFTER {
            self.last_minute = now;
        }
        self.previous_tick = now;

        self.mode = if self.mode == Mode::Asleep || idle >= SLEEP_AFTER {
            Mode::Asleep
        } else if idle >= DOZE_AFTER {
            Mode::Dozing
        } else {
            Mode::Awake
        };

        while now - self.last_minute >= MINUTE {
            self.stats.elapse_minute(self.mode);
            self.last_minute += MINUTE;
        }
    }

    pub fn spend_fullness_on_activity(&mut self, amount: f32) {
        self.activity_hunger += amount;
        let units = self.activity_hunger.floor() as u16;
        self.stats.fullness = self.stats.fullness.saturating_sub(units);
        self.activity_hunger = if self.stats.fullness == 0 {
            0.0
        } else {
            self.activity_hunger - units as f32
        };
    }

    pub fn walked_sideways(&mut self, pixels: f32) {
        self.spend_fullness_on_activity(pixels / 100.0);
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn wake_clicks(&self) -> u8 {
        self.wake_clicks
    }

    pub fn sleep(&mut self) {
        if self.mode != Mode::Asleep {
            self.mode = Mode::Asleep;
            self.wake_clicks = 0;
        }
    }

    pub fn can_care(&self) -> bool {
        self.mode != Mode::Asleep
    }

    pub fn cooldown_seconds(&self, now: u64) -> [u64; 3] {
        self.care_ready_at
            .map(|time| time.saturating_sub(now).div_ceil(1000))
    }

    pub fn care(&mut self, care: Care, now: u64) -> bool {
        if !self.can_care() || now < self.care_ready_at[care.slot()] {
            return false;
        }
        self.stats.care(care);
        self.care_ready_at[care.slot()] = now + care.cooldown();
        true
    }

    pub fn click_cat(&mut self) -> bool {
        if self.mode != Mode::Asleep {
            return true;
        }
        self.wake_clicks += 1;
        if self.wake_clicks < WAKE_CLICKS {
            return false;
        }
        self.wake_clicks = 0;
        self.mode = Mode::Awake;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_unicode_name_round_trips_and_legacy_saves_use_the_default() {
        let legacy = SavedPet::decode("1 710 600 860\n").unwrap();
        assert_eq!(legacy.name.as_ref(), CAT_NAME);
        assert_eq!(legacy.encode(), "2 710 600\n");
        let renamed = SavedPet {
            name: "밤톨 고양이".into(),
            ..legacy
        };
        assert_eq!(SavedPet::decode(&renamed.encode()), Some(renamed));
        assert_eq!(valid_name("  밤톨  "), Some("밤톨"));
        assert_eq!(valid_name("  "), None);
        assert_eq!(valid_name("밤톨\n옥키"), None);
        assert_eq!(valid_name(&"냥".repeat(NAME_LIMIT + 1)), None);
    }

    #[test]
    fn continued_input_keeps_okky_awake() {
        let mut game = Game::new(0, Stats::default());
        for minute in 0..=120 {
            game.tick(minute * MINUTE, 0);
            assert_eq!(game.mode, Mode::Awake);
        }
    }

    #[test]
    fn dozes_sleeps_and_requires_three_cat_clicks() {
        let mut game = Game::new(0, Stats::default());
        game.tick(DOZE_AFTER - 1, DOZE_AFTER - 1);
        assert_eq!(game.mode, Mode::Awake);
        game.tick(DOZE_AFTER, DOZE_AFTER);
        assert_eq!(game.mode, Mode::Dozing);
        game.tick(SLEEP_AFTER, SLEEP_AFTER);
        assert_eq!(game.mode, Mode::Asleep);
        game.tick(SLEEP_AFTER + 1, 0);
        assert_eq!(game.mode, Mode::Asleep);
        assert!(!game.click_cat());
        assert!(!game.click_cat());
        assert_eq!(game.wake_clicks, 2);
        assert!(game.click_cat());
        assert_eq!(game.mode, Mode::Awake);
        assert_eq!(game.wake_clicks, 0);
    }

    #[test]
    fn a_long_timer_gap_does_not_drain_fullness() {
        let mut game = Game::new(0, Stats::default());
        for minute in 0..60 {
            game.tick(minute * MINUTE, 0);
        }
        let fullness = game.stats.fullness;
        game.tick(70 * MINUTE, 0);
        assert_eq!(game.mode, Mode::Awake);
        assert_eq!(game.stats.fullness, fullness);
    }

    #[test]
    fn care_is_bounded_and_save_round_trips() {
        let mut stats = Stats::default();
        for _ in 0..25 {
            stats.care(Care::Play);
        }
        assert_eq!(stats.percentages(), [7, 100]);
        stats.care(Care::Feed);
        stats.care(Care::Pet);
        assert_eq!(stats.percentages(), [35, 100]);
        assert_eq!(Stats::decode(&stats.encode()), Some(stats));
        assert_eq!(Stats::decode("1 1500 1000 0"), None);
        assert_eq!(Stats::decode("1 350 1000 930"), Some(stats));
    }

    #[test]
    fn sleeping_on_demand_uses_the_same_wake_clicks() {
        let mut game = Game::new(0, Stats::default());
        game.sleep();
        game.tick(31_000, 0);
        assert_eq!(game.mode(), Mode::Asleep);
        assert!(!game.care(Care::Feed, 31_000));
        assert!(!game.click_cat());
        game.sleep();
        assert!(!game.click_cat());
        assert_eq!(game.wake_clicks(), 2);
        assert!(game.click_cat());
        assert_eq!(game.mode(), Mode::Awake);
    }

    #[test]
    fn care_uses_independent_cooldowns_and_only_rewards_accepted_actions() {
        let mut game = Game::new(0, Stats::default());
        assert!(game.care(Care::Feed, 0));
        let after_feed = game.stats;
        assert!(!game.care(Care::Feed, 100));
        assert_eq!(game.stats, after_feed);
        assert!(game.care(Care::Pet, 100));
        assert!(game.care(Care::Play, 100));
        assert_eq!(game.cooldown_seconds(100), [60, 10, 30]);
        assert!(!game.care(Care::Pet, 10_099));
        assert!(game.care(Care::Pet, 10_100));
        assert_eq!(game.cooldown_seconds(59_999), [1, 0, 0]);
        assert!(game.care(Care::Feed, 60_000));
    }

    #[test]
    fn activity_hunger_accumulates_fractional_motion_and_stops_at_zero() {
        let mut game = Game::new(0, Stats::default());
        game.walked_sideways(50.0);
        assert_eq!(game.stats.fullness, 820);
        game.spend_fullness_on_activity(0.5);
        assert_eq!(game.stats.fullness, 819);
        game.walked_sideways(1_000.0);
        assert_eq!(game.stats.fullness, 809);
        game.spend_fullness_on_activity(1_000.5);
        assert_eq!(game.stats.fullness, 0);
        assert_eq!(game.activity_hunger, 0.0);
    }
}
