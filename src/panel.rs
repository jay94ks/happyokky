use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontFamily, FontId, Rect, RichText, Stroke, Vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::core::w;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, InvalidateRect, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::game::{valid_name, Care, Mode, NAME_LIMIT, WAKE_CLICKS};
use crate::native::{self, Command, Status};
use crate::render::Sprites;
use crate::{APP_NAME, CAT_NAME};

const PANEL_WIDTH: f32 = 360.0;
const PANEL_HEIGHT: f32 = 278.0;
const THROW_CHARGE_SECONDS: f64 = 1.0;
const OPEN_PANEL: u32 = WM_APP + 1;
const SHUTDOWN_PANEL: u32 = WM_APP + 2;
const FINISH_STARTUP: u32 = WM_APP + 3;
const MOVE_PANEL: u32 = WM_APP + 4;
const PANEL_SUBCLASS: usize = 1;

struct Palette {
    paper: Color32,
    header: Color32,
    control: Color32,
    ink: Color32,
    muted: Color32,
    accents: [Color32; 3],
}

impl Palette {
    fn system(dark: bool) -> Self {
        let rgb = Color32::from_rgb;
        if dark {
            Self {
                paper: rgb(25, 25, 37),
                header: rgb(36, 38, 58),
                control: rgb(53, 56, 80),
                ink: rgb(240, 240, 250),
                muted: rgb(177, 181, 205),
                accents: [rgb(146, 177, 255), rgb(247, 140, 184), rgb(195, 168, 255)],
            }
        } else {
            Self {
                paper: rgb(253, 252, 255),
                header: rgb(238, 240, 255),
                control: rgb(222, 228, 250),
                ink: rgb(35, 42, 70),
                muted: rgb(78, 86, 114),
                accents: [rgb(70, 111, 213), rgb(195, 77, 126), rgb(117, 91, 189)],
            }
        }
    }
}

fn tint(base: Color32, accent: Color32, amount: f32) -> Color32 {
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * amount).round() as u8;
    Color32::from_rgb(
        mix(base.r(), accent.r()),
        mix(base.g(), accent.g()),
        mix(base.b(), accent.b()),
    )
}

pub fn run(sprites: Arc<Sprites>) -> eframe::Result {
    let icon = sprites.app_icon();
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([PANEL_WIDTH, PANEL_HEIGHT])
        .with_resizable(false)
        .with_decorations(false)
        .with_taskbar(false)
        .with_always_on_top()
        // eframe 0.29 briefly shows the window after its first paint.
        .with_position([-32_000.0, -32_000.0])
        .with_visible(false)
        .with_icon(egui::IconData {
            width: icon.width(),
            height: icon.height(),
            rgba: icon.as_raw().clone(),
        });
    let options = eframe::NativeOptions {
        viewport,
        vsync: true,
        ..Default::default()
    };
    let (command_tx, command_rx) = mpsc::channel();
    let latest_status = Arc::new(Mutex::new(Status::default()));
    let mut desktop = None;
    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(|creation| {
            let raw = creation.window_handle()?.as_raw();
            let RawWindowHandle::Win32(handle) = raw else {
                return Err(io::Error::other("Windows window handle unavailable").into());
            };
            let hwnd = handle.hwnd.get();
            if unsafe { SetWindowSubclass(hwnd, Some(panel_proc), PANEL_SUBCLASS, 0) } == 0 {
                return Err(io::Error::last_os_error().into());
            }
            let ctx = creation.egui_ctx.clone();
            configure(&ctx);
            let avatar = sprites.avatar(104);
            let texture = ctx.load_texture(
                CAT_NAME,
                egui::ColorImage::from_rgba_unmultiplied([104, 104], avatar.as_raw()),
                egui::TextureOptions::LINEAR,
            );
            let corner = DWMWCP_ROUND;
            unsafe {
                DwmSetWindowAttribute(
                    hwnd,
                    DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                    (&corner as *const i32).cast(),
                    std::mem::size_of_val(&corner) as u32,
                );
            }
            let desktop_status = Arc::clone(&latest_status);
            desktop = Some(
                thread::Builder::new()
                    .name("Happy Okky desktop".into())
                    .spawn(move || native::run(sprites, command_rx, desktop_status, ctx, hwnd))?,
            );
            Ok(Box::new(Panel {
                hwnd,
                texture,
                latest_status,
                commands: command_tx,
                status: Status::default(),
                name_draft: None,
                starting: true,
                pressed_care: None,
            }))
        }),
    );
    if let Some(thread) = desktop {
        let _ = thread.join();
    }
    result
}

fn configure(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(windows) = std::env::var_os("WINDIR") {
        let font = std::path::PathBuf::from(windows)
            .join("Fonts")
            .join("malgun.ttf");
        if let Ok(data) = std::fs::read(font) {
            fonts
                .font_data
                .insert("Malgun Gothic".into(), egui::FontData::from_owned(data));
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .push("Malgun Gothic".into());
        }
    }
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::ThemePreference::System);
}

struct Panel {
    hwnd: HWND,
    texture: egui::TextureHandle,
    latest_status: Arc<Mutex<Status>>,
    commands: Sender<Command>,
    status: Status,
    name_draft: Option<String>,
    starting: bool,
    pressed_care: Option<(Care, f64)>,
}

impl Panel {
    fn hide(&mut self) {
        self.name_draft = None;
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    fn name(&mut self, ui: &mut egui::Ui, area: Rect, palette: &Palette) {
        let editor = ui.id().with("cat-name-editor");
        if let Some(draft) = &mut self.name_draft {
            let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
            let response = ui.put(
                area,
                egui::TextEdit::singleline(draft)
                    .id(editor)
                    .font(FontId::proportional(20.0))
                    .text_color(palette.ink)
                    .char_limit(NAME_LIMIT)
                    .desired_width(area.width())
                    .vertical_align(egui::Align::Center),
            );
            if enter {
                if let Some(name) = valid_name(draft) {
                    let _ = self.commands.send(Command::Rename(name.to_owned()));
                    self.name_draft = None;
                    response.surrender_focus();
                } else {
                    response.request_focus();
                }
            } else if response.lost_focus() {
                self.name_draft = None;
            }
            return;
        }

        let text = RichText::new(self.status.name.as_ref())
            .size(24.0)
            .color(palette.ink);
        let label = egui::Label::new(text)
            .truncate()
            .sense(egui::Sense::click());
        let response = ui
            .put(area, label)
            .on_hover_text("더블클릭해서 이름 바꾸기");
        if response.double_clicked() {
            let draft = self.status.name.to_string();
            let mut state = egui::text_edit::TextEditState::default();
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(draft.chars().count()),
                )));
            egui::TextEdit::store_state(ui.ctx(), editor, state);
            ui.memory_mut(|memory| memory.request_focus(editor));
            self.name_draft = Some(draft);
            ui.ctx().request_repaint();
        }
    }

    fn content(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, palette: &Palette) {
        let origin = ui.max_rect().min;
        let rect = |x, y, w, h| Rect::from_min_size(origin + vec2(x, y), vec2(w, h));
        let painter = ui.painter().clone();
        let header = rect(14.0, 14.0, 332.0, 80.0);
        painter.rect_filled(header, 16.0, palette.header);
        painter.circle_filled(origin + vec2(59.0, 54.0), 31.0, palette.control);
        painter.image(
            self.texture.id(),
            rect(31.0, 26.0, 56.0, 56.0),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        self.name(ui, rect(102.0, 27.0, 203.0, 34.0), palette);
        let [blue, pink, purple] = palette.accents;
        let (mood, color) = match self.status.mode {
            Mode::Dozing => ("꾸벅꾸벅 조는 중", purple),
            Mode::Asleep => ("깊이 잠든 중", purple),
            Mode::Awake if self.status.kneading => ("꾹꾹이하는 중", pink),
            Mode::Awake if self.status.watching => ("작업하는 모습을 보는 중", blue),
            Mode::Awake => ("작업 표시줄을 산책 중", blue),
        };
        painter.circle_filled(origin + vec2(106.0, 70.0), 3.0, color);
        painter.text(
            origin + vec2(115.0, 69.0),
            Align2::LEFT_CENTER,
            mood,
            FontId::proportional(13.0),
            palette.muted,
        );
        let close = ui
            .put(
                rect(304.0, 23.0, 30.0, 28.0),
                egui::Button::new(RichText::new("×").size(21.0).color(palette.ink))
                    .fill(palette.control)
                    .stroke(Stroke::NONE)
                    .rounding(8.0),
            )
            .on_hover_text("닫기 (Esc)");
        if close.clicked() {
            self.hide();
        }

        for (index, ((label, color), value)) in [("포만감", blue), ("기분", pink)]
            .into_iter()
            .zip(self.status.stats.percentages())
            .enumerate()
        {
            let y = 108.0 + index as f32 * 40.0;
            painter.text(
                origin + vec2(22.0, y),
                Align2::LEFT_TOP,
                label,
                FontId::proportional(14.0),
                palette.ink,
            );
            painter.text(
                origin + vec2(338.0, y),
                Align2::RIGHT_TOP,
                format!("{value}%"),
                FontId::proportional(13.0),
                palette.muted,
            );
            let track = rect(22.0, y + 22.0, 316.0, 6.0);
            painter.rect_filled(track, 3.0, palette.control);
            if value > 0 {
                painter.rect_filled(
                    Rect::from_min_size(
                        track.min,
                        vec2(track.width() * value as f32 / 100.0, track.height()),
                    ),
                    3.0,
                    color,
                );
            }
        }

        let footnote = if self.status.save_failed {
            "상태를 저장하지 못했어요. 잠시 후 다시 시도해요.".into()
        } else if self.status.mode == Mode::Asleep {
            format!(
                "고양이를 클릭해서 깨워 주세요.  {}/{WAKE_CLICKS}",
                self.status.wake_clicks
            )
        } else if let Some(feedback) = self.status.feedback {
            feedback.into()
        } else {
            "고양이를 누른 채 끌어서 옮길 수 있어요.".into()
        };
        painter.text(
            origin + vec2(22.0, 195.0),
            Align2::LEFT_TOP,
            footnote,
            FontId::proportional(13.0),
            palette.muted,
        );

        let (time, down, pressed, released) = ctx.input(|input| {
            let mut pressed = None;
            let mut released = None;
            for event in &input.events {
                if let egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: is_press,
                    ..
                } = event
                {
                    if *is_press {
                        pressed = Some(*pos);
                    } else {
                        released = Some(*pos);
                    }
                }
            }
            (input.time, input.pointer.primary_down(), pressed, released)
        });
        ui.scope(|ui| {
            for (index, (care, seconds)) in Care::ALL
                .into_iter()
                .zip(self.status.cooldown_seconds)
                .enumerate()
            {
                let edge = palette.accents[index];
                let fill = tint(palette.paper, edge, 0.15);
                let hover = tint(palette.paper, edge, 0.25);
                let widgets = &mut ui.visuals_mut().widgets;
                widgets.inactive.weak_bg_fill = fill;
                widgets.inactive.bg_stroke = Stroke::NONE;
                widgets.hovered.weak_bg_fill = hover;
                widgets.hovered.bg_stroke = Stroke::new(1.0, edge);
                widgets.active.weak_bg_fill = hover;
                widgets.active.bg_stroke = Stroke::new(1.0, edge);
                let enabled = self.status.can_care && seconds == 0;
                let button = egui::Button::new("")
                    .min_size(vec2(100.0, 38.0))
                    .rounding(10.0);
                let response = ui
                    .add_enabled_ui(enabled, |ui| {
                        ui.put(
                            rect(22.0 + index as f32 * 108.0, 222.0, 100.0, 38.0),
                            button,
                        )
                    })
                    .inner;
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, care.label())
                });
                let tooltip = if care == Care::Play && enabled {
                    "공 던지기 · 길게 누를수록 멀리 던져요"
                } else {
                    care.label()
                };
                let response = response
                    .on_hover_text(tooltip)
                    .on_disabled_hover_text(tooltip);
                let center = response.rect.center();
                let icon = if seconds == 0 {
                    center
                } else {
                    center - vec2(17.0, 0.0)
                };
                let color = if enabled { edge } else { palette.muted };
                paint_care_icon(&painter, care, icon, color, palette.paper);
                if seconds > 0 {
                    painter.text(
                        center + vec2(15.0, 0.0),
                        Align2::CENTER_CENTER,
                        format!("{seconds}초"),
                        FontId::proportional(12.0),
                        palette.muted,
                    );
                }
                if enabled && pressed.is_some_and(|position| response.rect.contains(position)) {
                    self.pressed_care = Some((care, time));
                }
                let began = self.pressed_care.filter(|(action, _)| *action == care);
                let completed = enabled
                    && began.is_some()
                    && released.is_some_and(|position| response.rect.contains(position));
                let activate = response.clicked() || completed;
                if care == Care::Play {
                    let held = enabled && down && began.is_some();
                    if held {
                        ctx.request_repaint_after(std::time::Duration::from_millis(16));
                    }
                    let power = began.map_or(0.0, |(_, start)| {
                        ((time - start) / THROW_CHARGE_SECONDS).clamp(0.0, 1.0) as f32
                    });
                    if held {
                        let bar = Rect::from_min_size(center + vec2(-26.0, 13.0), vec2(52.0, 2.0));
                        painter.rect_filled(bar, 1.0, palette.control);
                        painter.rect_filled(
                            Rect::from_min_size(bar.min, vec2(52.0 * power, 2.0)),
                            1.0,
                            edge,
                        );
                    }
                    if activate {
                        let _ = self.commands.send(Command::ThrowBall(power));
                    }
                } else if activate {
                    let _ = self.commands.send(Command::Care(care));
                }
            }
        });
        if !down {
            self.pressed_care = None;
        }
    }
}

fn paint_care_icon(
    painter: &egui::Painter,
    care: Care,
    center: egui::Pos2,
    ink: Color32,
    highlight: Color32,
) {
    let p = |x, y| center + vec2(x, y);
    match care {
        Care::Feed => {
            painter.add(egui::Shape::convex_polygon(
                vec![p(-10.0, -2.0), p(10.0, -2.0), p(7.0, 7.0), p(-7.0, 7.0)],
                ink,
                Stroke::NONE,
            ));
            for (x, y) in [(-5.0, -5.5), (0.0, -7.0), (5.0, -5.5)] {
                painter.circle_filled(p(x, y), 2.0, ink);
            }
            painter.line_segment([p(-4.0, 3.0), p(4.0, 3.0)], Stroke::new(1.4, highlight));
        }
        Care::Pet => {
            painter.circle_filled(p(-4.3, -3.4), 5.2, ink);
            painter.circle_filled(p(4.3, -3.4), 5.2, ink);
            painter.add(egui::Shape::convex_polygon(
                vec![p(-9.0, -1.0), p(9.0, -1.0), p(0.0, 9.0)],
                ink,
                Stroke::NONE,
            ));
        }
        Care::Play => {
            painter.circle_filled(center, 8.5, ink);
            painter.circle_filled(p(-2.5, -2.8), 2.1, highlight);
            painter.line_segment([p(1.4, 5.1), p(5.1, 1.9)], Stroke::new(1.15, highlight));
        }
    }
}

impl eframe::App for Panel {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if std::mem::take(&mut self.starting) {
            unsafe { PostMessageW(self.hwnd, FINISH_STARTUP, 0, 0) };
        }
        self.status = self.latest_status.lock().unwrap().clone();
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            && self.name_draft.take().is_none()
        {
            self.hide();
        }
        let palette = Palette::system(ctx.style().visuals.dark_mode);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(palette.paper))
            .show(ctx, |ui| {
                self.content(ctx, ui, &palette);
            });
    }

    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        let _ = self.commands.send(Command::Quit);
    }
}

// Hidden windows may not receive WM_PAINT. Visibility and shutdown must be
// handled on the window thread independently of egui's next frame.
unsafe extern "system" fn panel_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    match message {
        OPEN_PANEL | MOVE_PANEL => {
            let opening = message == OPEN_PANEL;
            if opening {
                SetPropW(window, w!("NyangArchi.Opened"), 1);
            }
            SetWindowPos(
                window,
                HWND_TOPMOST,
                wparam as i32,
                lparam as i32,
                0,
                0,
                SWP_NOSIZE
                    | SWP_NOACTIVATE
                    | if opening {
                        SWP_SHOWWINDOW
                    } else {
                        SWP_NOZORDER
                    },
            );
            if opening {
                SetForegroundWindow(window);
                InvalidateRect(window, std::ptr::null(), 0);
            }
            return 0;
        }
        SHUTDOWN_PANEL => {
            DestroyWindow(window);
            return 0;
        }
        WM_CLOSE => {
            ShowWindow(window, SW_HIDE);
            return 0;
        }
        WM_SYSCOMMAND if (wparam & 0xfff0) == SC_MOVE as usize => return 0,
        FINISH_STARTUP => {
            if GetPropW(window, w!("NyangArchi.Opened")) == 0 {
                ShowWindow(window, SW_HIDE);
            }
            return 0;
        }
        WM_NCDESTROY => {
            RemovePropW(window, w!("NyangArchi.Opened"));
            RemoveWindowSubclass(window, Some(panel_proc), id);
        }
        _ => {}
    }
    DefSubclassProc(window, message, wparam, lparam)
}

pub fn shutdown(window: HWND) {
    unsafe { PostMessageW(window, SHUTDOWN_PANEL, 0, 0) };
}

pub fn open(window: HWND, anchor: RECT) {
    place(window, anchor, OPEN_PANEL);
}

pub fn follow(window: HWND, anchor: RECT) {
    place(window, anchor, MOVE_PANEL);
}

fn place(window: HWND, anchor: RECT, message: u32) {
    unsafe {
        let monitor = MonitorFromRect(&anchor, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let mut dpi = 96;
        let mut dpi_y = 96;
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) < 0 {
            dpi = 96;
        }
        let size: Vec2 = vec2(PANEL_WIDTH, PANEL_HEIGHT) * (dpi as f32 / 96.0);
        let width = size.x.round() as i32;
        let height = size.y.round() as i32;
        let work = info.rcWork;
        let x = (anchor.left + (anchor.right - anchor.left - width) / 2)
            .clamp(work.left, (work.right - width).max(work.left));
        let above = anchor.top - height - 8;
        let y = if above >= work.top {
            above
        } else {
            anchor.bottom + 8
        }
        .clamp(work.top, (work.bottom - height).max(work.top));
        PostMessageW(window, message, x as usize, y as isize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_click_selects_the_name_and_enter_submits_the_unicode_replacement() {
        let ctx = egui::Context::default();
        let (commands, received) = mpsc::channel();
        let mut panel = Panel {
            hwnd: 0,
            texture: ctx.load_texture(
                "test avatar",
                egui::ColorImage::new([1, 1], Color32::WHITE),
                egui::TextureOptions::LINEAR,
            ),
            latest_status: Arc::new(Mutex::new(Status::default())),
            commands,
            status: Status::default(),
            name_draft: None,
            starting: false,
            pressed_care: None,
        };
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(PANEL_WIDTH, PANEL_HEIGHT));
        let mut frame = |time, events| {
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::none())
                    .show(ctx, |ui| {
                        panel.content(ctx, ui, &Palette::system(false));
                    });
            });
        };
        let name = pos2(145.0, 45.0);
        let click = || {
            vec![
                egui::Event::PointerMoved(name),
                egui::Event::PointerButton {
                    pos: name,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: name,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        frame(0.0, vec![]);
        frame(0.1, click());
        assert!(received.try_recv().is_err());
        frame(0.2, click());
        frame(0.22, vec![]);
        frame(0.3, vec![egui::Event::Text("밤톨".into())]);
        frame(
            0.4,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: Some(egui::Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(matches!(received.try_recv(), Ok(Command::Rename(name)) if name == "밤톨"));
    }
}
