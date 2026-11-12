#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod behavior;
mod game;
mod motion;
mod native;
mod panel;
mod render;
mod typing;

pub const APP_NAME: &str = "행복한 옥키";
pub const CAT_NAME: &str = "옥키 (Okky)";

use std::ptr::null;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW};

fn main() {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(null(), 0, native::wide("Local\\NyangArchi").as_ptr());
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        if mutex == 0 {
            native::error_dialog("행복한 옥키를 시작하지 못했어요.");
            return;
        }
        if already_running {
            let class = native::wide(native::PET_CLASS);
            for _ in 0..40 {
                let window = FindWindowW(class.as_ptr(), null());
                if window != 0 {
                    PostMessageW(window, native::SHOW_STATUS, 0, 0);
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        } else {
            match render::Sprites::load() {
                Ok(sprites) => {
                    if let Err(error) = panel::run(Arc::new(sprites)) {
                        native::error_dialog(&format!(
                            "행복한 옥키의 상태창을 열지 못했어요.\n{error}"
                        ));
                    }
                }
                Err(error) => {
                    native::error_dialog(&format!("옥키 이미지를 읽지 못했어요.\n{error}"))
                }
            }
        }
        CloseHandle(mutex);
    }
}
