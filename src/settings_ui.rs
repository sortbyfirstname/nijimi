use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::component::{
    Disableable as _, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    slider::{Slider, SliderEvent, SliderState},
    switch::Switch,
    theme::{Theme, ThemeMode},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    Animation, AnimationExt as _, App, AppContext as _, Bounds, ClipboardItem, Context, Entity,
    IntoElement, ObjectFit, ParentElement as _, Render, RenderImage, Styled as _, StyledImage as _,
    Subscription, SvgRenderer, Transformation, Window, WindowBounds, WindowOptions, div, img, px,
    radians, rgb, size, svg,
};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
use windows::core::w;

use crate::geometry::ContentArea;
use crate::render::{Preview, PreviewImage, primary_screen_size};
use crate::settings::{
    self, AppProfile, AppSettings, BlurResolution, EffectSettings, FrameRate, paths_match,
};
use crate::{app, app_icon, startup, system_info};

pub const WINDOW_TITLE: &str = "nijimi";
/// `WINDOW_TITLE` as a NUL-terminated UTF-16 string, for Win32 calls.
pub const WINDOW_TITLE_WIDE: &[u16] = &[110, 105, 106, 105, 109, 105, 0];
const ASPECT_PRESETS: [&str; 5] = ["16:9", "21:9", "32:9", "16:10", "4:3"];

fn render_logo() -> Result<Arc<RenderImage>, String> {
    SvgRenderer::new(Arc::new(()))
        // Mark only, at 2x its 20px display size; the wordmark is real text.
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
        CreateMutexW(None, true, w!("Local\\NijimiSettingsWindow"))
            .map_err(|error| error.to_string())?
    };
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        let _ = unsafe { CloseHandle(mutex) };
        if let Some(window) = app::find_settings_window() {
            let _ = unsafe { SetForegroundWindow(window) };
        }
        return Ok(());
    }
    let _settings_mutex = SettingsMutex(mutex);
    app::exit_with_tray();
    let settings_path = settings::config_path().map_err(|error| error.to_string())?;
    let (app_settings, warning) =
        AppSettings::load(&settings_path).map_err(|error| error.to_string())?;
    if let Some(warning) = warning {
        app::show_error(&warning);
    }
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
                app::show_error(&format!("nijimi could not open:\n{error}"));
                cx.quit();
                return;
            }
            if let Some(window) = app::find_settings_window()
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
    About,
}

fn updated_settings(
    settings: &AppSettings,
    page: SettingsPage,
    selected_profile: Option<&Path>,
    value: &str,
) -> Option<AppSettings> {
    let ratio = ContentArea::parse(value)?;
    let mut updated = settings.clone();
    match page {
        SettingsPage::Global => updated.default_content_area = ratio,
        SettingsPage::Program => {
            let executable = selected_profile?;
            updated.upsert(AppProfile {
                executable: executable.to_path_buf(),
                content_area: ratio,
                effect: settings
                    .profile_for(executable)
                    .and_then(|profile| profile.effect),
                show_overlays: settings
                    .profile_for(executable)
                    .and_then(|profile| profile.show_overlays),
            });
        }
        SettingsPage::Add | SettingsPage::About => return None,
    }
    (updated != *settings).then_some(updated)
}

fn effect_on_page(
    settings: &AppSettings,
    page: SettingsPage,
    selected_profile: Option<&Path>,
) -> EffectSettings {
    match (page, selected_profile) {
        (SettingsPage::Program, Some(path)) => settings
            .profile_for(path)
            .and_then(|profile| profile.effect)
            .unwrap_or(settings.default_effect),
        _ => settings.default_effect,
    }
}

/// Applies `change` to the effect on `page`. A profile's effect only changes once it has its own.
fn with_effect_change(
    settings: &AppSettings,
    page: SettingsPage,
    selected_profile: Option<&Path>,
    change: impl FnOnce(&mut EffectSettings),
) -> Option<AppSettings> {
    let mut updated = settings.clone();
    let effect = match page {
        SettingsPage::Global => &mut updated.default_effect,
        SettingsPage::Program => {
            let path = selected_profile?;
            updated
                .profiles
                .iter_mut()
                .find(|profile| paths_match(&profile.executable, path))?
                .effect
                .as_mut()?
        }
        SettingsPage::Add | SettingsPage::About => return None,
    };
    change(effect);
    (updated != *settings).then_some(updated)
}

fn strength_slider(value: u8) -> SliderState {
    SliderState::new()
        .min(0.)
        .max(EffectSettings::MAX_STRENGTH as f32)
        .step(1.)
        .default_value(value as f32)
}

fn slider_strength(slider: &SliderState) -> u8 {
    slider
        .value()
        .start()
        .round()
        .clamp(0., EffectSettings::MAX_STRENGTH as f32) as u8
}

#[derive(Default)]
struct PreviewState {
    /// Created on first use, off the UI thread, and reused while the window is open.
    renderer: Arc<Mutex<Option<Preview>>>,
    screen_size: Option<(u32, u32)>,
    image: Option<Arc<RenderImage>>,
    error: Option<String>,
    rendered: Option<(ContentArea, EffectSettings)>,
    busy: bool,
}

fn render_preview(
    renderer: &Mutex<Option<Preview>>,
    (screen_width, screen_height): (u32, u32),
    (content_area, effect): (ContentArea, EffectSettings),
) -> Result<Arc<RenderImage>, String> {
    let mut renderer = renderer.lock().map_err(|error| error.to_string())?;
    if renderer.is_none() {
        *renderer = Some(Preview::new().map_err(|error| error.to_string())?);
    }
    let PreviewImage {
        width,
        height,
        bgra,
    } = renderer
        .as_mut()
        .unwrap()
        .render(screen_width, screen_height, content_area, effect)
        .map_err(|error| error.to_string())?;
    // GPUI's images hold BGRA bytes in an RGBA-typed buffer.
    let buffer = image::RgbaImage::from_raw(width, height, bgra)
        .ok_or("the preview image has the wrong size")?;
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
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
    edge_softness: Entity<SliderState>,
    edge_blend: Entity<SliderState>,
    blur: Entity<SliderState>,
    brightness: Entity<SliderState>,
    preview: PreviewState,
    search_query: String,
    suppress_ratio_events: bool,
    start_with_windows: bool,
    system_info: Option<String>,
    copied_system_info: bool,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsView {
    fn new(data: InitialData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selected_profile = settings::sorted_profile_paths(&data.settings.profiles)
            .into_iter()
            .next();
        let initial_ratio = selected_profile
            .as_deref()
            .and_then(|path| data.settings.content_area_for(path))
            .unwrap_or(data.settings.default_content_area);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search apps or paths"));
        let ratio =
            cx.new(|cx| InputState::new(window, cx).default_value(initial_ratio.to_string()));
        let initial_effect = effect_on_page(
            &data.settings,
            SettingsPage::Program,
            selected_profile.as_deref(),
        );
        let edge_softness = cx.new(|_| strength_slider(initial_effect.edge_softness));
        let edge_blend = cx.new(|_| strength_slider(initial_effect.edge_blend));
        let blur = cx.new(|_| strength_slider(initial_effect.blur));
        let brightness = cx.new(|_| strength_slider(initial_effect.brightness));
        let slider_subscriptions =
            [&edge_softness, &edge_blend, &blur, &brightness].map(|slider| {
                cx.subscribe_in(
                    slider,
                    window,
                    |this, _, event: &SliderEvent, window, cx| match event {
                        SliderEvent::Change(_) => cx.notify(),
                        SliderEvent::Release(_) => this.save_effect_sliders(window, cx),
                    },
                )
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
            edge_softness,
            edge_blend,
            blur,
            brightness,
            preview: PreviewState {
                screen_size: primary_screen_size(),
                ..PreviewState::default()
            },
            search_query: String::new(),
            suppress_ratio_events: false,
            start_with_windows: startup::is_enabled(),
            system_info: None,
            copied_system_info: false,
            error: None,
            _subscriptions: [search_subscription, ratio_subscription]
                .into_iter()
                .chain(slider_subscriptions)
                .collect(),
        }
    }

    fn set_ratio_text(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.suppress_ratio_events = true;
        self.ratio.update(cx, |input, cx| {
            input.set_value(value, window, cx);
        });
        self.suppress_ratio_events = false;
    }

    fn current_ratio(&self) -> Option<ContentArea> {
        match self.page {
            SettingsPage::Global => Some(self.settings.default_content_area),
            SettingsPage::Program => self
                .selected_profile
                .as_deref()
                .and_then(|path| self.settings.content_area_for(path)),
            SettingsPage::Add | SettingsPage::About => None,
        }
    }

    fn sync_ratio_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self
            .current_ratio()
            .map(|area| area.to_string())
            .unwrap_or_default();
        self.set_ratio_text(&value, window, cx);
        self.sync_effect_sliders(window, cx);
    }

    fn current_effect(&self) -> EffectSettings {
        effect_on_page(&self.settings, self.page, self.selected_profile.as_deref())
    }

    fn preview_target(&self, cx: &App) -> Option<(ContentArea, EffectSettings)> {
        let content_area = self.current_ratio()?;
        let mut effect = self.current_effect();
        if self.effect_editable() {
            effect.edge_softness = slider_strength(self.edge_softness.read(cx));
            effect.edge_blend = slider_strength(self.edge_blend.read(cx));
            effect.blur = slider_strength(self.blur.read(cx));
            effect.brightness = slider_strength(self.brightness.read(cx));
        }
        Some((content_area, effect))
    }

    /// One render at a time; when it finishes, the next view render catches up with any changes.
    fn refresh_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview.busy {
            return;
        }
        let Some(target) = self.preview_target(cx) else {
            return;
        };
        if self.preview.rendered == Some(target) {
            return;
        }
        let Some(screen_size) = self.preview.screen_size else {
            self.preview.error = Some("Could not read the screen size.".into());
            return;
        };
        self.preview.busy = true;
        self.preview.rendered = Some(target);
        let renderer = self.preview.renderer.clone();
        cx.spawn(async move |view, cx| {
            let result = cx
                .background_spawn(async move { render_preview(&renderer, screen_size, target) })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.preview.busy = false;
                match result {
                    Ok(image) => {
                        if let Some(previous) = this.preview.image.replace(image) {
                            cx.drop_image(previous, None);
                        }
                        this.preview.error = None;
                    }
                    Err(error) => this.preview.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_preview(&self) -> impl IntoElement {
        let (screen_width, screen_height) = self.preview.screen_size.unwrap_or((16, 9));
        let aspect = screen_width as f32 / screen_height as f32;
        let height = (680.0 / aspect).clamp(140.0, 360.0);
        let frame = div()
            .flex()
            .items_center()
            .justify_center()
            .w(px(height * aspect))
            .max_w_full()
            .h(px(height))
            .rounded_md()
            .overflow_hidden()
            .bg(rgb(0x000000))
            .border_1()
            .border_color(rgb(0x3a3a3a));
        match (&self.preview.image, &self.preview.error) {
            (_, Some(error)) => frame
                .text_sm()
                .text_color(rgb(0xa6a6a6))
                .child(format!("Preview unavailable: {error}")),
            (Some(image), None) => frame.child(
                img(image.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            ),
            (None, None) => frame
                .text_sm()
                .text_color(rgb(0xa6a6a6))
                .child("Rendering preview…"),
        }
    }

    fn effect_editable(&self) -> bool {
        match self.page {
            SettingsPage::Global => true,
            SettingsPage::Program => self
                .selected_profile
                .as_deref()
                .and_then(|path| self.settings.profile_for(path))
                .is_some_and(|profile| profile.effect.is_some()),
            SettingsPage::Add | SettingsPage::About => false,
        }
    }

    /// The look the Look card's reset button restores: the built-in defaults on the Global page,
    /// or the global look for an app with its own.
    fn reset_effect_target(&self) -> Option<EffectSettings> {
        match self.page {
            SettingsPage::Global => Some(EffectSettings::default()),
            SettingsPage::Program => self
                .effect_editable()
                .then_some(self.settings.default_effect),
            SettingsPage::Add | SettingsPage::About => None,
        }
    }

    fn sync_effect_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let effect = self.current_effect();
        for (slider, value) in [
            (&self.edge_softness, effect.edge_softness),
            (&self.edge_blend, effect.edge_blend),
            (&self.blur, effect.blur),
            (&self.brightness, effect.brightness),
        ] {
            slider.update(cx, |slider, cx| slider.set_value(value as f32, window, cx));
        }
    }

    fn update_effect(
        &mut self,
        change: impl FnOnce(&mut EffectSettings),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(updated) = with_effect_change(
            &self.settings,
            self.page,
            self.selected_profile.as_deref(),
            change,
        ) {
            self.save_settings(updated, cx);
        }
        self.sync_effect_sliders(window, cx);
        cx.notify();
    }

    fn save_effect_sliders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let edge_softness = slider_strength(self.edge_softness.read(cx));
        let edge_blend = slider_strength(self.edge_blend.read(cx));
        let blur = slider_strength(self.blur.read(cx));
        let brightness = slider_strength(self.brightness.read(cx));
        self.update_effect(
            |effect| {
                effect.edge_softness = edge_softness;
                effect.edge_blend = edge_blend;
                effect.blur = blur;
                effect.brightness = brightness;
            },
            window,
            cx,
        );
    }

    fn set_custom_effect(&mut self, custom: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.selected_profile.clone() else {
            return;
        };
        let default_effect = self.settings.default_effect;
        self.update_settings(
            |settings| {
                if let Some(profile) = settings
                    .profiles
                    .iter_mut()
                    .find(|profile| paths_match(&profile.executable, &path))
                {
                    profile.effect = custom.then_some(default_effect);
                }
            },
            cx,
        );
        self.sync_effect_sliders(window, cx);
    }

    fn set_custom_overlays(&mut self, custom: bool, cx: &mut Context<Self>) {
        let Some(path) = self.selected_profile.clone() else {
            return;
        };
        self.update_settings(
            |settings| {
                let global = settings.show_overlays;
                if let Some(profile) = settings
                    .profiles
                    .iter_mut()
                    .find(|profile| paths_match(&profile.executable, &path))
                {
                    profile.show_overlays = custom.then_some(global);
                }
            },
            cx,
        );
    }

    fn set_show_overlays(&mut self, show: bool, cx: &mut Context<Self>) {
        let page = self.page;
        let path = self.selected_profile.clone();
        self.update_settings(
            |settings| match (page, path) {
                (SettingsPage::Global, _) => settings.show_overlays = show,
                (SettingsPage::Program, Some(path)) => {
                    if let Some(profile) = settings
                        .profiles
                        .iter_mut()
                        .find(|profile| paths_match(&profile.executable, &path))
                    {
                        profile.show_overlays = Some(show);
                    }
                }
                _ => {}
            },
            cx,
        );
    }

    fn restore_ratio(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if ContentArea::parse(&self.ratio.read(cx).value()).is_none() {
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
        if page == SettingsPage::About {
            self.system_info = Some(system_info::summary(&self.settings, &self.settings_path));
            self.copied_system_info = false;
        }
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
        let paths = settings::sorted_profile_paths(&self.settings.profiles);
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
        if self.settings.content_area_for(&executable).is_some() {
            self.select_profile(executable, window, cx);
            return;
        }
        let mut updated = self.settings.clone();
        updated.upsert(AppProfile {
            executable: executable.clone(),
            content_area: self.settings.default_content_area,
            effect: None,
            show_overlays: None,
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
        self.selected_profile = settings::sorted_profile_paths(&self.settings.profiles)
            .into_iter()
            .next();
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
        .cursor_pointer()
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
        .cursor_pointer()
        .on_click(cx.listener(|this, _, window, cx| {
            this.set_page(SettingsPage::Global, window, cx);
        }));
        let about = Button::new("about").label("About");
        let about = if self.page == SettingsPage::About {
            about.primary()
        } else {
            about.ghost()
        }
        .cursor_pointer()
        .on_click(cx.listener(|this, _, window, cx| {
            this.set_page(SettingsPage::About, window, cx);
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
                                    // Lowercase text sits low in its line box; align it with the mark.
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
                    .child(global_style)
                    .child(div().flex_1())
                    .child(about),
            )
            .child(div().h(px(1.)).bg(rgb(0x383838)))
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let paths = match self.page {
            SettingsPage::Add => self.filtered_candidates(),
            SettingsPage::Program | SettingsPage::Global | SettingsPage::About => {
                self.filtered_profiles()
            }
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
                .cursor_pointer()
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
                        .cursor_pointer()
                        .label("Browse…")
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| this.browse(cx))),
                )
                .child(
                    Button::new("add-selected-app")
                        .cursor_pointer()
                        .label("Add selected")
                        .primary()
                        .on_click(
                            cx.listener(|this, _, window, cx| this.add_candidate(window, cx)),
                        ),
                )
                .child(
                    Button::new("refresh-apps")
                        .cursor_pointer()
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
                    .cursor_pointer()
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
            .filter(|path| self.settings.content_area_for(path).is_some())
        else {
            return div()
                .flex()
                .flex_1()
                .flex_shrink_0()
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
            .flex_shrink_0()
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
            .child(self.render_ratio_card("Content area", cx))
            .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(
                "Where the game, film or other content is visible. It stays clear; the glow fills the space around it.",
            ))
            .child(self.render_effect_card(cx))
            .child(self.render_overlays_card(cx))
            .child(
                Button::new("remove-profile")
                    .cursor_pointer()
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
            .flex_shrink_0()
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
            .child(self.render_ratio_card("Default content area", cx))
            .child(div().text_sm().text_color(rgb(0xb0b0b0)).child(
                "New profiles start with this content area. Existing app settings are not changed.",
            ))
            .child(self.render_effect_card(cx))
            .child(self.render_overlays_card(cx))
            .child(
                toggle_card(
                    "Frame rate",
                    "How often the effect updates. 60 is smoother; 30 uses half the GPU time.",
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .flex_shrink_0()
                        .children(FrameRate::ALL.map(|rate| {
                            let button = Button::new(format!("frame-rate-{}", rate.label()))
                                .cursor_pointer()
                                .label(rate.label())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.update_settings(|settings| settings.frame_rate = rate, cx);
                                }));
                            if rate == self.settings.frame_rate {
                                button.primary()
                            } else {
                                button.ghost()
                            }
                        })),
                ),
            )
            .child(
                toggle_card(
                    "Always on",
                    "Show the effect over every app, not just profiled games. Apps without a profile use the default content area.",
                )
                .child(
                    Switch::new("always-on")
                        .cursor_pointer()
                        .checked(self.settings.always_on)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.update_settings(|settings| settings.always_on = *checked, cx);
                        })),
                ),
            )
            .child(
                toggle_card(
                    "Show in screenshots",
                    "Let screenshots and recordings capture the effect. When off, they show black bars instead.",
                )
                .child(
                    Switch::new("show-in-screenshots")
                        .cursor_pointer()
                        .checked(self.settings.show_in_screenshots)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.update_settings(
                                |settings| settings.show_in_screenshots = *checked,
                                cx,
                            );
                        })),
                ),
            )
            .child(
                toggle_card(
                    "Hide idle cursor",
                    "Hide the mouse cursor after it rests in the bars for 2 seconds. It comes back as soon as you move it.",
                )
                .child(
                    Switch::new("hide-idle-cursor")
                        .cursor_pointer()
                        .checked(self.settings.hide_idle_cursor)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.update_settings(
                                |settings| settings.hide_idle_cursor = *checked,
                                cx,
                            );
                        })),
                ),
            )
            .child(
                toggle_card(
                    "Start with Windows",
                    "Launch nijimi in the tray when you sign in.",
                )
                .child(
                    Switch::new("start-with-windows")
                        .cursor_pointer()
                        .checked(self.start_with_windows)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.set_start_with_windows(*checked, cx);
                        })),
                ),
            )
    }

    fn update_settings(&mut self, change: impl FnOnce(&mut AppSettings), cx: &mut Context<Self>) {
        let mut updated = self.settings.clone();
        change(&mut updated);
        self.save_settings(updated, cx);
    }

    fn save_settings(&mut self, updated: AppSettings, cx: &mut Context<Self>) {
        match updated.save(&self.settings_path) {
            Ok(()) => {
                self.settings = updated;
                self.error = None;
                app::notify_settings_changed();
            }
            Err(error) => self.error = Some(format!("Could not save settings: {error}")),
        }
        cx.notify();
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

    fn render_about_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
        let card = || {
            div()
                .flex()
                .flex_col()
                .gap_4()
                .p_5()
                .rounded_lg()
                .bg(rgb(0x252525))
                .border_1()
                .border_color(rgb(0x383838))
        };
        let link = |id: &'static str, label: &'static str, url: String| {
            Button::new(id)
                .cursor_pointer()
                .label(label)
                .ghost()
                .small()
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        let system_info = self.system_info.clone().unwrap_or_default();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .flex_shrink_0()
            .gap_6()
            .p_8()
            .child(div().text_xl().text_color(rgb(0xf0f0f0)).child("About"))
            .child(
                card()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(img(self.logo.clone()).size(px(40.)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_lg().text_color(rgb(0xf0f0f0)).child("nijimi"))
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(rgb(0xa6a6a6))
                                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .text_sm()
                            .child(
                                div()
                                    .text_color(rgb(0xe5e5e5))
                                    .child("滲み: colour that bleeds beyond its edges."),
                            )
                            .child(
                                div()
                                    .text_color(rgb(0xa6a6a6))
                                    .child("Made by Aaron Seabrook. Released under the MIT License."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(link("github", "View on GitHub", REPOSITORY.to_string()))
                            .child(link(
                                "report-issue",
                                "Report an issue",
                                format!("{REPOSITORY}/issues/new"),
                            )),
                    ),
            )
            .child(
                card()
                    .child(
                        setting_row(
                            "System info",
                            "Include this when reporting an issue. It doesn't name any apps or file paths.",
                        )
                        .child(
                            Button::new("copy-system-info")
                                .cursor_pointer()
                                .label(if self.copied_system_info {
                                    "Copied"
                                } else {
                                    "Copy"
                                })
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        system_info.clone(),
                                    ));
                                    this.copied_system_info = true;
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_3()
                            .rounded_md()
                            .bg(rgb(0x1c1c1c))
                            .text_xs()
                            .text_color(rgb(0xc8c8c8))
                            .children(
                                self.system_info
                                    .iter()
                                    .flat_map(|info| info.lines())
                                    .map(|line| div().child(line.to_string())),
                            ),
                    ),
            )
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
            .flex_shrink_0()
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
                        "New profiles use the global default: {}",
                        self.settings.default_content_area
                    )))
                    .child(
                        Button::new("confirm-add-app")
                            .cursor_pointer()
                            .label("Add selected app")
                            .primary()
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_candidate(window, cx)),
                            ),
                    ),
            )
    }

    fn render_effect_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let editable = self.effect_editable();
        let effect = self.current_effect();
        let custom_row = (self.page == SettingsPage::Program).then(|| {
            setting_row(
                "Custom look for this app",
                "When off, this app follows the look set in Global settings.",
            )
            .child(
                Switch::new("custom-effect")
                    .checked(editable)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, checked: &bool, window, cx| {
                        this.set_custom_effect(*checked, window, cx);
                    })),
            )
        });
        let slider_row =
            |title: &'static str, description: &'static str, slider: &Entity<SliderState>| {
                let value = slider_strength(slider.read(cx));
                setting_row(title, description).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .w(px(230.))
                        .flex_shrink_0()
                        .child(
                            div().flex_1().child(
                                Slider::new(slider)
                                    .accessibility_label(title)
                                    .disabled(!editable)
                                    .when(editable, |slider| slider.cursor_pointer()),
                            ),
                        )
                        .child(
                            div()
                                .w(px(40.))
                                .text_right()
                                .text_sm()
                                .text_color(rgb(if editable { 0xe5e5e5 } else { 0x808080 }))
                                .child(format!("{value}%")),
                        ),
                )
            };
        let resolution_buttons = BlurResolution::ALL.map(|resolution| {
            let button = Button::new(format!("blur-resolution-{}", resolution.label()))
                .label(resolution.label())
                .disabled(!editable)
                .when(editable, |button| button.cursor_pointer())
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.update_effect(|effect| effect.resolution = resolution, window, cx);
                }));
            if resolution == effect.resolution {
                button.primary()
            } else {
                button.ghost()
            }
        });

        let reset_target = self.reset_effect_target();
        let reset_disabled = reset_target.is_none_or(|target| target == effect);
        let reset_button = Button::new("reset-look")
            .label(if self.page == SettingsPage::Global {
                "Reset to defaults"
            } else {
                "Reset to global"
            })
            .ghost()
            .small()
            .disabled(reset_disabled)
            .when(!reset_disabled, |button| button.cursor_pointer())
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(target) = reset_target {
                    this.update_effect(|effect| *effect = target, window, cx);
                }
            }));

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
                    .items_center()
                    .justify_between()
                    .pb_3()
                    .border_b_1()
                    .border_color(rgb(0x3a3a3a))
                    .child(div().text_xs().text_color(rgb(0xa6a6a6)).child("LOOK"))
                    .child(reset_button),
            )
            .child(div().flex().justify_center().child(self.render_preview()))
            .children(custom_row)
            .child(slider_row(
                "Edge softness",
                "Softens the outermost few pixels of the picture so its edge isn't razor sharp.",
                &self.edge_softness,
            ))
            .child(slider_row(
                "Edge blend",
                "Blurs the picture gradually into the bars so there's no hard line.",
                &self.edge_blend,
            ))
            .child(slider_row(
                "Blur strength",
                "How far colours spread into the bars.",
                &self.blur,
            ))
            .child(slider_row(
                "Brightness",
                "How bright the glow is. At 100% it matches the picture.",
                &self.brightness,
            ))
            .child(
                setting_row(
                    "Blur resolution",
                    "Lower is lighter on the GPU and smoother; higher keeps more detail.",
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .flex_shrink_0()
                        .children(resolution_buttons),
                ),
            )
    }

    fn render_overlays_card(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let program = self.page == SettingsPage::Program;
        let custom = self
            .selected_profile
            .as_deref()
            .filter(|_| program)
            .and_then(|path| self.settings.profile_for(path))
            .and_then(|profile| profile.show_overlays);
        let editable = !program || custom.is_some();
        let show = custom.unwrap_or(self.settings.show_overlays);
        let custom_row = program.then(|| {
            setting_row(
                "Custom setting for this app",
                "When off, this app follows the setting in Global settings.",
            )
            .child(
                Switch::new("custom-overlays")
                    .checked(custom.is_some())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                        this.set_custom_overlays(*checked, cx);
                    })),
            )
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
                    .pb_3()
                    .border_b_1()
                    .border_color(rgb(0x3a3a3a))
                    .child(div().text_xs().text_color(rgb(0xa6a6a6)).child("OVERLAYS")),
            )
            .children(custom_row)
            .child(
                setting_row(
                    "Show overlays",
                    "Let Steam notifications, the Steam overlay and anything else a game draws in its bars show through the glow, while it fills the screen. Turn off for games that draw artwork in their bars. Not available while Show in screenshots is on.",
                )
                .child(
                    Switch::new("show-overlays")
                        .checked(show)
                        .disabled(!editable)
                        .when(editable, |switch| switch.cursor_pointer())
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.set_show_overlays(*checked, cx);
                        })),
                ),
            )
    }

    fn render_ratio_card(&self, label: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let preset_buttons = ASPECT_PRESETS.iter().enumerate().map(|(index, preset)| {
            let preset_value = (*preset).to_owned();
            Button::new(format!("ratio-preset-{index}"))
                .cursor_pointer()
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
                                    .child("A ratio like 16:9, or an exact size like 1920x1080"),
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
        self.refresh_preview(cx);
        // Fills the pane so pages can centre, and scrolls when a page is taller than the window.
        let details_pane = || {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
        };
        let details = match self.page {
            SettingsPage::Program => details_pane().child(self.render_profile_details(cx)),
            SettingsPage::Global => details_pane().child(self.render_global_details(cx)),
            SettingsPage::Add => details_pane().child(self.render_add_details(cx)),
            SettingsPage::About => details_pane().child(self.render_about_details(cx)),
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
                    .children(
                        matches!(self.page, SettingsPage::Program | SettingsPage::Add)
                            .then(|| self.render_sidebar(cx)),
                    )
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

fn setting_row(title: &'static str, description: &'static str) -> gpui_kit::Div {
    div().flex().items_center().justify_between().gap_5().child(
        div()
            .flex()
            .flex_col()
            .gap_1()
            .min_w_0()
            .child(div().text_sm().text_color(rgb(0xe5e5e5)).child(title))
            .child(div().text_xs().text_color(rgb(0xa6a6a6)).child(description)),
    )
}

fn toggle_card(title: &'static str, description: &'static str) -> gpui_kit::Div {
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
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().text_color(rgb(0xe5e5e5)).child(title))
                .child(div().text_xs().text_color(rgb(0xa6a6a6)).child(description)),
        )
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
        assert_eq!(WINDOW_TITLE, "nijimi");
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
            content_area: ContentArea::ratio(16, 9).unwrap(),
            effect: None,
            show_overlays: None,
        });

        let updated =
            updated_settings(&settings, SettingsPage::Program, Some(&executable), "32:9").unwrap();

        assert_eq!(updated.default_content_area, settings.default_content_area);
        assert_eq!(
            updated.content_area_for(&executable),
            ContentArea::ratio(32, 9)
        );
    }

    #[test]
    fn global_ratio_edits_preserve_existing_profiles() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: executable.clone(),
            content_area: ContentArea::ratio(16, 9).unwrap(),
            effect: None,
            show_overlays: None,
        });

        let updated = updated_settings(&settings, SettingsPage::Global, None, "16:10").unwrap();

        assert_eq!(
            updated.default_content_area,
            ContentArea::ratio(16, 10).unwrap()
        );
        assert_eq!(
            updated.content_area_for(&executable),
            ContentArea::ratio(16, 9)
        );
    }

    #[test]
    fn invalid_or_unselected_ratio_edits_do_not_change_settings() {
        let settings = AppSettings::default();

        assert!(updated_settings(&settings, SettingsPage::Global, None, "invalid").is_none());
        assert!(updated_settings(&settings, SettingsPage::Program, None, "21:9").is_none());
        assert!(updated_settings(&settings, SettingsPage::Add, None, "21:9").is_none());
    }

    #[test]
    fn profile_ratio_edits_keep_a_custom_effect() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let custom = EffectSettings {
            edge_blend: 60,
            ..EffectSettings::default()
        };
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: executable.clone(),
            content_area: ContentArea::ratio(16, 9).unwrap(),
            effect: Some(custom),
            show_overlays: None,
        });

        let updated =
            updated_settings(&settings, SettingsPage::Program, Some(&executable), "4:3").unwrap();

        assert_eq!(
            updated.profile_for(&executable).unwrap().effect,
            Some(custom)
        );
    }

    #[test]
    fn resetting_a_custom_look_copies_the_global_one_and_keeps_it_custom() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let mut settings = AppSettings {
            default_effect: EffectSettings {
                edge_blend: 40,
                ..EffectSettings::default()
            },
            ..AppSettings::default()
        };
        settings.upsert(AppProfile {
            executable: executable.clone(),
            content_area: ContentArea::ratio(16, 9).unwrap(),
            effect: Some(EffectSettings {
                blur: 90,
                ..EffectSettings::default()
            }),
            show_overlays: None,
        });
        let global = settings.default_effect;

        let reset = with_effect_change(
            &settings,
            SettingsPage::Program,
            Some(&executable),
            |effect| *effect = global,
        )
        .unwrap();

        assert_eq!(reset.profile_for(&executable).unwrap().effect, Some(global));
        assert_eq!(reset.default_effect, global);
    }

    #[test]
    fn effect_edits_target_the_page_and_skip_profiles_that_follow_global() {
        let executable = PathBuf::from(r"C:\Games\Example.exe");
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: executable.clone(),
            content_area: ContentArea::ratio(16, 9).unwrap(),
            effect: None,
            show_overlays: None,
        });
        let soften = |effect: &mut EffectSettings| effect.edge_blend = 50;

        let global = with_effect_change(&settings, SettingsPage::Global, None, soften).unwrap();
        assert_eq!(global.default_effect.edge_blend, 50);
        assert_eq!(global.profile_for(&executable).unwrap().effect, None);
        assert_eq!(
            effect_on_page(&global, SettingsPage::Program, Some(&executable)),
            global.default_effect
        );

        assert!(
            with_effect_change(&settings, SettingsPage::Program, Some(&executable), soften)
                .is_none()
        );

        settings.profiles[0].effect = Some(EffectSettings::default());
        let program =
            with_effect_change(&settings, SettingsPage::Program, Some(&executable), soften)
                .unwrap();
        assert_eq!(program.default_effect, settings.default_effect);
        assert_eq!(
            effect_on_page(&program, SettingsPage::Program, Some(&executable)).edge_blend,
            50
        );
    }
}
