use std::collections::HashSet;
use std::mem::size_of;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use std::os::windows::ffi::OsStringExt;

use windows::Win32::Foundation::{
    BOOL, CloseHandle, ERROR_ALREADY_EXISTS, FILETIME, HANDLE, HINSTANCE, HWND, LPARAM, LRESULT,
    POINT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    MONITOR_DEFAULTTONULL, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    CreateMutexW, GetProcessTimes, INFINITE, OpenProcess, PROCESS_NAME_FORMAT,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
    WaitForSingleObject,
};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    GetThreadDpiAwarenessContext, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW, GWLP_USERDATA,
    GetCursorPos, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowThreadProcessId,
    HICON, HMENU, IDC_ARROW, KillTimer, LoadCursorW, MENU_ITEM_FLAGS, MF_STRING, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetTimer, SetWindowLongPtrW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_LBUTTONUP, WM_NCCREATE, WM_NCDESTROY, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};
use windows::core::{Error, PCWSTR, PWSTR, Result, w};

use crate::frame_timer::FrameTimer;
use crate::render::Renderer;
use crate::settings::{self, ActiveProfile, AppSettings, paths_match};

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
const TRAY_WINDOW_TITLE: PCWSTR = w!("nijimi tray");

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
    recent_apps: Vec<PathBuf>,
    tray_status: TrayStatus,
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
        renderer.set_hide_idle_cursor(settings.hide_idle_cursor);
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
            recent_apps: Vec::new(),
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
                    state
                        .renderer
                        .set_hide_idle_cursor(settings.hide_idle_cursor);
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
    state.renderer.set_show_overlays(selected.show_overlays);
    if let Err(error) = state
        .renderer
        .configure(selected.content_area, selected.effect)
    {
        stop_after_error(state);
        return Some(format!(
            "nijimi could not apply the selected content area:\n{error}"
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

const TRAY_PROCESS_VARIABLE: &str = "NIJIMI_TRAY_PROCESS";

/// Exits Settings when the tray that opened it exits, however it exits. Closing the window from
/// the tray can miss it, e.g. if Settings is still starting when Exit is chosen.
pub(crate) fn exit_with_tray() {
    let Some(process_id) = std::env::var(TRAY_PROCESS_VARIABLE)
        .ok()
        .and_then(|id| id.parse::<u32>().ok())
    else {
        return;
    };
    // Holding the handle stops the ID being reused before the executable is checked.
    let Ok(tray) = (unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, process_id) }) else {
        std::process::exit(0);
    };
    let is_tray = process_executable(process_id)
        .zip(std::env::current_exe().ok())
        .is_some_and(|(tray, executable)| paths_match(&tray, &executable));
    if !is_tray {
        std::process::exit(0);
    }
    let raw_tray = tray.0 as isize;
    let _ = std::thread::Builder::new()
        .name("nijimi tray watch".into())
        .spawn(move || {
            unsafe { WaitForSingleObject(HANDLE(raw_tray as *mut std::ffi::c_void), INFINITE) };
            std::process::exit(0);
        });
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
        process.env(TRAY_PROCESS_VARIABLE, std::process::id().to_string());
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
