#![cfg_attr(windows, windows_subsystem = "windows")]

mod geometry;
mod settings;

#[cfg(windows)]
mod app;

#[cfg(windows)]
mod app_icon;

#[cfg(windows)]
mod frame_timer;

#[cfg(windows)]
mod render;

#[cfg(windows)]
mod settings_ui;

#[cfg(windows)]
mod startup;

#[cfg(windows)]
mod system_info;

#[cfg(windows)]
fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first
        .as_ref()
        .is_some_and(|argument| argument == "--settings")
    {
        let recent_apps = if args.next().is_some_and(|argument| argument == "--recent") {
            args.map(std::path::PathBuf::from).collect()
        } else {
            Vec::new()
        };
        if let Err(error) = settings_ui::run(recent_apps) {
            app::show_error(&format!("nijimi could not start Settings:\n{error}"));
        }
    } else if let Err(error) =
        app::run(first.is_none_or(|argument| argument != startup::BACKGROUND_ARGUMENT))
    {
        app::show_error(&format!("nijimi could not start:\n{error}"));
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("nijimi is a Windows-only application.");
}
