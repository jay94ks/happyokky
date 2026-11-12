use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use eframe::egui::Pos2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Action {
    #[default]
    Rest,
    Wander,
    Approach(Pos2),
    Greet,
    Belly,
    Keyboard,
    EarsBack,
    Arch,
    KneadTaskbar,
    KneadDesktop,
}

pub struct Signals {
    pub mouse: Option<Pos2>,
    pub mouse_still: bool,
    pub fullness: u16,
    pub mood: u16,
    pub fast_typing: bool,
    pub desktop_visible: bool,
    pub approach_radius: f32,
}

#[derive(Default)]
pub struct Behavior {
    action: Action,
    started: u64,
    until: u64,
    next_keyboard: u64,
    next_knead: u64,
    random: RandomState,
}

impl Behavior {
    pub fn progress(&self, now: u64) -> f32 {
        let duration = (self.until - self.started).max(1);
        (now.saturating_sub(self.started) as f32 / duration as f32).min(1.0)
    }

    pub fn update(&mut self, now: u64, signals: Signals) -> Action {
        if self.action == Action::KneadDesktop && !signals.desktop_visible {
            return self.start(Action::Rest, now, 1_100);
        }
        if let Action::Approach(anchor) = self.action {
            if signals.mouse.map_or(true, |mouse| {
                mouse.distance(anchor) >= signals.approach_radius
            }) {
                return self.start(Action::Rest, now, 1_100);
            }
        }
        if now < self.until {
            return self.action;
        }
        self.choose(now, signals, self.random.hash_one(now) % 100)
    }

    fn choose(&mut self, now: u64, signals: Signals, roll: u64) -> Action {
        if signals.fast_typing && signals.mood < 92 && now >= self.next_keyboard {
            self.next_keyboard = now + 45_000;
            return self.start(Action::Keyboard, now, 5_200);
        }
        if signals.fullness < 18 {
            return self.start(Action::Rest, now, 2_200 + roll * 8);
        }
        if signals.mood >= 60 && signals.mouse_still && roll < 35 {
            if let Some(mouse) = signals.mouse {
                return self.start(Action::Approach(mouse), now, 4_600);
            }
        }
        match roll {
            35..=48 if signals.mood >= 68 => self.start(Action::Greet, now, 3_400),
            49..=55 if signals.mood >= 84 => self.start(Action::Belly, now, 4_800),
            56..=86 => self.start(Action::Wander, now, 3_600),
            87..=99
                if signals.mood >= 72
                    && signals.mouse_still
                    && !signals.fast_typing
                    && now >= self.next_knead =>
            {
                self.next_knead = now + 16_000;
                if signals.desktop_visible && roll < 94 {
                    self.start(Action::KneadDesktop, now, 6_000)
                } else {
                    self.start(Action::KneadTaskbar, now, 4_000)
                }
            }
            _ => self.start(Action::Rest, now, 1_300 + roll * 15),
        }
    }

    pub fn relax(&mut self, now: u64) {
        self.start(Action::Belly, now, 4_800);
    }

    pub fn shaken(&mut self, count: u8, now: u64) {
        if count >= 4 {
            self.start(Action::Arch, now, 3_000);
        } else if count >= 2 {
            self.start(Action::EarsBack, now, 2_700);
        }
    }

    fn start(&mut self, action: Action, now: u64, duration: u64) -> Action {
        self.action = action;
        self.started = now;
        self.until = now + duration;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(mouse: Option<Pos2>) -> Signals {
        Signals {
            mouse,
            mouse_still: true,
            fullness: 80,
            mood: 80,
            fast_typing: false,
            desktop_visible: false,
            approach_radius: 24.0,
        }
    }

    #[test]
    fn an_approach_commits_to_one_point_without_chasing_a_moving_cursor() {
        let mut behavior = Behavior::default();
        let anchor = Pos2::new(100.0, 20.0);
        behavior.start(Action::Approach(anchor), 0, 4_600);
        assert_eq!(
            behavior.update(100, signals(Some(anchor))),
            Action::Approach(anchor)
        );
        assert_eq!(
            behavior.update(120, signals(Some(Pos2::new(130.0, 20.0)))),
            Action::Rest
        );
    }

    #[test]
    fn fast_typing_and_rough_handling_have_distinct_reactions() {
        let mut behavior = Behavior::default();
        let mut signals = signals(None);
        signals.mood = 65;
        signals.fast_typing = true;
        assert_eq!(behavior.update(0, signals), Action::Keyboard);
        behavior.shaken(2, 100);
        assert_eq!(behavior.action, Action::EarsBack);
        behavior.shaken(4, 120);
        assert_eq!(behavior.action, Action::Arch);
    }

    #[test]
    fn kneading_chooses_a_surface_and_rear_kneading_ends_when_the_desktop_is_covered() {
        let mut behavior = Behavior::default();
        let mut desktop = signals(None);
        desktop.desktop_visible = true;
        assert_eq!(behavior.choose(0, desktop, 90), Action::KneadDesktop);
        assert_eq!(behavior.update(100, signals(None)), Action::Rest);
        assert_eq!(
            behavior.choose(16_000, signals(None), 90),
            Action::KneadTaskbar
        );
    }
}
