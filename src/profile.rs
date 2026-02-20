use crate::parse::Level;
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub min_level: Level,
    pub theme: Theme,
    pub highlights: Vec<String>,
}

impl Profile {
    pub fn default_profile() -> Self {
        Profile {
            name: "default".into(),
            min_level: Level::Info,
            theme: Theme::matrix(),
            highlights: vec![],
        }
    }

    pub fn ops() -> Self {
        Profile {
            name: "ops".into(),
            min_level: Level::Warn,
            theme: Theme::matrix(),
            highlights: vec![
                "panic".into(),
                "timeout".into(),
                "error".into(),
                "fail".into(),
                "refused".into(),
                "disconnect".into(),
            ],
        }
    }

    pub fn network() -> Self {
        Profile {
            name: "network".into(),
            min_level: Level::Warn,
            theme: Theme::matrix(),
            highlights: vec![
                "down".into(),
                "up".into(),
                "flap".into(),
                "reset".into(),
                "timeout".into(),
                "link".into(),
                "vpn".into(),
                "error".into(),
            ],
        }
    }

    pub fn all_profiles() -> Vec<Profile> {
        vec![Self::default_profile(), Self::ops(), Self::network()]
    }
}
