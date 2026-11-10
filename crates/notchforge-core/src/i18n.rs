//! A small translation scaffold.
//!
//! The UI is English by default; `Language` picks from the config and
//! [`tr`] translates the strings that have been carried over so far (the
//! panel's headers and its most visible states). Strings not yet in the
//! table fall back to English, so adding UI never waits on translations.

use crate::config::GeneralConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    #[default]
    English,
    French,
    Arabic,
}

impl Language {
    pub fn from_config(cfg: &GeneralConfig) -> Self {
        match cfg.language.as_str() {
            "fr" => Self::French,
            "ar" => Self::Arabic,
            _ => Self::English,
        }
    }

    /// Right-to-left layout?
    pub fn is_rtl(self) -> bool {
        matches!(self, Self::Arabic)
    }
}

/// The strings the UI reads through [`tr`]. Each has an English base that
/// other languages fall back to.
#[derive(Debug, Clone, Copy)]
pub enum Str {
    CaughtUp,
    NoOlderNotifications,
    Notifications,
    Tray,
    History,
    Reply,
    Snooze,
    AllCaughtUpHint,
}

/// Translate `key` into `lang`.
pub fn tr(lang: Language, key: Str) -> &'static str {
    match (lang, key) {
        (_, Str::CaughtUp) => "You're all caught up",
        (_, Str::NoOlderNotifications) => "No older notifications",
        (Language::French, Str::Notifications) => "Notifications",
        (Language::Arabic, Str::Notifications) => "الإشعارات",
        (_, Str::Notifications) => "Notifications",
        (Language::French, Str::Tray) => "Plateau",
        (Language::Arabic, Str::Tray) => "الحافظة",
        (_, Str::Tray) => "Tray",
        (Language::French, Str::History) => "Historique",
        (Language::Arabic, Str::History) => "السجل",
        (_, Str::History) => "History",
        (Language::French, Str::Reply) => "Répondre",
        (Language::Arabic, Str::Reply) => "رد",
        (_, Str::Reply) => "Reply",
        (Language::French, Str::Snooze) => "Plus tard",
        (Language::Arabic, Str::Snooze) => "لاحقًا",
        (_, Str::Snooze) => "Snooze",
        (_, Str::AllCaughtUpHint) => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_english() {
        assert_eq!(tr(Language::Arabic, Str::CaughtUp), "You're all caught up");
        assert_eq!(tr(Language::French, Str::Tray), "Plateau");
        assert_eq!(tr(Language::English, Str::Tray), "Tray");
        assert_eq!(Language::from_config(&GeneralConfig::default()), Language::English);
    }
}
