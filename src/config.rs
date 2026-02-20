use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

use crate::parse::Level;
use crate::profile::Profile;
use crate::theme::Theme;

#[derive(Debug, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub profiles: std::collections::HashMap<String, ProfileConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ProfileConfig {
    #[serde(default = "default_min_level")]
    pub min_level: String,
    #[serde(default = "default_theme_name")]
    pub theme: String,
    #[serde(default)]
    pub highlights: Vec<String>,
}

fn default_min_level() -> String {
    "INFO".into()
}

fn default_theme_name() -> String {
    "matrix".into()
}

impl Config {
    pub fn load(explicit_path: Option<&str>) -> Result<Config> {
        if let Some(path) = explicit_path {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }

        // Try ./logradar.toml
        let local = PathBuf::from("logradar.toml");
        if local.exists() {
            let content = std::fs::read_to_string(&local)?;
            let config: Config = toml::from_str(&content)?;
            return Ok(config);
        }

        // Try ~/.config/logradar/config.toml
        if let Some(config_dir) = dirs::config_dir() {
            let global = config_dir.join("logradar").join("config.toml");
            if global.exists() {
                let content = std::fs::read_to_string(&global)?;
                let config: Config = toml::from_str(&content)?;
                return Ok(config);
            }
        }

        Ok(Config::default())
    }

    pub fn into_profiles(self) -> Vec<Profile> {
        let mut profiles = Profile::all_profiles();

        for (name, pc) in self.profiles {
            let level = parse_level(&pc.min_level);
            let theme = Theme::by_name(&pc.theme).unwrap_or_else(Theme::matrix);
            // Check if this overrides a built-in profile
            if let Some(existing) = profiles.iter_mut().find(|p| p.name == name) {
                existing.min_level = level;
                existing.theme = theme;
                existing.highlights = pc.highlights;
            } else {
                profiles.push(Profile {
                    name,
                    min_level: level,
                    theme,
                    highlights: pc.highlights,
                });
            }
        }

        profiles
    }
}

fn parse_level(s: &str) -> Level {
    match s.to_ascii_uppercase().as_str() {
        "TRACE" => Level::Trace,
        "DEBUG" => Level::Debug,
        "INFO" => Level::Info,
        "WARN" | "WARNING" => Level::Warn,
        "ERROR" => Level::Error,
        _ => Level::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_config() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.default_profile.is_none());
        assert!(cfg.profiles.is_empty());
    }

    #[test]
    fn parse_default_profile() {
        let cfg: Config = toml::from_str(r#"default_profile = "ops""#).unwrap();
        assert_eq!(cfg.default_profile.as_deref(), Some("ops"));
    }

    #[test]
    fn parse_custom_profile() {
        let toml_str = r#"
[profiles.myapp]
min_level = "DEBUG"
theme = "mono"
highlights = ["panic", "crash"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let pc = &cfg.profiles["myapp"];
        assert_eq!(pc.min_level, "DEBUG");
        assert_eq!(pc.theme, "mono");
        assert_eq!(pc.highlights, vec!["panic", "crash"]);
    }

    #[test]
    fn into_profiles_adds_custom() {
        let toml_str = r#"
[profiles.myapp]
min_level = "TRACE"
theme = "color"
highlights = ["custom"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let profiles = cfg.into_profiles();
        // 3 built-in + 1 custom
        assert_eq!(profiles.len(), 4);
        let custom = profiles.iter().find(|p| p.name == "myapp").unwrap();
        assert_eq!(custom.min_level, Level::Trace);
        assert_eq!(custom.highlights, vec!["custom"]);
    }

    #[test]
    fn into_profiles_overrides_builtin() {
        let toml_str = r#"
[profiles.default]
min_level = "ERROR"
theme = "mono"
highlights = ["critical"]
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let profiles = cfg.into_profiles();
        // Should still be 3 (override, not add)
        assert_eq!(profiles.len(), 3);
        let default = profiles.iter().find(|p| p.name == "default").unwrap();
        assert_eq!(default.min_level, Level::Error);
        assert_eq!(default.highlights, vec!["critical"]);
    }

    #[test]
    fn parse_level_variants() {
        assert_eq!(parse_level("TRACE"), Level::Trace);
        assert_eq!(parse_level("debug"), Level::Debug);
        assert_eq!(parse_level("Info"), Level::Info);
        assert_eq!(parse_level("WARN"), Level::Warn);
        assert_eq!(parse_level("WARNING"), Level::Warn);
        assert_eq!(parse_level("ERROR"), Level::Error);
        assert_eq!(parse_level("garbage"), Level::Info); // fallback
    }

    #[test]
    fn load_returns_default_when_no_file() {
        let cfg = Config::load(None).unwrap();
        assert!(cfg.default_profile.is_none());
        assert!(cfg.profiles.is_empty());
    }
}
