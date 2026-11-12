use std::collections::HashSet;
use std::fs;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

use crate::geometry::AspectRatio;

const CONFIG_FILE_NAME: &str = "nijimi.ini";
const APP_DIR_NAME: &str = "nijimi";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurResolution {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl BlurResolution {
    pub const ALL: [Self; 4] = [Self::Eighth, Self::Quarter, Self::Half, Self::Full];

    pub fn scale(self) -> u32 {
        match self {
            Self::Full => 1,
            Self::Half => 2,
            Self::Quarter => 4,
            Self::Eighth => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::Half => "Half",
            Self::Quarter => "Quarter",
            Self::Eighth => "Eighth",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Half => "half",
            Self::Quarter => "quarter",
            Self::Eighth => "eighth",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|resolution| resolution.key() == key)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FrameRate {
    #[default]
    Thirty,
    Sixty,
}

impl FrameRate {
    pub const ALL: [Self; 2] = [Self::Thirty, Self::Sixty];

    pub fn label(self) -> &'static str {
        match self {
            Self::Thirty => "30",
            Self::Sixty => "60",
        }
    }

    pub fn fps(self) -> u32 {
        match self {
            Self::Thirty => 30,
            Self::Sixty => 60,
        }
    }

    fn key(self) -> &'static str {
        self.label()
    }

    fn from_key(key: &str) -> Option<Self> {
        // Saved by earlier builds, which could match the display's refresh rate.
        if key == "match" {
            return Some(Self::Sixty);
        }
        Self::ALL.into_iter().find(|rate| rate.key() == key)
    }
}

/// How the ambient fill looks. Strengths are percentages from 0 to 100.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectSettings {
    pub edge_softness: u8,
    pub edge_blend: u8,
    pub blur: u8,
    pub resolution: BlurResolution,
    /// Scales the glow; 100 is as bright as the picture it comes from.
    pub brightness: u8,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            edge_softness: 50,
            edge_blend: 60,
            blur: 80,
            resolution: BlurResolution::Quarter,
            brightness: EffectSettings::MAX_STRENGTH,
        }
    }
}

impl EffectSettings {
    pub const MAX_STRENGTH: u8 = 100;

    /// Parses `softness=10,blend=20,blur=25,resolution=quarter`. Missing keys keep their defaults.
    fn parse(value: &str) -> Option<Self> {
        let mut effect = Self::default();
        for pair in value.split(',') {
            let (key, value) = pair.split_once('=')?;
            let value = value.trim();
            match key.trim() {
                "softness" => effect.edge_softness = parse_strength(value)?,
                "blend" => effect.edge_blend = parse_strength(value)?,
                "blur" => effect.blur = parse_strength(value)?,
                "resolution" => effect.resolution = BlurResolution::from_key(value)?,
                "brightness" => effect.brightness = parse_strength(value)?,
                _ => return None,
            }
        }
        Some(effect)
    }

    fn serialize(&self) -> String {
        let mut value = format!(
            "softness={},blend={},blur={},resolution={}",
            self.edge_softness,
            self.edge_blend,
            self.blur,
            self.resolution.key()
        );
        // Omitted at full brightness so builds without the setting can still read the line.
        if self.brightness != Self::MAX_STRENGTH {
            value.push_str(&format!(",brightness={}", self.brightness));
        }
        value
    }
}

fn parse_strength(value: &str) -> Option<u8> {
    value
        .parse()
        .ok()
        .filter(|strength| *strength <= EffectSettings::MAX_STRENGTH)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppProfile {
    pub executable: PathBuf,
    pub aspect_ratio: AspectRatio,
    /// `None` follows the global effect settings.
    pub effect: Option<EffectSettings>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSettings {
    pub default_aspect_ratio: AspectRatio,
    pub default_effect: EffectSettings,
    pub always_on: bool,
    pub show_in_screenshots: bool,
    /// Hides the cursor once it rests in a bar while the effect is on.
    pub hide_idle_cursor: bool,
    pub frame_rate: FrameRate,
    pub profiles: Vec<AppProfile>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_aspect_ratio: AspectRatio::new(21, 9).unwrap(),
            default_effect: EffectSettings::default(),
            always_on: false,
            show_in_screenshots: false,
            hide_idle_cursor: true,
            frame_rate: FrameRate::default(),
            profiles: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveProfile {
    /// `None` when shown only because of "Always on".
    pub executable: Option<PathBuf>,
    pub aspect_ratio: AspectRatio,
    pub effect: EffectSettings,
}

impl AspectRatio {
    pub fn parse(value: &str) -> Option<Self> {
        let (width, height) = value.trim().split_once(':')?;
        Self::new(width.trim().parse().ok()?, height.trim().parse().ok()?)
    }
}

impl AppSettings {
    /// Skips unreadable lines, copying the file aside first because the next save would drop them.
    pub fn load(path: &Path) -> Result<(Self, Option<String>)> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok((Self::default(), None));
            }
            Err(error) => return Err(error),
        };

        let (settings, bad_lines) = Self::parse(&contents);
        if bad_lines.is_empty() {
            return Ok((settings, None));
        }
        let lines = bad_lines
            .iter()
            .map(|line_number| (line_number + 1).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let backup = backup_path(path);
        let backup_note = match fs::copy(path, &backup) {
            Ok(_) => format!("The original file was copied to:\n{}", backup.display()),
            Err(error) => format!("nijimi could not copy the original file aside:\n{error}"),
        };
        let warning = format!(
            "nijimi skipped unreadable lines ({lines}) in its settings file:\n{}\n\n{backup_note}",
            path.display()
        );
        Ok((settings, Some(warning)))
    }

    fn parse(contents: &str) -> (Self, Vec<usize>) {
        let mut settings = Self::default();
        let mut bad_lines = Vec::new();
        for (line_number, line) in contents.lines().enumerate() {
            if !line.trim().is_empty() && settings.parse_line(line).is_none() {
                bad_lines.push(line_number);
            }
        }
        (settings, bad_lines)
    }

    fn parse_line(&mut self, line: &str) -> Option<()> {
        let (ratio, executable) = line.split_once('|')?;
        match ratio {
            "@global" => self.default_aspect_ratio = AspectRatio::parse(executable)?,
            "@effect" => self.default_effect = EffectSettings::parse(executable)?,
            "@frame_rate" => self.frame_rate = FrameRate::from_key(executable.trim())?,
            "@always_on" => self.always_on = parse_flag(executable)?,
            "@show_in_screenshots" => self.show_in_screenshots = parse_flag(executable)?,
            "@hide_idle_cursor" => self.hide_idle_cursor = parse_flag(executable)?,
            _ => {
                let (ratio, effect) = match ratio.split_once(',') {
                    Some((ratio, effect)) => (ratio, Some(EffectSettings::parse(effect)?)),
                    None => (ratio, None),
                };
                let aspect_ratio = AspectRatio::parse(ratio)?;
                let executable = executable.trim();
                if executable.is_empty() {
                    return None;
                }
                self.upsert(AppProfile {
                    executable: PathBuf::from(executable),
                    aspect_ratio,
                    effect,
                });
            }
        }
        Some(())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut contents = String::new();
        contents.push_str(&format!(
            "@global|{}:{}\n",
            self.default_aspect_ratio.width, self.default_aspect_ratio.height
        ));
        // Omitted when unchanged so older builds, which reject unknown lines, can still load the file.
        if self.default_effect != EffectSettings::default() {
            contents.push_str(&format!("@effect|{}\n", self.default_effect.serialize()));
        }
        if self.always_on {
            contents.push_str("@always_on|1\n");
        }
        if self.show_in_screenshots {
            contents.push_str("@show_in_screenshots|1\n");
        }
        if !self.hide_idle_cursor {
            contents.push_str("@hide_idle_cursor|0\n");
        }
        if self.frame_rate != FrameRate::default() {
            contents.push_str(&format!("@frame_rate|{}\n", self.frame_rate.key()));
        }
        for profile in &self.profiles {
            let executable = profile.executable.to_str().ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidData,
                    "executable path is not valid Unicode",
                )
            })?;
            if executable.contains(['\n', '\r', '|']) {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "executable path contains a reserved settings character",
                ));
            }
            let effect = profile
                .effect
                .map(|effect| format!(",{}", effect.serialize()))
                .unwrap_or_default();
            contents.push_str(&format!(
                "{}:{}{effect}|{executable}\n",
                profile.aspect_ratio.width, profile.aspect_ratio.height
            ));
        }

        let temporary = path.with_extension("tmp");
        fs::write(&temporary, contents)?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn upsert(&mut self, profile: AppProfile) {
        if let Some(existing) = self
            .profiles
            .iter_mut()
            .find(|existing| paths_match(&existing.executable, &profile.executable))
        {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn remove(&mut self, executable: &Path) -> bool {
        let old_len = self.profiles.len();
        self.profiles
            .retain(|profile| !paths_match(&profile.executable, executable));
        self.profiles.len() != old_len
    }

    pub fn profile_for(&self, executable: &Path) -> Option<&AppProfile> {
        self.profiles
            .iter()
            .find(|profile| paths_match(&profile.executable, executable))
    }

    pub fn ratio_for(&self, executable: &Path) -> Option<AspectRatio> {
        self.profile_for(executable)
            .map(|profile| profile.aspect_ratio)
    }

    pub fn active_profile(
        &self,
        enabled: bool,
        on_primary_display: bool,
        foreground: Option<&Path>,
    ) -> Option<ActiveProfile> {
        if !enabled {
            return None;
        }
        let profile = foreground
            .filter(|_| on_primary_display)
            .and_then(|executable| {
                self.profile_for(executable).map(|profile| ActiveProfile {
                    executable: Some(executable.to_path_buf()),
                    aspect_ratio: profile.aspect_ratio,
                    effect: profile.effect.unwrap_or(self.default_effect),
                })
            });
        if profile.is_none() && self.always_on {
            return Some(ActiveProfile {
                executable: None,
                aspect_ratio: self.default_aspect_ratio,
                effect: self.default_effect,
            });
        }
        profile
    }
}

pub fn paths_match(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
}

pub fn path_key(path: &Path) -> String {
    path.as_os_str().to_string_lossy().to_ascii_lowercase()
}

pub fn add_app_choices(
    recent: &[PathBuf],
    running: &[PathBuf],
    profiles: &[AppProfile],
    application_executable: &Path,
    preferred: Option<&Path>,
) -> Vec<PathBuf> {
    let mut choices = Vec::new();
    if let Some(path) = preferred {
        choices.push(path.to_path_buf());
    }
    choices.extend(recent.iter().cloned());
    choices.extend(running.iter().cloned());

    let mut seen: HashSet<String> = profiles
        .iter()
        .map(|profile| path_key(&profile.executable))
        .collect();
    seen.insert(path_key(application_executable));
    choices
        .into_iter()
        .filter(|path| seen.insert(path_key(path)))
        .collect()
}

pub fn sorted_profile_paths(profiles: &[AppProfile]) -> Vec<PathBuf> {
    let key = |path: &Path| {
        (
            path.file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .to_lowercase(),
            path.to_string_lossy().to_lowercase(),
        )
    };
    let mut paths: Vec<PathBuf> = profiles
        .iter()
        .map(|profile| profile.executable.clone())
        .collect();
    paths.sort_by_cached_key(|path| key(path));
    paths
}

pub fn filter_app_choices(paths: &[PathBuf], query: &str) -> Vec<PathBuf> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return paths.to_vec();
    }
    paths
        .iter()
        .filter(|path| path.to_string_lossy().to_lowercase().contains(&query))
        .cloned()
        .collect()
}

/// Settings live beside the executable (portable), unless that folder isn't writable (e.g. it's
/// in Program Files), in which case they go in `%APPDATA%\nijimi`.
pub fn config_path() -> Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "application directory is unavailable"))?;
    let appdata_dir = std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join(APP_DIR_NAME));
    let path = resolve_config_path(executable_dir, appdata_dir.as_deref(), is_dir_writable);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(path)
}

fn resolve_config_path(
    executable_dir: &Path,
    appdata_dir: Option<&Path>,
    is_writable: impl Fn(&Path) -> bool,
) -> PathBuf {
    let portable = executable_dir.join(CONFIG_FILE_NAME);
    if portable.exists() {
        return portable;
    }
    if let Some(appdata_dir) = appdata_dir {
        let per_user = appdata_dir.join(CONFIG_FILE_NAME);
        if per_user.exists() || !is_writable(executable_dir) {
            return per_user;
        }
    }
    portable
}

fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".nijimi-write-test-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn parse_flag(value: &str) -> Option<bool> {
    match value.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bad");
    PathBuf::from(backup)
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn parse_all(contents: &str) -> std::result::Result<AppSettings, Vec<usize>> {
        match AppSettings::parse(contents) {
            (settings, bad_lines) if bad_lines.is_empty() => Ok(settings),
            (_, bad_lines) => Err(bad_lines),
        }
    }

    #[test]
    fn parses_and_bounds_aspect_ratios() {
        assert_eq!(AspectRatio::parse(" 21 : 9 "), AspectRatio::new(21, 9));
        assert_eq!(AspectRatio::parse("16:9:4"), None);
        assert_eq!(AspectRatio::parse("0:9"), None);
        assert_eq!(AspectRatio::parse("5:1"), None);
    }

    #[test]
    fn new_settings_default_to_21_9() {
        assert_eq!(
            AppSettings::default().default_aspect_ratio,
            AspectRatio::new(21, 9).unwrap()
        );
    }

    #[test]
    fn matches_executable_paths_without_case_sensitivity() {
        assert!(paths_match(
            Path::new(r"C:\Games\Example.exe"),
            Path::new(r"c:\games\EXAMPLE.EXE")
        ));
    }

    #[test]
    fn config_path_uses_the_application_name() {
        assert_eq!(
            config_path().unwrap().file_name().unwrap(),
            CONFIG_FILE_NAME
        );
    }

    #[test]
    fn settings_stay_portable_unless_the_executable_folder_is_read_only() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nijimi-config-path-{}-{unique}",
            std::process::id()
        ));
        let exe_dir = root.join("app");
        let appdata = root.join("appdata");
        fs::create_dir_all(&exe_dir).unwrap();
        fs::create_dir_all(&appdata).unwrap();
        let portable = exe_dir.join(CONFIG_FILE_NAME);
        let per_user = appdata.join(CONFIG_FILE_NAME);
        let writable = |_: &Path| true;
        let read_only = |_: &Path| false;

        // Fresh portable copy: settings go beside the executable.
        assert_eq!(
            resolve_config_path(&exe_dir, Some(&appdata), writable),
            portable
        );
        // Installed somewhere read-only, like Program Files: settings go to %APPDATA%.
        assert_eq!(
            resolve_config_path(&exe_dir, Some(&appdata), read_only),
            per_user
        );
        // No %APPDATA%: fall back to beside the executable.
        assert_eq!(resolve_config_path(&exe_dir, None, read_only), portable);
        // Existing per-user settings are kept even if the folder later becomes writable.
        fs::write(&per_user, "").unwrap();
        assert_eq!(
            resolve_config_path(&exe_dir, Some(&appdata), writable),
            per_user
        );
        // An existing portable file always wins.
        fs::write(&portable, "").unwrap();
        assert_eq!(
            resolve_config_path(&exe_dir, Some(&appdata), read_only),
            portable
        );

        assert!(is_dir_writable(&exe_dir));
        assert!(!is_dir_writable(&root.join("missing")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manually_copied_settings_load_and_the_source_file_is_preserved() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("nijimi-migration-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join(CONFIG_FILE_NAME);
        let source_path = directory.join("previous.ini");
        fs::write(&source_path, "16:9|G:\\Games\\Example.exe\n").unwrap();

        let missing_config = AppSettings::load(&path).unwrap().0;
        assert!(missing_config.profiles.is_empty());
        fs::copy(&source_path, &path).unwrap();
        let loaded = AppSettings::load(&path).unwrap().0;

        assert_eq!(
            loaded.ratio_for(Path::new(r"G:\Games\Example.exe")),
            AspectRatio::new(16, 9)
        );
        assert!(source_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_lines_are_skipped_and_unreadable_settings_are_reported() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("nijimi-invalid-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join(CONFIG_FILE_NAME);
        let contents =
            "@global|32:9\nnot-a-profile\n@always_on|maybe\n16:9|G:\\Games\\Example.exe\n";
        fs::write(&path, contents).unwrap();

        let (loaded, warning) = AppSettings::load(&path).unwrap();
        assert_eq!(
            loaded.default_aspect_ratio,
            AspectRatio::new(32, 9).unwrap()
        );
        assert!(!loaded.always_on);
        assert_eq!(
            loaded.ratio_for(Path::new(r"G:\Games\Example.exe")),
            AspectRatio::new(16, 9)
        );
        assert!(warning.unwrap().contains("(2, 3)"));
        let backup = directory.join(format!("{CONFIG_FILE_NAME}.bad"));
        assert_eq!(fs::read_to_string(&backup).unwrap(), contents);
        fs::remove_file(&backup).unwrap();
        fs::remove_file(&path).unwrap();

        fs::write(&path, "16:9|G:\\Games\\Example.exe\n").unwrap();
        assert!(AppSettings::load(&path).unwrap().1.is_none());
        assert!(!backup.exists());
        fs::remove_file(&path).unwrap();

        fs::create_dir(&path).unwrap();
        assert!(AppSettings::load(&path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settings_round_trip_and_replace_existing_profiles() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-{}-{unique}.ini",
            std::process::id()
        ));
        let mut settings = AppSettings::default();
        assert_eq!(
            settings.default_aspect_ratio,
            AspectRatio::new(21, 9).unwrap()
        );
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Example.exe"),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: None,
        });
        settings.default_aspect_ratio = AspectRatio::new(21, 9).unwrap();
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"c:\games\example.EXE"),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: None,
        });

        settings.save(&path).unwrap();
        let mut loaded = AppSettings::load(&path).unwrap().0;
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(
            loaded.default_aspect_ratio,
            AspectRatio::new(21, 9).unwrap()
        );
        assert_eq!(
            loaded.ratio_for(Path::new(r"C:\GAMES\EXAMPLE.exe")),
            AspectRatio::new(16, 9)
        );
        loaded.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Example.exe"),
            aspect_ratio: AspectRatio::new(32, 9).unwrap(),
            effect: None,
        });
        loaded.save(&path).unwrap();
        loaded = AppSettings::load(&path).unwrap().0;
        assert_eq!(
            loaded.ratio_for(Path::new(r"C:\Games\Example.exe")),
            AspectRatio::new(32, 9)
        );
        assert!(loaded.remove(Path::new(r"c:\games\example.exe")));
        assert_eq!(loaded.profiles.len(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn settings_without_global_ratio_keep_the_default_and_load_profiles() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-default-{}-{unique}.ini",
            std::process::id()
        ));
        fs::write(&path, "16:9|G:\\Games\\Example.exe\n").unwrap();
        let loaded = AppSettings::load(&path).unwrap().0;
        assert_eq!(
            loaded.default_aspect_ratio,
            AspectRatio::new(21, 9).unwrap()
        );
        assert_eq!(
            loaded.ratio_for(Path::new(r"G:\Games\Example.exe")),
            AspectRatio::new(16, 9)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn add_choices_keep_recent_order_and_exclude_configured_apps() {
        let recent = vec![
            PathBuf::from(r"G:\Games\Recent.exe"),
            PathBuf::from(r"C:\Games\Shared.exe"),
        ];
        let running = vec![
            PathBuf::from(r"g:\games\recent.EXE"),
            PathBuf::from(r"D:\Tools\Running.exe"),
        ];
        let profiles = vec![AppProfile {
            executable: PathBuf::from(r"C:\Games\Shared.exe"),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: None,
        }];
        let choices = add_app_choices(
            &recent,
            &running,
            &profiles,
            Path::new(r"P:\Apps\nijimi.exe"),
            Some(Path::new(r"E:\Browse\Chosen.exe")),
        );
        assert_eq!(
            choices,
            vec![
                PathBuf::from(r"E:\Browse\Chosen.exe"),
                PathBuf::from(r"G:\Games\Recent.exe"),
                PathBuf::from(r"D:\Tools\Running.exe"),
            ]
        );
    }

    #[test]
    fn profiles_sort_by_file_name_ignoring_case_then_path() {
        let profile = |path: &str| AppProfile {
            executable: PathBuf::from(path),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: None,
        };
        let profiles = [
            profile(r"G:\Games\P5R.exe"),
            profile(r"C:\Games\witcher3.exe"),
            profile(r"D:\Other\Cairn.exe"),
            profile(r"C:\Games\Cairn.exe"),
            profile(r"E:\METAPHOR.exe"),
            profile(r"F:\Esoteric Ebb.exe"),
        ];
        assert_eq!(
            sorted_profile_paths(&profiles),
            [
                r"C:\Games\Cairn.exe",
                r"D:\Other\Cairn.exe",
                r"F:\Esoteric Ebb.exe",
                r"E:\METAPHOR.exe",
                r"G:\Games\P5R.exe",
                r"C:\Games\witcher3.exe",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn app_search_matches_case_insensitive_names_and_paths_without_changing_values() {
        let paths = vec![
            PathBuf::from(r"G:\Games\ExampleGame.exe"),
            PathBuf::from(r"C:\Tools\editor.exe"),
        ];
        assert_eq!(
            filter_app_choices(&paths, "examplegame"),
            vec![PathBuf::from(r"G:\Games\ExampleGame.exe")]
        );
        assert_eq!(
            filter_app_choices(&paths, r"c:\tools"),
            vec![PathBuf::from(r"C:\Tools\editor.exe")]
        );
        assert_eq!(filter_app_choices(&paths, "  "), paths);
    }

    #[test]
    fn foreground_profile_activation_fails_closed_when_disabled_or_unmatched() {
        let mut settings = AppSettings::default();
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Example.exe"),
            aspect_ratio: AspectRatio::new(21, 9).unwrap(),
            effect: None,
        });
        let matching = Path::new(r"c:\games\EXAMPLE.EXE");
        assert_eq!(
            settings.active_profile(true, true, Some(matching)),
            Some(ActiveProfile {
                executable: Some(matching.to_path_buf()),
                aspect_ratio: AspectRatio::new(21, 9).unwrap(),
                effect: EffectSettings::default(),
            })
        );
        assert_eq!(settings.active_profile(false, true, Some(matching)), None);
        assert_eq!(settings.active_profile(true, false, Some(matching)), None);
        assert_eq!(
            settings.active_profile(true, true, Some(Path::new(r"C:\Games\Other.exe"))),
            None
        );
        assert_eq!(settings.active_profile(true, true, None), None);
    }

    #[test]
    fn always_on_renders_unprofiled_apps_at_the_default_ratio() {
        let mut settings = AppSettings {
            default_aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            always_on: true,
            ..AppSettings::default()
        };
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Example.exe"),
            aspect_ratio: AspectRatio::new(4, 3).unwrap(),
            effect: None,
        });
        let profiled = Path::new(r"C:\Games\Example.exe");
        let default = Some(ActiveProfile {
            executable: None,
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: EffectSettings::default(),
        });

        assert_eq!(
            settings.active_profile(true, true, Some(profiled)),
            Some(ActiveProfile {
                executable: Some(profiled.to_path_buf()),
                aspect_ratio: AspectRatio::new(4, 3).unwrap(),
                effect: EffectSettings::default(),
            })
        );
        assert_eq!(
            settings.active_profile(true, true, Some(Path::new(r"C:\Other.exe"))),
            default
        );
        assert_eq!(
            settings.active_profile(true, false, Some(profiled)),
            default
        );
        assert_eq!(settings.active_profile(true, true, None), default);
        assert_eq!(settings.active_profile(false, true, Some(profiled)), None);
    }

    #[test]
    fn flags_round_trip_and_default_to_off() {
        let defaults = parse_all("@global|21:9\n").unwrap();
        assert!(!defaults.always_on);
        assert!(!defaults.show_in_screenshots);
        let always_on = parse_all("@always_on|1\n").unwrap();
        assert!(always_on.always_on && !always_on.show_in_screenshots);
        let capturable = parse_all("@show_in_screenshots|1\n").unwrap();
        assert!(capturable.show_in_screenshots && !capturable.always_on);
        assert!(!parse_all("@always_on|0\n").unwrap().always_on);
        assert!(parse_all("@always_on|yes\n").is_err());
        assert!(parse_all("@show_in_screenshots|\n").is_err());

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-always-on-{}-{unique}.ini",
            std::process::id()
        ));
        for (always_on, show_in_screenshots) in [(true, false), (false, true), (true, true)] {
            let settings = AppSettings {
                always_on,
                show_in_screenshots,
                ..AppSettings::default()
            };
            settings.save(&path).unwrap();
            assert_eq!(AppSettings::load(&path).unwrap().0, settings);
        }

        let cursor_shown = AppSettings {
            hide_idle_cursor: false,
            ..AppSettings::default()
        };
        cursor_shown.save(&path).unwrap();
        assert_eq!(AppSettings::load(&path).unwrap().0, cursor_shown);

        AppSettings::default().save(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "@global|21:9\n");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn brightness_round_trips_and_is_left_out_at_full() {
        let dimmed = EffectSettings {
            brightness: 40,
            ..EffectSettings::default()
        };
        assert!(dimmed.serialize().ends_with(",brightness=40"));
        assert_eq!(EffectSettings::parse(&dimmed.serialize()), Some(dimmed));
        assert!(!EffectSettings::default().serialize().contains("brightness"));
        assert_eq!(
            EffectSettings::parse("softness=50,blend=60,blur=80,resolution=quarter")
                .unwrap()
                .brightness,
            100
        );
        assert!(EffectSettings::parse("brightness=101").is_none());
    }

    #[test]
    fn effect_settings_round_trip_globally_and_per_profile() {
        let custom = EffectSettings {
            edge_softness: 20,
            edge_blend: 40,
            blur: 70,
            resolution: BlurResolution::Eighth,
            brightness: 35,
        };
        let mut settings = AppSettings {
            default_effect: EffectSettings {
                edge_blend: 15,
                ..EffectSettings::default()
            },
            ..AppSettings::default()
        };
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Custom.exe"),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            effect: Some(custom),
        });
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"C:\Games\Global.exe"),
            aspect_ratio: AspectRatio::new(4, 3).unwrap(),
            effect: None,
        });

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-effect-{}-{unique}.ini",
            std::process::id()
        ));
        settings.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "@global|21:9\n\
             @effect|softness=50,blend=15,blur=80,resolution=quarter\n\
             16:9,softness=20,blend=40,blur=70,resolution=eighth,brightness=35|C:\\Games\\Custom.exe\n\
             4:3|C:\\Games\\Global.exe\n"
        );
        let loaded = AppSettings::load(&path).unwrap().0;
        fs::remove_file(path).unwrap();
        assert_eq!(loaded, settings);

        let custom_profile = loaded
            .active_profile(true, true, Some(Path::new(r"C:\Games\Custom.exe")))
            .unwrap();
        assert_eq!(custom_profile.effect, custom);
        let global_profile = loaded
            .active_profile(true, true, Some(Path::new(r"C:\Games\Global.exe")))
            .unwrap();
        assert_eq!(global_profile.effect, loaded.default_effect);
    }

    #[test]
    fn full_resolution_round_trips() {
        let loaded = parse_all("@effect|resolution=full\n").unwrap();
        assert_eq!(loaded.default_effect.resolution, BlurResolution::Full);
        assert_eq!(BlurResolution::Full.scale(), 1);
        assert_eq!(
            loaded.default_effect.serialize(),
            "softness=50,blend=60,blur=80,resolution=full"
        );
    }

    #[test]
    fn frame_rate_round_trips_and_defaults_to_30() {
        assert_eq!(AppSettings::default().frame_rate, FrameRate::Thirty);
        for (line, rate) in [
            ("@frame_rate|30\n", FrameRate::Thirty),
            ("@frame_rate|60\n", FrameRate::Sixty),
            ("@frame_rate|match\n", FrameRate::Sixty),
        ] {
            assert_eq!(parse_all(line).unwrap().frame_rate, rate);
        }
        assert!(parse_all("@frame_rate|144\n").is_err());
        assert_eq!(FrameRate::Thirty.fps(), 30);
        assert_eq!(FrameRate::Sixty.fps(), 60);

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-frame-rate-{}-{unique}.ini",
            std::process::id()
        ));
        let settings = AppSettings {
            frame_rate: FrameRate::Sixty,
            ..AppSettings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "@global|21:9\n@frame_rate|60\n"
        );
        assert_eq!(AppSettings::load(&path).unwrap().0, settings);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn partial_effect_lines_keep_defaults_and_invalid_values_are_rejected() {
        let loaded = parse_all("@effect|softness=30\n").unwrap();
        assert_eq!(
            loaded.default_effect,
            EffectSettings {
                edge_softness: 30,
                ..EffectSettings::default()
            }
        );
        assert!(parse_all("@effect|blend=101\n").is_err());
        assert!(parse_all("@effect|softness=101\n").is_err());
        assert!(parse_all("@effect|resolution=double\n").is_err());
        assert!(parse_all("@effect|glow=3\n").is_err());
        assert!(parse_all("16:9,blur=x|C:\\Game.exe\n").is_err());
    }
}
