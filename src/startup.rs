//! "Start with Windows" via the per-user `Run` registry key.

use std::io::{Error, Result};
use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};
use windows::core::{PCWSTR, w};

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
/// Where Windows' Settings > Apps > Startup and Task Manager record that a user turned a
/// startup entry off, without removing it from `Run`.
const STARTUP_APPROVED_KEY: PCWSTR =
    w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run");
const VALUE_NAME: PCWSTR = w!("nijimi");
/// Passed by the startup entry so signing in starts nijimi in the tray without opening Settings.
pub const BACKGROUND_ARGUMENT: &str = "--background";

/// True if Windows will launch *this* executable at sign-in. An entry left behind by a copy
/// that has since been moved counts as off, so turning the toggle on repairs the path.
pub fn is_enabled() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let Some(command) = read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) else {
        return false;
    };
    command_matches(&command, &executable) && !disabled_in_windows_settings()
}

pub fn set_enabled(enabled: bool) -> Result<()> {
    if enabled {
        let executable = std::env::current_exe()?;
        let command = command_for(&executable);
        let data: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        unsafe {
            check(RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE_NAME,
                REG_SZ.0,
                Some(data.as_ptr().cast()),
                (data.len() * size_of::<u16>()) as u32,
            ))?;
        }
        // Clear any "disabled" flag from Task Manager / Settings so the entry really runs.
        delete_value(STARTUP_APPROVED_KEY, VALUE_NAME)
    } else {
        delete_value(RUN_KEY, VALUE_NAME)?;
        delete_value(STARTUP_APPROVED_KEY, VALUE_NAME)
    }
}

/// Rewrites an enabled entry whose command is out of date, such as one saved by a version that
/// didn't pass [`BACKGROUND_ARGUMENT`].
pub fn refresh() -> Result<()> {
    let executable = std::env::current_exe()?;
    if is_enabled()
        && read_string(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) != Some(command_for(&executable))
    {
        set_enabled(true)?;
    }
    Ok(())
}

fn command_for(executable: &Path) -> String {
    format!("\"{}\" {BACKGROUND_ARGUMENT}", executable.display())
}

fn command_matches(command: &str, executable: &Path) -> bool {
    let command = command.trim();
    let registered = match command
        .strip_prefix('"')
        .and_then(|rest| rest.split_once('"'))
    {
        Some((path, _)) => path,
        None => command,
    };
    registered.eq_ignore_ascii_case(&executable.to_string_lossy())
}

fn disabled_in_windows_settings() -> bool {
    // The first byte is 0x02 when enabled and 0x03 when the user has disabled the entry.
    let mut data = [0u8; 12];
    let mut size = data.len() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            STARTUP_APPROVED_KEY,
            VALUE_NAME,
            RRF_RT_REG_BINARY,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    status.is_ok() && size > 0 && data[0] == 0x03
}

pub(crate) fn read_string(root: HKEY, key: PCWSTR, value: PCWSTR) -> Option<String> {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(root, key, value, RRF_RT_REG_SZ, None, None, Some(&mut size))
            .ok()
            .ok()?;
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        RegGetValueW(
            root,
            key,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

fn delete_value(key: PCWSTR, value: PCWSTR) -> Result<()> {
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key, value) };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(());
    }
    check(status)
}

fn check(status: WIN32_ERROR) -> Result<()> {
    if status.is_ok() {
        Ok(())
    } else {
        Err(Error::from_raw_os_error(status.0 as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_command_quotes_the_path_and_matches_it_back() {
        let executable = Path::new(r"C:\Program Files\nijimi\nijimi.exe");
        let command = command_for(executable);
        assert_eq!(
            command,
            r#""C:\Program Files\nijimi\nijimi.exe" --background"#
        );
        assert!(command_matches(&command, executable));
        assert!(
            command_matches(r#""c:\program files\NIJIMI\nijimi.exe""#, executable),
            "entries from before --background still count, so they can be refreshed"
        );
        assert!(!command_matches(r#""D:\Old\nijimi.exe""#, executable));
    }
}
