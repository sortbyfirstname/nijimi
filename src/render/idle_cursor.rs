use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetCursorPos, HCURSOR, HMENU, HWND_TOPMOST,
    IsWindowVisible, MA_NOACTIVATE, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    SetCursor, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WM_LBUTTONDOWN, WM_MOUSEACTIVATE,
    WM_MOUSEHWHEEL, WM_SETCURSOR, WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows::core::{Result, w};

use crate::geometry::ScreenRect;

const WINDOW_CLASS: windows::core::PCWSTR = w!("NijimiIdleCursorWindow");
const IDLE_DELAY: Duration = Duration::from_secs(2);
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

/// Hides the cursor once it rests in a bar, by putting a 1x1 invisible window of nijimi's own
/// under it that shows no cursor. Any movement takes the pointer off that pixel and Windows
/// shows the cursor again; nothing system-wide changes, and closing nijimi destroys the window.
pub(super) struct IdleCursor {
    hwnd: HWND,
    last_position: Option<POINT>,
    still_since: Instant,
    hiding: bool,
}

impl IdleCursor {
    pub(super) fn new() -> Result<Self> {
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None)?.0);
            if CLASS_REGISTERED.get().is_none() {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    lpszClassName: WINDOW_CLASS,
                    ..Default::default()
                };
                if RegisterClassW(&class) == 0 {
                    return Err(windows::core::Error::from_win32());
                }
                let _ = CLASS_REGISTERED.set(());
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(
                    WS_EX_TOPMOST.0
                        | WS_EX_NOACTIVATE.0
                        | WS_EX_TOOLWINDOW.0
                        | WS_EX_NOREDIRECTIONBITMAP.0,
                ),
                WINDOW_CLASS,
                w!("nijimi idle cursor"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                HWND::default(),
                HMENU::default(),
                instance,
                None,
            )?;
            Ok(Self {
                hwnd,
                last_position: None,
                still_since: Instant::now(),
                hiding: false,
            })
        }
    }

    /// Called on each frame tick while the glow is on.
    pub(super) fn update(&mut self, content: ScreenRect, width: u32, height: u32) {
        let mut position = POINT::default();
        if unsafe { GetCursorPos(&mut position) }.is_err() {
            self.reset();
            return;
        }
        let now = Instant::now();
        // The window hides itself on a click or scroll; count that as activity.
        let clicked = self.hiding && !unsafe { IsWindowVisible(self.hwnd) }.as_bool();
        if clicked || self.last_position != Some(position) {
            self.last_position = Some(position);
            self.still_since = now;
            self.reset();
            return;
        }
        if !self.hiding
            && now - self.still_since >= IDLE_DELAY
            && in_bars(position, content, width, height)
        {
            let shown = unsafe {
                SetWindowPos(
                    self.hwnd,
                    HWND_TOPMOST,
                    position.x,
                    position.y,
                    1,
                    1,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            self.hiding = shown.is_ok();
        }
    }

    pub(super) fn reset(&mut self) {
        if self.hiding {
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
            self.hiding = false;
        }
    }
}

impl Drop for IdleCursor {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn in_bars(position: POINT, content: ScreenRect, width: u32, height: u32) -> bool {
    let on_screen = position.x >= 0
        && position.y >= 0
        && position.x < width as i32
        && position.y < height as i32;
    let in_picture = position.x >= content.left
        && position.x < content.right
        && position.y >= content.top
        && position.y < content.bottom;
    on_screen && !in_picture
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_SETCURSOR => {
            unsafe { SetCursor(HCURSOR::default()) };
            LRESULT(1)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // Any mouse button or wheel.
        _ if (WM_LBUTTONDOWN..=WM_MOUSEHWHEEL).contains(&message) => {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_points_in_the_bars_of_the_primary_screen_count() {
        let content = ScreenRect {
            left: 440,
            top: 0,
            right: 3000,
            bottom: 1440,
        };
        let at = |x, y| in_bars(POINT { x, y }, content, 3440, 1440);
        assert!(at(0, 0));
        assert!(at(439, 700));
        assert!(at(3000, 700));
        assert!(at(3439, 1439));
        assert!(!at(440, 700));
        assert!(!at(2999, 0));
        assert!(!at(-1, 700), "another monitor to the left");
        assert!(!at(3440, 700), "another monitor to the right");
    }
}
