use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    TitleBar,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    switch::Switch,
    theme::{Theme, ThemeMode},
};
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, Context, Entity, IntoElement,
    ParentElement as _, Render, RenderImage, Styled as _, Subscription, SvgRenderer,
    Transformation, Window, WindowBounds, WindowOptions, div, img, px, radians, rgb, size, svg,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, SetForegroundWindow};
use windows::core::{PCWSTR, w};

use crate::geometry::AspectRatio;
use crate::settings::{self, AppProfile, AppSettings, paths_match};
use crate::{app, app_icon, startup};

pub const WINDOW_TITLE: &str = "um•bee•ants";
pub const WINDOW_TITLE_WIDE: &[u16] =
    &[117, 109, 0x2022, 98, 101, 101, 0x2022, 97, 110, 116, 115, 0];
const ASPECT_PRESETS: [&str; 5] = ["16:9", "21:9", "32:9", "16:10", "4:3"];

fn render_logo() -> Result<Arc<RenderImage>, String> {
    SvgRenderer::new(Arc::new(()))
        // Mark only (800x800); the wordmark is drawn as real text so it gets proper hinting/AA.
        // Rasterized at 2x the 20px display size so it stays crisp on high-DPI displays.
        .render_single_frame(include_bytes!("../assets/tresbient-logo.svg"), 0.05)
        .map_err(|error| format!("Could not render the settings logo: {error}"))
}

struct SettingsMutex(HANDLE);

impl Drop for SettingsMutex {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn run(recent_apps: Vec<PathBuf>) -> std::result::Result<(), String> {
    let mutex = unsafe {
        CreateMutexW(None, true, w!("Local\\UmBeeAntsSettingsWindow"))
            .map_err(|error| error.to_string())?
    };
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(mutex) };
        unsafe {
            let window = FindWindowW(
                windows::core::PCWSTR::null(),
                PCWSTR(WINDOW_TITLE_WIDE.as_ptr()),
            )
            .unwrap_or_default();
            if !window.0.is_null() {
                let _ = SetForegroundWindow(window);
            }
        }
        return Ok(());
    }
    let _settings_mutex = SettingsMutex(mutex);
    let settings_path = settings::config_path().map_err(|error| error.to_string())?;
    let app_settings = AppSettings::load(&settings_path).map_err(|error| error.to_string())?;
    let application_executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let initial_data = InitialData {
        logo: render_logo()?,
        settings_path,
        settings: app_settings,
        application_executable,
        recent_apps,
    };

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            Theme::change(ThemeMode::Dark, None, cx);
            let bounds = Bounds::centered(None, size(px(1120.), px(760.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(860.), px(620.))),
                ..TitleBar::window_options()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, move |window, cx| {
                window.set_window_title(WINDOW_TITLE);
                cx.new(|cx| SettingsView::new(initial_data, window, cx))
            }) {
                app::show_error(&format!("um•bee•ants could not open:\n{error}"));
                cx.quit();
                return;
            }
            if let Ok(window) = unsafe {
                FindWindowW(
                    windows::core::PCWSTR::null(),
                    PCWSTR(WINDOW_TITLE_WIDE.as_ptr()),
                )
            } && !window.0.is_null()
                && let Err(error) = app_icon::set_window_icons(window)
            {
                app::show_error(&format!("Could not set the Settings window icon:\n{error}"));
            }
            cx.activate(true);
        });
    Ok(())
}

struct InitialData {
    logo: Arc<RenderImage>,
    settings_path: PathBuf,
    settings: AppSettings,
    application_executable: PathBuf,
    recent_apps: Vec<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Program,
    Global,
    Add,
}

fn updated_settings(
    settings: &AppSettings,
    page: SettingsPage,
    selected_profile: Option<&Path>,
    value: &str,
) -> Option<AppSettings> {
    let ratio = AspectRatio::parse(value)?;
    let mut updated = settings.clone();
    match page {
        SettingsPage::Global => updated.default_aspect_ratio = ratio,
        SettingsPage::Program => {
            let executable = selected_profile?;
            updated.upsert(AppProfile {
                executable: executable.to_path_buf(),
                aspect_ratio: ratio,
            });
        }
        SettingsPage::Add => return None,
    }
    (updated != *settings).then_some(updated)
}

struct SettingsView {
    logo: Arc<RenderImage>,
    settings_path: PathBuf,
    settings: AppSettings,
    application_executable: PathBuf,
    recent_apps: Vec<PathBuf>,
    running_apps: Vec<PathBuf>,
    refreshing_apps: bool,
    refresh_animation_id: Option<u64>,
    icons: HashMap<PathBuf, Option<Arc<RenderImage>>>,
    pending_icons: HashSet<PathBuf>,
    selected_profile: Option<PathBuf>,
    selected_candidate: Option<PathBuf>,
    page: SettingsPage,
    search: Entity<InputState>,
    ratio: Entity<InputState>,
    search_query: String,
    suppress_ratio_events: bool,
    start_with_windows: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(data: InitialData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected_profile = data
            .settings
            .profiles
            .first()
            .map(|profile| profile.executable.clone());
        let initial_ratio = selected_profile
            .as_deref()
            .and_then(|path| data.settings.ratio_for(path))
            .unwrap_or(data.settings.default_aspect_ratio);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search apps or paths"));
        let ratio = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{}:{}", initial_ratio.width, initial_ratio.height))
        });

        let search_subscription = cx.subscribe_in(&search, window, |this, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = input.read(cx).value().to_string();
                cx.notify();
            }
        });
        let ratio_subscription =
            cx.subscribe_in(&ratio, window, |this, input, event, window, cx| {
                if this.suppress_ratio_events {
                    return;
                }
                match event {
                    InputEvent::Change => {
                        let value = input.read(cx).value().to_string();
                        this.save_ratio(&value, window, cx);
                    }
                    InputEvent::Blur => this.restore_ratio(window, cx),
                    _ => {}
                }
            });

        Self {
            logo: data.logo,
            settings_path: data.settings_path,
            settings: data.settings,
            application_executable: data.application_executable,
            recent_apps: data.recent_apps,
            running_apps: Vec::new(),
            refreshing_apps: false,
            refresh_animation_id: None,
            icons: HashMap::new(),
            pending_icons: HashSet::new(),
            selected_profile,
            selected_candidate: None,
            page: SettingsPage::Program,
            search,
            ratio,
            search_query: String::new(),
            suppress_ratio_events: false,
            start_with_windows: startup::is_enabled(),
            error: None,
            _subscriptions: vec![search_subscription, ratio_subscription],
        }
    }

    fn set_ratio_text(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.suppress_ratio_events = true;
        self.ratio.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
        self.suppress_ratio_events = false;
    }

    fn current_ratio(&self) -> Option<AspectRatio> {
        match self.page {
            SettingsPage::Global => Some(self.settings.default_aspect_ratio),
            SettingsPage::Program => self
                .selected_profile
                .as_deref()
                .and_then(|path| self.settings.ratio_for(path)),
            SettingsPage::Add => None,
        }
    }

    fn sync_ratio_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .current_ratio()
            .map(|ratio| format!("{}:{}", ratio.width, ratio.height))
            .unwrap_or_default();
        self.set_ratio_text(&value, window, cx);
    }

    fn restore_ratio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if AspectRatio::parse(&self.ratio.read(cx).value()).is_none() {
            self.sync_ratio_text(window, cx);
        }
    }

    fn save_ratio(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(updated) = updated_settings(
            &self.settings,
            self.page,
            self.selected_profile.as_deref(),
            value,
        ) else {
            return;
        };
        match updated.save(&self.settings_path) {
            Ok(()) => {
                self.settings = updated;
                self.error = None;
                app::notify_settings_changed();
            }
            Err(error) => {
                self.error = Some(format!("Could not save settings: {error}"));
                self.sync_ratio_text(window, cx);
            }
        }
        cx.notify();
    }

    fn set_page(&mut self, page: SettingsPage, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page;
        self.error = None;
        self.sync_ratio_text(window, cx);
        cx.notify();
    }

    fn select_profile(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.page = SettingsPage::Program;
        self.selected_profile = Some(path);
        self.error = None;
        self.sync_ratio_text(window, cx);
        cx.notify();
    }

    fn filtered_profiles(&self) -> Vec<PathBuf> {
        let paths: Vec<PathBuf> = self
            .settings
            .profiles
            .iter()
            .map(|profile| profile.executable.clone())
            .collect();
        settings::filter_app_choices(&paths, &self.search_query)
    }

    fn filtered_candidates(&self) -> Vec<PathBuf> {
        let choices = settings::add_app_choices(
            &self.recent_apps,
            &self.running_apps,
            &self.settings.profiles,
            &self.application_executable,
            None,
        );
        settings::filter_app_choices(&choices, &self.search_query)
    }

    fn refresh_running_apps(&mut self, cx: &mut Context<Self>) {
        if self.refreshing_apps {
            return;
        }
        self.refreshing_apps = true;
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async { app::running_process_executables() })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.refreshing_apps = false;
                match result {
                    Ok(apps) => this.running_apps = apps,
                    Err(error) => {
                        this.error = Some(format!("Could not refresh running apps: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn load_icons(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let missing: Vec<_> = paths
            .iter()
            .filter(|path| !self.icons.contains_key(*path) && !self.pending_icons.contains(*path))
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        self.pending_icons.extend(missing.iter().cloned());
        cx.spawn(async move |view, cx| {
            for path in missing {
                let icon_path = path.clone();
                let icon = cx
                    .background_spawn(async move { app_icon::executable_icon(&icon_path) })
                    .await;
                if view
                    .update(cx, |this, cx| {
                        this.pending_icons.remove(&path);
                        this.icons.insert(path, icon);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn add_candidate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(executable) = self.selected_candidate.clone() else {
            self.error =
                Some("Select a recent or running app, or browse for an executable.".into());
            cx.notify();
            return;
        };
        if self.settings.ratio_for(&executable).is_some() {
            self.select_profile(executable, window, cx);
            return;
        }
        let mut updated = self.settings.clone();
        updated.upsert(AppProfile {
            executable: executable.clone(),
            aspect_ratio: self.settings.default_aspect_ratio,
        });
        if let Err(error) = updated.save(&self.settings_path) {
            self.error = Some(format!("Could not save settings: {error}"));
            cx.notify();
            return;
        }
        self.settings = updated;
        self.selected_profile = Some(executable);
        self.selected_candidate = None;
        self.page = SettingsPage::Program;
        self.error = None;
        self.sync_ratio_text(window, cx);
        app::notify_settings_changed();
        cx.notify();
    }

    fn remove_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(executable) = self.selected_profile.as_deref() else {
            return;
        };
        let mut updated = self.settings.clone();
        if !updated.remove(executable) {
            return;
        }
        if let Err(error) = updated.save(&self.settings_path) {
            self.error = Some(format!("Could not save settings: {error}"));
            cx.notify();
            return;
        }
        self.settings = updated;
        self.selected_profile = self
            .settings
            .profiles
            .first()
            .map(|profile| profile.executable.clone());
        self.error = None;
        self.sync_ratio_text(window, cx);
        app::notify_settings_changed();
        cx.notify();
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = app::browse_executable_for_settings() {
            self.selected_candidate = Some(path);
            self.error = None;
            cx.notify();
        }
    }

    fn error_banner(&self) -> impl IntoElement {
        if let Some(error) = &self.error {
            div()
                .w_full()
                .px_4()
                .py_3()
                .rounded_md()
                .bg(rgb(0x3a2024))
                .text_color(rgb(0xffc4c9))
                .child(error.clone())
        } else {
            div()
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let program_style = if self.page == SettingsPage::Program || self.page == SettingsPage::Add
        {
            Button::new("program-settings")
                .label("Program settings")
                .primary()
        } else {
            Button::new("program-settings")
                .label("Program settings")
                .ghost()
        }
        .on_click(cx.listener(|this, _, window, cx| {
            this.set_page(SettingsPage::Program, window, cx);
        }));
        let global_style = if self.page == SettingsPage::Global {
            Button::new("global-settings")
                .label("Global settings")
                .primary()
        } else {
            Button::new("global-settings")
                .label("Global settings")
                .ghost()
        }
        .on_click(cx.listener(|this, _, window, cx| {
            this.set_page(SettingsPage::Global, window, cx);
        }));

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(
                TitleBar::new()
                    .bg(rgb(0x202020))
                    .border_color(rgb(0x383838))
                    .pl(px(6.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .gap(px(6.))
                            .child(img(self.logo.clone()).size(px(20.)))
                            .child(
                                div()
                                    .relative()
                                    // The wordmark is all lowercase, so its visual centre sits
                                    // below the line box's centre; nudge it up to align with the mark.
                                    .top(px(-2.))
                                    .text_sm()
                                    .text_color(rgb(0xf0f0f0))
                                    .child(WINDOW_TITLE),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_7()
                    .py_3()
                    .child(program_style)
                    .child(global_style),
            )
            .child(div().h(px(1.)).bg(rgb(0x383838)))
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let paths = match self.page {
            SettingsPage::Add => self.filtered_candidates(),
            SettingsPage::Program | SettingsPage::Global => self.filtered_profiles(),
        };
        self.load_icons(&paths, cx);
        let mut rows = Vec::new();
        for (index, path) in paths.into_iter().enumerate() {
            let selected = match self.page {
                SettingsPage::Add => self
                    .selected_candidate
                    .as_deref()
                    .is_some_and(|candidate| paths_match(candidate, &path)),
                _ => self
                    .selected_profile
                    .as_deref()
                    .is_some_and(|profile| paths_match(profile, &path)),
            };
            let label = file_name(&path);
            let callback_path = path.clone();
            let mut icon = div().size(px(22.)).flex_shrink_0();
            if let Some(Some(image)) = self.icons.get(&path) {
                icon = icon.child(img(image.clone()).size(px(22.)));
            } else {
                icon = icon.rounded_sm().bg(rgb(0x3a3a3a));
            }
            let button = Button::new(format!("application-{index}"))
                .accessibility_label(label.clone())
                .tooltip(path.to_string_lossy().into_owned())
                .w_full()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .w_full()
                        .min_w_0()
                        .child(icon)
                        .child(div().min_w_0().text_ellipsis().child(label)),
                );
            let button = if selected {
                button.primary()
            } else {
                button.ghost()
            };
            let button = match self.page {
                SettingsPage::Add => button.on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_candidate = Some(callback_path.clone());
                    this.error = None;
                    cx.notify();
                })),
                _ => button.on_click(cx.listener(move |this, _, window, cx| {
                    this.select_profile(callback_path.clone(), window, cx);
                })),
            };
            rows.push(button);
        }

        let title = if self.page == SettingsPage::Add {
            "RECENT & RUNNING APPS"
        } else {
            "APP PROFILES"
        };
        let list = if rows.is_empty() {
            div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .px_4()
                .text_sm()
                .text_color(rgb(0xa6a6a6))
                .child(if self.page == SettingsPage::Add {
                    "No matching apps found"
                } else {
                    "No app profiles yet"
                })
        } else {
            div().flex_1().min_h_0().child(
                div()
                    .flex()
                    .flex_col()
                    .size_full()
                    .gap_1()
                    .overflow_y_scrollbar()
                    .children(rows),
            )
        };
        let action = if self.page == SettingsPage::Add {
            div()
                .flex()
                .gap_2()
                .child(
                    Button::new("browse-app")
                        .label("Browse…")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                )
                .child(
                    Button::new("add-selected-app")
                        .label("Add selected")
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.add_candidate(window, cx)),
                        ),
                )
                .child(
                    Button::new("refresh-apps")
                        .accessibility_label(if self.refreshing_apps {
                            "Refreshing apps"
                        } else {
                            "Refresh apps"
                        })
                        .tooltip("Refresh apps")
                        .ghost()
                        .child(if let Some(animation_id) = self.refresh_animation_id {
                            div().child(
                                svg()
                                    .data(include_bytes!("../assets/sync.svg"))
                                    .size(px(18.))
                                    .text_color(rgb(0xc8c8c8))
                                    .with_animation(
                                        format!("refresh-spin-{animation_id}"),
                                        Animation::new(Duration::from_millis(500)),
                                        |icon, delta| {
                                            icon.with_transformation(Transformation::rotate(
                                                radians(std::f32::consts::TAU * delta),
                                            ))
                                        },
                                    ),
                            )
                        } else {
                            div().child(
                                svg()
                                    .data(include_bytes!("../assets/sync.svg"))
                                    .size(px(18.))
                                    .text_color(rgb(0xc8c8c8)),
                            )
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.refresh_animation_id = Some(
                                this.refresh_animation_id
                                    .unwrap_or_default()
                                    .wrapping_add(1),
                            );
                            this.refresh_running_apps(cx);
                            cx.notify();
                        })),
                )
        } else {
            div().child(
                Button::new("add-app")
                    .accessibility_label("Add app")
                    .primary()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                svg()
                                    .data(include_bytes!("../assets/plus.svg"))
                                    .size(px(20.))
                                    .text_color(rgb(0x181818)),
                            )
                            .child("Add app"),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.selected_candidate = None;
                        this.search_query.clear();
                        this.page = SettingsPage::Add;
                        this.error = None;
                        this.search
                            .update(cx, |input, cx| input.set_value("", window, cx));
                        this.refresh_running_apps(cx);
                        cx.notify();
                    })),
            )
        };

        div()
            .flex()
            .flex_col()
            .w(px(300.))
            .h_full()
            .gap_4()
            .p_5()
            .bg(rgb(0x1e1e1e))
            .border_r_1()
            .border_color(rgb(0x383838))
            .child(div().text_xs().text_color(rgb(0xb0b0b0)).child(title))
            .child(
                div()
                    .w_full()
                    .rounded_md()
                    .bg(rgb(0x2c2c2c))
                    .child(Input::new(&self.search)),
            )
            .child(list)
            .child(action)
    }

    fn render_profile_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(path) = self
            .selected_profile
            .as_deref()
            .filter(|path| self.settings.ratio_for(path).is_some())
        else {
            return div()
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_color(rgb(0xb0b0b0))
                .child("Select a profile, or add an app to get started.");
        };
        let name = file_name(path);
        let full_path = path.to_string_lossy().to_string();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_6()
            .p_8()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_xl().text_color(rgb(0xf0f0f0)).child(name))
                    .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(full_path)),
            )
            .child(self.render_ratio_card("Content aspect ratio", cx))
            .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(
                "The selected content area stays clear; ambient blur fills the space outside it.",
            ))
            .child(
                Button::new("remove-profile")
                    .label("Remove app")
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.remove_profile(window, cx))),
            )
    }

    fn render_global_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_6()
            .p_8()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0xf0f0f0))
                            .child("Global settings"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xb0b0b0))
                            .child("Set the starting content area used for new app profiles."),
                    ),
            )
            .child(self.render_ratio_card("Default content aspect ratio", cx))
            .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(
                "New profiles start with this ratio. Existing app settings are not changed.",
            ))
            .child(self.render_startup_card(cx))
    }

    fn render_startup_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_5()
            .p_5()
            .rounded_lg()
            .bg(rgb(0x252525))
            .border_1()
            .border_color(rgb(0x383838))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xe5e5e5))
                            .child("Start with Windows"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0xa6a6a6))
                            .child("Launch um•bee•ants in the tray when you sign in."),
                    ),
            )
            .child(
                Switch::new("start-with-windows")
                    .checked(self.start_with_windows)
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_start_with_windows(*checked, cx);
                    })),
            )
    }

    fn set_start_with_windows(&mut self, enabled: bool, cx: &mut Context<Self>) {
        match startup::set_enabled(enabled) {
            Ok(()) => {
                self.start_with_windows = startup::is_enabled();
                self.error = None;
            }
            Err(error) => {
                self.start_with_windows = startup::is_enabled();
                self.error = Some(format!("Could not change Start with Windows: {error}"));
            }
        }
        cx.notify();
    }

    fn render_add_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_candidate.as_deref();
        let selected_name = selected
            .map(file_name)
            .unwrap_or_else(|| "No app selected".into());
        let selected_path = selected
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                "Choose a recent or running app, or browse to an executable.".into()
            });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .gap_6()
            .p_8()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_xl()
                            .text_color(rgb(0xf0f0f0))
                            .child("Add an app"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xb0b0b0))
                            .child("Recently foregrounded apps appear first, followed by running apps, newest first. Browse if needed."),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_5()
                    .rounded_lg()
                    .bg(rgb(0x252525))
                    .border_1()
                    .border_color(rgb(0x383838))
                    .child(
                        div()
                            .text_lg()
                            .text_color(rgb(0xf0f0f0))
                            .child(selected_name),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0xb0b0b0))
                            .child(selected_path),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(format!(
                        "New profiles use the global default: {}:{}",
                        self.settings.default_aspect_ratio.width,
                        self.settings.default_aspect_ratio.height
                    )))
                    .child(
                        Button::new("confirm-add-app")
                            .label("Add selected app")
                            .primary()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_candidate(window, cx)),
                            ),
                    ),
            )
    }

    fn render_ratio_card(&self, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let preset_buttons = ASPECT_PRESETS.iter().enumerate().map(|(index, preset)| {
            let preset_value = (*preset).to_owned();
            Button::new(format!("ratio-preset-{index}"))
                .label(*preset)
                .ghost()
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_ratio_text(&preset_value, window, cx);
                    this.save_ratio(&preset_value, window, cx);
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .rounded_lg()
            .bg(rgb(0x252525))
            .border_1()
            .border_color(rgb(0x383838))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .pb_3()
                    .border_b_1()
                    .border_color(rgb(0x3a3a3a))
                    .child(div().text_xs().text_color(rgb(0xa6a6a6)).child("SETTING"))
                    .child(div().text_xs().text_color(rgb(0xa6a6a6)).child("VALUE")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().text_sm().text_color(rgb(0xe5e5e5)).child(label))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(0xa6a6a6))
                                    .child("Use width:height, from 1:4 through 4:1"),
                            ),
                    )
                    .child(
                        div()
                            .w(px(230.))
                            .rounded_md()
                            .bg(rgb(0x2c2c2c))
                            .child(Input::new(&self.ratio)),
                    ),
            )
            .child(div().flex().items_center().gap_2().children(preset_buttons))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Fill the remaining pane height so `flex_1` / centering inside each page works.
        let details_pane = || div().flex().flex_col().flex_1().min_h_0();
        let details = match self.page {
            SettingsPage::Program => details_pane().child(self.render_profile_details(cx)),
            SettingsPage::Global => details_pane().child(self.render_global_details(cx)),
            SettingsPage::Add => details_pane().child(self.render_add_details(cx)),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x181818))
            .text_color(rgb(0xe5e5e5))
            .child(self.render_header(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(self.error_banner())
                            .child(details),
                    ),
            )
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_bar_glyphs_are_available_from_the_registered_assets() {
        use gpui_kit::AssetSource as _;
        use gpui_kit::component::{IconName, IconNamed as _};

        for icon in [
            IconName::WindowMinimize,
            IconName::WindowMaximize,
            IconName::WindowRestore,
            IconName::WindowClose,
        ] {
            assert!(
                gpui_kit::assets::Assets
                    .load(&icon.path())
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn settings_window_title_matches_the_windows_lookup_text() {
        let decoded =
            String::from_utf16(&WINDOW_TITLE_WIDE[..WINDOW_TITLE_WIDE.len() - 1]).unwrap();
        assert_eq!(decoded, WINDOW_TITLE);
        assert_eq!(WINDOW_TITLE, "um•bee•ants");
    }

    #[test]
    fn logo_rasterization_retains_colored_pixels() {
        let logo = render_logo().unwrap();
        let pixels = logo.as_bytes(0).unwrap();
        assert!(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| { pixel[3] > 128 && (pixel[0] != pixel[1] || pixel[1] != pixel[2]) })
        );
    }

    #[test]
    fn add_icon_is_a_renderable_embedded_svg() {
        let icon = SvgRenderer::new(Arc::new(()))
            .render_single_frame(include_bytes!("../assets/plus.svg"), 1.0)
            .unwrap();
        assert!(
            icon.as_bytes(0)
                .unwrap()
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[3] > 0)
        );
    }

    #[test]
    fn profile_ratio_edits_preserve_the_global_default() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: executable.clone(),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
        });

        let updated =
            updated_settings(&settings, SettingsPage::Program, Some(&executable), "32:9").unwrap();

        assert_eq!(updated.default_aspect_ratio, settings.default_aspect_ratio);
        assert_eq!(updated.ratio_for(&executable), AspectRatio::new(32, 9));
    }

    #[test]
    fn global_ratio_edits_preserve_existing_profiles() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: executable.clone(),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
        });

        let updated = updated_settings(&settings, SettingsPage::Global, None, "16:10").unwrap();

        assert_eq!(
            updated.default_aspect_ratio,
            AspectRatio::new(16, 10).unwrap()
        );
        assert_eq!(updated.ratio_for(&executable), AspectRatio::new(16, 9));
    }

    #[test]
    fn invalid_or_unselected_ratio_edits_do_not_change_settings() {
        let settings = AppSettings::default();

        assert!(updated_settings(&settings, SettingsPage::Global, None, "invalid").is_none());
        assert!(updated_settings(&settings, SettingsPage::Program, None, "21:9").is_none());
        assert!(updated_settings(&settings, SettingsPage::Add, None, "21:9").is_none());
    }
}
