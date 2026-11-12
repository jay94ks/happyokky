use eframe::egui::{vec2, Vec2};

use super::{sample_pixels, Sprite};

const GRID: usize = 8;
const SPACING: usize = 6;
const SIDE: usize = (GRID - 1) * SPACING + 1;
const LAST: f32 = (SIDE - 1) as f32;

#[derive(Clone, Copy)]
struct Bounds {
    min: Vec2,
    span: Vec2,
}

impl Bounds {
    fn of(sprite: &Sprite) -> Self {
        let mut min = vec2(sprite.width as f32, sprite.height as f32);
        let mut max = Vec2::ZERO;
        for (y, row) in sprite.pixels.chunks_exact(sprite.width).enumerate() {
            for (x, pixel) in row.iter().enumerate() {
                if pixel >> 24 >= 64 {
                    let point = vec2(x as f32, y as f32);
                    min = min.min(point);
                    max = max.max(point);
                }
            }
        }
        let span = (max - min).max(Vec2::splat(1.0));
        Self {
            min: min.min(max),
            span,
        }
    }

    fn at(self, point: Vec2) -> Vec2 {
        self.min + self.span * (point / LAST)
    }
}

#[derive(Clone, Copy, Default)]
struct Feature {
    alpha: i16,
    ink: i16,
    blue: i16,
}

impl Feature {
    fn from_pixel(pixel: u32) -> Self {
        let [blue, green, red, alpha] = pixel.to_le_bytes().map(i16::from);
        let light = (3 * red + 6 * green + blue) / 10;
        Self {
            alpha,
            ink: alpha - light,
            blue: blue - red,
        }
    }

    fn difference(self, other: Self) -> i32 {
        let alpha = i32::from(self.alpha - other.alpha);
        let ink = i32::from(self.ink - other.ink);
        let blue = i32::from(self.blue - other.blue);
        alpha * alpha + 2 * ink * ink + blue * blue
    }
}

fn features(sprite: &Sprite, bounds: Bounds) -> Vec<Feature> {
    (0..SIDE * SIDE)
        .map(|index| {
            let point = bounds.at(vec2((index % SIDE) as f32, (index / SIDE) as f32));
            Feature::from_pixel(sprite.sample(point.x, point.y))
        })
        .collect()
}

struct Field([Vec2; GRID * GRID]);

impl Field {
    fn match_features(from: &[Feature], to: &[Feature]) -> Self {
        fn pixel(image: &[Feature], x: i32, y: i32) -> Feature {
            if x < 0 || y < 0 || x >= SIDE as i32 || y >= SIDE as i32 {
                Feature::default()
            } else {
                image[y as usize * SIDE + x as usize]
            }
        }

        let mut offsets = [Vec2::ZERO; GRID * GRID];
        for (index, offset) in offsets.iter_mut().enumerate() {
            let x = (index % GRID * SPACING) as i32;
            let y = (index / GRID * SPACING) as i32;
            let patch: [Feature; 25] = std::array::from_fn(|i| {
                pixel(from, x + (i % 5) as i32 - 2, y + (i / 5) as i32 - 2)
            });
            let mut best = i32::MAX;
            for dy in -6_i32..=6 {
                for dx in -6_i32..=6 {
                    // Favor coherent motion over matches between repeated fur patterns.
                    let mut error = (dx * dx + dy * dy) * 400;
                    for (i, feature) in patch.iter().enumerate() {
                        let sample =
                            pixel(to, x + dx + (i % 5) as i32 - 2, y + dy + (i / 5) as i32 - 2);
                        error += feature.difference(sample);
                    }
                    if error < best {
                        best = error;
                        *offset = vec2(dx as f32, dy as f32);
                    }
                }
            }
        }
        // Soften the field, especially where a paw or ear is hidden in one keyframe.
        Self(std::array::from_fn(|index| {
            let x = index % GRID;
            let y = index / GRID;
            let neighbors = [
                y * GRID + x.saturating_sub(1),
                y * GRID + (x + 1).min(GRID - 1),
                y.saturating_sub(1) * GRID + x,
                (y + 1).min(GRID - 1) * GRID + x,
            ];
            let nearby = neighbors
                .iter()
                .fold(Vec2::ZERO, |sum, &i| sum + offsets[i]);
            let mut offset = offsets[index] * 0.68 + nearby * 0.08;
            if y == 0 || y == GRID - 1 {
                offset.y = 0.0;
            }
            offset
        }))
    }

    fn at(&self, point: Vec2) -> Vec2 {
        let point = (point / SPACING as f32).clamp(Vec2::ZERO, Vec2::splat((GRID - 1) as f32));
        let x = (point.x as usize).min(GRID - 2);
        let y = (point.y as usize).min(GRID - 2);
        let offset = y * GRID + x;
        let left = self.0[offset] + (self.0[offset + GRID] - self.0[offset]) * (point.y - y as f32);
        let right = self.0[offset + 1]
            + (self.0[offset + GRID + 1] - self.0[offset + 1]) * (point.y - y as f32);
        left + (right - left) * (point.x - x as f32)
    }

    fn inverse(&self, amount: f32) -> Self {
        Self(std::array::from_fn(|index| {
            let point = vec2(
                (index % GRID * SPACING) as f32,
                (index / GRID * SPACING) as f32,
            );
            let estimate = point - self.0[index] * amount;
            -self.at(estimate) * amount
        }))
    }
}

pub struct PoseMorph {
    from_bounds: Bounds,
    to_bounds: Bounds,
    forward: Field,
    backward: Field,
}

impl PoseMorph {
    pub fn new(from: &Sprite, to: &Sprite) -> Self {
        let from_bounds = Bounds::of(from);
        let to_bounds = Bounds::of(to);
        let a = features(from, from_bounds);
        let b = features(to, to_bounds);
        Self {
            from_bounds,
            to_bounds,
            forward: Field::match_features(&a, &b),
            backward: Field::match_features(&b, &a),
        }
    }

    pub(super) fn draw(&self, output: &mut [u32], source: &[u32], target: &Sprite, progress: f32) {
        let width = target.width;
        let height = target.height;
        if progress <= 0.0 || progress >= 1.0 {
            let pixels = if progress <= 0.0 {
                source
            } else {
                &target.pixels
            };
            output.copy_from_slice(&pixels[..output.len()]);
            return;
        }
        let before = 1.0 - progress;
        let min = self.from_bounds.min * before + self.to_bounds.min * progress;
        let span = self.from_bounds.span * before + self.to_bounds.span * progress;
        let step = vec2(LAST / span.x, LAST / span.y);
        // Restrict the gradual texture change to the middle of the movement.
        let mix = ((progress - 0.24) / 0.52).clamp(0.0, 1.0);
        let mix = (mix * mix * (3.0 - 2.0 * mix) * 256.0).round() as u32;
        let from_warp = self.forward.inverse(progress);
        let to_warp = self.backward.inverse(before);
        let last = LAST + 9.0;
        for (y, row) in output.chunks_exact_mut(width).enumerate() {
            let v = (y as f32 - min.y) * step.y;
            if !(-9.0..=last).contains(&v) {
                row.fill(0);
                continue;
            }
            for (x, pixel) in row.iter_mut().enumerate() {
                let u = (x as f32 - min.x) * step.x;
                if !(-9.0..=last).contains(&u) {
                    *pixel = 0;
                    continue;
                }
                let point = vec2(u, v);
                let first = || {
                    let a = self.from_bounds.at(point + from_warp.at(point));
                    sample_pixels(source, width, height, a.x, a.y)
                };
                let second = || {
                    let b = self.to_bounds.at(point + to_warp.at(point));
                    target.sample(b.x, b.y)
                };
                *pixel = match mix {
                    0 => first(),
                    256 => second(),
                    _ => blend_registered(first(), second(), mix),
                };
            }
        }
    }
}

fn blend_registered(from: u32, to: u32, mix: u32) -> u32 {
    let (near, far, fraction) = if mix < 128 {
        (from, to, mix)
    } else {
        (to, from, 256 - mix)
    };
    let near = near.to_le_bytes().map(u32::from);
    let far = far.to_le_bytes().map(u32::from);
    let difference = (0..4)
        .map(|channel| near[channel].abs_diff(far[channel]))
        .max()
        .unwrap_or(0);
    // An unmatched eye or silhouette edge belongs to one pose. Averaging it
    // with the other creates a second translucent outline.
    let fraction = fraction * 96_u32.saturating_sub(difference) / 96;
    u32::from_le_bytes(std::array::from_fn(|channel| {
        ((near[channel] * (256 - fraction) + far[channel] * fraction + 128) >> 8) as u8
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn a_translated_pose_keeps_one_opaque_outline_and_sharp_features() {
        let pose = |left: u32| {
            let mut image = RgbaImage::new(52, 36);
            for y in 5..31 {
                for x in left..left + 16 {
                    let color = if (left + 4..left + 7).contains(&x) && (11..16).contains(&y) {
                        [12, 20, 48, 255]
                    } else {
                        [245, 248, 253, 255]
                    };
                    image.put_pixel(x, y, Rgba(color));
                }
            }
            Sprite::resize(&image, 52, 36)
        };
        let from = pose(6);
        let to = pose(26);
        let morph = PoseMorph::new(&from, &to);
        let mut output = vec![0; 52 * 36];
        morph.draw(&mut output, &from.pixels, &to, 0.5);
        let pixel = |x: usize, y: usize| output[y * 52 + x];
        assert_eq!(pixel(8, 18), 0);
        assert_eq!(pixel(39, 18), 0);
        assert_eq!(pixel(18, 18) >> 24, 255);
        assert_eq!(pixel(21, 13), from.pixels[13 * 52 + 11]);
    }

    #[test]
    fn unmatched_detail_keeps_the_nearer_poses_coverage_and_contrast() {
        let eye = u32::from_le_bytes([48, 20, 12, 255]);
        let fur = u32::from_le_bytes([253, 248, 245, 255]);
        assert_eq!(blend_registered(0, fur, 120), 0);
        assert_eq!(blend_registered(0, fur, 136), fur);
        assert_eq!(blend_registered(fur, eye, 136), eye);
    }
}
