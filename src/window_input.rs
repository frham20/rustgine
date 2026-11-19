use crate::settings::PanelInput;
use glam::Vec2;
use std::{cell::Cell, mem::size_of};
use windows::{
    core::{w, Error, Result, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::ClientToScreen,
        UI::{
            Input::{
                GetRawInputData,
                KeyboardAndMouse::{GetFocus, GetKeyState, VK_A, VK_D, VK_S, VK_SHIFT, VK_W},
                RegisterRawInputDevices, HRAWINPUT, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTDEVICE,
                RAWINPUTHEADER, RAWMOUSE, RID_INPUT, RIM_TYPEMOUSE,
            },
            WindowsAndMessaging::*,
        },
    },
};

pub const IDLE_TITLE: PCWSTR = w!("Rustgine  |  Click for mouse look  |  F5 settings");
const LOOK_TITLE: PCWSTR =
    w!("Rustgine  |  WASD to fly  |  Shift to boost  |  Esc releases mouse  |  F5 settings");

pub struct WindowInput {
    pub panel: PanelInput,
    interactive: bool,
    cursor: HCURSOR,
    captured: Cell<bool>,
    keys: Cell<[bool; 4]>,
    pressed: Cell<[bool; 4]>,
    mouse: Cell<Vec2>,
    relative_mouse: Cell<bool>,
    cursor_center: Cell<POINT>,
}

impl WindowInput {
    pub fn new(interactive: bool, cursor: HCURSOR) -> Self {
        Self {
            panel: PanelInput::default(),
            interactive,
            cursor,
            captured: Cell::new(false),
            keys: Cell::new([false; 4]),
            pressed: Cell::new([false; 4]),
            mouse: Cell::new(Vec2::ZERO),
            relative_mouse: Cell::new(false),
            cursor_center: Cell::new(POINT::default()),
        }
    }

    pub unsafe fn register_mouse(&self, hwnd: HWND) -> Result<()> {
        if self.interactive {
            RegisterRawInputDevices(
                &[RAWINPUTDEVICE {
                    usUsagePage: 1,
                    usUsage: 2,
                    hwndTarget: hwnd,
                    ..Default::default()
                }],
                size_of::<RAWINPUTDEVICE>() as u32,
            )?;
        }
        Ok(())
    }

    pub unsafe fn sample(&self, hwnd: HWND) -> (Vec2, Vec2, bool) {
        if self.panel.is_open() {
            return (Vec2::ZERO, Vec2::ZERO, false);
        }
        let keys = self.keys.get();
        let pressed = self.pressed.replace([false; 4]);
        let [w, a, s, d] = std::array::from_fn(|i| keys[i] || pressed[i]);
        let movement = Vec2::new(
            d as u8 as f32 - a as u8 as f32,
            w as u8 as f32 - s as u8 as f32,
        );
        let mouse = self.mouse.replace(Vec2::ZERO);
        // Absolute and remote pointers may only move the Windows cursor.
        let look = if self.captured.get() && !self.relative_mouse.get() {
            let center = self.cursor_center.get();
            let mut point = POINT::default();
            if GetCursorPos(&mut point).is_ok()
                && point != center
                && SetCursorPos(center.x, center.y).is_ok()
            {
                Vec2::new((point.x - center.x) as f32, (point.y - center.y) as f32)
            } else {
                Vec2::ZERO
            }
        } else {
            mouse
        };
        let boost =
            self.interactive && GetFocus() == hwnd && GetKeyState(i32::from(VK_SHIFT.0)) < 0;
        (movement, look, boost)
    }

    unsafe fn clip_to_client(&self, hwnd: HWND) -> Result<()> {
        let mut client = RECT::default();
        GetClientRect(hwnd, &mut client)?;
        let mut top_left = POINT {
            x: client.left,
            y: client.top,
        };
        let mut bottom_right = POINT {
            x: client.right,
            y: client.bottom,
        };
        if !ClientToScreen(hwnd, &mut top_left).as_bool()
            || !ClientToScreen(hwnd, &mut bottom_right).as_bool()
        {
            return Err(Error::from_win32());
        }
        ClipCursor(Some(&RECT {
            left: top_left.x,
            top: top_left.y,
            right: bottom_right.x,
            bottom: bottom_right.y,
        }))?;
        let center = POINT {
            x: top_left.x + (bottom_right.x - top_left.x) / 2,
            y: top_left.y + (bottom_right.y - top_left.y) / 2,
        };
        self.cursor_center.set(center);
        if !self.relative_mouse.get() {
            if let Err(error) = SetCursorPos(center.x, center.y) {
                let _ = ClipCursor(None);
                return Err(error);
            }
        }
        Ok(())
    }

    pub unsafe fn release_mouse(&self, hwnd: HWND) -> bool {
        if !self.captured.replace(false) {
            return false;
        }
        self.mouse.set(Vec2::ZERO);
        self.relative_mouse.set(false);
        let _ = ClipCursor(None);
        SetCursor(self.cursor);
        let _ = SetWindowTextW(hwnd, IDLE_TITLE);
        true
    }

    pub unsafe fn set_panel_open(&self, hwnd: HWND, open: bool) {
        self.keys.set([false; 4]);
        self.pressed.set([false; 4]);
        self.release_mouse(hwnd);
        self.panel.set_open(hwnd, open);
    }

    unsafe fn read_mouse(&self, lparam: LPARAM) {
        let mut raw = RAWINPUT::default();
        let mut bytes = size_of::<RAWINPUT>() as u32;
        let header_bytes = size_of::<RAWINPUTHEADER>() as u32;
        let read = GetRawInputData(
            HRAWINPUT(lparam.0 as *mut _),
            RID_INPUT,
            Some(std::ptr::from_mut(&mut raw).cast()),
            &mut bytes,
            header_bytes,
        );
        if read != u32::MAX
            && read >= header_bytes + size_of::<RAWMOUSE>() as u32
            && raw.header.dwType == RIM_TYPEMOUSE.0
        {
            let mouse = raw.data.mouse;
            let delta = Vec2::new(mouse.lLastX as f32, mouse.lLastY as f32);
            if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 == 0 && delta != Vec2::ZERO {
                self.relative_mouse.set(true);
                self.mouse.set(self.mouse.get() + delta);
            }
        }
    }

    pub unsafe fn handle_message(
        &self,
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        if self.panel.is_open() {
            return self.panel.handle_message(hwnd, message, wparam, lparam);
        }
        if !self.interactive {
            return None;
        }
        match message {
            WM_KEYDOWN | WM_KEYUP if GetFocus() == hwnd => {
                let key = wparam.0 as u16;
                if let Some(i) = [VK_W.0, VK_A.0, VK_S.0, VK_D.0]
                    .iter()
                    .position(|&v| v == key)
                {
                    let mut keys = self.keys.get();
                    if message == WM_KEYDOWN && !keys[i] {
                        let mut pressed = self.pressed.get();
                        pressed[i] = true;
                        self.pressed.set(pressed);
                    }
                    keys[i] = message == WM_KEYDOWN;
                    self.keys.set(keys);
                    return Some(LRESULT(0));
                }
            }
            WM_LBUTTONDOWN if GetFocus() == hwnd => {
                if !self.captured.get() && self.clip_to_client(hwnd).is_ok() {
                    self.captured.set(true);
                    SetCursor(None);
                    let _ = SetWindowTextW(hwnd, LOOK_TITLE);
                }
                return Some(LRESULT(0));
            }
            WM_INPUT if self.captured.get() => {
                self.read_mouse(lparam);
                // The default procedure still needs to clean up the raw input handle.
            }
            WM_SETCURSOR if self.captured.get() => {
                SetCursor(None);
                return Some(LRESULT(1));
            }
            WM_KILLFOCUS | WM_CANCELMODE | WM_ENTERSIZEMOVE | WM_NCDESTROY => {
                self.keys.set([false; 4]);
                self.pressed.set([false; 4]);
                self.release_mouse(hwnd);
            }
            WM_SIZE if wparam.0 == SIZE_MINIMIZED as usize => {
                self.keys.set([false; 4]);
                self.pressed.set([false; 4]);
                self.release_mouse(hwnd);
            }
            WM_SIZE | WM_MOVE if self.captured.get() && self.clip_to_client(hwnd).is_err() => {
                self.release_mouse(hwnd);
            }
            _ => {}
        }
        None
    }
}
