use std::collections::HashSet;
use std::mem::size_of;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use std::os::windows::ffi::OsStringExt;

use windows::Win32::Foundation::{
    BOOL, COLORREF, CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, HINSTANCE, HWND, LPARAM,
    LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DEFAULT_GUI_FONT, DT_CENTER, DT_END_ELLIPSIS, DT_SINGLELINE, DT_VCENTER,
    DeleteObject, DrawFocusRect, DrawTextW, FillRect, GetStockObject, HBRUSH, HDC, HGDIOBJ,
    MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, MonitorFromWindow,
    SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetProcessTimes, OpenProcess, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW, GWLP_USERDATA,
    GetCursorPos, GetDlgItem, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, HICON, HMENU, IDC_ARROW, KillTimer, LB_ADDSTRING, LB_GETCURSEL,
    LB_RESETCONTENT, LB_SETCURSEL, LoadCursorW, MENU_ITEM_FLAGS, MF_STRING, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SW_HIDE, SW_SHOW,
    SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLORBTN, WM_CTLCOLOREDIT,
    WM_CTLCOLORLISTBOX, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DISPLAYCHANGE, WM_DRAWITEM,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_RBUTTONUP, WM_SETFONT, WM_TIMER,
    WNDCLASSW,
};
use windows::core::{Error, PCWSTR, PWSTR, Result, w};

use crate::frame_timer::FrameTimer;
use crate::geometry::AspectRatio;
use crate::render::Renderer;
use crate::settings::{self, ActiveProfile, AppProfile, AppSettings, paths_match};

const TRAY_CALLBACK: u32 = WM_APP + 1;
const FOREGROUND_CHANGED: u32 = WM_APP + 2;
const SETTINGS_RELOADED: u32 = WM_APP + 3;
const RENDER_FRAME: u32 = WM_APP + 4;
const EVENT_SYSTEM_FOREGROUND: u32 = 3;
const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800b;
const EXIT_COMMAND: u32 = 2;
const SETTINGS_COMMAND: u32 = 3;
/// Frees the GPU after a pause this long; keeping it briefly makes Alt-Tabbing back instant.
const RELEASE_TIMER: usize = 2;
const RELEASE_AFTER_MS: u32 = 60_000;
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);
static FOREGROUND_UPDATE_PENDING: AtomicBool = AtomicBool::new(false);
static SETTINGS_CLASS_REGISTERED: OnceLock<()> = OnceLock::new();
const SETTINGS_LIST_ID: i32 = 100;
const SETTINGS_RATIO_ID: i32 = 101;
const SETTINGS_BROWSE_ID: i32 = 102;
const SETTINGS_REMOVE_ID: i32 = 104;
const SETTINGS_CLOSE_ID: i32 = 105;
const SETTINGS_ADD_ID: i32 = 106;
const SETTINGS_GLOBAL_RATIO_ID: i32 = 107;
const SETTINGS_ADD_SELECTED_ID: i32 = 109;
const SETTINGS_HEADER_ID: i32 = 110;
const SETTINGS_HELP_ID: i32 = 111;
const SETTINGS_RATIO_LABEL_ID: i32 = 112;
const SETTINGS_ADD_HELP_ID: i32 = 113;
const SETTINGS_PROGRAM_TAB_ID: i32 = 114;
const SETTINGS_GLOBAL_TAB_ID: i32 = 115;
const SETTINGS_SEARCH_ID: i32 = 116;
const SETTINGS_PATH_ID: i32 = 117;
const SETTINGS_PROGRAM_TITLE_ID: i32 = 118;
const SETTINGS_GLOBAL_TITLE_ID: i32 = 119;
const SETTINGS_EMPTY_ID: i32 = 120;
const SETTINGS_GLOBAL_HELP_ID: i32 = 210;
const SETTINGS_ADD_TITLE_ID: i32 = 211;
const SETTINGS_ADD_DESCRIPTION_ID: i32 = 213;
const SETTINGS_GLOBAL_SIDE_HELP_ID: i32 = 214;
const SETTINGS_TABLE_SETTING_ID: i32 = 215;
const SETTINGS_TABLE_VALUE_ID: i32 = 216;
const TRAY_WINDOW_TITLE: PCWSTR = w!("nijimi tray");
const LB_SETHORIZONTALEXTENT_MESSAGE: u32 = 0x0194;
const EN_CHANGE_NOTIFICATION: usize = 0x0300;
const CBN_SELENDOK_NOTIFICATION: u16 = 9;
const CBN_KILLFOCUS_NOTIFICATION: u16 = 4;
const CB_ADDSTRING_MESSAGE: u32 = 0x0143;

#[repr(C)]
struct OpenFileNameW {
    l_struct_size: u32,
    hwnd_owner: HWND,
    h_instance: HINSTANCE,
    filter: PCWSTR,
    custom_filter: PWSTR,
    max_custom_filter: u32,
    filter_index: u32,
    file: PWSTR,
    max_file: u32,
    file_title: PWSTR,
    max_file_title: u32,
    initial_dir: PCWSTR,
    title: PCWSTR,
    flags: u32,
    file_offset: u16,
    file_extension: u16,
    default_extension: PCWSTR,
    custom_data: LPARAM,
    hook: Option<unsafe extern "system" fn()>,
    template_name: PCWSTR,
    reserved: *mut std::ffi::c_void,
    reserved_value: u32,
    flags_ex: u32,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(file_name: *mut OpenFileNameW) -> BOOL;
}

struct AppState {
    hwnd: HWND,
    renderer: Renderer,
    /// Only exists while the effect is on or briefly paused, so an idle tray holds no extra thread.
    frame_timer: Option<FrameTimer>,
    location_hook: Option<(u32, WinEventHook)>,
    /// False after a failure, so it isn't retried and re-reported on every foreground change.
    /// Re-armed when settings are saved or the display changes.
    enabled: bool,
    rendering: bool,
    active: Option<ActiveProfile>,
    application_executable: PathBuf,
    icon: HICON,
    taskbar_created: u32,
    settings: AppSettings,
    settings_path: PathBuf,
    settings_adding: bool,
    settings_global_view: bool,
    recent_apps: Vec<PathBuf>,
    running_apps: Vec<PathBuf>,
    settings_choices: Vec<PathBuf>,
    settings_hwnd: HWND,
    settings_palette: Option<SettingsPalette>,
    tray_status: TrayStatus,
}

struct SettingsPalette {
    background: HBRUSH,
    input: HBRUSH,
    button: HBRUSH,
    accent: HBRUSH,
}

#[repr(C)]
struct OwnerDrawItem {
    ctl_type: u32,
    ctl_id: u32,
    item_id: u32,
    item_action: u32,
    item_state: u32,
    hwnd_item: HWND,
    hdc: HDC,
    rect: RECT,
    item_data: usize,
}

impl SettingsPalette {
    fn new() -> Self {
        unsafe {
            Self {
                background: CreateSolidBrush(COLORREF(rgb(19, 21, 26))),
                input: CreateSolidBrush(COLORREF(rgb(35, 39, 48))),
                button: CreateSolidBrush(COLORREF(rgb(46, 51, 62))),
                accent: CreateSolidBrush(COLORREF(rgb(73, 128, 232))),
            }
        }
    }
}

impl Drop for SettingsPalette {
    fn drop(&mut self) {
        unsafe {
            for brush in [self.background, self.input, self.button, self.accent] {
                let _ = DeleteObject(HGDIOBJ(brush.0));
            }
        }
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

fn ensure_dpi_awareness() -> Result<()> {
    unsafe {
        let current_context = GetThreadDpiAwarenessContext();
        if AreDpiAwarenessContextsEqual(current_context, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
            .as_bool()
        {
            return Ok(());
        }

        match SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) {
            Ok(()) => Ok(()),
            Err(error) => {
                let current_context = GetThreadDpiAwarenessContext();
                if AreDpiAwarenessContextsEqual(
                    current_context,
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
                .as_bool()
                {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
}

/// `show_settings` opens Settings too, via the running instance if there is one.
pub fn run(show_settings: bool) -> Result<()> {
    unsafe {
        ensure_dpi_awareness()?;
        let mutex = CreateMutexW(None, false, w!("Local\\NijimiAmbientLight"))?;
        if windows::Win32::Foundation::GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(mutex);
            if show_settings {
                ask_running_tray_to_open_settings();
            }
            return Ok(());
        }
        let _instance_mutex = InstanceMutex(mutex);
        // Entries saved by earlier versions launch without the flag that keeps nijimi in the tray.
        let _ = crate::startup::refresh();
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let class_icon = crate::app_icon::load_application_icon(32)?;
        let icon = crate::app_icon::load_application_icon(16)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: class_icon,
            lpszClassName: w!("NijimiTrayWindow"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            return Err(Error::from_win32());
        }

        let settings_path = settings::config_path()?;
        let (settings, settings_warning) = AppSettings::load(&settings_path)?;
        let application_executable = std::env::current_exe()?;
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        if taskbar_created == 0 {
            return Err(Error::from_win32());
        }
        let mut renderer = Renderer::new()?;
        renderer.set_capturable(settings.show_in_screenshots)?;
        let state = OwnedState(Box::into_raw(Box::new(AppState {
            hwnd: HWND::default(),
            renderer,
            frame_timer: None,
            location_hook: None,
            enabled: true,
            rendering: false,
            active: None,
            application_executable,
            icon,
            taskbar_created,
            settings,
            settings_path,
            settings_adding: false,
            settings_global_view: false,
            recent_apps: Vec::new(),
            running_apps: Vec::new(),
            settings_choices: Vec::new(),
            settings_hwnd: HWND::default(),
            settings_palette: None,
            tray_status: TrayStatus::Ready,
        })));
        let state_ptr = state.0;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("NijimiTrayWindow"),
            TRAY_WINDOW_TITLE,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND::default(),
            HMENU::default(),
            instance,
            Some(state_ptr.cast()),
        )?;
        (*state_ptr).hwnd = hwnd;

        let tray = tray_data(hwnd, icon, TrayStatus::Ready);
        if !Shell_NotifyIconW(NIM_ADD, &tray).as_bool() {
            let error = Error::from_win32();
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }

        MAIN_WINDOW.store(hwnd.0 as isize, Ordering::Release);
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_event),
            0,
            0,
            0,
        );
        if hook.0.is_null() {
            let error = Error::from_win32();
            MAIN_WINDOW.store(0, Ordering::Release);
            let _ = Shell_NotifyIconW(NIM_DELETE, &tray);
            let _ = DestroyWindow(hwnd);
            return Err(error);
        }
        let _foreground_hook = WinEventHook(hook);
        if let Some(error) = update_foreground(&mut *state_ptr) {
            show_error(&error);
        }
        if let Some(warning) = settings_warning {
            show_error(&warning);
        }
        if show_settings && let Some(error) = open_settings(state_ptr) {
            show_error(&error);
        }
        let mut message = MSG::default();
        loop {
            let result = GetMessageW(&mut message, HWND::default(), 0, 0);
            if result.0 == -1 {
                let error = Error::from_win32();
                let _ = Shell_NotifyIconW(NIM_DELETE, &tray);
                MAIN_WINDOW.store(0, Ordering::Release);
                let _ = DestroyWindow(hwnd);
                return Err(error);
            }
            if result.0 == 0 {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        let _ = Shell_NotifyIconW(NIM_DELETE, &tray);
        MAIN_WINDOW.store(0, Ordering::Release);
        let _ = DestroyWindow(hwnd);
    }
    Ok(())
}

/// Freed only after the window that holds the pointer is destroyed.
struct OwnedState(*mut AppState);

impl Drop for OwnedState {
    fn drop(&mut self) {
        drop(unsafe { Box::from_raw(self.0) });
    }
}

struct InstanceMutex(HANDLE);

impl Drop for InstanceMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn show_error(message: &str) {
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(
            HWND::default(),
            windows::core::PCWSTR(message.as_ptr()),
            w!("nijimi"),
            windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe {
            &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
        };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }

    if message == WM_DESTROY {
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }

    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState };
    if state_ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    if message == TRAY_CALLBACK {
        match lparam.0 as u32 {
            WM_LBUTTONUP => {
                if let Some(error) = unsafe { open_settings(state_ptr) } {
                    show_error(&error);
                }
            }
            WM_RBUTTONUP => {
                show_menu(unsafe { (*state_ptr).hwnd });
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if message == WM_COMMAND {
        match (wparam.0 & 0xffff) as u32 {
            EXIT_COMMAND => unsafe {
                if let Some(settings) = find_settings_window() {
                    let _ = PostMessageW(settings, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                let _ = DestroyWindow(hwnd);
            },
            SETTINGS_COMMAND => {
                if let Some(error) = unsafe { open_settings(state_ptr) } {
                    show_error(&error);
                }
            }
            _ => {}
        }
        return LRESULT(0);
    }
    let taskbar_created = unsafe { (*state_ptr).taskbar_created };
    if message == taskbar_created {
        let state = unsafe { &*state_ptr };
        let tray = tray_data(state.hwnd, state.icon, state.tray_status);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &tray);
        }
        return LRESULT(0);
    }
    if message == FOREGROUND_CHANGED {
        FOREGROUND_UPDATE_PENDING.store(false, Ordering::Release);
        if let Some(error) = unsafe { update_foreground(&mut *state_ptr) } {
            show_error(&error);
        }
        return LRESULT(0);
    }
    if message == SETTINGS_RELOADED {
        // Error dialogs run a message loop that re-enters this procedure, so show them only after
        // the borrow of the state has ended.
        let mut errors = Vec::new();
        {
            let state = unsafe { &mut *state_ptr };
            match AppSettings::load(&state.settings_path) {
                Ok((settings, warning)) => {
                    errors.extend(warning);
                    if let Err(error) = state.renderer.set_capturable(settings.show_in_screenshots)
                    {
                        errors.push(format!(
                            "nijimi could not change whether screenshots show the effect:\n{error}"
                        ));
                    }
                    let frame_rate_changed = settings.frame_rate != state.settings.frame_rate;
                    state.settings = settings;
                    rearm_after_error(state);
                    if let Some(error) = update_foreground(state) {
                        errors.push(error);
                    } else if frame_rate_changed
                        && state.rendering
                        && let Err(error) = start_frame_timer(state)
                    {
                        errors.push(format!("nijimi could not change the frame rate:\n{error}"));
                    }
                }
                Err(error) => errors.push(format!("nijimi could not reload settings:\n{error}")),
            }
        }
        for error in errors {
            show_error(&error);
        }
        return LRESULT(0);
    }
    if message == WM_TIMER && wparam.0 == RELEASE_TIMER {
        let state = unsafe { &mut *state_ptr };
        unsafe {
            let _ = KillTimer(state.hwnd, RELEASE_TIMER);
        }
        if !state.rendering {
            state.renderer.release_gpu();
            state.frame_timer = None;
        }
        return LRESULT(0);
    }
    if message == RENDER_FRAME {
        let error = unsafe {
            let state = &mut *state_ptr;
            if !state.rendering {
                return LRESULT(0);
            }
            let result = state.renderer.render().and_then(|()| {
                state
                    .frame_timer
                    .as_mut()
                    .map_or(Ok(()), FrameTimer::frame_done)
            });
            if let Err(error) = result {
                stop_after_error(state);
                sync_tray_status(state);
                Some(format!("nijimi stopped rendering:\n{error}"))
            } else {
                sync_tray_status(state);
                None
            }
        };
        if let Some(error) = error {
            show_error(&error);
        }
        return LRESULT(0);
    }
    if message == WM_DISPLAYCHANGE {
        let error = unsafe {
            let state = &mut *state_ptr;
            if let Err(error) = state.renderer.refresh(state.rendering) {
                state.renderer.set_enabled(false);
                state.rendering = false;
                state.active = None;
                state.frame_timer = None;
                sync_tray_status(state);
                Some(format!("nijimi could not refresh the display:\n{error}"))
            } else {
                None
            }
        };
        if let Some(error) = error {
            show_error(&error);
        } else if let Some(error) = unsafe {
            rearm_after_error(&mut *state_ptr);
            update_foreground(&mut *state_ptr)
        } {
            show_error(&error);
        }
        return LRESULT(0);
    }

    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn rearm_after_error(state: &mut AppState) {
    if !state.enabled {
        state.enabled = true;
        sync_tray_status(state);
    }
}

struct WinEventHook(HWINEVENTHOOK);

impl Drop for WinEventHook {
    fn drop(&mut self) {
        unsafe {
            let _ = UnhookWinEvent(self.0);
        }
    }
}

unsafe extern "system" fn foreground_event(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object: i32,
    child: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    if event == EVENT_OBJECT_LOCATIONCHANGE
        && (object != 0 || child != 0 || hwnd != unsafe { GetForegroundWindow() })
    {
        return;
    }
    let hwnd = MAIN_WINDOW.load(Ordering::Acquire);
    if hwnd != 0 && !FOREGROUND_UPDATE_PENDING.swap(true, Ordering::AcqRel) {
        unsafe {
            if PostMessageW(
                HWND(hwnd as *mut std::ffi::c_void),
                FOREGROUND_CHANGED,
                WPARAM(0),
                LPARAM(0),
            )
            .is_err()
            {
                FOREGROUND_UPDATE_PENDING.store(false, Ordering::Release);
            }
        }
    }
}

struct Foreground {
    executable: PathBuf,
    process_id: u32,
    on_primary: bool,
}

fn foreground_window() -> Option<Foreground> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let process_id = window_process_id(hwnd);
        let executable = process_executable(process_id)?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL);
        let primary = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        Some(Foreground {
            executable,
            process_id,
            on_primary: !monitor.0.is_null() && monitor == primary,
        })
    }
}

fn window_process_id(hwnd: HWND) -> u32 {
    let mut process_id = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    process_id
}

fn window_executable(hwnd: HWND) -> Option<PathBuf> {
    process_executable(window_process_id(hwnd))
}

fn process_executable(process_id: u32) -> Option<PathBuf> {
    unsafe {
        if process_id == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id).ok()?;
        let mut path = [0u16; 32768];
        let mut length = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(path.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        result.ok()?;
        Some(PathBuf::from(std::ffi::OsString::from_wide(
            &path[..length as usize],
        )))
    }
}

pub(crate) fn running_process_executables() -> Result<Vec<PathBuf>> {
    let mut processes: Vec<(PathBuf, Option<u64>)> = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                if let Ok(process) = OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION,
                    false,
                    entry.th32ProcessID,
                ) {
                    let mut path = [0u16; 32768];
                    let mut length = path.len() as u32;
                    let queried = QueryFullProcessImageNameW(
                        process,
                        PROCESS_NAME_FORMAT(0),
                        PWSTR(path.as_mut_ptr()),
                        &mut length,
                    );
                    if queried.is_ok() {
                        let executable =
                            PathBuf::from(std::ffi::OsString::from_wide(&path[..length as usize]));
                        let mut creation_time = FILETIME::default();
                        let mut exit_time = FILETIME::default();
                        let mut kernel_time = FILETIME::default();
                        let mut user_time = FILETIME::default();
                        let creation_time = GetProcessTimes(
                            process,
                            &mut creation_time,
                            &mut exit_time,
                            &mut kernel_time,
                            &mut user_time,
                        )
                        .ok()
                        .map(|()| {
                            ((creation_time.dwHighDateTime as u64) << 32)
                                | creation_time.dwLowDateTime as u64
                        });
                        processes.push((executable, creation_time));
                    }
                    let _ = CloseHandle(process);
                }
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(order_process_executables(processes))
}

fn order_process_executables(mut processes: Vec<(PathBuf, Option<u64>)>) -> Vec<PathBuf> {
    processes.sort_by(
        |(_, left_time), (_, right_time)| match (left_time, right_time) {
            (Some(left), Some(right)) => right.cmp(left),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        },
    );

    let mut seen = HashSet::new();
    processes
        .into_iter()
        .map(|(executable, _)| executable)
        .filter(|executable| seen.insert(settings::path_key(executable)))
        .collect()
}

pub(crate) fn browse_executable_for_settings() -> Option<PathBuf> {
    let owner = unsafe { GetForegroundWindow() };
    browse_executable(owner)
}

fn update_foreground(state: &mut AppState) -> Option<String> {
    let error = apply_foreground(state);
    sync_tray_status(state);
    error
}

fn apply_foreground(state: &mut AppState) -> Option<String> {
    let foreground = foreground_window();
    sync_location_hook(state, foreground.as_ref());
    let executable = foreground.as_ref().map(|foreground| &foreground.executable);
    if let Some(executable) = executable
        && !paths_match(executable, &state.application_executable)
    {
        if !state
            .recent_apps
            .iter()
            .any(|recent| paths_match(recent, executable))
        {
            state.recent_apps.insert(0, executable.clone());
            state.recent_apps.truncate(32);
        } else if let Some(index) = state
            .recent_apps
            .iter()
            .position(|recent| paths_match(recent, executable))
        {
            let recent = state.recent_apps.remove(index);
            state.recent_apps.insert(0, recent);
        }
    }

    let selected = state.settings.active_profile(
        state.enabled,
        foreground
            .as_ref()
            .is_some_and(|foreground| foreground.on_primary),
        executable
            .filter(|path| !paths_match(path, &state.application_executable))
            .map(PathBuf::as_path),
    );
    if !state.enabled || selected.is_none() {
        if state.rendering {
            state.renderer.pause();
            state.rendering = false;
            state.active = None;
            if let Some(timer) = state.frame_timer.as_mut() {
                timer.stop();
            }
            unsafe {
                if SetTimer(state.hwnd, RELEASE_TIMER, RELEASE_AFTER_MS, None) == 0 {
                    state.renderer.release_gpu();
                    state.frame_timer = None;
                }
            }
        }
        return None;
    }

    let selected = selected.unwrap();
    if state.rendering && state.active.as_ref() == Some(&selected) {
        return None;
    }
    if let Err(error) = state
        .renderer
        .configure(selected.aspect_ratio, selected.effect)
    {
        stop_after_error(state);
        return Some(format!(
            "nijimi could not apply the selected aspect ratio:\n{error}"
        ));
    }
    state.renderer.set_enabled(true);
    if let Err(error) = start_frame_timer(state) {
        stop_after_error(state);
        return Some(format!("nijimi could not start its render timer:\n{error}"));
    }
    unsafe {
        let _ = KillTimer(state.hwnd, RELEASE_TIMER);
    }
    state.rendering = true;
    state.active = Some(selected);
    None
}

/// Only hooks the foreground app when the effect could apply; a global hook wakes on every mouse move.
fn sync_location_hook(state: &mut AppState, foreground: Option<&Foreground>) {
    let process_id = foreground
        .filter(|foreground| {
            state.enabled
                && !paths_match(&foreground.executable, &state.application_executable)
                && (state.settings.always_on
                    || state.settings.profile_for(&foreground.executable).is_some())
        })
        .map(|foreground| foreground.process_id);
    if state.location_hook.as_ref().map(|(id, _)| *id) == process_id {
        return;
    }
    state.location_hook = process_id.and_then(|process_id| {
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_LOCATIONCHANGE,
                EVENT_OBJECT_LOCATIONCHANGE,
                None,
                Some(foreground_event),
                process_id,
                0,
                0,
            )
        };
        (!hook.0.is_null()).then(|| (process_id, WinEventHook(hook)))
    });
}

fn stop_after_error(state: &mut AppState) {
    state.renderer.set_enabled(false);
    state.enabled = false;
    state.rendering = false;
    state.active = None;
    state.frame_timer = None;
}

fn start_frame_timer(state: &mut AppState) -> Result<()> {
    let fps = state.settings.frame_rate.fps();
    let timer = match state.frame_timer.as_mut() {
        Some(timer) => timer,
        None => state
            .frame_timer
            .insert(FrameTimer::new(post_render_frame)?),
    };
    timer.start(fps)
}

fn post_render_frame() {
    let hwnd = MAIN_WINDOW.load(Ordering::Acquire);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(
                HWND(hwnd as *mut std::ffi::c_void),
                RENDER_FRAME,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TrayStatus {
    Ready,
    CaptureUnavailable,
    Stopped,
}

fn sync_tray_status(state: &mut AppState) {
    let status = if !state.enabled {
        TrayStatus::Stopped
    } else if state.rendering && !state.renderer.is_capturing() {
        TrayStatus::CaptureUnavailable
    } else {
        TrayStatus::Ready
    };
    if status == state.tray_status {
        return;
    }
    state.tray_status = status;
    unsafe {
        let mut tray = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: state.hwnd,
            uID: 1,
            uFlags: NIF_TIP,
            ..Default::default()
        };
        set_tip(&mut tray, status);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &tray);
    }
}

fn tray_data(hwnd: HWND, icon: HICON, status: TrayStatus) -> NOTIFYICONDATAW {
    let mut tray = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK,
        hIcon: icon,
        ..Default::default()
    };
    set_tip(&mut tray, status);
    tray
}

fn show_menu(hwnd: HWND) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        if AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(MF_STRING.0),
            SETTINGS_COMMAND as usize,
            w!("Settings"),
        )
        .is_err()
            || AppendMenuW(
                menu,
                MENU_ITEM_FLAGS(MF_STRING.0),
                EXIT_COMMAND as usize,
                w!("Exit"),
            )
            .is_err()
        {
            let _ = DestroyMenu(menu);
            return;
        }
        let mut point = POINT::default();
        if GetCursorPos(&mut point).is_ok() {
            let _ = SetForegroundWindow(hwnd);
            let command = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                point.x,
                point.y,
                0,
                hwnd,
                None,
            );
            if command.0 != 0 {
                let _ = PostMessageW(hwnd, WM_COMMAND, WPARAM(command.0 as usize), LPARAM(0));
            }
            let _ = PostMessageW(
                hwnd,
                windows::Win32::UI::WindowsAndMessaging::WM_NULL,
                WPARAM(0),
                LPARAM(0),
            );
        }
        let _ = DestroyMenu(menu);
    }
}

fn set_tip(tray: &mut NOTIFYICONDATAW, status: TrayStatus) {
    let tip = match status {
        TrayStatus::Ready => "nijimi",
        TrayStatus::CaptureUnavailable => "nijimi - screen capture unavailable, retrying",
        TrayStatus::Stopped => "nijimi - stopped after an error",
    };
    for (destination, source) in tray.szTip.iter_mut().zip(tip.encode_utf16().chain(Some(0))) {
        *destination = source;
    }
}

/// Matched by title and executable, so an Explorer window on a "nijimi" folder never matches.
pub(crate) fn find_settings_window() -> Option<HWND> {
    let executable = std::env::current_exe().ok()?;
    let mut after = HWND::default();
    loop {
        let window = unsafe {
            FindWindowExW(
                HWND::default(),
                after,
                PCWSTR::null(),
                PCWSTR(crate::settings_ui::WINDOW_TITLE_WIDE.as_ptr()),
            )
        }
        .ok()
        .filter(|window| !window.0.is_null())?;
        if window_executable(window).is_some_and(|path| paths_match(&path, &executable)) {
            return Some(window);
        }
        after = window;
    }
}

/// Passes this launch's foreground rights on so the running instance can raise Settings.
fn ask_running_tray_to_open_settings() {
    unsafe {
        if let Ok(tray) = FindWindowW(w!("NijimiTrayWindow"), PCWSTR::null())
            && !tray.0.is_null()
        {
            let mut tray_process = 0;
            if GetWindowThreadProcessId(tray, Some(&mut tray_process)) != 0 {
                let _ = AllowSetForegroundWindow(tray_process);
            }
            let _ = PostMessageW(
                tray,
                WM_COMMAND,
                WPARAM(SETTINGS_COMMAND as usize),
                LPARAM(0),
            );
        }
    }
}

unsafe fn open_settings(state_ptr: *mut AppState) -> Option<String> {
    unsafe {
        if let Some(settings_window) = find_settings_window() {
            let _ = SetForegroundWindow(settings_window);
            return None;
        }
        let state = &*state_ptr;
        let mut process = Command::new(&state.application_executable);
        process.arg("--settings").arg("--recent");
        process.args(&state.recent_apps);
        match process.spawn() {
            Ok(child) => {
                // The new Settings process needs foreground rights to raise its window.
                let _ = AllowSetForegroundWindow(child.id());
                None
            }
            Err(error) => Some(format!("nijimi could not open Settings:\n{error}")),
        }
    }
}

pub(crate) fn notify_settings_changed() {
    unsafe {
        let hwnd =
            FindWindowW(w!("NijimiTrayWindow"), windows::core::PCWSTR::null()).unwrap_or_default();
        if !hwnd.0.is_null() {
            let _ = PostMessageW(hwnd, SETTINGS_RELOADED, WPARAM(0), LPARAM(0));
        }
    }
}

unsafe extern "system" fn settings_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe {
            &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW)
        };
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
    }
    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState };
    if message == WM_CREATE && !state_ptr.is_null() {
        unsafe {
            (*state_ptr).settings_hwnd = hwnd;
            if create_settings_controls(hwnd).is_err() {
                return LRESULT(-1);
            }
            (*state_ptr).settings_palette = Some(SettingsPalette::new());
            initialize_ratio_choices(hwnd);
            let cue: Vec<u16> = "Search app names or paths"
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let _ = SendMessageW(
                settings_control(hwnd, SETTINGS_SEARCH_ID),
                0x1501,
                WPARAM(0),
                LPARAM(cue.as_ptr() as isize),
            );
            (*state_ptr).running_apps = running_process_executables().unwrap_or_default();
            update_settings_mode(hwnd, &*state_ptr);
            refresh_settings_choices(&mut *state_ptr, hwnd, None);
        }
        return LRESULT(0);
    }
    if message == WM_ERASEBKGND && !state_ptr.is_null() {
        unsafe {
            if let Some(palette) = (*state_ptr).settings_palette.as_ref() {
                let mut rect = windows::Win32::Foundation::RECT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
                let _ = FillRect(HDC(wparam.0 as *mut _), &rect, palette.background);
                return LRESULT(1);
            }
        }
    }
    if matches!(
        message,
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX | WM_CTLCOLORSTATIC | WM_CTLCOLORBTN
    ) && !state_ptr.is_null()
    {
        unsafe {
            if let Some(palette) = (*state_ptr).settings_palette.as_ref() {
                let dc = HDC(wparam.0 as *mut _);
                let (brush, color) = match message {
                    WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => (palette.input, rgb(225, 228, 235)),
                    WM_CTLCOLORSTATIC => (palette.background, rgb(218, 222, 230)),
                    _ => (palette.button, rgb(240, 242, 246)),
                };
                let _ = SetTextColor(dc, COLORREF(color));
                let _ = SetBkColor(
                    dc,
                    COLORREF(if message == WM_CTLCOLORSTATIC {
                        rgb(19, 21, 26)
                    } else {
                        rgb(35, 39, 48)
                    }),
                );
                let _ = SetBkMode(dc, TRANSPARENT);
                return LRESULT(brush.0 as isize);
            }
        }
    }
    if message == WM_DRAWITEM && !state_ptr.is_null() {
        unsafe {
            let item = &*(lparam.0 as *const OwnerDrawItem);
            if draw_settings_button(hwnd, item, &*state_ptr) {
                return LRESULT(1);
            }
        }
    }
    if message == WM_COMMAND && !state_ptr.is_null() {
        let command = (wparam.0 & 0xffff) as i32;
        let notification = (wparam.0 >> 16) as u16;
        if command == SETTINGS_SEARCH_ID && notification as usize == EN_CHANGE_NOTIFICATION {
            unsafe {
                let state = &mut *state_ptr;
                let preferred = selected_executable(hwnd, state);
                refresh_settings_choices(state, hwnd, preferred);
            }
            return LRESULT(0);
        }
        if (command == SETTINGS_RATIO_ID || command == SETTINGS_GLOBAL_RATIO_ID)
            && matches!(
                notification,
                CBN_SELENDOK_NOTIFICATION | CBN_KILLFOCUS_NOTIFICATION
            )
        {
            let error = unsafe {
                let state = &mut *state_ptr;
                if command == SETTINGS_RATIO_ID {
                    save_selected_profile(hwnd, state)
                } else {
                    save_global_ratio(hwnd, state)
                }
            };
            if let Some(error) = error {
                show_error(&error);
            }
            return LRESULT(0);
        }
        let error = match command {
            SETTINGS_LIST_ID if (wparam.0 >> 16) as u16 == 1 => {
                unsafe {
                    update_ratio_control(hwnd, &*state_ptr);
                    update_selected_path(hwnd, &*state_ptr);
                };
                None
            }
            SETTINGS_BROWSE_ID if unsafe { (*state_ptr).settings_adding } => {
                if let Some(path) = browse_executable(hwnd) {
                    set_control_text(hwnd, SETTINGS_SEARCH_ID, "");
                    unsafe { refresh_settings_choices(&mut *state_ptr, hwnd, Some(path)) };
                }
                None
            }
            SETTINGS_REMOVE_ID => unsafe { remove_selected_profile(hwnd, &mut *state_ptr) },
            SETTINGS_PROGRAM_TAB_ID => unsafe {
                let state = &mut *state_ptr;
                state.settings_global_view = false;
                state.settings_adding = false;
                set_control_text(hwnd, SETTINGS_SEARCH_ID, "");
                update_settings_mode(hwnd, state);
                refresh_settings_choices(state, hwnd, None);
                None
            },
            SETTINGS_GLOBAL_TAB_ID => unsafe {
                let state = &mut *state_ptr;
                state.settings_global_view = true;
                state.settings_adding = false;
                update_settings_mode(hwnd, state);
                update_global_ratio_control(hwnd, state);
                None
            },
            SETTINGS_ADD_ID => unsafe {
                let state = &mut *state_ptr;
                state.settings_global_view = false;
                state.settings_adding = !state.settings_adding;
                if state.settings_adding
                    && let Ok(running_apps) = running_process_executables()
                {
                    state.running_apps = running_apps;
                }
                set_control_text(hwnd, SETTINGS_SEARCH_ID, "");
                update_settings_mode(hwnd, state);
                refresh_settings_choices(state, hwnd, None);
                None
            },
            SETTINGS_ADD_SELECTED_ID => unsafe { add_selected_profile(hwnd, &mut *state_ptr) },
            SETTINGS_CLOSE_ID => unsafe {
                let _ = DestroyWindow(hwnd);
                None
            },
            _ => None,
        };
        if let Some(error) = error {
            show_error(&error);
        }
        return LRESULT(0);
    }
    if message == WM_CLOSE {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY && !state_ptr.is_null() {
        unsafe {
            (*state_ptr).settings_hwnd = HWND::default();
            (*state_ptr).settings_palette = None;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
    }
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn create_settings_controls(parent: HWND) -> Result<()> {
    unsafe {
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("NIJIMI"),
            0x50000000,
            [26, 22, 400, 34],
            SETTINGS_HEADER_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Ambient lighting for your apps"),
            0x50000000,
            [26, 54, 600, 24],
            SETTINGS_HELP_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Program Settings"),
            settings_button_style(),
            [26, 94, 190, 38],
            SETTINGS_PROGRAM_TAB_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Global Settings"),
            settings_button_style(),
            [224, 94, 190, 38],
            SETTINGS_GLOBAL_TAB_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("APP PROFILES"),
            0x50000000,
            [26, 154, 260, 22],
            SETTINGS_ADD_HELP_ID,
        )?;
        create_settings_control(
            parent,
            w!("EDIT"),
            w!(""),
            0x50000000 | 0x00800000 | 0x0080 | 0x00010000,
            [26, 182, 260, 34],
            SETTINGS_SEARCH_ID,
        )?;
        create_settings_control(
            parent,
            w!("LISTBOX"),
            w!(""),
            0x50000000 | 0x00800000 | 0x00200000 | 0x0001 | 0x00100000 | 0x00010000,
            [26, 226, 260, 306],
            SETTINGS_LIST_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Add app..."),
            settings_button_style(),
            [26, 546, 126, 38],
            SETTINGS_ADD_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Close"),
            settings_button_style(),
            [856, 24, 94, 36],
            SETTINGS_CLOSE_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Select an app"),
            0x50000000,
            [326, 132, 610, 28],
            SETTINGS_PROGRAM_TITLE_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Choose a profile from the list or add a new executable."),
            0x50000000,
            [326, 166, 610, 40],
            SETTINGS_PATH_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("SETTING"),
            0x50000000,
            [326, 226, 280, 22],
            SETTINGS_TABLE_SETTING_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("VALUE"),
            0x50000000,
            [626, 226, 310, 22],
            SETTINGS_TABLE_VALUE_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Content aspect ratio"),
            0x50000000,
            [326, 256, 280, 36],
            SETTINGS_RATIO_LABEL_ID,
        )?;
        create_settings_control(
            parent,
            w!("COMBOBOX"),
            w!("21:9"),
            0x50000000 | 0x0002 | 0x00200000 | 0x0040 | 0x00010000,
            [626, 250, 310, 148],
            SETTINGS_RATIO_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Remove app"),
            settings_button_style(),
            [326, 378, 160, 38],
            SETTINGS_REMOVE_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("The selected content area stays clear; ambient blur fills the space outside it."),
            0x50000000,
            [326, 304, 610, 48],
            SETTINGS_EMPTY_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Global Settings"),
            0x50000000,
            [326, 132, 610, 28],
            SETTINGS_GLOBAL_TITLE_ID,
        )?;
        create_settings_control(
            parent,
            w!("COMBOBOX"),
            w!("21:9"),
            0x50000000 | 0x0002 | 0x00200000 | 0x0040 | 0x00010000,
            [626, 250, 310, 148],
            SETTINGS_GLOBAL_RATIO_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("Change this value to set the starting content area for new app profiles."),
            0x50000000,
            [326, 166, 610, 40],
            SETTINGS_GLOBAL_HELP_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("New profiles start here; existing app settings are not changed."),
            0x50000000,
            [26, 154, 260, 112],
            SETTINGS_GLOBAL_SIDE_HELP_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!("RECENT AND RUNNING APPS"),
            0x50000000,
            [326, 132, 600, 26],
            SETTINGS_ADD_TITLE_ID,
        )?;
        create_settings_control(
            parent,
            w!("STATIC"),
            w!(
                "Recent foreground apps appear first, followed by accessible running processes. Browse if an app is missing."
            ),
            0x50000000,
            [326, 170, 600, 56],
            SETTINGS_ADD_DESCRIPTION_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Browse..."),
            settings_button_style(),
            [326, 248, 160, 38],
            SETTINGS_BROWSE_ID,
        )?;
        create_settings_control(
            parent,
            w!("BUTTON"),
            w!("Add selected"),
            settings_button_style(),
            [498, 248, 184, 38],
            SETTINGS_ADD_SELECTED_ID,
        )?;
        Ok(())
    }
}

fn settings_button_style() -> u32 {
    0x5001_000b
}

fn scale_layout(value: i32, dpi: u32) -> i32 {
    ((value as i64 * dpi.max(96) as i64 + 48) / 96) as i32
}

unsafe fn create_settings_control(
    parent: HWND,
    class: PCWSTR,
    text: PCWSTR,
    style: u32,
    bounds: [i32; 4],
    id: i32,
) -> Result<HWND> {
    unsafe {
        let instance = HINSTANCE(GetModuleHandleW(None)?.0);
        let dpi = GetDpiForWindow(parent);
        let [x, y, width, height] = bounds.map(|value| scale_layout(value, dpi));
        let control = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            text,
            WINDOW_STYLE(style),
            x,
            y,
            width,
            height,
            parent,
            HMENU(id as usize as *mut std::ffi::c_void),
            instance,
            None,
        )?;
        let font = GetStockObject(DEFAULT_GUI_FONT);
        let _ = SendMessageW(control, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        Ok(control)
    }
}

fn refresh_settings_choices(state: &mut AppState, hwnd: HWND, preferred: Option<PathBuf>) {
    let choices = if state.settings_adding {
        settings::add_app_choices(
            &state.recent_apps,
            &state.running_apps,
            &state.settings.profiles,
            &state.application_executable,
            preferred.as_deref(),
        )
    } else {
        state
            .settings
            .profiles
            .iter()
            .map(|profile| profile.executable.clone())
            .collect()
    };
    let query = read_control_text(hwnd, SETTINGS_SEARCH_ID);
    state.settings_choices = settings::filter_app_choices(&choices, &query);
    unsafe {
        let list = settings_control(hwnd, SETTINGS_LIST_ID);
        let _ = SendMessageW(list, LB_RESETCONTENT, WPARAM(0), LPARAM(0));
        let _ = SendMessageW(
            list,
            LB_SETHORIZONTALEXTENT_MESSAGE,
            WPARAM(1400),
            LPARAM(0),
        );
        let mut selected = None;
        for (index, path) in state.settings_choices.iter().enumerate() {
            let display_name = path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| path.as_os_str().to_string_lossy());
            let wide: Vec<u16> = display_name.encode_utf16().chain(Some(0)).collect();
            let _ = SendMessageW(
                list,
                LB_ADDSTRING,
                WPARAM(0),
                LPARAM(wide.as_ptr() as isize),
            );
            if preferred
                .as_ref()
                .is_some_and(|preferred| paths_match(preferred, path))
                || (preferred.is_none() && index == 0)
            {
                selected = Some(index);
            }
        }
        if let Some(selected) = selected {
            let _ = SendMessageW(list, LB_SETCURSEL, WPARAM(selected), LPARAM(0));
        }
        update_ratio_control(hwnd, state);
        update_selected_path(hwnd, state);
    }
}

fn update_settings_mode(hwnd: HWND, state: &AppState) {
    unsafe {
        let global = state.settings_global_view;
        let adding = state.settings_adding && !global;
        let program = !global && !adding;
        let _ = ShowWindow(
            settings_control(hwnd, SETTINGS_ADD_HELP_ID),
            if global { SW_HIDE } else { SW_SHOW },
        );
        for id in [SETTINGS_SEARCH_ID, SETTINGS_LIST_ID] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if global { SW_HIDE } else { SW_SHOW },
            );
        }
        for id in [
            SETTINGS_PROGRAM_TITLE_ID,
            SETTINGS_PATH_ID,
            SETTINGS_REMOVE_ID,
            SETTINGS_EMPTY_ID,
        ] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if program { SW_SHOW } else { SW_HIDE },
            );
        }
        for id in [SETTINGS_GLOBAL_TITLE_ID, SETTINGS_GLOBAL_HELP_ID] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if global { SW_SHOW } else { SW_HIDE },
            );
        }
        let _ = ShowWindow(
            settings_control(hwnd, SETTINGS_GLOBAL_SIDE_HELP_ID),
            if global { SW_SHOW } else { SW_HIDE },
        );
        for id in [SETTINGS_TABLE_SETTING_ID, SETTINGS_TABLE_VALUE_ID] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if adding { SW_HIDE } else { SW_SHOW },
            );
        }
        for (id, visible) in [
            (SETTINGS_RATIO_LABEL_ID, program || global),
            (SETTINGS_RATIO_ID, program),
            (SETTINGS_GLOBAL_RATIO_ID, global),
        ] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if visible { SW_SHOW } else { SW_HIDE },
            );
        }
        for id in [
            SETTINGS_ADD_TITLE_ID,
            SETTINGS_ADD_DESCRIPTION_ID,
            SETTINGS_BROWSE_ID,
            SETTINGS_ADD_SELECTED_ID,
        ] {
            let _ = ShowWindow(
                settings_control(hwnd, id),
                if adding { SW_SHOW } else { SW_HIDE },
            );
        }
        let _ = ShowWindow(
            settings_control(hwnd, SETTINGS_ADD_ID),
            if global { SW_HIDE } else { SW_SHOW },
        );
        set_control_text(
            hwnd,
            SETTINGS_ADD_ID,
            if adding {
                "Back to profiles"
            } else {
                "Add app..."
            },
        );
        set_control_text(
            hwnd,
            SETTINGS_ADD_HELP_ID,
            if adding {
                "APPS TO ADD"
            } else {
                "APP PROFILES"
            },
        );
        set_control_text(
            hwnd,
            SETTINGS_PROGRAM_TITLE_ID,
            if adding {
                "Choose an app to add"
            } else {
                "Select a profile to configure its content area."
            },
        );
        set_control_text(
            hwnd,
            SETTINGS_RATIO_LABEL_ID,
            if global {
                "Default ratio for new apps"
            } else {
                "Content aspect ratio"
            },
        );
        set_control_text(hwnd, SETTINGS_PROGRAM_TAB_ID, "Program Settings");
        set_control_text(hwnd, SETTINGS_GLOBAL_TAB_ID, "Global Settings");
        update_global_ratio_control(hwnd, state);
        update_ratio_control(hwnd, state);
        update_selected_path(hwnd, state);
    }
}

unsafe fn draw_settings_button(hwnd: HWND, item: &OwnerDrawItem, state: &AppState) -> bool {
    unsafe {
        let Some(palette) = state.settings_palette.as_ref() else {
            return false;
        };
        let id = item.ctl_id as i32;
        let selected_tab = (id == SETTINGS_PROGRAM_TAB_ID && !state.settings_global_view)
            || (id == SETTINGS_GLOBAL_TAB_ID && state.settings_global_view);
        let accent = selected_tab || matches!(id, SETTINGS_ADD_ID | SETTINGS_ADD_SELECTED_ID);
        let brush = if accent {
            palette.accent
        } else {
            palette.button
        };
        let _ = FillRect(item.hdc, &item.rect, brush);
        let _ = SetBkMode(item.hdc, TRANSPARENT);
        let _ = SetTextColor(item.hdc, COLORREF(rgb(245, 247, 251)));
        let mut text = [0u16; 128];
        let length = GetWindowTextW(item.hwnd_item, &mut text);
        if length > 0 {
            let mut rect = item.rect;
            let _ = DrawTextW(
                item.hdc,
                &mut text[..length as usize],
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        if item.item_state & 0x0010 != 0 {
            let _ = DrawFocusRect(item.hdc, &item.rect);
        }
        let _ = item.ctl_type;
        let _ = item.item_id;
        let _ = item.item_action;
        let _ = item.item_data;
        let _ = hwnd;
        true
    }
}

fn set_control_text(hwnd: HWND, id: i32, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = SetWindowTextW(settings_control(hwnd, id), PCWSTR(wide.as_ptr()));
    }
}

fn read_control_text(hwnd: HWND, id: i32) -> String {
    let mut text = [0u16; 2048];
    let length = unsafe { GetWindowTextW(settings_control(hwnd, id), &mut text) };
    if length <= 0 {
        String::new()
    } else {
        String::from_utf16_lossy(&text[..length as usize])
    }
}

fn selected_executable(hwnd: HWND, state: &AppState) -> Option<PathBuf> {
    unsafe {
        let selected = SendMessageW(
            settings_control(hwnd, SETTINGS_LIST_ID),
            LB_GETCURSEL,
            WPARAM(0),
            LPARAM(0),
        )
        .0;
        (selected >= 0)
            .then(|| state.settings_choices.get(selected as usize).cloned())
            .flatten()
    }
}

fn update_ratio_control(hwnd: HWND, state: &AppState) {
    let ratio = selected_executable(hwnd, state).and_then(|path| state.settings.ratio_for(&path));
    let Some(ratio) = ratio else {
        set_control_text(hwnd, SETTINGS_RATIO_ID, "");
        return;
    };
    let text: Vec<u16> = format!("{}:{}", ratio.width, ratio.height)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        let _ = SetWindowTextW(
            settings_control(hwnd, SETTINGS_RATIO_ID),
            PCWSTR(text.as_ptr()),
        );
    }
}

fn update_selected_path(hwnd: HWND, state: &AppState) {
    if state.settings_adding || state.settings_global_view {
        return;
    }
    let Some(path) = selected_executable(hwnd, state) else {
        set_control_text(hwnd, SETTINGS_PATH_ID, "No matching app selected.");
        return;
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    set_control_text(hwnd, SETTINGS_PROGRAM_TITLE_ID, &name);
    set_control_text(hwnd, SETTINGS_PATH_ID, &path.to_string_lossy());
}

fn update_global_ratio_control(hwnd: HWND, state: &AppState) {
    set_control_text(
        hwnd,
        SETTINGS_GLOBAL_RATIO_ID,
        &format!(
            "{}:{}",
            state.settings.default_aspect_ratio.width, state.settings.default_aspect_ratio.height
        ),
    );
}

fn initialize_ratio_choices(hwnd: HWND) {
    for (id, choices) in [
        (SETTINGS_RATIO_ID, ["16:9", "21:9", "32:9", "16:10", "4:3"]),
        (
            SETTINGS_GLOBAL_RATIO_ID,
            ["16:9", "21:9", "32:9", "16:10", "4:3"],
        ),
    ] {
        for choice in choices {
            let wide: Vec<u16> = choice.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let _ = SendMessageW(
                    settings_control(hwnd, id),
                    CB_ADDSTRING_MESSAGE,
                    WPARAM(0),
                    LPARAM(wide.as_ptr() as isize),
                );
            }
        }
    }
}

fn read_ratio_control(hwnd: HWND, id: i32) -> Option<AspectRatio> {
    let mut text = [0u16; 64];
    let length = unsafe { GetWindowTextW(settings_control(hwnd, id), &mut text) };
    if length <= 0 {
        return None;
    }
    AspectRatio::parse(&String::from_utf16_lossy(&text[..length as usize]))
}

fn settings_control(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(hwnd, id).unwrap_or_default() }
}

fn save_selected_profile(hwnd: HWND, state: &mut AppState) -> Option<String> {
    if state.settings_adding || state.settings_global_view {
        return None;
    }
    let Some(executable) = selected_executable(hwnd, state) else {
        update_ratio_control(hwnd, state);
        return None;
    };
    let Some(aspect_ratio) = read_ratio_control(hwnd, SETTINGS_RATIO_ID) else {
        update_ratio_control(hwnd, state);
        return Some("Enter a valid aspect ratio, such as 21:9.".to_owned());
    };
    if state.settings.ratio_for(&executable) == Some(aspect_ratio) {
        return None;
    }
    let previous = state.settings.clone();
    let effect = state
        .settings
        .profile_for(&executable)
        .and_then(|profile| profile.effect);
    state.settings.upsert(AppProfile {
        executable: executable.clone(),
        aspect_ratio,
        effect,
    });
    if let Err(error) = state.settings.save(&state.settings_path) {
        state.settings = previous;
        update_ratio_control(hwnd, state);
        return Some(format!("nijimi could not save settings:\n{error}"));
    }
    update_foreground(state)
}

fn save_global_ratio(hwnd: HWND, state: &mut AppState) -> Option<String> {
    if !state.settings_global_view {
        return None;
    }
    let Some(aspect_ratio) = read_ratio_control(hwnd, SETTINGS_GLOBAL_RATIO_ID) else {
        update_global_ratio_control(hwnd, state);
        return Some("Enter a valid default ratio, such as 21:9.".to_owned());
    };
    if state.settings.default_aspect_ratio == aspect_ratio {
        return None;
    }
    let previous = state.settings.clone();
    state.settings.default_aspect_ratio = aspect_ratio;
    if let Err(error) = state.settings.save(&state.settings_path) {
        state.settings = previous;
        update_global_ratio_control(hwnd, state);
        return Some(format!("nijimi could not save settings:\n{error}"));
    }
    None
}

fn add_selected_profile(hwnd: HWND, state: &mut AppState) -> Option<String> {
    if !state.settings_adding {
        return Some("Choose Add app... before adding an executable.".to_owned());
    }
    let Some(executable) = selected_executable(hwnd, state) else {
        return Some("Select a recent or running executable, or use Browse...".to_owned());
    };
    let profile = AppProfile {
        executable: executable.clone(),
        aspect_ratio: state.settings.default_aspect_ratio,
        effect: None,
    };
    let previous = state.settings.clone();
    state.settings.upsert(profile);
    if let Err(error) = state.settings.save(&state.settings_path) {
        state.settings = previous;
        return Some(format!("nijimi could not save settings:\n{error}"));
    }
    state.settings_adding = false;
    update_settings_mode(hwnd, state);
    refresh_settings_choices(state, hwnd, Some(executable));
    update_foreground(state)
}

fn remove_selected_profile(hwnd: HWND, state: &mut AppState) -> Option<String> {
    if state.settings_adding {
        return Some("Return to profiles before removing an app.".to_owned());
    }
    let executable = selected_executable(hwnd, state)?;
    let previous = state.settings.clone();
    if !state.settings.remove(&executable) {
        return Some("The selected executable has no saved aspect ratio.".to_owned());
    }
    if let Err(error) = state.settings.save(&state.settings_path) {
        state.settings = previous;
        return Some(format!("nijimi could not save settings:\n{error}"));
    }
    refresh_settings_choices(state, hwnd, Some(executable));
    update_foreground(state)
}

fn browse_executable(owner: HWND) -> Option<PathBuf> {
    let filter: Vec<u16> = "Executable files\0*.exe\0All files\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title: Vec<u16> = "Select executable\0".encode_utf16().collect();
    let mut file = [0u16; 32768];
    let mut dialog = OpenFileNameW {
        l_struct_size: size_of::<OpenFileNameW>() as u32,
        hwnd_owner: owner,
        h_instance: HINSTANCE::default(),
        filter: PCWSTR(filter.as_ptr()),
        custom_filter: PWSTR::null(),
        max_custom_filter: 0,
        filter_index: 1,
        file: PWSTR(file.as_mut_ptr()),
        max_file: file.len() as u32,
        file_title: PWSTR::null(),
        max_file_title: 0,
        initial_dir: PCWSTR::null(),
        title: PCWSTR(title.as_ptr()),
        flags: 0x0000_1000 | 0x0000_0800 | 0x0000_0008,
        file_offset: 0,
        file_extension: 0,
        default_extension: PCWSTR::null(),
        custom_data: LPARAM(0),
        hook: None,
        template_name: PCWSTR::null(),
        reserved: std::ptr::null_mut(),
        reserved_value: 0,
        flags_ex: 0,
    };
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return None;
    }
    let length = file.iter().position(|character| *character == 0)?;
    Some(PathBuf::from(std::ffi::OsString::from_wide(
        &file[..length],
    )))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{TRAY_WINDOW_TITLE, ensure_dpi_awareness, order_process_executables};
    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, FindWindowW, GetWindowTextW, HMENU, WINDOW_EX_STYLE,
        WINDOW_STYLE,
    };
    use windows::core::{PCWSTR, w};

    #[test]
    fn tray_window_title_does_not_collide_with_settings_lookup() {
        unsafe {
            let tray = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                TRAY_WINDOW_TITLE,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                HINSTANCE::default(),
                None,
            )
            .unwrap();
            let settings = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR(crate::settings_ui::WINDOW_TITLE_WIDE.as_ptr()),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND::default(),
                HMENU::default(),
                HINSTANCE::default(),
                None,
            )
            .unwrap();
            let found = FindWindowW(
                PCWSTR::null(),
                PCWSTR(crate::settings_ui::WINDOW_TITLE_WIDE.as_ptr()),
            )
            .unwrap_or_default();
            let mut found_title = [0u16; 64];
            let found_title_len = if found.0.is_null() {
                0
            } else {
                GetWindowTextW(found, &mut found_title)
            };
            let _ = DestroyWindow(settings);
            let _ = DestroyWindow(tray);

            assert!(!found.0.is_null());
            assert_ne!(found, tray);
            assert_eq!(
                String::from_utf16_lossy(&found_title[..found_title_len as usize]),
                crate::settings_ui::WINDOW_TITLE
            );
        }
    }

    #[test]
    fn orders_processes_newest_first_and_keeps_newest_duplicate_path() {
        let executables = order_process_executables(vec![
            (PathBuf::from(r"C:\Apps\Example.exe"), Some(10)),
            (PathBuf::from(r"D:\Apps\Example.exe"), Some(20)),
            (PathBuf::from(r"c:\apps\EXAMPLE.EXE"), Some(30)),
            (PathBuf::from(r"F:\Apps\Other.exe"), Some(15)),
        ]);

        assert_eq!(
            executables,
            vec![
                PathBuf::from(r"c:\apps\EXAMPLE.EXE"),
                PathBuf::from(r"D:\Apps\Example.exe"),
                PathBuf::from(r"F:\Apps\Other.exe"),
            ]
        );
    }

    #[test]
    fn places_processes_with_unavailable_creation_times_after_known_times() {
        let executables = order_process_executables(vec![
            (PathBuf::from(r"C:\Apps\Unknown.exe"), None),
            (PathBuf::from(r"D:\Apps\Newer.exe"), Some(20)),
            (PathBuf::from(r"E:\Apps\Older.exe"), Some(10)),
            (PathBuf::from(r"c:\apps\UNKNOWN.EXE"), None),
        ]);

        assert_eq!(
            executables,
            vec![
                PathBuf::from(r"D:\Apps\Newer.exe"),
                PathBuf::from(r"E:\Apps\Older.exe"),
                PathBuf::from(r"C:\Apps\Unknown.exe"),
            ]
        );
    }

    #[test]
    fn dpi_awareness_initialization_is_idempotent() {
        ensure_dpi_awareness().expect("first DPI initialization should succeed");
        ensure_dpi_awareness().expect("repeated DPI initialization should succeed");
    }
}
