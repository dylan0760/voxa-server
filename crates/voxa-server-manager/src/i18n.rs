//! Voxa Server Manager in other languages (1.30.0+): `t!("English text")` is the text in
//! the chosen language (🌐 in the header, else Windows' language). The translations are
//! Voxa's (voxa-proto `i18n/<code>.json`).

/// The translation of an English text literal.
macro_rules! t {
    ($s:literal) => {
        voxa_proto::i18n::tr($s)
    };
}
pub(crate) use t;

/// A translated `format!`: `tf!("{} of {}", a, b; name = name)` (positional values, then
/// the `{name}` ones after a `;`).
macro_rules! tf {
    ($s:literal $(, $a:expr)* $(; $($k:ident = $v:expr),* )? $(,)?) => {
        voxa_proto::i18n::fill(voxa_proto::i18n::tr($s), &[$(&$a as &dyn std::fmt::Display),*], &[$($((stringify!($k), &$v as &dyn std::fmt::Display)),*)?])
    };
}
pub(crate) use tf;

/// The language to use: the one chosen in Options, or the system's.
pub fn chosen(setting: Option<&str>) -> String {
    match setting.filter(|s| !s.is_empty()) {
        Some(code) => voxa_proto::i18n::lang(code).map(|l| l.code).unwrap_or("en").to_string(),
        None => voxa_proto::i18n::from_locale(&system_locale()).to_string(),
    }
}

/// Switches the language (and the font for its letters).
pub fn apply(code: &str, ctx: &egui::Context) {
    let rtl = voxa_proto::i18n::lang(code).is_some_and(|l| l.rtl);
    if rtl {
        voxa_proto::i18n::set_language(code, Some(shape));
    } else {
        voxa_proto::i18n::set_language(code, None);
    }
    crate::install_fonts(ctx);
}

/// Where the chosen language is kept (per Windows user).
fn file() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Voxa Server Manager").join("language.txt"))
}

/// The language chosen in the Manager (None: Windows' language).
pub fn saved() -> Option<String> {
    file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| s.trim().to_string()).filter(|s| voxa_proto::i18n::lang(s).is_some())
}

pub fn save(code: Option<&str>) {
    if let Some(f) = file() {
        match code {
            Some(c) => {
                let _ = std::fs::create_dir_all(f.parent().unwrap_or(&f));
                let _ = std::fs::write(&f, c);
            }
            None => {
                let _ = std::fs::remove_file(&f);
            }
        }
    }
}

/// Right-to-left text (Arabic, Persian, Urdu) prepared for Voxa's left-to-right drawing:
/// letters joined (the forms they take inside words) and each line put in visual order.
pub fn shape(s: &str) -> String {
    s.split('\n').map(shape_line).collect::<Vec<_>>().join("\n")
}

fn shape_line(line: &str) -> String {
    let joined = ar_reshaper::reshape_line(line);
    let info = unicode_bidi::BidiInfo::new(&joined, Some(unicode_bidi::Level::rtl()));
    match info.paragraphs.first() {
        Some(para) => info.reorder_line(para, para.range.clone()).into_owned(),
        None => joined,
    }
}

/// The system's language, like `de-DE`.
#[cfg(windows)]
pub fn system_locale() -> String {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    let mut buf = [0u16; 85];
    // SAFETY: the buffer is LOCALE_NAME_MAX_LENGTH long, as the call expects.
    let n = unsafe { GetUserDefaultLocaleName(&mut buf) };
    if n > 1 {
        String::from_utf16_lossy(&buf[..n as usize - 1])
    } else {
        "en".into()
    }
}

/// Android: the system's language from its properties.
#[cfg(target_os = "android")]
pub fn system_locale() -> String {
    for prop in ["persist.sys.locale", "ro.product.locale", "persist.sys.language"] {
        if let Ok(out) = std::process::Command::new("/system/bin/getprop").arg(prop).output() {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !v.is_empty() {
                return v;
            }
        }
    }
    "en".into()
}

#[cfg(not(any(windows, target_os = "android")))]
pub fn system_locale() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "en".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_is_joined_and_reversed() {
        // "سلام" (four letters) becomes presentation forms (lam + alef join into one),
        // drawn right to left: the meem (alone, alef does not join forward) comes first.
        let s = shape("سلام");
        assert_eq!(s.chars().count(), 3);
        assert_eq!(s.chars().next(), Some('\u{FEE1}'));
        assert!(s.chars().all(|c| ('\u{FB50}'..='\u{FEFF}').contains(&c)), "{s:?}");
        // Numbers and Latin words keep their own direction.
        let mixed = shape("Voxa 1.38");
        assert_eq!(mixed, "Voxa 1.38");
    }
}
