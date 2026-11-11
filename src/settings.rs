use std::fs;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

use crate::geometry::AspectRatio;

const CONFIG_FILE_NAME: &str = "nijimi.ini";
const APP_DIR_NAME: &str = "nijimi";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppProfile {
    pub executable: PathBuf,
    pub aspect_ratio: AspectRatio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSettings {
    pub default_aspect_ratio: AspectRatio,
    pub always_on: bool,
    pub show_in_screenshots: bool,
    pub profiles: Vec<AppProfile>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_aspect_ratio: AspectRatio::new(21, 9).unwrap(),
            always_on: false,
            show_in_screenshots: false,
            profiles: Vec::new(),
        }
    }
}

impl AspectRatio {
    pub fn parse(value: &str) -> Option<Self> {
        let (width, height) = value.trim().split_once(':')?;
        Self::new(width.trim().parse().ok()?, height.trim().parse().ok()?)
    }
}

impl AppSettings {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };

        Self::parse(&contents)
    }

    fn parse(contents: &str) -> Result<Self> {
        let mut settings = Self::default();
        for (line_number, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let Some((ratio, executable)) = line.split_once('|') else {
                return Err(invalid_line(line_number));
            };
            if ratio == "@global" {
                settings.default_aspect_ratio =
                    AspectRatio::parse(executable).ok_or_else(|| invalid_line(line_number))?;
                continue;
            }
            let flag = match ratio {
                "@always_on" => Some(&mut settings.always_on),
                "@show_in_screenshots" => Some(&mut settings.show_in_screenshots),
                _ => None,
            };
            if let Some(flag) = flag {
                *flag = match executable.trim() {
                    "1" => true,
                    "0" => false,
                    _ => return Err(invalid_line(line_number)),
                };
                continue;
            }
            let aspect_ratio =
                AspectRatio::parse(ratio).ok_or_else(|| invalid_line(line_number))?;
            let executable = executable.trim();
            if executable.is_empty() {
                return Err(invalid_line(line_number));
            }
            settings.upsert(AppProfile {
                executable: PathBuf::from(executable),
                aspect_ratio,
            });
        }
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut contents = String::new();
        contents.push_str(&format!(
            "@global|{}:{}\n",
            self.default_aspect_ratio.width, self.default_aspect_ratio.height
        ));
        // Omitted when off so older builds, which reject unknown lines, can still load the file.
        if self.always_on {
            contents.push_str("@always_on|1\n");
        }
        if self.show_in_screenshots {
            contents.push_str("@show_in_screenshots|1\n");
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
            contents.push_str(&format!(
                "{}:{}|{}\n",
                profile.aspect_ratio.width, profile.aspect_ratio.height, executable
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

    pub fn ratio_for(&self, executable: &Path) -> Option<AspectRatio> {
        self.profiles
            .iter()
            .find(|profile| paths_match(&profile.executable, executable))
            .map(|profile| profile.aspect_ratio)
    }

    pub fn active_profile(
        &self,
        enabled: bool,
        on_primary_display: bool,
        foreground: Option<&Path>,
    ) -> Option<(Option<PathBuf>, AspectRatio)> {
        if !enabled {
            return None;
        }
        let profile = foreground
            .filter(|_| on_primary_display)
            .and_then(|executable| {
                self.ratio_for(executable)
                    .map(|ratio| (Some(executable.to_path_buf()), ratio))
            });
        if profile.is_none() && self.always_on {
            return Some((None, self.default_aspect_ratio));
        }
        profile
    }
}

pub fn paths_match(left: &Path, right: &Path) -> bool {
    left.as_os_str()
        .to_string_lossy()
        .eq_ignore_ascii_case(&right.as_os_str().to_string_lossy())
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

    let mut unique: Vec<PathBuf> = Vec::new();
    for path in choices {
        if paths_match(&path, application_executable)
            || profiles
                .iter()
                .any(|profile| paths_match(&profile.executable, &path))
            || unique.iter().any(|existing| paths_match(existing, &path))
        {
            continue;
        }
        unique.push(path);
    }
    unique
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

/// Picks the settings file, in order: an existing portable file, an existing per-user file,
/// a new portable file if the executable's folder is writable, otherwise a new per-user file.
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

fn invalid_line(line_number: usize) -> Error {
    Error::new(
        ErrorKind::InvalidData,
        format!("invalid nijimi settings line {}", line_number + 1),
    )
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

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

        let missing_config = AppSettings::load(&path).unwrap();
        assert!(missing_config.profiles.is_empty());
        fs::copy(&source_path, &path).unwrap();
        let loaded = AppSettings::load(&path).unwrap();

        assert_eq!(
            loaded.ratio_for(Path::new(r"G:\Games\Example.exe")),
            AspectRatio::new(16, 9)
        );
        assert!(source_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_or_unreadable_settings_are_reported() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("nijimi-invalid-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join(CONFIG_FILE_NAME);
        fs::write(&path, "not-a-profile\n").unwrap();
        assert_eq!(
            AppSettings::load(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
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
        });
        settings.default_aspect_ratio = AspectRatio::new(21, 9).unwrap();
        settings.upsert(AppProfile {
            executable: PathBuf::from(r"c:\games\example.EXE"),
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
        });

        settings.save(&path).unwrap();
        let mut loaded = AppSettings::load(&path).unwrap();
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
        });
        loaded.save(&path).unwrap();
        loaded = AppSettings::load(&path).unwrap();
        assert_eq!(
            loaded.ratio_for(Path::new(r"C:\Games\Example.exe")),
            AspectRatio::new(32, 9)
        );
        assert!(loaded.remove(Path::new(r"c:\games\example.exe")));
        assert_eq!(loaded.profiles.len(), 0);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_settings_are_reported_instead_of_silently_discarded() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nijimi-settings-invalid-{}-{unique}.ini",
            std::process::id()
        ));
        fs::write(&path, "not-a-profile\n").unwrap();
        assert_eq!(
            AppSettings::load(&path).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
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
        let loaded = AppSettings::load(&path).unwrap();
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
        });
        let matching = Path::new(r"c:\games\EXAMPLE.EXE");
        assert_eq!(
            settings.active_profile(true, true, Some(matching)),
            Some((
                Some(matching.to_path_buf()),
                AspectRatio::new(21, 9).unwrap()
            ))
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
        });
        let profiled = Path::new(r"C:\Games\Example.exe");
        let default = Some((None, AspectRatio::new(16, 9).unwrap()));

        assert_eq!(
            settings.active_profile(true, true, Some(profiled)),
            Some((
                Some(profiled.to_path_buf()),
                AspectRatio::new(4, 3).unwrap()
            ))
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
        let defaults = AppSettings::parse("@global|21:9\n").unwrap();
        assert!(!defaults.always_on);
        assert!(!defaults.show_in_screenshots);
        let always_on = AppSettings::parse("@always_on|1\n").unwrap();
        assert!(always_on.always_on && !always_on.show_in_screenshots);
        let capturable = AppSettings::parse("@show_in_screenshots|1\n").unwrap();
        assert!(capturable.show_in_screenshots && !capturable.always_on);
        assert!(!AppSettings::parse("@always_on|0\n").unwrap().always_on);
        assert!(AppSettings::parse("@always_on|yes\n").is_err());
        assert!(AppSettings::parse("@show_in_screenshots|\n").is_err());

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
            assert_eq!(AppSettings::load(&path).unwrap(), settings);
        }

        AppSettings::default().save(&path).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "@global|21:9\n");
        fs::remove_file(path).unwrap();
    }
}
