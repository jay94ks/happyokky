use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::io;
use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_OEM_102, VK_RETURN, VK_SPACE};
use windows_sys::Win32::UI::Input::{
    GetRawInputData, RegisterRawInputDevices, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER,
    RAWKEYBOARD, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RID_INPUT, RIM_TYPEKEYBOARD,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1};

const HID_GENERIC_DESKTOP: u16 = 1;
const HID_KEYBOARD: u16 = 6;
const FAST_TYPING_KEYS: usize = 5;
const FAST_TYPING_PERIOD: u64 = 1_000;
const HUNGER_PER_FAST_KEY: f32 = 0.33;

thread_local! {
    static TYPING: RefCell<Typing> = RefCell::default();
}

pub fn register(window: HWND) -> io::Result<()> {
    let keyboard = RAWINPUTDEVICE {
        usUsagePage: HID_GENERIC_DESKTOP,
        usUsage: HID_KEYBOARD,
        dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        hwndTarget: window,
    };
    if unsafe { RegisterRawInputDevices(&keyboard, 1, size_of::<RAWINPUTDEVICE>() as u32) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn on_input(message_data: LPARAM) {
    unsafe {
        let mut input: RAWINPUT = zeroed();
        let mut size = size_of::<RAWINPUT>() as u32;
        let copied = GetRawInputData(
            message_data,
            RID_INPUT,
            (&mut input as *mut RAWINPUT).cast(),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if copied != u32::MAX
            && copied as usize >= size_of::<RAWINPUTHEADER>() + size_of::<RAWKEYBOARD>()
            && input.header.dwType == RIM_TYPEKEYBOARD
        {
            let now = GetTickCount64();
            TYPING.with(|typing| {
                typing
                    .borrow_mut()
                    .key(input.header.hDevice, input.data.keyboard, now)
            });
        }
    }
}

pub fn forget_device(device: HANDLE) {
    TYPING.with(|typing| {
        typing
            .borrow_mut()
            .held
            .retain(|(owner, _, _)| *owner != device)
    });
}

pub fn take_hunger() -> f32 {
    TYPING.with(|typing| std::mem::take(&mut typing.borrow_mut().hunger))
}

#[derive(Default)]
struct Typing {
    // Key identities are kept only while held, so repeating a key doesn't count as typing.
    held: HashSet<(HANDLE, u16, u16)>,
    recent_presses: VecDeque<u64>,
    hunger: f32,
}

impl Typing {
    fn key(&mut self, device: HANDLE, input: RAWKEYBOARD, now: u64) {
        if !matches!(
            input.VKey,
            0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDF
                | VK_BACK | VK_RETURN | VK_SPACE | VK_OEM_102
        ) || input.MakeCode == 0xFF
        {
            return;
        }
        let scan = if input.MakeCode == 0 {
            0x100 | input.VKey
        } else {
            input.MakeCode
        };
        let extended = input.Flags & (RI_KEY_E0 | RI_KEY_E1) as u16;
        let key = (device, scan, extended);
        if input.Flags & RI_KEY_BREAK as u16 != 0 {
            self.held.remove(&key);
            return;
        }
        if !self.held.insert(key) {
            return;
        }

        if self.recent_presses.len() == FAST_TYPING_KEYS {
            if let Some(oldest) = self.recent_presses.pop_front() {
                let interval = (now - oldest).min(FAST_TYPING_PERIOD);
                let above_threshold = 1.0 - interval as f32 / FAST_TYPING_PERIOD as f32;
                self.hunger += HUNGER_PER_FAST_KEY * above_threshold;
            }
        }
        self.recent_presses.push_back(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(vkey: u16, flags: u16) -> RAWKEYBOARD {
        RAWKEYBOARD {
            MakeCode: 0,
            Flags: flags,
            Reserved: 0,
            VKey: vkey,
            Message: 0,
            ExtraInformation: 0,
        }
    }

    #[test]
    fn quick_typing_spends_fullness_and_normal_typing_does_not() {
        for (interval, expected) in [(200, false), (125, true)] {
            let mut typing = Typing::default();
            for i in 0..16 {
                typing.key(1, key(0x41, 0), i * interval);
                typing.key(1, key(0x41, RI_KEY_BREAK as u16), i * interval + 1);
            }
            assert_eq!(typing.hunger > 0.0, expected);
        }
    }

    #[test]
    fn modifiers_and_held_repeats_are_ignored() {
        let mut typing = Typing::default();
        for i in 0..16 {
            typing.key(1, key(0x10, 0), i * 10);
            typing.key(1, key(0x41, 0), i * 10);
        }
        assert_eq!(typing.recent_presses.len(), 1);
        assert_eq!(typing.hunger, 0.0);
    }
}
