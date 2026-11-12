//! A plain-text summary of the system and settings for issue reports. It names no apps or paths.

use std::path::Path;

use windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIDevice, IDXGIFactory2, IDXGIOutput6,
};
use windows::Win32::Graphics::Gdi::{DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplaySettingsW};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
use windows::core::{Interface, PCWSTR, w};

use crate::render::find_primary_output;
use crate::settings::AppSettings;
use crate::startup::read_string;

const CURRENT_VERSION_KEY: PCWSTR = w!(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
const PROCESSOR_KEY: PCWSTR = w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");

pub fn summary(settings: &AppSettings, settings_path: &Path) -> String {
    let on_off = |value: bool| if value { "on" } else { "off" };
    let executable_dir = std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_path_buf));
    let settings_location = if executable_dir.as_deref() == settings_path.parent() {
        "beside the executable"
    } else {
        r"%APPDATA%\nijimi"
    };
    let custom_looks = settings
        .profiles
        .iter()
        .filter(|profile| profile.effect.is_some())
        .count();
    let mut lines = vec![
        format!("nijimi {}", env!("CARGO_PKG_VERSION")),
        windows_version(),
        cpu(),
    ];
    lines.extend(gpu_and_display());
    lines.extend([
        format!("Monitors: {}", unsafe { GetSystemMetrics(SM_CMONITORS) }),
        format!(
            "Frame rate: {} fps, Always on: {}, Show in screenshots: {}, Hide idle cursor: {}",
            settings.frame_rate.fps(),
            on_off(settings.always_on),
            on_off(settings.show_in_screenshots),
            on_off(settings.hide_idle_cursor),
        ),
        format!("Default look: {}", settings.default_effect.serialize()),
        format!(
            "Profiles: {} ({custom_looks} with their own look)",
            settings.profiles.len()
        ),
        format!("Settings file: {settings_location}"),
    ]);
    lines.join("\n")
}

fn windows_version() -> String {
    let build = read_string(
        HKEY_LOCAL_MACHINE,
        CURRENT_VERSION_KEY,
        w!("CurrentBuildNumber"),
    );
    let release = read_string(
        HKEY_LOCAL_MACHINE,
        CURRENT_VERSION_KEY,
        w!("DisplayVersion"),
    );
    let revision = read_dword(CURRENT_VERSION_KEY, w!("UBR"));
    let name = match build.as_deref().and_then(|build| build.parse::<u32>().ok()) {
        Some(build) if build >= 22000 => "Windows 11",
        Some(_) => "Windows 10",
        None => "Windows",
    };
    let mut version = name.to_string();
    if let Some(release) = release {
        version.push_str(&format!(" {release}"));
    }
    if let Some(build) = build {
        version.push_str(&format!(" (build {build}"));
        if let Some(revision) = revision {
            version.push_str(&format!(".{revision}"));
        }
        version.push(')');
    }
    version
}

fn cpu() -> String {
    let name = read_string(HKEY_LOCAL_MACHINE, PROCESSOR_KEY, w!("ProcessorNameString"))
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    match std::thread::available_parallelism() {
        Ok(threads) => format!("CPU: {name} ({threads} threads)"),
        Err(_) => format!("CPU: {name}"),
    }
}

fn gpu_and_display() -> Vec<String> {
    let primary = unsafe { CreateDXGIFactory1::<IDXGIFactory2>() }
        .ok()
        .and_then(|factory| find_primary_output(&factory).ok());
    let Some((adapter, output)) = primary else {
        return vec!["GPU and display: unknown".to_string()];
    };
    unsafe {
        let gpu = adapter
            .GetDesc1()
            .map(|desc| utf16(&desc.Description))
            .unwrap_or_else(|_| "unknown".to_string());
        let gpu = match adapter.CheckInterfaceSupport(&IDXGIDevice::IID) {
            Ok(version) => {
                let version = version as u64;
                format!(
                    "GPU: {gpu} (driver {}.{}.{}.{})",
                    version >> 48,
                    (version >> 32) & 0xffff,
                    (version >> 16) & 0xffff,
                    version & 0xffff
                )
            }
            Err(_) => format!("GPU: {gpu}"),
        };

        let Ok(desc) = output.GetDesc() else {
            return vec![gpu, "Primary display: unknown".to_string()];
        };
        let mut mode = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        let mode = if EnumDisplaySettingsW(
            PCWSTR(desc.DeviceName.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut mode,
        )
        .as_bool()
        {
            format!(
                "{}x{} at {} Hz",
                mode.dmPelsWidth, mode.dmPelsHeight, mode.dmDisplayFrequency
            )
        } else {
            "unknown mode".to_string()
        };
        let (mut dpi, mut unused) = (0, 0);
        let scaling = GetDpiForMonitor(desc.Monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut unused)
            .map(|()| format!("{}% scaling", dpi * 100 / 96))
            .unwrap_or_else(|_| "unknown scaling".to_string());
        let hdr = match output
            .cast::<IDXGIOutput6>()
            .and_then(|output| output.GetDesc1())
        {
            Ok(desc) if desc.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 => "on",
            Ok(_) => "off",
            Err(_) => "unknown",
        };
        vec![
            gpu,
            format!("Primary display: {mode}, {scaling}, HDR {hdr}"),
        ]
    }
}

fn read_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32).cast()),
            Some(&mut size),
        )
    }
    .is_ok()
    .then_some(data)
}

fn utf16(text: &[u16]) -> String {
    let length = text.iter().position(|&c| c == 0).unwrap_or(text.len());
    String::from_utf16_lossy(&text[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_covers_the_system_without_naming_apps() {
        let mut settings = AppSettings::default();
        settings.upsert(crate::settings::AppProfile {
            executable: r"C:\Users\someone\Games\Secret.exe".into(),
            aspect_ratio: crate::geometry::AspectRatio::new(16, 9).unwrap(),
            effect: None,
        });
        let summary = summary(&settings, Path::new(r"C:\Elsewhere\nijimi.ini"));
        assert!(summary.starts_with(&format!("nijimi {}", env!("CARGO_PKG_VERSION"))));
        assert!(summary.contains("Windows"));
        assert!(summary.contains("CPU: "));
        assert!(summary.contains("Profiles: 1 (0 with their own look)"));
        assert!(!summary.contains("Secret") && !summary.contains("someone"));
    }
}
