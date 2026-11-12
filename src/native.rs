use std::collections::{hash_map::RandomState, HashMap, VecDeque};
use std::fs::{self, File};
use std::hash::BuildHasher;
use std::io::{self, Write};
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::null;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use eframe::egui::{self, pos2, vec2, Pos2, Vec2};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Media::Audio::{PlaySoundA, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_LBUTTON, VK_RBUTTON,
};
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::behavior::{Action, Behavior, Signals};
use crate::game::{valid_name, Care, Game, Mode, SavedPet, Stats};
use crate::motion::{phase, sample, Heading, Turn};
use crate::panel;
use crate::render::{
    behavior_keyframes, body_turn_frame, create_layer, mouth_anchor, move_layer,
    transition_keyframe, Badge, Cycle, Look, PoseDetail, PoseMorph, Sprite, Sprites, Surface,
    ALERT_MID, ARCHING, BELLY_UP, CYCLE_FRAMES, DESKTOP_KNEAD, EARS_BACK, FRONT, GREETING,
    KEYBOARD_LOAF, KNEADING, LIFT_MID, LOWER_MID, ROLL_MID, RUN_LEFT, RUN_RIGHT, SITTING_UP,
    SIT_MID, SLEEPING, SLEEP_BEND, SLEEP_CONTACT, SLEEP_CURL, SLEEP_TUCK, STRETCH_MID,
    TASKBAR_KNEAD,
};
use crate::{typing, APP_NAME, CAT_NAME};

pub const PET_CLASS: &str = "NyangArchi.Pet";
const EFFECT_CLASS: &str = "NyangArchi.Effect";
const TRAY_CALLBACK: u32 = WM_APP + 1;
pub const SHOW_STATUS: u32 = WM_APP + 2;
const CAT_PRESS: u32 = WM_APP + 3;
const SHOW_MENU: u32 = WM_APP + 4;
const RESTORE_TRAY: u32 = WM_APP + 5;
const REFRESH_DOCK: u32 = WM_APP + 6;
const SLEEP_CAT: u32 = WM_APP + 8;
const CAT_MOVE: u32 = WM_APP + 10;
const CAT_RELEASE: u32 = WM_APP + 11;
const PRIMARY_BUTTON_MASK: usize = 1;
const TIMER_ID: usize = 1;
const TRAY_ID: u32 = 1;
const APP_ICON_RESOURCE_ID: usize = 1;
const STATUS_ID: u16 = 1;
const EXIT_ID: u16 = 2;
const SLEEP_CAT_ID: u16 = 4;
const SIT_SETTLE_TIME: u64 = 220;
const SIT_RESUME_DISTANCE: f32 = 24.0;
const HUNT_DELAY: f32 = 0.50;
const FETCH_TURN_TIME: u64 = 504;
const BODY_TURN_STEP: f32 = 0.038;
const PURR: &[u8] = include_bytes!("../assets/okky-purr.wav");
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub name: Arc<str>,
    pub stats: Stats,
    pub mode: Mode,
    pub wake_clicks: u8,
    pub feedback: Option<&'static str>,
    pub save_failed: bool,
    pub watching: bool,
    pub kneading: bool,
    pub can_care: bool,
    pub cooldown_seconds: [u64; 3],
}

impl Default for Status {
    fn default() -> Self {
        Self {
            name: CAT_NAME.into(),
            stats: Stats::default(),
            mode: Mode::Awake,
            wake_clicks: 0,
            feedback: None,
            save_failed: false,
            watching: false,
            kneading: false,
            can_care: true,
            cooldown_seconds: [0; 3],
        }
    }
}

pub enum Command {
    Care(Care),
    ThrowBall(f32),
    Rename(String),
    Quit,
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

pub fn run(
    sprites: Arc<Sprites>,
    commands: Receiver<Command>,
    latest_status: Arc<Mutex<Status>>,
    ctx: egui::Context,
    panel_window: HWND,
) {
    let result =
        Desktop::new(sprites, commands, latest_status, ctx, panel_window).and_then(Desktop::run);
    if let Err(error) = result {
        error_dialog(&format!("바탕 화면에 옥키를 띄우지 못했어요.\n{error}"));
    }
    panel::shutdown(panel_window);
}

pub fn error_dialog(text: &str) {
    unsafe {
        MessageBoxW(
            0,
            wide(text).as_ptr(),
            wide(APP_NAME).as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

unsafe extern "system" fn pet_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let event = match message {
        WM_MOUSEACTIVATE => return MA_NOACTIVATE as LRESULT,
        WM_INPUT => {
            typing::on_input(lparam);
            return DefWindowProcW(window, message, wparam, lparam);
        }
        WM_INPUT_DEVICE_CHANGE if wparam as u32 == GIDC_REMOVAL => {
            typing::forget_device(lparam);
            return 0;
        }
        WM_LBUTTONDOWN => {
            SetCapture(window);
            post_cat_pointer(window, CAT_PRESS, lparam);
            return 0;
        }
        WM_MOUSEMOVE if wparam & PRIMARY_BUTTON_MASK != 0 => {
            post_cat_pointer(window, CAT_MOVE, lparam);
            return 0;
        }
        WM_LBUTTONUP => {
            post_cat_pointer(window, CAT_RELEASE, lparam);
            if GetCapture() == window {
                ReleaseCapture();
            }
            return 0;
        }
        WM_RBUTTONUP => SHOW_MENU,
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => REFRESH_DOCK,
        SHOW_STATUS => SHOW_STATUS,
        TRAY_CALLBACK => match lparam as u32 {
            WM_LBUTTONUP | WM_LBUTTONDBLCLK => SHOW_STATUS,
            WM_RBUTTONUP => SHOW_MENU,
            _ => return 0,
        },
        WM_COMMAND => match wparam as u16 {
            STATUS_ID => SHOW_STATUS,
            SLEEP_CAT_ID => SLEEP_CAT,
            EXIT_ID => {
                PostQuitMessage(0);
                return 0;
            }
            _ => return DefWindowProcW(window, message, wparam, lparam),
        },
        WM_CLOSE => {
            PostQuitMessage(0);
            return 0;
        }
        WM_ENDSESSION if wparam != 0 => {
            PostQuitMessage(0);
            return 0;
        }
        value if value == TASKBAR_CREATED.load(Ordering::Relaxed) && value != 0 => RESTORE_TRAY,
        _ => return DefWindowProcW(window, message, wparam, lparam),
    };
    PostMessageW(0, event, 0, 0);
    0
}

unsafe fn post_cat_pointer(window: HWND, message: u32, coordinates: LPARAM) {
    let mut position = POINT {
        x: coordinates as i16 as i32,
        y: (coordinates >> 16) as i16 as i32,
    };
    if ClientToScreen(window, &mut position) != 0 {
        PostMessageW(0, message, position.x as usize, position.y as isize);
    }
}

fn primary_button_down() -> bool {
    unsafe {
        let button = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(button as i32) < 0
    }
}

fn register(class: &str, procedure: WNDPROC, instance: HINSTANCE) -> io::Result<()> {
    let name = wide(class);
    unsafe {
        let definition = WNDCLASSW {
            lpfnWndProc: procedure,
            hInstance: instance,
            hCursor: LoadCursorW(0, IDC_ARROW),
            lpszClassName: name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&definition) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

struct Window(HWND);

impl Drop for Window {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

struct Tray {
    data: NOTIFYICONDATAW,
}

impl Tray {
    fn new(window: HWND, instance: HINSTANCE) -> Self {
        unsafe {
            let icon = LoadImageW(
                instance,
                APP_ICON_RESOURCE_ID as *const u16,
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_SHARED,
            );
            let mut data: NOTIFYICONDATAW = zeroed();
            data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = window;
            data.uID = TRAY_ID;
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = TRAY_CALLBACK;
            data.hIcon = if icon != 0 {
                icon
            } else {
                LoadIconW(0, IDI_APPLICATION)
            };
            let tip = wide(APP_NAME);
            data.szTip[..tip.len()].copy_from_slice(&tip);
            let tray = Self { data };
            tray.add();
            tray
        }
    }

    fn add(&self) {
        unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) };
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
    }
}

struct Save {
    path: PathBuf,
    legacy: PathBuf,
}

impl Save {
    fn new() -> Self {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        Self {
            path: base.join("NyangArchi").join("pet.txt"),
            legacy: base.join("OKKY Tamagotchi").join("pet-v1.txt"),
        }
    }

    fn load(&self) -> SavedPet {
        fs::read_to_string(&self.path)
            .or_else(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    fs::read_to_string(&self.legacy)
                } else {
                    Err(error)
                }
            })
            .ok()
            .as_deref()
            .and_then(SavedPet::decode)
            .unwrap_or_default()
    }

    fn write(&self, pet: SavedPet) -> io::Result<()> {
        fs::create_dir_all(self.path.parent().expect("save directory"))?;
        let temporary = self.path.with_extension("tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(pet.encode().as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary, &self.path)
    }
}

struct SaveWorker {
    jobs: Option<Sender<SavedPet>>,
    results: Receiver<io::Result<()>>,
    thread: Option<JoinHandle<()>>,
}

impl SaveWorker {
    fn start(save: Save) -> io::Result<Self> {
        let (jobs, pending) = mpsc::channel();
        let (done, results) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("Happy Okky saves".into())
            .spawn(move || {
                while let Ok(pet) = pending.recv() {
                    let latest = pending.try_iter().last().unwrap_or(pet);
                    let _ = done.send(save.write(latest));
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            results,
            thread: Some(thread),
        })
    }

    fn request(&self, pet: SavedPet) -> bool {
        self.jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(pet).is_ok())
    }
}

impl Drop for SaveWorker {
    fn drop(&mut self) {
        drop(self.jobs.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[derive(Clone, Copy)]
struct Dock {
    work: RECT,
    edge: u32,
    dpi: u32,
}

impl Dock {
    fn primary() -> io::Result<Self> {
        unsafe {
            let mut appbar: APPBARDATA = zeroed();
            appbar.cbSize = size_of::<APPBARDATA>() as u32;
            let taskbar = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            let monitor = MonitorFromWindow(taskbar, MONITOR_DEFAULTTOPRIMARY);
            let mut dock = Self::on_monitor(monitor)?;
            if SHAppBarMessage(ABM_GETTASKBARPOS, &mut appbar) != 0 {
                dock.edge = appbar.uEdge;
            }
            Ok(dock)
        }
    }

    fn at(point: POINT) -> io::Result<Self> {
        unsafe { Self::on_monitor(MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST)) }
    }

    fn taskbar_at(point: POINT) -> io::Result<Self> {
        let secondary_class = wide("Shell_SecondaryTrayWnd");
        unsafe {
            let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
            let mut previous = 0;
            loop {
                let taskbar = FindWindowExW(0, previous, secondary_class.as_ptr(), null());
                if taskbar == 0 {
                    return Self::primary();
                }
                if MonitorFromWindow(taskbar, MONITOR_DEFAULTTONEAREST) == monitor {
                    return Self::on_monitor(monitor);
                }
                previous = taskbar;
            }
        }
    }

    fn on_monitor(monitor: HMONITOR) -> io::Result<Self> {
        unsafe {
            let mut info: MONITORINFO = zeroed();
            info.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(monitor, &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut dpi = 96;
            let mut dpi_y = 96;
            if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) < 0 {
                dpi = 96;
            }
            let work = info.rcWork;
            let bounds = info.rcMonitor;
            let edge = [
                (work.left - bounds.left, ABE_LEFT),
                (work.top - bounds.top, ABE_TOP),
                (bounds.right - work.right, ABE_RIGHT),
                (bounds.bottom - work.bottom, ABE_BOTTOM),
            ]
            .into_iter()
            .max()
            .map_or(ABE_BOTTOM, |(_, edge)| edge);
            Ok(Self { work, edge, dpi })
        }
    }

    fn scale(self) -> f32 {
        self.dpi as f32 / 96.0
    }

    fn clamp(self, point: Pos2, width: f32, height: f32) -> Pos2 {
        let fit = |value: f32, min: i32, max: i32, length: f32| {
            value.clamp(min as f32, (max as f32 - length).max(min as f32))
        };
        pos2(
            fit(point.x, self.work.left, self.work.right, width),
            fit(point.y, self.work.top, self.work.bottom, height),
        )
    }

    fn lane(self, x: f32, width: f32, height: f32) -> Pos2 {
        let y = if self.edge == ABE_TOP {
            self.work.top as f32 + 3.0
        } else {
            self.work.bottom as f32 - height - 3.0
        };
        self.clamp(pos2(x, y), width, height)
    }

    fn landing_target(self, from: Pos2, width: f32, height: f32) -> Pos2 {
        let x = match self.edge {
            ABE_LEFT => self.work.left as f32 + 3.0,
            ABE_RIGHT => self.work.right as f32 - width - 3.0,
            _ => from.x,
        };
        self.lane(x, width, height)
    }
}

struct Drawing {
    surface: Surface,
    sprites: Vec<Sprite>,
    badge_surface: Surface,
    badge: Option<Badge>,
    dpi: u32,
    width: usize,
    height: usize,
    badge_size: usize,
    morphs: HashMap<(usize, usize), PoseMorph>,
    stamp: Option<(usize, usize, PoseDetail)>,
    last_position: Option<POINT>,
    last_effect: Option<(i32, i32, u8)>,
}

impl Drawing {
    fn new(sprites: &Sprites, dpi: u32) -> io::Result<Self> {
        let width = (146 * dpi / 96) as usize;
        let frames = sprites.scaled(width as u32);
        let height = frames[FRONT].height + width / 8;
        let badge_size = (31 * dpi / 96) as usize;
        Ok(Self {
            surface: Surface::new(width, height)?,
            sprites: frames,
            badge_surface: Surface::new(badge_size, badge_size)?,
            badge: None,
            dpi,
            width,
            height,
            badge_size,
            morphs: HashMap::new(),
            stamp: None,
            last_position: None,
            last_effect: None,
        })
    }

    fn cat(
        &mut self,
        window: HWND,
        position: Pos2,
        frame: usize,
        bob: f32,
        detail: PoseDetail,
    ) -> bool {
        let top = self.cat_top(bob);
        let stamp = (frame, top, detail);
        let changed = self.stamp != Some(stamp);
        if changed {
            let sprite = &self.sprites[frame];
            let transition = detail.blend.map(|(next, amount)| {
                let key = (frame, next);
                if self.morphs.len() >= 128 && !self.morphs.contains_key(&key) {
                    self.morphs.clear();
                }
                let target = &self.sprites[next];
                let morph = self
                    .morphs
                    .entry(key)
                    .or_insert_with(|| PoseMorph::new(sprite, target));
                (target, &*morph, amount)
            });
            self.surface.stamp_pose(sprite, top, detail, transition);
            self.stamp = Some(stamp);
        }
        let x = position.x.round() as i32;
        let y = position.y.round() as i32;
        if !changed
            && self
                .last_position
                .is_some_and(|last| last.x == x && last.y == y)
        {
            return true;
        }
        if (!changed && self.last_position.is_some() && move_layer(window, x, y))
            || self.surface.present(window, x, y, 255)
        {
            self.last_position = Some(POINT { x, y });
            true
        } else {
            self.last_position = None;
            false
        }
    }

    fn cat_top(&self, bob: f32) -> usize {
        (self.width as f32 / 8.0 + bob).round().max(0.0) as usize
    }

    fn ball_at_mouth(&self, position: Pos2, frame: usize, bob: f32, detail: PoseDetail) -> Pos2 {
        let anchor = |index| {
            let (x, y) = mouth_anchor(index, self.width as f32);
            vec2(x, y)
        };
        let origin = anchor(frame);
        let offset = detail.blend.map_or(origin, |(next, opacity)| {
            origin + (anchor(next) - origin) * (opacity as f32 / 255.0)
        });
        pos2(position.x.round(), position.y.round()) + offset + vec2(0.0, self.cat_top(bob) as f32)
            - Vec2::splat(self.badge_size as f32 / 2.0)
    }

    fn effect(&mut self, window: HWND, badge: Badge, position: Pos2, opacity: u8) {
        let changed = self.badge != Some(badge);
        if changed {
            self.badge_surface.draw_badge(badge);
            self.badge = Some(badge);
        }
        let x = position.x.round() as i32;
        let y = position.y.round() as i32;
        if !changed {
            if self.last_effect == Some((x, y, opacity)) {
                return;
            }
            if self.last_effect.is_some_and(|(_, _, last)| last == opacity)
                && move_layer(window, x, y)
            {
                self.last_effect = Some((x, y, opacity));
                return;
            }
        }
        self.last_effect = self
            .badge_surface
            .present(window, x, y, opacity)
            .then_some((x, y, opacity));
    }
}

struct Interaction {
    care: Care,
    start: u64,
    position: Pos2,
}

impl Interaction {
    fn duration(&self) -> u64 {
        match self.care {
            Care::Pet => 3_400,
            _ => 1_700,
        }
    }

    fn pose(&self, now: u64, scale: f32) -> (Look, f32) {
        let seconds = (now - self.start) as f32 / 1000.0;
        match self.care {
            Care::Feed => {
                let bow = (seconds / 0.50 * std::f32::consts::PI)
                    .min(std::f32::consts::PI)
                    .sin();
                let chewing = ((seconds - 0.38) * 4.6 * std::f32::consts::TAU).cos();
                let chew_weight = ((seconds - 0.38) / 0.16).clamp(0.0, 1.0)
                    * ((1.30 - seconds) / 0.20).clamp(0.0, 1.0);
                (
                    Look::from_direction(
                        0.12 * bow,
                        bow * 0.86 + (1.0 - chewing) * 0.36 * chew_weight,
                    ),
                    -0.8 * chew_weight * (1.0 + chewing) * scale,
                )
            }
            Care::Pet => {
                let lean = (std::f32::consts::PI * (seconds / 3.4).min(1.0))
                    .sin()
                    .max(0.0);
                let rub = 0.75 + 0.25 * (seconds * 17.0).sin();
                (
                    Look::from_direction(-lean * rub, -0.85 * lean),
                    -2.7 * lean * scale,
                )
            }
            Care::Play => (Look::default(), 0.0),
        }
    }
}
#[derive(Clone, Copy)]
struct BallThrow {
    home: Pos2,
    launch: Pos2,
    ball_offset: Vec2,
    distance: f32,
    scale: f32,
}

impl BallThrow {
    fn new(dock: Dock, position: Pos2, size: Vec2, power: f32) -> Self {
        let scale = dock.scale();
        let launch = dock.clamp(position, size.x, size.y);
        let left = launch.x - dock.work.left as f32;
        let right = dock.work.right as f32 - size.x - launch.x;
        let to_right = right >= left;
        let paw_x = size.x * if to_right { 0.76 } else { 0.24 };
        let range = 190.0 + 330.0 * power.clamp(0.0, 1.0);
        let distance = if to_right { right } else { left }.clamp(0.0, range * scale);
        Self {
            home: position,
            launch,
            ball_offset: vec2(paw_x - 15.0 * scale, size.y - 28.0 * scale),
            distance: if to_right { distance } else { -distance },
            scale,
        }
    }

    fn travel(self, seconds: f32) -> f32 {
        let flight = (seconds / 0.95).clamp(0.0, 1.0);
        let roll = ((seconds - 0.95) / 1.0).clamp(0.0, 1.0);
        self.distance * (0.72 * flight + 0.28 * (1.0 - (1.0 - roll).powi(2)))
    }

    fn ball(self, seconds: f32) -> Pos2 {
        let flight = (seconds / 0.95).clamp(0.0, 1.0);
        let roll = ((seconds - 0.95) / 1.0).clamp(0.0, 1.0);
        let arc = (1.0 - flight) * (17.0 + 228.0 * flight);
        let bounce = (roll * std::f32::consts::TAU).sin().abs() * (1.0 - roll) * 12.0;
        self.launch + self.ball_offset + vec2(self.travel(seconds), -(arc + bounce) * self.scale)
    }

    fn cat_target(self, seconds: f32) -> Pos2 {
        self.launch + vec2(self.travel(seconds - HUNT_DELAY), 0.0)
    }

    fn catch_point(self) -> Pos2 {
        self.launch + vec2(self.distance, 0.0)
    }

    fn settled_ball(self) -> Pos2 {
        self.catch_point() + self.ball_offset
    }
}

#[derive(Clone, Copy)]
enum FetchPhase {
    Chase,
    PickUp(u64),
    Turn(u64),
    Return,
    Drop(u64),
    Celebrate(u64),
}

struct Fetch {
    throw: BallThrow,
    start: u64,
    phase: FetchPhase,
}

impl Fetch {
    fn new(throw: BallThrow, now: u64) -> Self {
        Self {
            throw,
            start: now,
            phase: FetchPhase::Chase,
        }
    }

    fn advance(&mut self, now: u64, position: Pos2) {
        let at = |target| position.distance(target) < 0.5;
        self.phase = match self.phase {
            FetchPhase::Chase if now - self.start >= 2_290 && at(self.throw.catch_point()) => {
                FetchPhase::PickUp(now)
            }
            FetchPhase::PickUp(start) if now - start >= 190 => FetchPhase::Turn(now),
            FetchPhase::Turn(start) if now - start >= FETCH_TURN_TIME => FetchPhase::Return,
            FetchPhase::Return if at(self.throw.home) => FetchPhase::Drop(now),
            FetchPhase::Drop(start) if now - start >= 570 => FetchPhase::Celebrate(now),
            phase => phase,
        };
    }

    fn finished(&self, now: u64) -> bool {
        matches!(self.phase, FetchPhase::Celebrate(start) if now - start >= 880)
    }

    fn target(&self, now: u64) -> Pos2 {
        match self.phase {
            FetchPhase::Chase => self.throw.cat_target((now - self.start) as f32 / 1000.0),
            FetchPhase::PickUp(_) | FetchPhase::Turn(_) => self.throw.catch_point(),
            FetchPhase::Return | FetchPhase::Drop(_) | FetchPhase::Celebrate(_) => self.throw.home,
        }
    }

    fn standing_pose(&self, now: u64) -> Option<(usize, PoseDetail)> {
        let direction = self.throw.distance.signum();
        Some(match self.phase {
            FetchPhase::PickUp(start) => {
                let run = if direction < 0.0 {
                    RUN_LEFT[4]
                } else {
                    RUN_RIGHT[4]
                };
                let settle = (now - start) as f32 / 190.0;
                let detail = PoseDetail::default().with_blend(
                    body_turn_frame((direction * 6.0) as i8),
                    sample(settle, &[0.0, 1.0]),
                );
                (run, detail)
            }
            FetchPhase::Turn(start) => {
                let sectors = (now - start) as f32 / (FETCH_TURN_TIME as f32 / 12.0);
                turn_pose(direction * (6.0 - sectors.min(12.0)))
            }
            FetchPhase::Drop(_) | FetchPhase::Celebrate(_) => (FRONT, PoseDetail::default()),
            _ => return None,
        })
    }

    fn feedback(&self) -> &'static str {
        match self.phase {
            FetchPhase::Chase => Care::Play.feedback(),
            FetchPhase::PickUp(_) | FetchPhase::Turn(_) | FetchPhase::Return => {
                "공을 물고 돌아와요!"
            }
            FetchPhase::Drop(_) | FetchPhase::Celebrate(_) => "공을 가져왔어요!",
        }
    }

    fn celebration(&self, now: u64, scale: f32) -> Option<(f32, f32)> {
        let FetchPhase::Celebrate(start) = self.phase else {
            return None;
        };
        let age = (now - start) as f32;
        let envelope = (age / 100.0).clamp(0.0, 1.0) * ((880.0 - age) / 150.0).clamp(0.0, 1.0);
        let tail = (age / 285.0 * std::f32::consts::TAU).sin() * envelope;
        let hop = ((age - 260.0) / 390.0).clamp(0.0, 1.0);
        Some((tail, -18.0 * scale * 4.0 * hop * (1.0 - hop)))
    }

    fn ball(&self, now: u64, mouth: Pos2) -> Option<(Pos2, u8)> {
        let ease = |elapsed: u64, duration: f32| {
            let t = (elapsed as f32 / duration).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        Some(match self.phase {
            FetchPhase::Chase => (self.throw.ball((now - self.start) as f32 / 1000.0), 255),
            FetchPhase::PickUp(start) => (
                self.throw
                    .settled_ball()
                    .lerp(mouth, ease(now - start, 190.0)),
                255,
            ),
            FetchPhase::Turn(_) | FetchPhase::Return => (mouth, 255),
            FetchPhase::Drop(start) => {
                let ground = pos2(mouth.x, self.throw.home.y + self.throw.ball_offset.y);
                let age = now - start;
                let position = mouth.lerp(ground, ease(age, 200.0));
                let opacity = ((1.0 - ease(age.saturating_sub(320), 250.0)) * 255.0) as u8;
                (position, opacity)
            }
            FetchPhase::Celebrate(_) => return None,
        })
    }
}

struct Desktop {
    tray: Tray,
    pet: Window,
    effect: Window,
    sprites: Arc<Sprites>,
    drawing: Drawing,
    game: Game,
    name: Arc<str>,
    saves: SaveWorker,
    last_requested_save: SavedPet,
    save_failed: bool,
    next_save: u64,
    next_probe: u64,
    slow_timer: bool,
    last_frame: Instant,
    animation_time: f32,
    gaze: Option<Pos2>,
    head_look: Vec2,
    cursor_motion: CursorMotion,
    cursor_seat: CursorSeat,
    mouse: MouseActivity,
    lane: LaneMotion,
    behavior: Behavior,
    transition: PoseTransition,
    grab: Option<Grab>,
    drop_monitor: Option<POINT>,
    landing: Option<Landing>,
    placement_hold: Option<Pos2>,
    watching: bool,
    kneading: bool,
    dock: Dock,
    refresh_dock: bool,
    position: Pos2,
    gait: Gait,
    interaction: Option<Interaction>,
    pending_care: VecDeque<(Care, f32)>,
    fetch: Option<Fetch>,
    last_cat_input: Option<u64>,
    last_fast_typing: Option<u64>,
    last_wake_click: Option<u64>,
    panel_window: HWND,
    panel_anchor: Option<[i32; 4]>,
    effect_visible: bool,
    commands: Receiver<Command>,
    latest_status: Arc<Mutex<Status>>,
    ctx: egui::Context,
    last_status: Option<Status>,
}

impl Desktop {
    fn new(
        sprites: Arc<Sprites>,
        commands: Receiver<Command>,
        latest_status: Arc<Mutex<Status>>,
        ctx: egui::Context,
        panel_window: HWND,
    ) -> io::Result<Self> {
        let instance = unsafe { GetModuleHandleW(null()) };
        register(PET_CLASS, Some(pet_proc), instance)?;
        register(EFFECT_CLASS, Some(DefWindowProcW), instance)?;
        TASKBAR_CREATED.store(
            unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
            Ordering::Relaxed,
        );
        let save = Save::new();
        let saved = save.load();
        let pet = Window(create_layer(
            wide(PET_CLASS).as_ptr(),
            wide(&saved.name).as_ptr(),
            instance,
            false,
        )?);
        let effect = Window(create_layer(
            wide(EFFECT_CLASS).as_ptr(),
            wide("옥키 효과").as_ptr(),
            instance,
            true,
        )?);
        typing::register(pet.0)?;
        let tray = Tray::new(pet.0, instance);
        let dock = Dock::primary()?;
        let drawing = Drawing::new(&sprites, dock.dpi)?;
        let width = drawing.width as f32;
        let initial_x =
            dock.work.left as f32 + ((dock.work.right - dock.work.left) as f32 - width) * 0.79;
        let position = dock.lane(initial_x, width, drawing.height as f32);
        let saves = SaveWorker::start(save)?;
        let now = unsafe { GetTickCount64() };
        let mut desktop = Self {
            tray,
            pet,
            effect,
            sprites,
            drawing,
            game: Game::new(now, saved.stats),
            name: Arc::clone(&saved.name),
            saves,
            last_requested_save: saved,
            save_failed: false,
            next_save: now + 60_000,
            next_probe: now,
            slow_timer: false,
            last_frame: Instant::now(),
            animation_time: 0.0,
            gaze: None,
            head_look: Vec2::ZERO,
            cursor_motion: CursorMotion::default(),
            cursor_seat: CursorSeat::default(),
            mouse: MouseActivity {
                position: None,
                last_move: now,
            },
            lane: LaneMotion::default(),
            behavior: Behavior::default(),
            transition: PoseTransition::default(),
            grab: None,
            drop_monitor: None,
            landing: None,
            placement_hold: None,
            watching: false,
            kneading: false,
            dock,
            refresh_dock: false,
            position,
            gait: Gait::default(),
            interaction: None,
            pending_care: VecDeque::new(),
            fetch: None,
            last_cat_input: None,
            last_fast_typing: None,
            last_wake_click: None,
            panel_window,
            panel_anchor: None,
            effect_visible: false,
            commands,
            latest_status,
            ctx,
            last_status: None,
        };
        if !desktop
            .drawing
            .cat(desktop.pet.0, position, FRONT, 0.0, PoseDetail::default())
        {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            ShowWindow(desktop.pet.0, SW_SHOWNOACTIVATE);
            if SetTimer(desktop.pet.0, TIMER_ID, 15, None) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        desktop.publish(now);
        Ok(desktop)
    }

    fn run(mut self) -> io::Result<()> {
        let result = loop {
            let mut message: MSG = unsafe { zeroed() };
            match unsafe { GetMessageW(&mut message, 0, 0, 0) } {
                -1 => break Err(io::Error::last_os_error()),
                0 => break Ok(()),
                _ => {}
            }
            if message.message == WM_TIMER
                && message.hwnd == self.pet.0
                && message.wParam == TIMER_ID
            {
                match self.tick() {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(error) => break Err(error),
                }
            } else if message.hwnd == 0 {
                match message.message {
                    CAT_PRESS | CAT_MOVE | CAT_RELEASE => {
                        let now = unsafe { GetTickCount64() };
                        let pointer =
                            pos2(message.wParam as i32 as f32, message.lParam as i32 as f32);
                        match message.message {
                            CAT_PRESS => {
                                self.grab =
                                    Some(Grab::new(pointer, self.position, primary_button_down()));
                            }
                            CAT_MOVE => self.drag_to(pointer, now),
                            _ => self.release_grab(pointer, now),
                        }
                    }
                    SHOW_STATUS => self.open(),
                    SHOW_MENU => self.menu(),
                    SLEEP_CAT => {
                        let now = unsafe { GetTickCount64() };
                        self.game.sleep();
                        self.next_probe = now;
                        self.publish(now);
                    }
                    RESTORE_TRAY => {
                        self.tray.add();
                        self.refresh_dock = true;
                    }
                    REFRESH_DOCK => self.refresh_dock = true,
                    _ => {}
                }
            } else {
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        };
        self.game.spend_fullness_on_activity(typing::take_hunger());
        self.save();
        result
    }

    fn open(&self) {
        let mut rect = unsafe { zeroed() };
        if unsafe { GetWindowRect(self.pet.0, &mut rect) } != 0 {
            panel::open(self.panel_window, rect);
        }
    }

    fn drag_to(&mut self, pointer: Pos2, now: u64) {
        let Some(grab) = &mut self.grab else { return };
        let was_lifted = grab.lifted_at.is_some();
        if let Some(position) = grab.move_to(pointer, now, self.dock.scale()) {
            if !was_lifted {
                self.landing = None;
                self.fetch = None;
                self.interaction = None;
                self.pending_care.clear();
                self.cursor_seat.pause = None;
            }
            self.position = position;
        }
    }

    fn release_grab(&mut self, pointer: Pos2, now: u64) {
        self.drag_to(pointer, now);
        let Some(grab) = self.grab.take() else { return };
        self.last_cat_input = Some(now);
        if let Some(carried_at) = grab.lifted_at {
            let point = POINT {
                x: pointer.x as i32,
                y: pointer.y as i32,
            };
            self.dock = Dock::taskbar_at(point).unwrap_or(self.dock);
            self.drop_monitor = Some(point);
            self.landing = Some(Landing {
                from: self.position,
                started: now,
                carried_at,
                contacted_at: None,
            });
            self.placement_hold = Some(pointer);
            self.gait = Gait::default();
            if self.game.mode() == Mode::Awake {
                self.behavior.shaken(grab.shakes, now);
            }
            self.lane = LaneMotion::default();
            self.panel_anchor = None;
        } else {
            let awake = self.game.click_cat();
            self.publish(now);
            if awake {
                self.open();
            } else {
                self.last_wake_click = Some(now);
            }
        }
    }

    fn follow_panel(&mut self, panel_open: bool) {
        if !panel_open {
            self.panel_anchor = None;
            return;
        }
        let Some(position) = self.drawing.last_position else {
            return;
        };
        let anchor = [
            position.x,
            position.y,
            position.x + self.drawing.width as i32,
            position.y + self.drawing.height as i32,
        ];
        if self.panel_anchor != Some(anchor) {
            let [left, top, right, bottom] = anchor;
            panel::follow(
                self.panel_window,
                RECT {
                    left,
                    top,
                    right,
                    bottom,
                },
            );
            self.panel_anchor = Some(anchor);
        }
    }

    fn menu(&self) {
        unsafe {
            let menu = CreatePopupMenu();
            if menu == 0 {
                return;
            }
            AppendMenuW(
                menu,
                MF_STRING,
                STATUS_ID as usize,
                wide("상태 열기").as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(
                menu,
                MF_STRING,
                SLEEP_CAT_ID as usize,
                wide("재우기").as_ptr(),
            );
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, EXIT_ID as usize, wide("종료").as_ptr());
            let mut cursor = zeroed::<POINT>();
            GetCursorPos(&mut cursor);
            SetForegroundWindow(self.pet.0);
            TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON,
                cursor.x,
                cursor.y,
                0,
                self.pet.0,
                null(),
            );
            PostMessageW(self.pet.0, WM_NULL, 0, 0);
            DestroyMenu(menu);
        }
    }

    fn save(&mut self) {
        let pet = SavedPet {
            stats: self.game.stats,
            name: Arc::clone(&self.name),
        };
        if pet != self.last_requested_save || self.save_failed {
            self.save_failed = !self.saves.request(pet.clone());
            self.last_requested_save = pet;
        }
    }

    fn publish(&mut self, now: u64) {
        let status = Status {
            name: Arc::clone(&self.name),
            stats: self.game.stats,
            mode: self.game.mode(),
            wake_clicks: self.game.wake_clicks(),
            feedback: self
                .interaction
                .as_ref()
                .map(|effect| effect.care.feedback())
                .or_else(|| self.fetch.as_ref().map(Fetch::feedback)),
            save_failed: self.save_failed,
            watching: self.watching,
            kneading: self.kneading,
            can_care: self.game.can_care(),
            cooldown_seconds: self.game.cooldown_seconds(now),
        };
        if self.last_status.as_ref() != Some(&status) {
            self.last_status = Some(status.clone());
            *self.latest_status.lock().unwrap() = status;
            if unsafe { IsWindowVisible(self.panel_window) } != 0 {
                self.ctx.request_repaint();
            }
        }
    }

    fn start_care(&mut self, care: Care, now: u64, throw_power: f32) {
        if !self.game.care(care, now) {
            return;
        }
        self.save();
        if care == Care::Play && self.landing.is_none() {
            self.launch_ball(now, throw_power);
        } else {
            self.pending_care.push_back((care, throw_power));
        }
    }

    fn launch_ball(&mut self, now: u64, power: f32) {
        let size = vec2(self.drawing.width as f32, self.drawing.height as f32);
        let center = self.position + size * 0.5;
        let dock = Dock::at(POINT {
            x: center.x as i32,
            y: center.y as i32,
        })
        .unwrap_or(self.dock);
        self.fetch = Some(Fetch::new(
            BallThrow::new(dock, self.position, size, power),
            now,
        ));
        if let Some(interaction) = self.interaction.take() {
            self.pending_care.push_front((interaction.care, 0.0));
        }
    }

    fn tick(&mut self) -> io::Result<bool> {
        let now = unsafe { GetTickCount64() };
        let hunger = typing::take_hunger();
        if hunger > 0.0 {
            self.last_fast_typing = Some(now);
            self.game.spend_fullness_on_activity(hunger);
        }
        let mut cursor: POINT = unsafe { zeroed() };
        if unsafe { GetCursorPos(&mut cursor) } != 0 {
            self.mouse
                .observe(pos2(cursor.x as f32, cursor.y as f32), now);
        }
        let frame = Instant::now();
        let dt = (frame - self.last_frame).as_secs_f32().min(0.067);
        self.last_frame = frame;
        self.animation_time = (self.animation_time + dt) % 600.0;
        if let Some(result) = self.saves.results.try_iter().last() {
            self.save_failed = result.is_err();
        }
        if now >= self.next_probe {
            self.next_probe = now + 150;
            let idle = self
                .mouse
                .idle(now)
                .min(self.last_cat_input.map_or(u64::MAX, |last| now - last));
            self.game.tick(now, idle);
            let sleeping = matches!(self.game.mode(), Mode::Dozing | Mode::Asleep);
            if sleeping != self.slow_timer {
                unsafe { SetTimer(self.pet.0, TIMER_ID, if sleeping { 30 } else { 15 }, None) };
                self.slow_timer = sleeping;
            }
            if !sleeping {
                if let Some(gaze) = foreground_target() {
                    self.gaze = gaze;
                }
            }
        }
        if std::mem::take(&mut self.refresh_dock) {
            self.dock = self
                .drop_monitor
                .map_or_else(Dock::primary, Dock::taskbar_at)
                .unwrap_or(self.dock);
            self.panel_anchor = None;
        }
        loop {
            match self.commands.try_recv() {
                Ok(Command::Care(care)) => self.start_care(care, now, 0.0),
                Ok(Command::ThrowBall(power)) => self.start_care(Care::Play, now, power),
                Ok(Command::Rename(name)) => {
                    if let Some(name) = valid_name(&name).filter(|&name| name != self.name.as_ref())
                    {
                        self.name = name.into();
                        unsafe { SetWindowTextW(self.pet.0, wide(name).as_ptr()) };
                        self.save();
                    }
                }
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => return Ok(false),
                Err(TryRecvError::Empty) => break,
            }
        }
        if now >= self.next_save {
            self.next_save = now + 60_000;
            self.save();
        }
        if self.game.mode() == Mode::Asleep {
            self.fetch = None;
            self.interaction = None;
            self.pending_care.clear();
        } else if let Some(care) = self
            .interaction
            .as_ref()
            .filter(|effect| now - effect.start >= effect.duration())
            .map(|effect| effect.care)
        {
            self.interaction = None;
            if care == Care::Pet && self.game.stats.percentages()[1] >= 82 {
                self.behavior.relax(now);
            }
        }
        self.animate(now, dt)?;
        self.publish(now);
        Ok(true)
    }

    fn animate(&mut self, now: u64, dt: f32) -> io::Result<()> {
        self.kneading = false;
        let mouse = self.mouse.position.unwrap_or(self.position);
        let panel_open = unsafe { IsWindowVisible(self.panel_window) } != 0;
        if self.grab.as_ref().is_some_and(|grab| grab.physical) {
            if primary_button_down() {
                self.drag_to(mouse, now);
            } else {
                self.release_grab(mouse, now);
                unsafe {
                    if GetCapture() == self.pet.0 {
                        ReleaseCapture();
                    }
                }
            }
        }
        if let Some(grab) = &self.grab {
            if let Some(start) = grab.lifted_at {
                let (frame, detail) = if self.game.mode() == Mode::Asleep {
                    let age = now - start;
                    let rise = sample(age as f32 / 150.0, &[0.0, 0.68, 1.0]);
                    (
                        sleep_carry_frame(age),
                        PoseDetail::default().with_blend(SLEEPING, 1.0 - rise),
                    )
                } else {
                    let frame = Cycle::Carry.frame(((now - start) / 25) as usize);
                    self.transition
                        .frame(now, Stance::Carry, frame)
                        .unwrap_or((frame, PoseDetail::default()))
                };
                self.drawing
                    .cat(self.pet.0, self.position, frame, 0.0, detail);
                self.follow_panel(panel_open);
                self.watching = false;
                if self.effect_visible {
                    unsafe { ShowWindow(self.effect.0, SW_HIDE) };
                    self.effect_visible = false;
                }
            }
            return Ok(());
        }
        if let Some(mut landing) = self.landing {
            if self.drawing.dpi != self.dock.dpi {
                self.drawing = Drawing::new(&self.sprites, self.dock.dpi)?;
            }
            let target = self.dock.landing_target(
                landing.from,
                self.drawing.width as f32,
                self.drawing.height as f32,
            );
            let (position, progress) = landing.position(now, target, self.dock.scale());
            self.position = position;
            let sleeping = self.game.mode() == Mode::Asleep;
            let (frame, detail, finished) = if sleeping {
                landing.sleep_pose(now, progress)
            } else {
                let carried = Cycle::Carry.frame(((now - landing.carried_at) / 25) as usize);
                let (frame, detail) = clip_pose(
                    progress,
                    &[carried, transition_keyframe(LIFT_MID), LIFT_MID, FRONT],
                );
                (frame, detail, progress >= 1.0)
            };
            self.drawing.cat(self.pet.0, position, frame, 0.0, detail);
            self.follow_panel(panel_open);
            self.landing = (!finished).then_some(landing);
            if finished {
                self.transition = PoseTransition {
                    pose: if sleeping { Stance::Sleep } else { Stance::Sit },
                    last_frame: Some(frame),
                    ..PoseTransition::default()
                };
            }
            return Ok(());
        }
        if let Some(fetch) = &mut self.fetch {
            fetch.advance(now, self.position);
            if fetch.finished(now) {
                self.fetch = None;
            }
        }
        if self.fetch.is_none() && self.interaction.is_none() {
            if let Some((care, power)) = self.pending_care.pop_front() {
                if care == Care::Play {
                    self.launch_ball(now, power);
                } else {
                    if care == Care::Pet {
                        unsafe {
                            PlaySoundA(PURR.as_ptr(), 0, SND_MEMORY | SND_ASYNC | SND_NODEFAULT)
                        };
                    }
                    self.interaction = Some(Interaction {
                        care,
                        start: now,
                        position: self.position,
                    });
                }
            }
        }
        let fetch = self.fetch.as_ref();
        let care = self.interaction.as_ref();
        let mode = self.game.mode();
        let mouse_over_panel = panel_open
            && unsafe {
                let mut panel_rect: RECT = zeroed();
                GetWindowRect(self.panel_window, &mut panel_rect) != 0
                    && PtInRect(
                        &panel_rect,
                        POINT {
                            x: mouse.x as i32,
                            y: mouse.y as i32,
                        },
                    ) != 0
            };
        let dock = self.dock;
        if self.drawing.dpi != dock.dpi {
            self.drawing = Drawing::new(&self.sprites, dock.dpi)?;
        }
        let scale = fetch.map_or(dock.scale(), |fetch| fetch.throw.scale);
        let width = self.drawing.width as f32;
        let height = self.drawing.height as f32;
        let seconds = self.animation_time;
        let mut gaze = self.gaze;
        let mut pointer_gaze = false;
        let mut speed = 410.0 * scale;
        let awake_on_lane = mode == Mode::Awake && fetch.is_none() && care.is_none();
        let placement_held = self.placement_hold.is_some_and(|point| {
            self.mouse.idle(now) < 8_000 && mouse.distance(point) < SIT_RESUME_DISTANCE * scale
        });
        if !placement_held {
            self.placement_hold = None;
        }
        let can_walk_on_lane = awake_on_lane && !mouse_over_panel && !placement_held;
        let [fullness, mood] = self.game.stats.percentages();
        let action = if awake_on_lane {
            let nearby = self.mouse.position.filter(|point| {
                can_walk_on_lane
                    && (point.x - self.position.x - width * 0.5).abs() <= 265.0 * scale
                    && point.y < self.position.y + height * 0.37
            });
            self.behavior.update(
                now,
                Signals {
                    mouse: nearby,
                    mouse_still: self.mouse.idle(now) >= 620,
                    fullness,
                    mood,
                    fast_typing: self.last_fast_typing.is_some_and(|time| now - time < 1_800),
                    desktop_visible: self.gaze.is_none() && !panel_open && !placement_held,
                    approach_radius: SIT_RESUME_DISTANCE * scale,
                },
            )
        } else {
            Action::Rest
        };
        self.kneading = matches!(action, Action::KneadTaskbar | Action::KneadDesktop);
        let action_progress = self.behavior.progress(now);
        let steer_on_lane =
            can_walk_on_lane && matches!(action, Action::Wander | Action::Approach(_));
        let mut target = dock.lane(self.position.x, width, height);
        if action == Action::KneadDesktop {
            let offset = desktop_knead_extension(action_progress) * 52.0 * scale;
            let into_desktop = if dock.edge == ABE_TOP {
                offset
            } else {
                -offset
            };
            target = dock.clamp(target + vec2(0.0, into_desktop), width, height);
        }
        let mut follow_target = None;
        if steer_on_lane {
            let bounds = [
                dock.work.left as f32,
                (dock.work.right as f32 - width).max(dock.work.left as f32),
            ];
            let pointer_x = match action {
                Action::Approach(point) => Some(point.x - width * 0.5),
                _ => None,
            };
            target.x = self
                .lane
                .target(self.position.x, pointer_x, bounds, scale, now);
            if let Some(x) = pointer_x {
                gaze = Some(mouse);
                pointer_gaze = true;
                follow_target = Some(dock.lane(x, width, height));
            }
        }
        if let Some(fetch) = fetch {
            target = fetch.target(now);
            gaze = None;
            speed = 490.0 * scale;
        } else if let Some(care) = care {
            target = care.position;
            gaze = None;
        }
        let sitting = self.cursor_seat.update(
            now,
            mouse,
            follow_target
                .filter(|_| mouse.y < self.position.y + height * 0.37)
                .map(|destination| {
                    self.position.distance(destination) <= 7.0 * scale
                        && self.lane.velocity.abs() <= 14.0 * scale
                }),
            scale,
        );
        if sitting {
            target = self.position;
            self.lane.velocity = 0.0;
        }
        let delta = target - self.position;
        let distance = delta.length();
        let max_step = speed * dt;
        let mut movement = if steer_on_lane && !sitting && delta.y.abs() < 0.5 {
            let x = self.lane.step(self.position.x, target.x, scale, dt);
            let destination = dock.lane(self.position.x + x, width, height);
            destination - self.position
        } else if distance > max_step {
            self.lane.velocity = 0.0;
            delta * (max_step / distance)
        } else {
            self.lane.velocity = 0.0;
            delta
        };
        let standing = fetch.and_then(|fetch| fetch.standing_pose(now));
        let walking = movement.x.abs() > 0.12 * scale
            && mode == Mode::Awake
            && !self.kneading
            && standing.is_none();
        let mut frame = FRONT;
        let mut bob = 0.0;
        let mut detail = PoseDetail::default();
        let mut stance = Stance::Upright;
        let mut authored_turn = false;
        if let Some(pose) = standing {
            (frame, detail) = pose;
            if let Some(fetch) = fetch.filter(|fetch| matches!(fetch.phase, FetchPhase::Turn(_))) {
                self.gait.yaw = -6.0 * fetch.throw.distance.signum();
                self.gait.stride_blend = 0.0;
            }
        } else if walking {
            (frame, bob, detail) = self.gait.step(movement / scale, dt, fetch.is_some());
            bob *= scale;
            if fetch.is_none() && self.gait.stride_blend == 0.0 {
                movement.x *= 0.30;
            }
            authored_turn = true;
        } else if awake_on_lane && self.gait.yaw != 0.0 {
            (frame, detail) = self.gait.pivot(0.0, dt);
            if self.gait.yaw == 0.0 {
                stance = Stance::Sit;
            }
            authored_turn = true;
        } else if sitting {
            frame = SITTING_UP;
            detail = detail
                .with_blink(self.cursor_seat.blink(now))
                .with_body(0.0, breath(seconds, 2.6) * 0.20);
            stance = Stance::Sit;
        } else if awake_on_lane && pointer_gaze {
            stance = Stance::Sit;
        } else if awake_on_lane {
            (frame, stance, detail) = behavior_pose(action, action_progress, seconds);
            authored_turn = self.kneading;
        }
        self.position += movement;
        self.game.walked_sideways(movement.x.abs() / scale);
        let mut render_position = self.position;
        if let Some(fetch) = fetch {
            let elapsed = (now - fetch.start) as f32 / 1000.0;
            if matches!(fetch.phase, FetchPhase::Chase) && elapsed < HUNT_DELAY {
                (frame, detail) = self.gait.pivot(6.0 * fetch.throw.distance.signum(), dt);
                let progress = elapsed / HUNT_DELAY;
                let wobble = (progress * std::f32::consts::TAU * 1.5).sin()
                    * sample(progress, &[0.0, 0.08, 1.0, 0.75, 0.0]);
                render_position.x += wobble * 2.8 * scale;
                bob = -wobble.abs() * 1.6 * scale;
            }
        }
        if let Some((tail, hop)) = fetch.and_then(|fetch| fetch.celebration(now, scale)) {
            detail = detail.with_tail(tail);
            render_position.y += hop;
        }
        self.watching = mode == Mode::Awake
            && !walking
            && !authored_turn
            && !pointer_gaze
            && fetch.is_none()
            && care.is_none()
            && gaze.is_some();
        if mode == Mode::Asleep {
            self.gait = Gait::default();
            frame = SLEEPING;
            stance = Stance::Sleep;
            detail = detail.with_body(0.0, breath(seconds, 2.9) * 0.18);
            if let Some(time) = self.last_wake_click.filter(|&time| now - time < 370) {
                bob -= ((now - time) as f32 / 370.0 * std::f32::consts::PI).sin() * 8.0 * scale;
            }
        } else if mode == Mode::Dozing {
            frame = LOWER_MID;
            stance = Stance::Low;
            let nod = phase((seconds / 3.8).fract(), 24);
            detail = detail.with_body(
                sample(nod, &[0.0, 0.12, 0.27, 0.06, 0.0]),
                sample(nod, &[0.0, -0.25, -0.63, 0.15, 0.0]),
            );
        }
        let head_moves = !authored_turn
            && (matches!(frame, FRONT | SITTING_UP) || awake_on_lane && pointer_gaze);
        let direction = if head_moves {
            let center = self.position + vec2(width * 0.49, height * 0.53);
            gaze.map_or(Vec2::ZERO, |point| {
                let range = width * if pointer_gaze { 0.64 } else { 0.85 };
                self.cursor_motion.heading(point - center, range, dt)
            })
        } else {
            Vec2::ZERO
        };
        self.head_look += (direction - self.head_look) * (1.0 - (-10.0 * dt).exp());
        if let Some(care) = care {
            self.gait = Gait::default();
            frame = FRONT;
            (detail.look, bob) = care.pose(now, scale);
            stance = Stance::Sit;
            let progress = (now - care.start) as f32 / care.duration() as f32;
            if care.care == Care::Pet {
                let [left, middle, right] = behavior_keyframes(KNEADING);
                (frame, detail) = clip_pose(
                    progress,
                    &[
                        FRONT, middle, left, middle, right, middle, left, middle, right, right,
                        middle,
                    ],
                );
                let rub = sample(progress, &[0.0, 0.0, -0.10, -0.28, 0.0]);
                detail = detail.with_body(rub, breath(seconds, 0.8) * 0.21);
            } else {
                let progress = phase(progress, 32);
                detail = detail.with_body(0.0, sample(progress, &[0.0, -0.45, -0.18, 0.22, 0.0]));
            }
        } else if head_moves && fetch.is_none() {
            detail.look = Look::from_direction(self.head_look.x, self.head_look.y);
        }
        if authored_turn || fetch.is_some() {
            self.transition = PoseTransition {
                pose: stance,
                last_frame: Some(frame),
                active: None,
            };
        } else if let Some((middle, middle_detail)) = self.transition.frame(now, stance, frame) {
            detail = middle_detail;
            frame = middle;
            bob = 0.0;
        }
        self.drawing
            .cat(self.pet.0, render_position, frame, bob, detail);

        let effect = if let Some(fetch) = fetch {
            let mouth = self
                .drawing
                .ball_at_mouth(render_position, frame, bob, detail);
            fetch
                .ball(now, mouth)
                .map(|(ball, opacity)| (Badge::Ball, ball, opacity))
        } else if let Some(interaction) = care {
            let age = ((now - interaction.start) as f32 / 1650.0).min(1.0);
            if interaction.care == Care::Feed {
                let mouth =
                    self.drawing
                        .ball_at_mouth(self.position, FRONT, bob, PoseDetail::default());
                let progress = (age / 0.27).min(1.0);
                let opacity = ((1.0 - ((age - 0.23) / 0.14).clamp(0.0, 1.0)) * 255.0) as u8;
                (opacity > 0).then_some((
                    Badge::Fish,
                    mouth + vec2(34.0, 16.0) * scale * (1.0 - progress),
                    opacity,
                ))
            } else {
                (age < 1.0).then_some((
                    Badge::Heart,
                    self.position + vec2(width * 0.69, height * 0.11 - age * 36.0 * scale),
                    ((1.0 - age * age) * 255.0) as u8,
                ))
            }
        } else if mode == Mode::Asleep {
            let rise = (now % 2400) as f32 / 2400.0;
            Some((
                Badge::Sleep,
                self.position
                    + vec2(
                        width * 0.73 + rise * 16.0 * scale,
                        height * 0.18 - rise * 34.0 * scale,
                    ),
                ((1.0 - rise) * 255.0) as u8,
            ))
        } else {
            None
        };
        if let Some((badge, position, opacity)) = effect {
            self.drawing.effect(self.effect.0, badge, position, opacity);
        }
        if self.effect_visible != effect.is_some() {
            self.effect_visible = effect.is_some();
            unsafe {
                if self.effect_visible {
                    SetWindowPos(
                        self.effect.0,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
                    );
                } else {
                    ShowWindow(self.effect.0, SW_HIDE);
                }
            }
        }
        self.follow_panel(panel_open);
        Ok(())
    }
}

fn breath(seconds: f32, period: f32) -> f32 {
    (phase((seconds / period).fract(), 24) * std::f32::consts::TAU).sin()
}

fn clip_pose(progress: f32, frames: &[usize]) -> (usize, PoseDetail) {
    let position = progress.clamp(0.0, 1.0) * (frames.len() - 1) as f32;
    let index = position.floor() as usize;
    let frame = frames[index];
    let detail = PoseDetail::default();
    let next = frames.get(index + 1).copied().unwrap_or(frame);
    if next == frame {
        return (frame, detail);
    }
    let tween = phase(position.fract(), 8);
    (frame, detail.with_blend(next, sample(tween, &[0.0, 1.0])))
}

fn turn_pose(yaw: f32) -> (usize, PoseDetail) {
    let sector = yaw.floor() as i8;
    let frame = body_turn_frame(sector);
    let amount = yaw - sector as f32;
    let detail = PoseDetail::default().with_blend(body_turn_frame(sector + 1), amount);
    (frame, detail)
}

const DESKTOP_TURN_PORTION: f32 = 0.16;

fn desktop_knead_extension(progress: f32) -> f32 {
    let edge = progress.min(1.0 - progress) / DESKTOP_TURN_PORTION;
    sample(edge, &[0.0, 1.0])
}

fn behavior_pose(action: Action, progress: f32, seconds: f32) -> (usize, Stance, PoseDetail) {
    let swell = sample(progress, &[0.0, 0.85, 1.0, 0.7, 0.0]);
    match action {
        Action::Greet => {
            let [start, middle, high] = behavior_keyframes(GREETING);
            let (frame, detail) = clip_pose(progress, &[start, middle, high, high, middle, start]);
            (
                frame,
                Stance::Upright,
                detail.with_body(swell * 0.22, swell * 0.37),
            )
        }
        Action::Belly => {
            let [start, middle, paws_up] = behavior_keyframes(BELLY_UP);
            let (frame, detail) = clip_pose(
                progress,
                &[start, middle, paws_up, middle, paws_up, middle, start],
            );
            let detail = detail.with_body(
                sample(progress, &[0.0, -0.32, 0.15, -0.17, 0.0]),
                breath(seconds, 1.15) * swell * 0.36,
            );
            (frame, Stance::Belly, detail)
        }
        Action::Keyboard => {
            let [start, middle, chin_down] = behavior_keyframes(KEYBOARD_LOAF);
            let (frame, detail) = clip_pose(
                progress,
                &[start, middle, chin_down, chin_down, middle, start],
            );
            (
                frame,
                Stance::Low,
                detail.with_body(0.0, breath(seconds, 1.7) * swell * 0.22 - swell * 0.18),
            )
        }
        Action::EarsBack => {
            let [low, middle, flick] = behavior_keyframes(EARS_BACK);
            let (frame, detail) = clip_pose(
                progress,
                &[low, middle, flick, low, flick, low, middle, low],
            );
            (
                frame,
                Stance::Alarm,
                detail.with_body(-0.25 * swell, -0.42 * swell),
            )
        }
        Action::Arch => {
            let [start, middle, high] = behavior_keyframes(ARCHING);
            let (frame, detail) = clip_pose(progress, &[start, middle, high, middle, start]);
            (
                frame,
                Stance::Alarm,
                detail.with_body(-0.20 * swell, swell * 0.68),
            )
        }
        Action::KneadTaskbar => {
            let [ready, left, right, settled] = TASKBAR_KNEAD;
            let (frame, detail) = clip_pose(
                progress,
                &[
                    FRONT, ready, left, ready, right, ready, left, ready, right, settled, settled,
                    FRONT,
                ],
            );
            (
                frame,
                Stance::Sit,
                detail.with_paws(breath(seconds, 0.74) * swell * 0.13),
            )
        }
        Action::KneadDesktop => {
            let (frame, detail) = if (DESKTOP_TURN_PORTION..1.0 - DESKTOP_TURN_PORTION)
                .contains(&progress)
            {
                let [ready, left, right, settled] = DESKTOP_KNEAD;
                let knead = (progress - DESKTOP_TURN_PORTION) / (1.0 - 2.0 * DESKTOP_TURN_PORTION);
                clip_pose(
                    knead,
                    &[
                        ready, left, ready, right, ready, left, ready, right, settled, ready,
                    ],
                )
            } else {
                turn_pose(11.0 * desktop_knead_extension(progress))
            };
            (frame, Stance::Sit, detail)
        }
        _ => (
            FRONT,
            Stance::Sit,
            PoseDetail::default()
                .with_body(0.0, breath(seconds, 3.0) * 0.17)
                .with_blink(sample(
                    (seconds % 5.5 - 4.1) / 1.25,
                    &[0.0, 0.68, 0.96, 0.62, 0.0],
                )),
        ),
    }
}

#[derive(Default)]
struct PoseTransition {
    pose: Stance,
    last_frame: Option<usize>,
    active: Option<StanceChange>,
}

struct StanceChange {
    lead: [usize; 3],
    start: u64,
    duration: u64,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Stance {
    #[default]
    Upright,
    Sit,
    Low,
    Belly,
    Alarm,
    Sleep,
    Carry,
}

impl PoseTransition {
    fn frame(&mut self, now: u64, pose: Stance, destination: usize) -> Option<(usize, PoseDetail)> {
        if self.pose != pose {
            let previous = self.pose;
            self.pose = pose;
            let (middle, reversed, duration) = match (previous, pose) {
                (_, Stance::Carry) => (LIFT_MID, false, 500),
                (Stance::Carry, _) => (LIFT_MID, true, 500),
                (Stance::Sleep, _) => (STRETCH_MID, false, 750),
                (_, Stance::Sleep) => (LOWER_MID, false, 680),
                (Stance::Belly, _) | (_, Stance::Belly) => (ROLL_MID, pose == Stance::Belly, 660),
                (Stance::Alarm, _) | (_, Stance::Alarm) => (ALERT_MID, pose == Stance::Alarm, 500),
                (_, Stance::Low) | (Stance::Low, _) => (LOWER_MID, pose == Stance::Low, 570),
                _ => (SIT_MID, pose == Stance::Sit, 470),
            };
            let extra = transition_keyframe(middle);
            let [first, second] = if pose == Stance::Sleep {
                [SLEEP_BEND, SLEEP_TUCK]
            } else if reversed {
                [extra, middle]
            } else {
                [middle, extra]
            };
            let source = self.last_frame.unwrap_or(FRONT);
            self.active = Some(StanceChange {
                lead: [source, first, second],
                start: now,
                duration,
            });
        }
        let Some(change) = &self.active else {
            self.last_frame = Some(destination);
            return None;
        };
        let age = now - change.start;
        if age >= change.duration {
            self.active = None;
            self.last_frame = Some(destination);
            return None;
        }
        let progress = age as f32 / change.duration as f32;
        let lift = sample(progress, &[0.0, -0.28, 0.35, 0.0]);
        let lean = sample(progress, &[0.0, 0.15, -0.13, 0.0]);
        let [source, first, second] = change.lead;
        let (frame, detail) = clip_pose(progress, &[source, first, second, destination]);
        self.last_frame = Some(
            detail
                .blend
                .filter(|&(_, opacity)| opacity >= 128)
                .map_or(frame, |(next, _)| next),
        );
        Some((frame, detail.with_body(lean, lift)))
    }
}

struct Grab {
    pressed: Pos2,
    initial: Pos2,
    physical: bool,
    lifted_at: Option<u64>,
    last_sample: Pos2,
    direction: Option<(Vec2, u64)>,
    shakes: u8,
}

fn sleep_carry_frame(age: u64) -> usize {
    Cycle::SleepingCarry.frame((age / 55) as usize)
}

#[derive(Clone, Copy)]
struct Landing {
    from: Pos2,
    started: u64,
    carried_at: u64,
    contacted_at: Option<u64>,
}

impl Landing {
    fn sleep_pose(&mut self, now: u64, descent: f32) -> (usize, PoseDetail, bool) {
        if descent < 1.0 {
            self.contacted_at = None;
            let carry = sleep_carry_frame(now - self.carried_at);
            let reach = ((descent - 0.65) / 0.35).max(0.0);
            let (frame, detail) = clip_pose(reach, &[carry, SLEEP_CONTACT]);
            return (frame, detail, false);
        }
        const SETTLE_TIME: u64 = 720;
        let contact = *self.contacted_at.get_or_insert(now);
        let elapsed = now - contact;
        let (frame, detail) = clip_pose(
            elapsed as f32 / SETTLE_TIME as f32,
            &[
                SLEEP_CONTACT,
                SLEEP_CONTACT,
                SLEEP_BEND,
                SLEEP_TUCK,
                SLEEP_CURL,
                SLEEPING,
            ],
        );
        (frame, detail, elapsed >= SETTLE_TIME)
    }

    fn position(self, now: u64, target: Pos2, scale: f32) -> (Pos2, f32) {
        let distance = self.from.distance(target);
        if distance < 0.5 {
            return (target, 1.0);
        }
        let duration = (180.0 + distance / scale * 0.60).clamp(220.0, 850.0);
        let progress = ((now - self.started) as f32 / duration).min(1.0);
        let eased = progress * progress * (3.0 - 2.0 * progress);
        (self.from.lerp(target, eased), progress)
    }
}

impl Grab {
    fn new(pointer: Pos2, position: Pos2, physical: bool) -> Self {
        Self {
            pressed: pointer,
            initial: position,
            physical,
            lifted_at: None,
            last_sample: pointer,
            direction: None,
            shakes: 0,
        }
    }

    fn move_to(&mut self, pointer: Pos2, now: u64, scale: f32) -> Option<Pos2> {
        let moved = pointer - self.pressed;
        if self.lifted_at.is_none() && moved.length() < 6.0 * scale {
            return None;
        }
        self.lifted_at.get_or_insert(now);
        let sample = pointer - self.last_sample;
        if sample.length() >= 23.0 * scale {
            let direction = sample.normalized();
            if self.direction.is_some_and(|(previous, time)| {
                now - time <= 400 && previous.dot(direction) < -0.65
            }) {
                self.shakes = self.shakes.saturating_add(1);
            }
            self.direction = Some((direction, now));
            self.last_sample = pointer;
        }
        Some(self.initial + moved)
    }
}

#[derive(Default)]
struct CursorSeat {
    pause: Option<CursorPause>,
}

struct CursorPause {
    anchor: Pos2,
    since: u64,
    sitting: bool,
}

impl CursorSeat {
    fn blink(&self, now: u64) -> f32 {
        let Some(pause) = &self.pause else { return 0.0 };
        let age = now - pause.since;
        let progress = ((age as f32 - 650.0) / 650.0).clamp(0.0, 1.0);
        (progress * std::f32::consts::PI).sin().max(0.0)
    }

    fn update(&mut self, now: u64, mouse: Pos2, arrived: Option<bool>, scale: f32) -> bool {
        let Some(arrived) = arrived else {
            self.pause = None;
            return false;
        };
        if let Some(pause) = &mut self.pause {
            if mouse.distance(pause.anchor) < SIT_RESUME_DISTANCE * scale
                && (pause.sitting || arrived)
            {
                pause.sitting |= now - pause.since >= SIT_SETTLE_TIME;
                return pause.sitting;
            }
        }
        self.pause = arrived.then_some(CursorPause {
            anchor: mouse,
            since: now,
            sitting: false,
        });
        false
    }
}

#[derive(Default)]
struct MouseActivity {
    position: Option<Pos2>,
    last_move: u64,
}

impl MouseActivity {
    fn observe(&mut self, point: Pos2, now: u64) {
        if self.position != Some(point) {
            self.position = Some(point);
            self.last_move = now;
        }
    }

    fn idle(&self, now: u64) -> u64 {
        now - self.last_move
    }
}

#[derive(Default)]
struct LaneMotion {
    velocity: f32,
    steering: bool,
    waypoint: Option<f32>,
    next_wander: u64,
    random: RandomState,
}

impl LaneMotion {
    fn target(
        &mut self,
        x: f32,
        pointer: Option<f32>,
        bounds: [f32; 2],
        scale: f32,
        now: u64,
    ) -> f32 {
        let [left, right] = bounds;
        if let Some(pointer) = pointer {
            self.waypoint = None;
            self.next_wander = 0;
            let target = pointer.clamp(left, right);
            let threshold = if self.steering { 6.0 } else { 19.0 };
            self.steering = (target - x).abs() > threshold * scale;
            return if self.steering { target } else { x };
        }

        self.steering = false;
        let random = self.random.hash_one(now);
        if let Some(waypoint) = self.waypoint {
            let waypoint = waypoint.clamp(left, right);
            if (waypoint - x).abs() > 5.0 * scale {
                return waypoint;
            }
            self.waypoint = None;
            self.next_wander = now + 1_400 + random % 1_800;
        }
        if now >= self.next_wander {
            let left_room = x - left;
            let right_room = right - x;
            let to_right = if left_room.min(right_room) < 105.0 * scale {
                right_room >= left_room
            } else {
                random & 1 == 0
            };
            let distance = (135.0 + ((random >> 1) % 186) as f32) * scale;
            let target = (x + if to_right { distance } else { -distance }).clamp(left, right);
            self.waypoint = Some(target);
            return target;
        }
        x
    }

    fn step(&mut self, x: f32, target: f32, scale: f32, dt: f32) -> f32 {
        let gap = target - x;
        let speed = if self.steering { 112.0 } else { 65.0 } * scale;
        let desired = (gap * 4.4).clamp(-speed, speed);
        self.velocity += (desired - self.velocity) * (1.0 - (-10.0 * dt).exp());
        let step = self.velocity * dt;
        if gap != 0.0 && step.signum() == gap.signum() && step.abs() >= gap.abs() {
            self.velocity = 0.0;
            gap
        } else {
            step
        }
    }
}

#[derive(Default)]
struct CursorMotion {
    sector: Option<Heading>,
    turn: Turn,
}

impl CursorMotion {
    fn heading(&mut self, delta: Vec2, range: f32, dt: f32) -> Vec2 {
        let strength = ((delta.length() / range) - 0.06).clamp(0.0, 1.0);
        if strength == 0.0 {
            return Vec2::ZERO;
        }
        let sector = self
            .sector
            .filter(|previous| previous.retains(delta))
            .unwrap_or_else(|| Heading::nearest(delta));
        self.sector = Some(sector);
        self.turn.toward(sector, dt).unit() * strength
    }
}

#[derive(Default)]
struct Gait {
    yaw: f32,
    phase: f32,
    stride_blend: f32,
}

impl Gait {
    fn pivot(&mut self, target: f32, dt: f32) -> (usize, PoseDetail) {
        let step = (dt / BODY_TURN_STEP).min(1.0);
        let delta = target - self.yaw;
        self.yaw = if delta.abs() <= step {
            target
        } else {
            self.yaw + step * delta.signum()
        };
        self.stride_blend = 0.0;
        turn_pose(self.yaw)
    }

    fn step(&mut self, movement: Vec2, dt: f32, running: bool) -> (usize, f32, PoseDetail) {
        let target = 6.0 * movement.x.signum();
        if self.yaw != target {
            let (frame, detail) = self.pivot(target, dt);
            return (frame, 0.0, detail);
        }
        let distance = movement.length();
        let (max_speed, stride_length) = if running {
            (192.0, 57.0)
        } else {
            (103.0, 34.0)
        };
        self.phase = (self.phase + distance.min(max_speed * dt) / stride_length) % 1.0;
        let cycle = match (running, target < 0.0) {
            (false, true) => Cycle::WalkLeft,
            (false, false) => Cycle::WalkRight,
            (true, true) => Cycle::RunLeft,
            (true, false) => Cycle::RunRight,
        };
        let stride = (self.phase * CYCLE_FRAMES as f32) as usize;
        let footfall = (self.phase * std::f32::consts::TAU).sin();
        let lift = if running { 2.0 } else { 0.9 };
        self.stride_blend = (self.stride_blend + dt / 0.09).min(1.0);
        let frame = cycle.frame(stride);
        let bob = -footfall.abs() * lift * self.stride_blend;
        if self.stride_blend < 1.0 {
            let detail = PoseDetail::default().with_blend(frame, self.stride_blend);
            return (body_turn_frame(target as i8), bob, detail);
        }
        (frame, bob, PoseDetail::default())
    }
}

fn foreground_target() -> Option<Option<Pos2>> {
    unsafe {
        let window = GetForegroundWindow();
        let mut owner = 0;
        GetWindowThreadProcessId(window, &mut owner);
        if owner == std::process::id() {
            return None;
        }
        let mut rect: RECT = zeroed();
        let valid = window != 0
            && window != GetShellWindow()
            && window != GetDesktopWindow()
            && IsWindowVisible(window) != 0
            && IsIconic(window) == 0
            && GetWindowRect(window, &mut rect) != 0;
        let mut class = [0; 64];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        let shell = matches!(
            String::from_utf16_lossy(&class[..length]).as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        );
        Some((valid && !shell).then(|| {
            pos2(
                (rect.left + rect.right) as f32 / 2.0,
                (rect.top + rect.bottom) as f32 / 2.0,
            )
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walking_uses_travel_direction_and_alternates_feet() {
        let mut gait = Gait {
            yaw: -6.0,
            stride_blend: 1.0,
            ..Gait::default()
        };
        let frames: [usize; CYCLE_FRAMES] =
            std::array::from_fn(|_| gait.step(vec2(-2.125, 0.0), 0.06, false).0);
        let expected = std::array::from_fn(|phase| Cycle::WalkLeft.frame(phase + 1));
        assert_eq!(frames, expected);
        for sector in -5..=6 {
            assert_eq!(
                gait.step(vec2(2.125, 0.0), BODY_TURN_STEP, false).0,
                body_turn_frame(sector)
            );
        }
        let mut sprint = Gait {
            yaw: 6.0,
            stride_blend: 1.0,
            ..Gait::default()
        };
        let frames: [usize; CYCLE_FRAMES] =
            std::array::from_fn(|_| sprint.step(vec2(3.5625, 0.0), 0.05, true).0);
        let expected = std::array::from_fn(|phase| Cycle::RunRight.frame(phase + 1));
        assert_eq!(frames, expected);
    }

    #[test]
    fn walking_from_front_uses_each_authored_yaw_in_both_directions() {
        for sign in [-1, 1] {
            let mut gait = Gait::default();
            for sector in 1..=6 {
                let (frame, _, _) =
                    gait.step(vec2(2.125 * f32::from(sign), 0.0), BODY_TURN_STEP, false);
                assert_eq!(frame, body_turn_frame(sign * sector));
            }
        }
    }

    #[test]
    fn ball_is_thrown_toward_the_longer_lane_and_the_cat_follows() {
        let work = RECT {
            left: -900,
            top: 0,
            right: 100,
            bottom: 720,
        };
        let dock = Dock {
            work,
            edge: ABE_BOTTOM,
            dpi: 96,
        };
        let size = vec2(146.0, 166.0);
        for (x, sign) in [(-820.0, 1.0), (-115.0, -1.0)] {
            let cat = pos2(x, 550.0);
            let throw = BallThrow::new(dock, cat, size, 1.0);
            let tap = BallThrow::new(dock, cat, size, 0.0);
            assert!(throw.distance.abs() > tap.distance.abs());
            assert_eq!(throw.distance.signum(), sign);
            assert_eq!(throw.cat_target(0.1), cat);
            assert!(throw.ball(0.35).y < throw.ball(0.0).y);
            assert!(throw.ball(0.35).y < throw.ball(2.2).y);
            let caught = throw.cat_target(2.2);
            assert_eq!((caught.x - x).signum(), sign);
            assert!(caught.x >= work.left as f32 && caught.x + size.x <= work.right as f32);
        }
    }

    #[test]
    fn fetch_keeps_the_ball_at_the_mouth_until_the_cat_returns_home() {
        let work = RECT {
            left: -900,
            top: 0,
            right: 100,
            bottom: 720,
        };
        let dock = Dock {
            work,
            edge: ABE_BOTTOM,
            dpi: 96,
        };
        let home = pos2(-823.5, 551.25);
        let throw = BallThrow::new(dock, home, vec2(146.0, 166.0), 1.0);
        let caught = throw.catch_point();
        let mouth = caught + vec2(12.0, 96.0);
        let mut fetch = Fetch::new(throw, 0);

        fetch.advance(2_500, home);
        assert!(matches!(fetch.phase, FetchPhase::Chase));
        fetch.advance(2_600, caught);
        assert!(matches!(fetch.phase, FetchPhase::PickUp(_)));
        assert_eq!(fetch.ball(2_600, mouth), Some((throw.settled_ball(), 255)));
        let halfway = fetch.ball(2_695, mouth).unwrap().0;
        assert!(halfway.y < throw.settled_ball().y && halfway.y > mouth.y);
        fetch.advance(2_790, caught);
        assert_eq!(fetch.ball(2_790, mouth), Some((mouth, 255)));
        for offset in 0..12 {
            let pose = fetch
                .standing_pose(2_790 + offset * (FETCH_TURN_TIME / 12))
                .unwrap();
            assert_eq!(pose.0, body_turn_frame(6 - offset as i8));
        }
        fetch.advance(2_790 + FETCH_TURN_TIME, caught);
        assert_eq!(fetch.target(2_790 + FETCH_TURN_TIME), home);

        let halfway_home = home.lerp(caught, 0.5);
        fetch.advance(4_000, halfway_home);
        let moving_mouth = halfway_home + vec2(12.0, 93.0);
        assert_eq!(fetch.ball(4_000, moving_mouth), Some((moving_mouth, 255)));
        assert!(!fetch.finished(4_000));

        fetch.advance(4_100, home);
        let home_mouth = home + vec2(67.0, 93.0);
        assert_eq!(fetch.ball(4_100, home_mouth), Some((home_mouth, 255)));
        assert_eq!(
            fetch.ball(4_310, home_mouth).unwrap().0,
            pos2(home_mouth.x, home.y + throw.ball_offset.y),
        );
        fetch.advance(4_670, home);
        assert_eq!(fetch.ball(4_670, home_mouth), None);
        assert!(fetch.celebration(4_760, 1.0).unwrap().0 > 0.2);
        assert_eq!(fetch.celebration(4_870, 1.0).unwrap().1, 0.0);
        assert!(fetch.celebration(5_125, 1.0).unwrap().1 < -17.0);
        assert_eq!(fetch.celebration(5_400, 1.0).unwrap().1, 0.0);
        assert!(fetch.finished(5_550));
    }

    #[test]
    fn mouse_idle_changes_only_when_the_pointer_moves() {
        let mut mouse = MouseActivity::default();
        let point = pos2(440.0, 25.0);
        mouse.observe(point, 40);
        mouse.observe(point, 5_000);
        assert_eq!(mouse.idle(8_040), 8_000);
        mouse.observe(point + Vec2::X, 8_041);
        assert_eq!(mouse.idle(8_041), 0);
        let mut lane = LaneMotion::default();
        assert_eq!(
            lane.target(100.0, Some(440.0), [0.0, 900.0], 1.0, 40),
            440.0
        );
        assert!(lane.step(100.0, 440.0, 1.0, 0.015) > 0.0);
        assert!(lane.target(0.0, None, [0.0, 900.0], 1.0, 8_040) > 0.0);
    }

    #[test]
    fn cursor_look_has_twenty_four_stable_sectors() {
        let vector = |degrees: f32| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            vec2(cos, sin) * 100.0
        };
        let mut motion = CursorMotion::default();
        for sector in 0..24 {
            let direction = vector(sector as f32 * 15.0);
            motion.heading(direction, 100.0, 0.02);
            assert_eq!(motion.sector, Some(Heading::nearest(direction)));
        }
        motion.heading(vector(0.0), 100.0, 0.02);
        motion.heading(vector(8.5), 100.0, 0.02);
        assert_eq!(motion.sector, Some(Heading::nearest(vector(0.0))));
        motion.heading(vector(11.0), 100.0, 0.02);
        assert_eq!(motion.sector, Some(Heading::nearest(vector(15.0))));
    }

    #[test]
    fn the_cat_waits_before_sitting_and_stays_seated_through_small_pointer_motion() {
        let mut seat = CursorSeat::default();
        let mouse = pos2(500.0, 200.0);
        assert!(!seat.update(0, mouse, Some(false), 1.0));
        assert!(!seat.update(100, mouse, Some(true), 1.0));
        assert!(!seat.update(319, mouse, Some(true), 1.0));
        assert!(seat.update(320, mouse, Some(true), 1.0));
        assert!(seat.update(350, mouse + vec2(23.0, 0.0), Some(false), 1.0));
        assert!(!seat.update(360, mouse + vec2(24.0, 0.0), Some(false), 1.0));
        assert!(!seat.update(370, mouse, None, 1.0));
    }

    #[test]
    fn picking_up_preserves_the_grip_offset_and_has_a_transition_pose() {
        let pointer = pos2(300.0, 400.0);
        let position = pos2(251.0, 370.0);
        let mut grab = Grab::new(pointer, position, false);
        assert_eq!(grab.move_to(pointer + vec2(3.0, 0.0), 0, 1.0), None);
        assert_eq!(
            grab.move_to(pointer + vec2(30.0, -20.0), 15, 1.0),
            Some(position + vec2(30.0, -20.0))
        );
        let mut transition = PoseTransition::default();
        let carry = Cycle::Carry.frame(0);
        let first = transition.frame(15, Stance::Carry, carry).unwrap();
        let second = transition.frame(195, Stance::Carry, carry).unwrap();
        let lifted = transition.frame(375, Stance::Carry, carry).unwrap();
        assert_eq!(first.0, FRONT);
        assert_eq!(second.0, LIFT_MID);
        assert_eq!(lifted.0, transition_keyframe(LIFT_MID));
        assert!(first.1 != second.1);
        assert!(transition.frame(515, Stance::Carry, carry).is_none());
        assert_eq!(transition.frame(530, Stance::Sit, FRONT).unwrap().0, carry);
    }

    #[test]
    fn sleep_landing_plants_the_feet_before_bending_and_curling() {
        let mut landing = Landing {
            from: pos2(150.0, 200.0),
            started: 100,
            carried_at: 50,
            contacted_at: None,
        };
        let target = pos2(150.0, 500.0);
        let (airborne, descent) = landing.position(300, target, 1.0);
        assert!(airborne.y < target.y);
        let (frame, detail, finished) = landing.sleep_pose(300, descent);
        assert_ne!(frame, SLEEPING);
        assert!(detail.blend.map_or(true, |(next, _)| next == SLEEP_CONTACT));
        assert!(!finished);
        assert_eq!(landing.position(500, target, 1.0), (target, 1.0));
        assert_eq!(landing.sleep_pose(500, 1.0).0, SLEEP_CONTACT);
        assert_eq!(landing.sleep_pose(600, 1.0).0, SLEEP_CONTACT);
        assert_eq!(landing.sleep_pose(800, 1.0).0, SLEEP_BEND);
        assert_eq!(landing.sleep_pose(950, 1.0).0, SLEEP_TUCK);
        assert_eq!(landing.sleep_pose(1100, 1.0).0, SLEEP_CURL);
        let (frame, _, finished) = landing.sleep_pose(1220, 1.0);
        assert_eq!(frame, SLEEPING);
        assert!(finished);
    }

    #[test]
    fn behaviors_draw_at_least_eight_different_frames() {
        for action in [
            Action::Greet,
            Action::Belly,
            Action::Keyboard,
            Action::EarsBack,
            Action::Arch,
            Action::KneadTaskbar,
            Action::KneadDesktop,
        ] {
            let mut poses = Vec::new();
            for step in 0..=72 {
                let progress = step as f32 / 72.0;
                let (frame, _, detail) = behavior_pose(action, progress, progress * 3.0);
                if !poses.contains(&(frame, detail)) {
                    poses.push((frame, detail));
                }
            }
            assert!(
                poses.len() >= 8,
                "{action:?} draws only {} frames",
                poses.len()
            );
        }
    }

    #[test]
    fn care_actions_have_distinct_head_and_body_motion() {
        let feed = Interaction {
            care: Care::Feed,
            start: 0,
            position: Pos2::ZERO,
        };
        let pet = Interaction {
            care: Care::Pet,
            ..feed
        };
        assert!(feed.pose(250, 1.0).0 != Look::default());
        assert!(pet.pose(550, 1.0).0 != Look::default());
        assert!(pet.pose(550, 1.0).1 < -1.0);
        assert!(feed.pose(550, 1.0).0 != pet.pose(550, 1.0).0);
    }
}
