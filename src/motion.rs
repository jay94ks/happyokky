use std::f32::consts::{PI, TAU};

use eframe::egui::{vec2, Vec2};

const SECTOR: f32 = TAU / 24.0;
const SECTOR_TIME: f32 = 0.018;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heading(i32);

impl Heading {
    pub fn nearest(direction: Vec2) -> Self {
        Self(((direction.y.atan2(direction.x) / SECTOR).round() as i32).rem_euclid(24))
    }

    pub fn unit(self) -> Vec2 {
        let (sin, cos) = (self.0 as f32 * SECTOR).sin_cos();
        vec2(cos, sin)
    }

    pub fn retains(self, direction: Vec2) -> bool {
        let angle = direction.y.atan2(direction.x);
        let difference = (angle - self.0 as f32 * SECTOR + PI).rem_euclid(TAU) - PI;
        difference.abs() <= SECTOR * 0.68
    }
}

#[derive(Default)]
pub struct Turn {
    current: Option<Heading>,
    elapsed: f32,
}

impl Turn {
    #[cfg(test)]
    pub fn reset(&mut self, heading: Heading) {
        self.current = Some(heading);
        self.elapsed = 0.0;
    }

    pub fn toward(&mut self, target: Heading, dt: f32) -> Heading {
        let current = self.current.unwrap_or(target);
        self.current = Some(current);
        if current == target {
            self.elapsed = 0.0;
            return current;
        }
        self.elapsed += dt;
        if self.elapsed < SECTOR_TIME {
            return current;
        }
        self.elapsed = (self.elapsed - SECTOR_TIME).min(SECTOR_TIME);
        let gap = (target.0 - current.0).rem_euclid(24);
        let sign = if gap < 12 || (gap == 12 && current.0 < target.0) {
            1
        } else {
            -1
        };
        let next = Heading((current.0 + sign).rem_euclid(24));
        self.current = Some(next);
        next
    }
}

pub fn phase(progress: f32, steps: u32) -> f32 {
    (progress.clamp(0.0, 1.0) * steps as f32).floor() / steps as f32
}

pub fn sample(progress: f32, keyframes: &[f32]) -> f32 {
    let position = progress.clamp(0.0, 1.0) * (keyframes.len() - 1) as f32;
    let first = position.floor() as usize;
    let blend = position.fract();
    let eased = blend * blend * (3.0 - 2.0 * blend);
    let next = keyframes[(first + 1).min(keyframes.len() - 1)];
    keyframes[first] + (next - keyframes[first]) * eased
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_visits_every_fifteen_degree_step() {
        let mut turn = Turn::default();
        turn.reset(Heading(0));
        for expected in 1..=12 {
            assert_eq!(turn.toward(Heading(12), 0.020), Heading(expected));
        }
        turn.reset(Heading(23));
        assert_eq!(turn.toward(Heading(1), 0.020), Heading(0));
        assert_eq!(turn.toward(Heading(1), 0.020), Heading(1));
    }

    #[test]
    fn the_clip_samples_between_keyframes_and_keeps_its_endpoints() {
        let values = [0.0, 0.7, -0.4, 0.0];
        assert_eq!(sample(0.0, &values), 0.0);
        assert_eq!(sample(1.0, &values), 0.0);
        assert!(sample(0.15, &values) > 0.0);
        assert!(sample(0.78, &values) < 0.0);
    }
}
