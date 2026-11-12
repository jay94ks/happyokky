use std::ffi::c_void;
use std::io;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use std::slice;

use image::{imageops, Rgba, RgbaImage};
use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

mod morph;
pub use morph::PoseMorph;

const FRAME_WIDTH: u32 = 340;
const FRAME_HEIGHT: u32 = 348;
pub const FRONT: usize = 4;
pub const SITTING_UP: usize = 1;
pub const WALK_LEFT: [usize; 8] = [28, 29, 30, 31, 32, 33, 34, 35];
pub const SLEEPING: usize = 10;
pub const WALK_RIGHT: [usize; 8] = [36, 37, 38, 39, 40, 41, 42, 43];
pub const RUN_LEFT: [usize; 8] = [12, 13, 14, 15, 16, 17, 18, 19];
pub const RUN_RIGHT: [usize; 8] = [20, 21, 22, 23, 24, 25, 26, 27];
pub const CARRY: [usize; 8] = [44, 45, 46, 47, 48, 49, 50, 51];
pub const GREETING: usize = 52;
pub const KNEADING: usize = 53;
pub const EARS_BACK: usize = 54;
pub const ARCHING: usize = 55;
pub const BELLY_UP: usize = 56;
pub const KEYBOARD_LOAF: usize = 57;
pub const SIT_MID: usize = 58;
pub const LOWER_MID: usize = 59;
pub const ROLL_MID: usize = 60;
pub const LIFT_MID: usize = 61;
pub const ALERT_MID: usize = 62;
pub const STRETCH_MID: usize = 63;
const TWEEN_START: usize = 64;
pub const CYCLE_FRAMES: usize = 16;
const SLEEP_CARRY_START: usize = TWEEN_START + 5 * 8;
pub const SLEEP_CONTACT: usize = SLEEP_CARRY_START + CYCLE_FRAMES;
pub const SLEEP_BEND: usize = SLEEP_CONTACT + 1;
pub const SLEEP_TUCK: usize = SLEEP_CONTACT + 2;
pub const SLEEP_CURL: usize = SLEEP_CONTACT + 3;
const BEHAVIOR_VARIANTS: usize = SLEEP_CONTACT + 4;
const TRANSITION_VARIANTS: usize = BEHAVIOR_VARIANTS + 12;
const BODY_TURN_START: usize = TRANSITION_VARIANTS + 6;
const REAR_TURN_START: usize = BODY_TURN_START + 12;
pub const TASKBAR_KNEAD: [usize; 4] = [
    REAR_TURN_START + 4,
    REAR_TURN_START + 5,
    REAR_TURN_START + 6,
    REAR_TURN_START + 7,
];
pub const DESKTOP_KNEAD: [usize; 4] = [
    REAR_TURN_START + 8,
    REAR_TURN_START + 9,
    REAR_TURN_START + 10,
    REAR_TURN_START + 11,
];
const FRAME_COUNT: usize = REAR_TURN_START + 12;

/// Positive angles turn toward screen-right, in increments of fifteen degrees.
pub fn body_turn_frame(sector: i8) -> usize {
    match sector {
        -6..=-1 => BODY_TURN_START + (-sector - 1) as usize,
        1..=6 => BODY_TURN_START + 6 + (sector - 1) as usize,
        7..=10 => REAR_TURN_START + (sector - 7) as usize,
        11 => DESKTOP_KNEAD[0],
        _ => FRONT,
    }
}

pub fn behavior_keyframes(base: usize) -> [usize; 3] {
    let cell = base - GREETING;
    let variant = BEHAVIOR_VARIANTS + (cell / 3) * 6 + cell % 3;
    [variant, base, variant + 3]
}

pub fn transition_keyframe(base: usize) -> usize {
    TRANSITION_VARIANTS + base - SIT_MID
}

#[derive(Clone, Copy)]
pub enum Cycle {
    RunLeft,
    RunRight,
    WalkLeft,
    WalkRight,
    Carry,
    SleepingCarry,
}

impl Cycle {
    const ALL: [Self; 5] = [
        Self::RunLeft,
        Self::RunRight,
        Self::WalkLeft,
        Self::WalkRight,
        Self::Carry,
    ];

    fn keys(self) -> [usize; 8] {
        match self {
            Self::RunLeft => RUN_LEFT,
            Self::RunRight => RUN_RIGHT,
            Self::WalkLeft => WALK_LEFT,
            Self::WalkRight => WALK_RIGHT,
            Self::Carry => CARRY,
            Self::SleepingCarry => unreachable!("sleeping carry frames are sampled directly"),
        }
    }

    pub fn frame(self, phase: usize) -> usize {
        let phase = phase % CYCLE_FRAMES;
        if matches!(self, Self::SleepingCarry) {
            return SLEEP_CARRY_START + phase;
        }
        let key = phase / 2;
        if phase % 2 == 0 {
            self.keys()[key]
        } else {
            TWEEN_START + self as usize * 8 + key
        }
    }
}

fn tween_sources(frame: usize) -> Option<(usize, usize)> {
    let index = frame.checked_sub(TWEEN_START)?;
    let keys = Cycle::ALL.get(index / 8)?.keys();
    let step = index % 8;
    Some((keys[step], keys[(step + 1) % 8]))
}
const RUN_CROP_WIDTH: u32 = 275;
const RUN_BASELINES: [u32; 4] = [236, 492, 748, 1002];
// Cell-local crop x, mouth x, and sheet-relative mouth y.
const RUN_LAYOUT: [[(u32, u32, u32); 4]; 4] = [
    [(64, 95, 135), (55, 86, 135), (43, 74, 138), (35, 68, 138)],
    [(57, 88, 394), (48, 80, 394), (37, 69, 392), (31, 63, 392)],
    [
        (66, 320, 648),
        (60, 308, 648),
        (55, 297, 650),
        (54, 296, 652),
    ],
    [
        (77, 318, 905),
        (64, 308, 904),
        (53, 301, 904),
        (46, 295, 904),
    ],
];

pub fn mouth_anchor(frame: usize, width: f32) -> (f32, f32) {
    if let Some((first, second)) = tween_sources(frame) {
        let (x1, y1) = mouth_anchor(first, width);
        let (x2, y2) = mouth_anchor(second, width);
        return ((x1 + x2) * 0.5, (y1 + y2) * 0.5);
    }
    // The anchors use the normalized atlas coordinates after paw alignment.
    let (x, y) = if (RUN_LEFT[0]..=RUN_RIGHT[7]).contains(&frame) {
        let cell = frame - RUN_LEFT[0];
        let (left, x, y) = RUN_LAYOUT[cell / 4][cell % 4];
        let scale = FRAME_WIDTH as f32 / RUN_CROP_WIDTH as f32;
        (
            (x - left) as f32 * scale,
            FRAME_HEIGHT as f32 - (RUN_BASELINES[cell / 4] - y) as f32 * scale,
        )
    } else if (BODY_TURN_START..REAR_TURN_START).contains(&frame) {
        const TURN_MOUTHS: [(f32, f32); 12] = [
            (134.0, 195.0),
            (97.0, 196.0),
            (67.0, 197.0),
            (55.0, 192.0),
            (43.0, 198.0),
            (35.0, 191.0),
            (211.0, 188.0),
            (248.0, 191.0),
            (285.0, 192.0),
            (289.0, 188.0),
            (302.0, 188.0),
            (309.0, 190.0),
        ];
        TURN_MOUTHS[frame - BODY_TURN_START]
    } else {
        (166.0, 203.0)
    };
    let scale = width / FRAME_WIDTH as f32;
    (x * scale, y * scale)
}

pub struct Sprites {
    frames: Vec<RgbaImage>,
    avatar: RgbaImage,
    app_icon: RgbaImage,
}

impl Sprites {
    pub fn load() -> image::ImageResult<Self> {
        let sheet = image::load_from_memory_with_format(
            include_bytes!("../assets/okky-full-body.png"),
            image::ImageFormat::Png,
        )?
        .into_rgba8();
        let avatar = imageops::crop_imm(&sheet, 370, 410, 274, 274).to_image();
        let mut frames = Vec::with_capacity(FRAME_COUNT - Cycle::ALL.len() * 8);
        // Align each atlas row by the paws, including the sleeping pose.
        for baseline in [370, 735, 1105, 1470] {
            for column in 0..3 {
                let x = column * sheet.width() / 3;
                let mut frame = imageops::crop_imm(
                    &sheet,
                    x,
                    baseline - FRAME_HEIGHT,
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                )
                .to_image();
                premultiply(&mut frame);
                frames.push(frame);
            }
        }
        let run_sheet = image::load_from_memory_with_format(
            include_bytes!("../assets/okky-run-cycle.png"),
            image::ImageFormat::Png,
        )?
        .into_rgba8();
        for (row, baseline) in RUN_BASELINES.into_iter().enumerate() {
            for (column, (left, _, _)) in RUN_LAYOUT[row].into_iter().enumerate() {
                let x = column as u32 * 384 + left;
                let mut pose =
                    imageops::crop_imm(&run_sheet, x, baseline - 232, RUN_CROP_WIDTH, 232)
                        .to_image();
                premultiply(&mut pose);
                let height = pose.height() * FRAME_WIDTH / pose.width();
                let pose =
                    imageops::resize(&pose, FRAME_WIDTH, height, imageops::FilterType::Triangle);
                let mut frame = RgbaImage::new(FRAME_WIDTH, FRAME_HEIGHT);
                imageops::replace(&mut frame, &pose, 0, (FRAME_HEIGHT - height) as i64);
                frames.push(frame);
            }
        }
        let walk_sheet = image::load_from_memory_with_format(
            include_bytes!("../assets/okky-walk.png"),
            image::ImageFormat::Png,
        )?
        .into_rgba8();
        for (row, (left, baseline)) in [(96, 236), (96, 487), (60, 743), (60, 998)]
            .into_iter()
            .enumerate()
        {
            for column in 0..4 {
                let x = column * 384 + left;
                let mut pose =
                    imageops::crop_imm(&walk_sheet, x, baseline - 232, 230, 232).to_image();
                premultiply(&mut pose);
                let pose = imageops::resize(
                    &pose,
                    FRAME_WIDTH,
                    FRAME_HEIGHT,
                    imageops::FilterType::Triangle,
                );
                frames.push(normalize_walk_head(pose, row < 2));
            }
        }
        append_carry_frames(
            &mut frames,
            include_bytes!("../assets/okky-lift.png"),
            CARRY.len(),
            [23.0, 7.0],
        )?;
        for (data, edges) in [
            (
                include_bytes!("../assets/okky-behaviors.png").as_slice(),
                [[0, 512, 985, 1536], [0, 493, 1008, 1536]],
            ),
            (
                include_bytes!("../assets/okky-transitions.png").as_slice(),
                [[0, 512, 1024, 1536], [0, 512, 1024, 1536]],
            ),
        ] {
            append_aligned_atlas(&mut frames, data, &edges, 0.78)?;
        }
        append_carry_frames(
            &mut frames,
            include_bytes!("../assets/okky-sleep-carry.png"),
            CYCLE_FRAMES,
            [16.0, 4.5],
        )?;
        append_aligned_atlas(
            &mut frames,
            include_bytes!("../assets/okky-sleep-landing.png"),
            &[[0, 627, 1254]; 2],
            0.64,
        )?;
        append_aligned_atlas(
            &mut frames,
            include_bytes!("../assets/okky-behaviors-upper-frames.png"),
            &[[0, 512, 985, 1536], [0, 490, 1008, 1536]],
            0.78,
        )?;
        append_aligned_atlas(
            &mut frames,
            include_bytes!("../assets/okky-behaviors-lower-frames.png"),
            &[[0, 515, 1008, 1536], [0, 493, 1008, 1536]],
            0.78,
        )?;
        append_aligned_atlas(
            &mut frames,
            include_bytes!("../assets/okky-transition-frames.png"),
            &[[0, 512, 1024, 1536]; 2],
            0.78,
        )?;
        append_aligned_atlas(
            &mut frames,
            include_bytes!("../assets/okky-body-turn.png"),
            &[[0, 341, 682, 1024]; 4],
            1.12,
        )?;
        for (data, scale) in [
            (
                include_bytes!("../assets/okky-rear-turn.png").as_slice(),
                0.69,
            ),
            (
                include_bytes!("../assets/okky-ground-knead.png").as_slice(),
                0.59,
            ),
            (
                include_bytes!("../assets/okky-desktop-knead.png").as_slice(),
                0.59,
            ),
        ] {
            append_aligned_atlas(&mut frames, data, &[[0, 627, 1254]; 2], scale)?;
        }
        let app_icon = image::load_from_memory_with_format(
            include_bytes!("../assets/happy-okky-icon.png"),
            image::ImageFormat::Png,
        )?
        .into_rgba8();
        Ok(Self {
            frames,
            avatar,
            app_icon,
        })
    }

    pub fn avatar(&self, width: u32) -> RgbaImage {
        imageops::resize(&self.avatar, width, width, imageops::FilterType::Triangle)
    }

    pub fn app_icon(&self) -> &RgbaImage {
        &self.app_icon
    }

    pub fn scaled(&self, width: u32) -> Vec<Sprite> {
        let height = width * FRAME_HEIGHT / FRAME_WIDTH;
        let mut frames: Vec<_> = self
            .frames
            .iter()
            .enumerate()
            .map(|(index, frame)| {
                let mut sprite = Sprite::resize(frame, width, height);
                sprite.face = if index == SITTING_UP {
                    Face::Up
                } else if WALK_LEFT.contains(&index) {
                    Face::Left
                } else if WALK_RIGHT.contains(&index) {
                    Face::Right
                } else {
                    Face::Front
                };
                sprite
            })
            .collect();
        let in_betweens: Vec<_> = (TWEEN_START..SLEEP_CARRY_START)
            .map(|index| {
                let (first, second) = tween_sources(index).expect("cycle frame");
                Sprite::in_between(&frames[first], &frames[second])
            })
            .collect();
        frames.splice(TWEEN_START..TWEEN_START, in_betweens);
        frames
    }
}

fn append_carry_frames(
    frames: &mut Vec<RgbaImage>,
    data: &[u8],
    count: usize,
    amplitude: [f32; 2],
) -> image::ImageResult<()> {
    let image = image::load_from_memory_with_format(data, image::ImageFormat::Png)?.into_rgba8();
    let mut cutout = imageops::crop_imm(&image, 241, 79, 744, 1043).to_image();
    premultiply(&mut cutout);
    let height = 336;
    let width = cutout.width() * height / cutout.height();
    let cutout = imageops::resize(&cutout, width, height, imageops::FilterType::Triangle);
    let mut pose = RgbaImage::new(FRAME_WIDTH, FRAME_HEIGHT);
    imageops::replace(&mut pose, &cutout, ((FRAME_WIDTH - width) / 2) as i64, 4);
    let source = Sprite::resize(&pose, FRAME_WIDTH, FRAME_HEIGHT);
    for frame in 0..count {
        let angle = frame as f32 * std::f32::consts::TAU / count as f32;
        let (sway, lift) = angle.sin_cos();
        let mut carried = pose.clone();
        for y in 184..FRAME_HEIGHT {
            // The grip stays fixed while the dangling paws and tail sway.
            let weight = ((y as f32 - 184.0) / (FRAME_HEIGHT as f32 - 184.0)).powi(2);
            for x in 0..FRAME_WIDTH {
                let [b, g, r, a] = source
                    .sample(
                        x as f32 - sway * amplitude[0] * weight,
                        y as f32 - lift * amplitude[1] * weight,
                    )
                    .to_le_bytes();
                carried.put_pixel(x, y, Rgba([r, g, b, a]));
            }
        }
        frames.push(carried);
    }
    Ok(())
}

fn append_aligned_atlas<const N: usize, const ROWS: usize>(
    frames: &mut Vec<RgbaImage>,
    data: &[u8],
    edges: &[[u32; N]; ROWS],
    max_scale: f32,
) -> image::ImageResult<()> {
    let atlas = image::load_from_memory_with_format(data, image::ImageFormat::Png)?.into_rgba8();
    let height = atlas.height() / ROWS as u32;
    for (row, boundaries) in edges.iter().enumerate() {
        for columns in boundaries.windows(2) {
            let x = columns[0];
            let cell = imageops::crop_imm(&atlas, x, row as u32 * height, columns[1] - x, height);
            frames.push(align_pose(cell.to_image(), max_scale));
        }
    }
    Ok(())
}

fn align_pose(cell: RgbaImage, max_scale: f32) -> RgbaImage {
    let mut left = cell.width();
    let mut top = cell.height();
    let mut right = 0;
    let mut bottom = 0;
    for (x, y, pixel) in cell.enumerate_pixels() {
        if pixel[3] >= 48 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x + 1);
            bottom = bottom.max(y + 1);
        }
    }
    let mut frame = RgbaImage::new(FRAME_WIDTH, FRAME_HEIGHT);
    if left >= right || top >= bottom {
        return frame;
    }
    let left = left.saturating_sub(2);
    let top = top.saturating_sub(2);
    let width = (right + 2).min(cell.width()) - left;
    let height = (bottom + 2).min(cell.height()) - top;
    let mut pose = imageops::crop_imm(&cell, left, top, width, height).to_image();
    premultiply(&mut pose);
    let scale = (322.0 / width as f32)
        .min(328.0 / height as f32)
        .min(max_scale);
    let width = (width as f32 * scale).round() as u32;
    let height = (height as f32 * scale).round() as u32;
    let pose = imageops::resize(&pose, width, height, imageops::FilterType::Triangle);
    imageops::replace(
        &mut frame,
        &pose,
        ((FRAME_WIDTH - width) / 2) as i64,
        (FRAME_HEIGHT - height - 4) as i64,
    );
    frame
}

fn normalize_walk_head(mut pose: RgbaImage, facing_left: bool) -> RgbaImage {
    const NECK_Y: f32 = 229.0;
    let neck_x = if facing_left { 125.0 } else { 221.0 };
    let source = Sprite::resize(&pose, FRAME_WIDTH, FRAME_HEIGHT);
    for y in 0..NECK_Y as u32 {
        let vertical = ((NECK_Y - y as f32) / 52.0).clamp(0.0, 1.0);
        for x in 0..FRAME_WIDTH {
            // Keep the tail and the lower legs out of the head transform.
            let lateral = if facing_left {
                (276.0 - x as f32) / 42.0
            } else {
                (x as f32 - 60.0) / 42.0
            }
            .clamp(0.0, 1.0);
            let scale = 1.0 - 0.115 * vertical * lateral;
            let [b, g, r, a] = source
                .sample(
                    neck_x + (x as f32 - neck_x) / scale,
                    NECK_Y + (y as f32 - NECK_Y) / scale,
                )
                .to_le_bytes();
            pose.put_pixel(x, y, Rgba([r, g, b, a]));
        }
    }
    pose
}

fn premultiply(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = pixel[3] as u16;
        for channel in &mut pixel.0[..3] {
            *channel = ((*channel as u16 * alpha + 127) / 255) as u8;
        }
    }
}

pub struct Sprite {
    pub height: usize,
    width: usize,
    pixels: Vec<u32>,
    face: Face,
}

#[derive(Clone, Copy, Default)]
enum Face {
    #[default]
    Front,
    Up,
    Left,
    Right,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Look {
    horizontal: i8,
    vertical: i8,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct PoseDetail {
    pub look: Look,
    pub blend: Option<(usize, u8)>,
    paws: i8,
    tail: i8,
    blink: u8,
    lean: i8,
    lift: i8,
}

impl PoseDetail {
    pub fn with_blend(mut self, frame: usize, weight: f32) -> Self {
        let opacity = (weight.clamp(0.0, 1.0) * 255.0).round() as u8;
        self.blend = (opacity != 0).then_some((frame, opacity));
        self
    }

    pub fn with_paws(mut self, amount: f32) -> Self {
        self.paws = (amount.clamp(-1.0, 1.0) * 20.0).round() as i8;
        self
    }

    pub fn with_tail(mut self, amount: f32) -> Self {
        self.tail = (amount.clamp(-1.0, 1.0) * 20.0).round() as i8;
        self
    }

    pub fn with_blink(mut self, amount: f32) -> Self {
        self.blink = (amount.clamp(0.0, 1.0) * 20.0).round() as u8;
        self
    }

    pub fn with_body(mut self, lean: f32, lift: f32) -> Self {
        self.lean = (lean.clamp(-1.0, 1.0) * 20.0).round() as i8;
        self.lift = (lift.clamp(-1.0, 1.0) * 20.0).round() as i8;
        self
    }
}

impl Look {
    pub fn from_direction(horizontal: f32, vertical: f32) -> Self {
        let step = |value: f32| (value.clamp(-1.0, 1.0) * 20.0).round() as i8;
        Self {
            horizontal: step(horizontal),
            vertical: step(vertical),
        }
    }
}

impl Sprite {
    fn resize(image: &RgbaImage, width: u32, height: u32) -> Self {
        let pixels = imageops::resize(image, width, height, imageops::FilterType::Triangle)
            .pixels()
            .map(|pixel| u32::from_le_bytes([pixel[2], pixel[1], pixel[0], pixel[3]]))
            .collect();
        Self {
            height: height as usize,
            width: width as usize,
            pixels,
            face: Face::Front,
        }
    }

    fn sample(&self, x: f32, y: f32) -> u32 {
        sample_pixels(&self.pixels, self.width, self.height, x, y)
    }

    fn in_between(from: &Self, to: &Self) -> Self {
        let mut pixels = vec![0; from.pixels.len()];
        PoseMorph::new(from, to).draw(&mut pixels, &from.pixels, to, 0.5);
        Self {
            pixels,
            width: from.width,
            height: from.height,
            face: from.face,
        }
    }
}

fn sample_pixels(pixels: &[u32], width: usize, height: usize, x: f32, y: f32) -> u32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let dx = x - x0 as f32;
    let dy = y - y0 as f32;
    let pixel = |x: i32, y: i32| {
        if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
            [0; 4]
        } else {
            pixels[y as usize * width + x as usize].to_le_bytes()
        }
    };
    let a = pixel(x0, y0);
    let b = pixel(x0 + 1, y0);
    let c = pixel(x0, y0 + 1);
    let d = pixel(x0 + 1, y0 + 1);
    u32::from_le_bytes(std::array::from_fn(|channel| {
        let top = a[channel] as f32 * (1.0 - dx) + b[channel] as f32 * dx;
        let bottom = c[channel] as f32 * (1.0 - dx) + d[channel] as f32 * dx;
        (top * (1.0 - dy) + bottom * dy).round() as u8
    }))
}

pub fn move_layer(window: HWND, x: i32, y: i32) -> bool {
    let destination = POINT { x, y };
    // Supplying only the destination lets the compositor reuse the last bitmap.
    unsafe { UpdateLayeredWindow(window, 0, &destination, null(), 0, null(), 0, null(), 0) != 0 }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Badge {
    Fish,
    Heart,
    Ball,
    Sleep,
}

impl Badge {
    fn color(self, x: f32, y: f32) -> [u8; 4] {
        let inside = |cx: f32, cy: f32, radius: f32| (x - cx).hypot(y - cy) < radius;
        match self {
            Self::Ball if inside(-0.26, -0.32, 0.19) => [255, 255, 255, 255],
            Self::Ball if inside(0.0, 0.0, 0.75) && x + y > 0.44 => [55, 89, 177, 255],
            Self::Ball if inside(0.0, 0.0, 0.75) => [115, 152, 250, 255],
            Self::Heart
                if inside(-0.34, -0.29, 0.41)
                    || inside(0.34, -0.29, 0.41)
                    || ((x.abs() + y * 0.85) < 0.65 && y > -0.24) =>
            {
                [239, 104, 143, 255]
            }
            Self::Fish if inside(-0.31, -0.09, 0.075) => [53, 62, 122, 255],
            Self::Fish
                if ((x + 0.16) / 0.56).hypot(y / 0.36) < 1.0
                    || (x > 0.17 && x < 0.81 && y.abs() < (x - 0.12) * 0.62) =>
            {
                [93, 166, 231, 255]
            }
            Self::Sleep
                if x.abs() < 0.60 && (y.abs() - 0.5).abs() < 0.10
                    || (x + y).abs() < 0.14 && y.abs() < 0.53 =>
            {
                [121, 110, 205, 255]
            }
            _ => [0; 4],
        }
    }
}

pub struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    original: HGDIOBJ,
    data: *mut u32,
    width: usize,
    height: usize,
    pose_pixels: Vec<u32>,
}

impl Surface {
    pub fn new(width: usize, height: usize) -> io::Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(0);
            if dc == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut info: BITMAPINFO = zeroed();
            info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            info.bmiHeader.biWidth = width as i32;
            info.bmiHeader.biHeight = -(height as i32);
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            info.bmiHeader.biCompression = BI_RGB;
            let mut data: *mut c_void = null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut data, 0, 0);
            if bitmap == 0 {
                let error = io::Error::last_os_error();
                DeleteDC(dc);
                return Err(error);
            }
            let original = SelectObject(dc, bitmap);
            Ok(Self {
                dc,
                bitmap,
                original,
                data: data.cast(),
                width,
                height,
                pose_pixels: Vec::new(),
            })
        }
    }

    pub fn stamp(&mut self, sprite: &Sprite, top: usize) {
        debug_assert_eq!(self.width, sprite.width);
        let width = self.width;
        let height = sprite.height.min(self.height.saturating_sub(top));
        let pixels = self.pixels();
        pixels.fill(0);
        pixels[top * width..(top + height) * width]
            .copy_from_slice(&sprite.pixels[..height * width]);
    }

    pub fn stamp_pose(
        &mut self,
        sprite: &Sprite,
        top: usize,
        detail: PoseDetail,
        transition: Option<(&Sprite, &PoseMorph, u8)>,
    ) {
        self.stamp(sprite, top);
        let face_moves = detail.look != Look::default() || detail.blink != 0;
        let body_moves = detail.paws != 0 || detail.tail != 0;
        if face_moves {
            self.stamp_look(sprite, top, detail.look, detail.blink as f32 / 20.0);
        }
        if body_moves {
            self.stamp_body_motion(sprite, top, detail);
        }
        if let Some((next, morph, amount)) = transition {
            let width = self.width;
            let len = width * sprite.height.min(self.height.saturating_sub(top));
            let mut scratch = std::mem::take(&mut self.pose_pixels);
            let output = &mut self.pixels()[top * width..top * width + len];
            let source = if face_moves || body_moves {
                scratch.clear();
                scratch.extend_from_slice(&sprite.pixels);
                scratch[..len].copy_from_slice(output);
                &scratch
            } else {
                &sprite.pixels
            };
            morph.draw(output, source, next, amount as f32 / 255.0);
            self.pose_pixels = scratch;
        }
        if detail.lean != 0 || detail.lift != 0 {
            self.stamp_weight_shift(sprite.height, top, detail);
        }
    }

    fn stamp_weight_shift(&mut self, sprite_height: usize, top: usize, detail: PoseDetail) {
        let width = self.width;
        let height = sprite_height.min(self.height.saturating_sub(top));
        let mut source = std::mem::take(&mut self.pose_pixels);
        source.clear();
        let pixels = self.pixels();
        let body = &mut pixels[top * width..(top + height) * width];
        source.extend_from_slice(body);
        let pivot = height as f32 * 0.88;
        let lean = detail.lean as f32 * width as f32 * (0.055 / 20.0);
        let lift = detail.lift as f32 * height as f32 * (0.055 / 20.0);
        for (y, row) in body.chunks_exact_mut(width).enumerate() {
            let weight = (1.0 - y as f32 / pivot).max(0.0);
            if weight == 0.0 {
                break;
            }
            for (x, pixel) in row.iter_mut().enumerate() {
                *pixel = sample_pixels(
                    &source,
                    width,
                    height,
                    x as f32 - lean * weight,
                    y as f32 + lift * weight,
                );
            }
        }
        self.pose_pixels = source;
    }

    fn stamp_look(&mut self, sprite: &Sprite, top: usize, look: Look, blink: f32) {
        let width = self.width;
        let scale = width as f32 / FRAME_WIDTH as f32;
        let (neck_x, neck_y, eye_y) = match sprite.face {
            Face::Front => (169.0, 237.0, 163.0),
            Face::Up => (162.0, 226.0, 124.0),
            Face::Left => (125.0, 229.0, 159.0),
            Face::Right => (221.0, 229.0, 159.0),
        };
        let head_end: f32 = neck_y + 7.0;
        let head_height = ((head_end * scale).ceil() as usize)
            .min(sprite.height)
            .min(self.height.saturating_sub(top));
        let horizontal = look.horizontal as f32 / 20.0;
        let vertical = look.vertical as f32 / 20.0;
        let (sin, cos) = (horizontal * 0.075).sin_cos();
        let pivot = (neck_x * scale, neck_y * scale);
        let pixels = self.pixels();
        for y in 0..head_height {
            // Ease the head transform into the neck so the body stays planted.
            let weight = ((head_end - y as f32 / scale) / 38.0).clamp(0.0, 1.0);
            let weight = weight * weight * (3.0 - 2.0 * weight);
            let row = (top + y) * width;
            let py = y as f32 - pivot.1;
            for x in 0..width {
                let px = x as f32 - pivot.0;
                let source_x =
                    x as f32 + (px * (cos - 1.0) + py * sin - 3.5 * scale * horizontal) * weight;
                let source_y =
                    y as f32 + (py * (cos - 1.0) - px * sin - 6.0 * scale * vertical) * weight;
                let eye_x = source_x / scale;
                let eye_center = match sprite.face {
                    Face::Front | Face::Up => {
                        if eye_x < 164.0 {
                            120.0
                        } else {
                            208.0
                        }
                    }
                    Face::Left => 98.0,
                    Face::Right => 245.0,
                };
                let eye_dx = eye_x - eye_center;
                let eye_dy = source_y / scale - eye_y;
                let iris = (1.0
                    - ((eye_x - eye_center) / 19.0).powi(2)
                    - ((source_y / scale - eye_y) / 22.0).powi(2))
                .max(0.0);
                let eyelid = (1.0 - (eye_dx / 30.0).powi(2) - (eye_dy / 30.0).powi(2)).max(0.0);
                pixels[row + x] = sprite.sample(
                    source_x - horizontal * 5.5 * scale * iris,
                    source_y - vertical * 4.5 * scale * iris
                        + blink * eye_dy * 3.8 * eyelid * scale,
                );
            }
        }
    }

    fn stamp_body_motion(&mut self, sprite: &Sprite, top: usize, detail: PoseDetail) {
        let width = self.width;
        let scale = width as f32 / FRAME_WIDTH as f32;
        let height = sprite.height.min(self.height.saturating_sub(top));
        let paws = detail.paws as f32 / 20.0;
        let tail = detail.tail as f32 / 20.0;
        let pixels = self.pixels();
        let smooth = |value: f32| {
            let t = value.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        for y in (232.0 * scale) as usize..height {
            let py = y as f32 / scale;
            for x in (85.0 * scale) as usize..width {
                let px = x as f32 / scale;
                let paw_area = smooth((py - 277.0) / 39.0)
                    * smooth((px - 92.0) / 27.0)
                    * smooth((231.0 - px) / 23.0);
                let left_lift = paws.max(0.0) * smooth((170.0 - px) / 12.0);
                let right_lift = (-paws).max(0.0) * smooth((px - 159.0) / 12.0);
                let tail_area = smooth((px - 226.0) / 51.0)
                    * smooth((py - 232.0) / 11.0)
                    * smooth((330.0 - py) / 24.0);
                let dx = tail * tail_area * 17.0;
                let dy = tail * tail_area * 8.0 - (left_lift + right_lift) * paw_area * 14.0;
                if dx.abs() + dy.abs() > 0.01 {
                    pixels[(top + y) * width + x] =
                        sprite.sample(x as f32 - dx * scale, y as f32 - dy * scale);
                }
            }
        }
    }

    pub fn draw_badge(&mut self, badge: Badge) {
        let width = self.width;
        let height = self.height;
        for (index, pixel) in self.pixels().iter_mut().enumerate() {
            let mut sum = [0u16; 4];
            for sub_y in [0.25, 0.75] {
                for sub_x in [0.25, 0.75] {
                    let x = 2.0 * ((index % width) as f32 + sub_x) / width as f32 - 1.0;
                    let y = 2.0 * ((index / width) as f32 + sub_y) / height as f32 - 1.0;
                    for (total, component) in sum.iter_mut().zip(badge.color(x, y)) {
                        *total += component as u16;
                    }
                }
            }
            *pixel =
                u32::from_le_bytes([sum[2], sum[1], sum[0], sum[3]].map(|value| (value / 4) as u8));
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // The selected, top-down 32-bit DIB owns exactly this many pixels.
        unsafe { slice::from_raw_parts_mut(self.data, self.width * self.height) }
    }

    pub fn present(&self, window: HWND, x: i32, y: i32, opacity: u8) -> bool {
        let position = POINT { x, y };
        let size = SIZE {
            cx: self.width as i32,
            cy: self.height as i32,
        };
        let origin = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            UpdateLayeredWindow(
                window, 0, &position, &size, self.dc, &origin, 0, &blend, ULW_ALPHA,
            ) != 0
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.original);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub fn create_layer(
    class: *const u16,
    title: *const u16,
    instance: isize,
    transparent: bool,
) -> io::Result<HWND> {
    let mut extended = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE;
    if transparent {
        extended |= WS_EX_TRANSPARENT;
    }
    let window = unsafe {
        CreateWindowExW(
            extended,
            class,
            title,
            WS_POPUP,
            0,
            0,
            0,
            0,
            0,
            0,
            instance,
            null(),
        )
    };
    if window == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::wide;
    use windows_sys::Win32::Foundation::RECT;

    #[test]
    fn atlas_poses_are_transparent_and_share_a_baseline() {
        let frames = Sprites::load().unwrap().scaled(146);
        assert_eq!(frames.len(), FRAME_COUNT);
        for (index, frame) in frames.iter().enumerate() {
            let alpha_at = |offset: usize| frame.pixels[offset] >> 24;
            assert_eq!(alpha_at(0), 0);
            assert_eq!(alpha_at(frame.width - 1), 0);
            let bottom = frame
                .pixels
                .iter()
                .rposition(|pixel| pixel >> 24 > 32)
                .unwrap()
                / frame.width;
            let source = tween_sources(index).map_or(index, |(first, _)| first);
            let lift = if (RUN_LEFT[0]..=RUN_RIGHT[7]).contains(&source)
                || CARRY.contains(&source)
                || (SLEEP_CARRY_START..SLEEP_CONTACT).contains(&index)
            {
                20
            } else {
                4
            };
            assert!(
                bottom >= frame.height - lift,
                "frame {index} stops at {bottom}"
            );
        }
        let top = |frame: &Sprite| {
            frame
                .pixels
                .iter()
                .position(|pixel| pixel >> 24 > 32)
                .unwrap()
                / frame.width
        };
        assert!(top(&frames[SLEEPING]) > top(&frames[FRONT]) + 20);
        for (higher, lower) in [
            (SLEEP_CONTACT, SLEEP_BEND),
            (SLEEP_BEND, SLEEP_TUCK),
            (SLEEP_TUCK, SLEEP_CURL),
        ] {
            assert!(top(&frames[lower]) > top(&frames[higher]));
        }
        let grip = (180 * frames[CARRY[0]].width / FRAME_WIDTH as usize) * frames[CARRY[0]].width;
        for frame in &CARRY[1..] {
            assert_eq!(
                &frames[CARRY[0]].pixels[..grip],
                &frames[*frame].pixels[..grip]
            );
        }
        assert_ne!(frames[CARRY[0]].pixels, frames[CARRY[2]].pixels);
        let asleep = Cycle::SleepingCarry;
        assert_ne!(
            frames[asleep.frame(0)].pixels[..grip],
            frames[CARRY[0]].pixels[..grip]
        );
        assert_eq!(
            frames[asleep.frame(0)].pixels[..grip],
            frames[asleep.frame(4)].pixels[..grip]
        );
        assert_ne!(
            frames[asleep.frame(0)].pixels[grip..],
            frames[asleep.frame(4)].pixels[grip..]
        );
    }

    #[test]
    fn looking_moves_the_head_while_the_paws_stay_planted() {
        let frames = Sprites::load().unwrap().scaled(146);
        let front = &frames[FRONT];
        let mut surface = Surface::new(146, front.height).unwrap();
        surface.stamp_pose(
            front,
            0,
            PoseDetail {
                look: Look::from_direction(1.0, -1.0),
                ..PoseDetail::default()
            },
            None,
        );
        let body = (245 * front.width / FRAME_WIDTH as usize + 1) * front.width;
        assert_ne!(&surface.pixels()[..body], &front.pixels[..body]);
        assert_eq!(&surface.pixels()[body..], &front.pixels[body..]);
    }

    #[test]
    fn layered_window_can_move_without_uploading_its_bitmap() {
        let class = wide("STATIC");
        let title = wide("OKKY render test");
        let window = create_layer(class.as_ptr(), title.as_ptr(), 0, true).unwrap();
        let mut surface = Surface::new(20, 20).unwrap();
        surface.draw_badge(Badge::Ball);
        let presented = surface.present(window, 20, 20, 255);
        let moved = presented && move_layer(window, 42, 53);
        let mut bounds: RECT = unsafe { zeroed() };
        unsafe {
            GetWindowRect(window, &mut bounds);
            DestroyWindow(window);
        }
        assert!(presented);
        assert!(moved);
        assert_eq!(
            (bounds.left, bounds.top, bounds.right, bounds.bottom),
            (42, 53, 62, 73)
        );
    }
}
