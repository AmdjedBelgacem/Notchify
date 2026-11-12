//! Interface language: English (the source), French and Arabic.
//!
//! Every piece of interface text passes through the shared text helpers in
//! `ui` (labels, attributed strings, buttons, menus, alerts), which look it up
//! here. The tables in `strings.json` map an English string to its French and
//! Arabic forms; anything not in them (a song title, a notification) shows as
//! it is. Arabic text is translated; the layout itself still reads left to
//! right.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Fr,
    Ar,
}

/// The choices offered in Settings, as (config value, name in its own language).
pub const CHOICES: [(&str, &str); 4] = [("system", "System"), ("en", "English"), ("fr", "Français"), ("ar", "العربية")];

thread_local! {
    static LANG: Cell<Lang> = const { Cell::new(Lang::En) };
    /// Strings seen while recording (`NOTCH_I18N_DUMP`), so the tables can be
    /// checked for anything the interface shows that has no translation.
    static SEEN: RefCell<Option<HashSet<String>>> = const { RefCell::new(None) };
}

type Table = HashMap<String, (String, String)>;

fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| {
        let raw: HashMap<String, HashMap<String, String>> =
            serde_json::from_str(include_str!("i18n/strings.json")).unwrap_or_default();
        raw.into_iter()
            .map(|(en, t)| {
                let fr = t.get("fr").cloned().unwrap_or_default();
                let ar = t.get("ar").cloned().unwrap_or_default();
                (en, (fr, ar))
            })
            .collect()
    })
}

/// Pick the language from the setting: "en", "fr", "ar", or "system" (the
/// first of the Mac's preferred languages we know).
pub fn set_language(setting: &str) {
    // NOTCH_LANG overrides the setting (renders in each language for checks).
    let forced = std::env::var("NOTCH_LANG").ok();
    let setting = forced.as_deref().unwrap_or(setting);
    let lang = match setting {
        "en" => Lang::En,
        "fr" => Lang::Fr,
        "ar" => Lang::Ar,
        _ => system_language(),
    };
    LANG.with(|l| l.set(lang));
}

pub fn language() -> Lang {
    LANG.with(|l| l.get())
}

fn system_language() -> Lang {
    let preferred = objc2_foundation::NSLocale::preferredLanguages();
    for code in preferred.iter() {
        let code = code.to_string();
        if code.starts_with("fr") {
            return Lang::Fr;
        }
        if code.starts_with("ar") {
            return Lang::Ar;
        }
        if code.starts_with("en") {
            return Lang::En;
        }
    }
    Lang::En
}

/// `s` in the interface language (itself when untranslated or English).
pub fn tr(s: &str) -> std::borrow::Cow<'_, str> {
    record(s);
    let lang = language();
    if lang == Lang::En || s.is_empty() {
        return std::borrow::Cow::Borrowed(s);
    }
    match table().get(s) {
        Some((fr, _)) if lang == Lang::Fr && !fr.is_empty() => std::borrow::Cow::Owned(fr.clone()),
        Some((_, ar)) if lang == Lang::Ar && !ar.is_empty() => std::borrow::Cow::Owned(ar.clone()),
        _ => std::borrow::Cow::Borrowed(s),
    }
}

/// While `NOTCH_I18N_DUMP` names a file, remember each distinct string shown
/// and append it there (one JSON string per line).
fn record(s: &str) {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    let Some(path) = PATH.get_or_init(|| std::env::var("NOTCH_I18N_DUMP").ok()) else { return };
    if s.trim().is_empty() || !s.chars().any(|c| c.is_alphabetic()) {
        return;
    }
    let new = SEEN.with(|seen| seen.borrow_mut().get_or_insert_with(HashSet::new).insert(s.to_owned()));
    if new {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{}", serde_json::to_string(s).unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_parse_and_translate() {
        assert!(!table().is_empty(), "strings.json must parse");
        set_language("fr");
        assert_eq!(tr("Settings"), "Réglages");
        set_language("ar");
        assert_eq!(tr("Settings"), "الإعدادات");
        set_language("en");
        assert_eq!(tr("Settings"), "Settings");
        assert_eq!(tr("A song nobody translated"), "A song nobody translated");
    }

    #[test]
    fn every_entry_has_both_languages() {
        let missing: Vec<&String> =
            table().iter().filter(|(_, (fr, ar))| fr.is_empty() || ar.is_empty()).map(|(en, _)| en).collect();
        assert!(missing.is_empty(), "untranslated: {missing:?}");
    }
}
