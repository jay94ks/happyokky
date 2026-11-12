#![cfg(windows)]

use std::fs;
use std::process::{Child, Command};
use std::ptr::null;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows_sys::core::w;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Shell::ExtractIconExW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct App(Child);

impl Drop for App {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn wait_for<T>(label: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(14);
    loop {
        if let Some(result) = check() {
            return result;
        }
        assert!(Instant::now() < deadline, "{label} timed out");
        sleep(Duration::from_millis(15));
    }
}

fn send(window: HWND, message: u32, wparam: usize) {
    assert_ne!(unsafe { PostMessageW(window, message, wparam, 0) }, 0);
}

fn cat_pointer(window: HWND, message: u32, x: i16, y: i16, pressed: bool) {
    let position = (x as u16 as u32 | ((y as u16 as u32) << 16)) as isize;
    assert_ne!(
        unsafe { PostMessageW(window, message, pressed as usize, position) },
        0
    );
}

fn click_cat(window: HWND) {
    cat_pointer(window, WM_LBUTTONDOWN, 45, 45, true);
    cat_pointer(window, WM_LBUTTONUP, 45, 45, false);
}

fn belongs_to(window: HWND, pid: u32) -> bool {
    let mut owner = 0;
    window != 0 && unsafe { GetWindowThreadProcessId(window, &mut owner) } != 0 && owner == pid
}

fn menu_open(pet: HWND) -> bool {
    unsafe {
        let mut info: GUITHREADINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        let thread = GetWindowThreadProcessId(pet, std::ptr::null_mut());
        GetGUIThreadInfo(thread, &mut info) != 0 && info.hwndMenuOwner == pet
    }
}

fn window_rect(window: HWND) -> RECT {
    let mut bounds = unsafe { std::mem::zeroed() };
    assert_ne!(unsafe { GetWindowRect(window, &mut bounds) }, 0);
    bounds
}

fn care_button(panel: HWND, button_x: i32, hold: Duration) {
    let mut client: RECT = unsafe { std::mem::zeroed() };
    assert_ne!(unsafe { GetClientRect(panel, &mut client) }, 0);
    let x = client.right * button_x / 360;
    let y = client.bottom * 241 / 278;
    let position = ((y << 16) | x) as isize;
    unsafe {
        assert_ne!(PostMessageW(panel, WM_MOUSEMOVE, 0, position), 0);
        assert_ne!(PostMessageW(panel, WM_LBUTTONDOWN, 1, position), 0);
        sleep(hold);
        let release_position = position + 2;
        assert_ne!(PostMessageW(panel, WM_MOUSEMOVE, 1, release_position), 0);
        assert_ne!(PostMessageW(panel, WM_LBUTTONUP, 0, release_position), 0);
    }
}

#[test]
#[ignore = "opens the test application on the current Windows desktop"]
fn desktop_pet_opens_panel_fetches_and_shuts_down() {
    let binary = env!("CARGO_BIN_EXE_happy-okky");
    let path: Vec<u16> = binary.encode_utf16().chain([0]).collect();
    assert_eq!(
        unsafe {
            ExtractIconExW(
                path.as_ptr(),
                -1,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        },
        1,
        "Windows should find the embedded Happy Okky icon group"
    );
    let existing = unsafe { FindWindowW(w!("NyangArchi.Pet"), null()) };
    assert_eq!(
        existing, 0,
        "close the running Happy Okky instance before this test"
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let data = std::env::temp_dir().join(format!("happy-okky-test-{}-{stamp}", std::process::id()));
    let legacy = data.join("OKKY Tamagotchi").join("pet-v1.txt");
    let old_stats = "1 710 600 860\n";
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, old_stats).unwrap();
    let current = data.join("NyangArchi").join("pet.txt");
    let mut app = App(Command::new(binary)
        .env("LOCALAPPDATA", &data)
        .spawn()
        .unwrap());
    let pid = app.0.id();
    let pet = wait_for("pet window", || unsafe {
        let window = FindWindowW(w!("NyangArchi.Pet"), null());
        (belongs_to(window, pid) && IsWindowVisible(window) != 0).then_some(window)
    });
    let panel = wait_for("egui window", || unsafe {
        let window = FindWindowW(null(), w!("행복한 옥키"));
        belongs_to(window, pid).then_some(window)
    });
    sleep(Duration::from_millis(400));
    assert_eq!(unsafe { IsWindowVisible(panel) }, 0);

    for _ in 0..2 {
        click_cat(pet);
        wait_for("panel visible after one click", || {
            (unsafe { IsWindowVisible(panel) } != 0).then_some(())
        });
        let mut bounds: RECT = unsafe { std::mem::zeroed() };
        assert_ne!(unsafe { GetWindowRect(panel, &mut bounds) }, 0);
        let monitor = unsafe {
            MonitorFromPoint(
                POINT {
                    x: bounds.left,
                    y: bounds.top,
                },
                MONITOR_DEFAULTTONEAREST,
            )
        };
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        assert_ne!(unsafe { GetMonitorInfoW(monitor, &mut info) }, 0);
        assert!(bounds.left >= info.rcWork.left && bounds.top >= info.rcWork.top);
        assert!(bounds.right <= info.rcWork.right && bounds.bottom <= info.rcWork.bottom);
        send(panel, WM_CLOSE, 0);
        wait_for("panel hidden", || {
            (unsafe { IsWindowVisible(panel) } == 0).then_some(())
        });
    }

    send(pet, WM_RBUTTONUP, 0);
    wait_for("right-click menu visible", || menu_open(pet).then_some(()));
    send(pet, WM_CANCELMODE, 0);
    wait_for("menu closed", || (!menu_open(pet)).then_some(()));

    send(pet, WM_COMMAND, 4);
    let effect = wait_for("sleep starts from the menu command", || unsafe {
        let effect = FindWindowW(w!("NyangArchi.Effect"), null());
        (belongs_to(effect, pid) && IsWindowVisible(effect) != 0).then_some(effect)
    });
    let before_sleep_drag = window_rect(pet);
    cat_pointer(pet, WM_LBUTTONDOWN, 45, 45, true);
    cat_pointer(pet, WM_MOUSEMOVE, 75, -70, true);
    wait_for("a sleeping cat can be lifted", || {
        (window_rect(pet).top <= before_sleep_drag.top - 95).then_some(())
    });
    cat_pointer(pet, WM_LBUTTONUP, 45, 45, false);
    wait_for("the sleeping cat plants its feet before settling", || {
        ((window_rect(pet).top - before_sleep_drag.top).abs() <= 1
            && unsafe { IsWindowVisible(effect) } == 0)
            .then_some(())
    });
    wait_for("sleep resumes at the taskbar after a drop", || {
        ((window_rect(pet).top - before_sleep_drag.top).abs() <= 1
            && unsafe { IsWindowVisible(effect) } != 0)
            .then_some(())
    });
    click_cat(pet);
    click_cat(pet);
    sleep(Duration::from_millis(100));
    assert_eq!(unsafe { IsWindowVisible(panel) }, 0);
    click_cat(pet);
    wait_for("third click wakes the sleeping cat", || {
        (unsafe { IsWindowVisible(panel) } != 0).then_some(())
    });
    wait_for("sleep effect has faded", || {
        (unsafe { IsWindowVisible(effect) } == 0).then_some(())
    });
    sleep(Duration::from_millis(100));
    care_button(panel, 288, Duration::from_millis(850));
    let launch = wait_for("ball appears before the cat runs", || {
        (unsafe { IsWindowVisible(effect) } != 0).then(|| window_rect(pet))
    });
    let migrated = wait_for("first action writes the new save", || {
        fs::read_to_string(&current).ok()
    });
    assert_eq!(migrated.split_whitespace().nth(2), Some("840"));
    assert_eq!(fs::read_to_string(&legacy).unwrap(), old_stats);
    let launch_panel = window_rect(panel);
    wait_for("cat runs away from the throw position", || {
        ((window_rect(pet).left - launch.left).abs() >= 24).then_some(())
    });
    let dpi = unsafe { GetDpiForWindow(pet) } as i32;
    if unsafe { GetSystemMetrics(SM_CXSCREEN) } >= (770 * dpi / 96) {
        wait_for(
            "charging the throw sends the cat beyond the tap distance",
            || ((window_rect(pet).left - launch.left).abs() > (215 * dpi / 96)).then_some(()),
        );
        wait_for("open status panel follows the running cat", || {
            ((window_rect(panel).left - launch_panel.left).abs() >= 15).then_some(())
        });
    }
    wait_for("cat brings the ball back to the throw position", || {
        let current = window_rect(pet);
        let home = (current.left - launch.left).abs() <= 1 && (current.top - launch.top).abs() <= 1;
        (home && unsafe { IsWindowVisible(effect) } != 0).then_some(())
    });
    wait_for("ball is put down and the fetch effect ends", || {
        (unsafe { IsWindowVisible(effect) } == 0).then_some(())
    });
    wait_for("cat hops once after putting the ball down", || {
        (window_rect(pet).top < launch.top - 7).then_some(())
    });
    wait_for("cat lands at the original position", || {
        let current = window_rect(pet);
        ((current.left - launch.left).abs() <= 1 && (current.top - launch.top).abs() <= 1)
            .then_some(())
    });
    care_button(panel, 72, Duration::from_millis(50));
    wait_for("feeding starts a fish effect", || {
        (unsafe { IsWindowVisible(effect) } != 0).then_some(())
    });
    wait_for("fish reaches the cat", || {
        (unsafe { IsWindowVisible(effect) } == 0).then_some(())
    });
    care_button(panel, 180, Duration::from_millis(50));
    wait_for("petting starts a heart effect", || {
        (unsafe { IsWindowVisible(effect) } != 0).then_some(())
    });

    let before_drag = window_rect(pet);
    let panel_before_drag = window_rect(panel);
    cat_pointer(pet, WM_LBUTTONDOWN, 45, 45, true);
    cat_pointer(pet, WM_MOUSEMOVE, 95, -55, true);
    wait_for("dragging lifts and moves the cat", || {
        (window_rect(pet).top <= before_drag.top - 95).then_some(())
    });
    wait_for("the panel follows the lifted cat", || {
        (window_rect(panel).top <= panel_before_drag.top - 85).then_some(())
    });
    cat_pointer(pet, WM_LBUTTONUP, 45, 45, false);
    wait_for("the cat lands on the taskbar", || {
        ((window_rect(pet).top - before_drag.top).abs() <= 1).then_some(())
    });
    wait_for("the panel follows the cat back to the taskbar", || {
        ((window_rect(panel).top - panel_before_drag.top).abs() <= 2).then_some(())
    });

    send(panel, WM_CLOSE, 0);
    wait_for("panel hidden", || {
        (unsafe { IsWindowVisible(panel) } == 0).then_some(())
    });

    // TrackPopupMenu sends this same command when the Exit item is selected.
    send(pet, WM_COMMAND, 2);
    let exit = wait_for("process exited with the panel hidden", || {
        app.0.try_wait().unwrap()
    });
    assert!(exit.success(), "{exit:?}");

    let saved = fs::read_to_string(&current).unwrap();
    let stats = saved.strip_prefix("2 ").unwrap().trim_end();
    fs::write(&current, format!("3 {stats}\n밤톨\n")).unwrap();
    let mut restarted = App(Command::new(binary)
        .env("LOCALAPPDATA", &data)
        .spawn()
        .unwrap());
    let pid = restarted.0.id();
    let renamed = wait_for("the renamed cat is restored after restart", || unsafe {
        let window = FindWindowW(w!("NyangArchi.Pet"), w!("밤톨"));
        (belongs_to(window, pid) && IsWindowVisible(window) != 0).then_some(window)
    });
    send(renamed, WM_COMMAND, 2);
    let exit = wait_for("the restarted process exited", || {
        restarted.0.try_wait().unwrap()
    });
    assert!(exit.success(), "{exit:?}");
}
