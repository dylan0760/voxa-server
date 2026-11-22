//! Languages (Voxa 1.38.0+, Voxa Server 1.30.0+): the list of languages, their
//! translations and the current one. Shared by Voxa, Voxa Server Manager and the web page
//! (which gets the same files from the server).
//!
//! A translation file (`i18n/<code>.json`) maps the English text to the translation:
//! `{ "Connect": "Verbinden", ... }`. Texts without a translation stay English. `{}` in a
//! text is filled in by the program (the translation keeps the same number of them).

use std::collections::HashMap;
use std::sync::RwLock;

/// The writing system: which extra font a language needs (most need none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Latin,
    Cyrillic,
    Greek,
    ChineseSimplified,
    ChineseTraditional,
    Japanese,
    Korean,
    Arabic,
    Devanagari,
    Bengali,
    Thai,
}

#[derive(Clone, Copy, Debug)]
pub struct Lang {
    pub code: &'static str,
    /// The language's own name for itself.
    pub name: &'static str,
    pub english: &'static str,
    /// Written right to left (Arabic, Persian, Urdu).
    pub rtl: bool,
    pub script: Script,
}

const fn l(code: &'static str, name: &'static str, english: &'static str, script: Script) -> Lang {
    Lang { code, name, english, rtl: false, script }
}

const fn rtl(code: &'static str, name: &'static str, english: &'static str) -> Lang {
    Lang { code, name, english, rtl: true, script: Script::Arabic }
}

use Script::*;

/// English first; then the 20 most spoken languages, then the other European ones.
pub const LANGS: &[Lang] = &[
    l("en", "English", "English", Latin),
    l("zh-CN", "简体中文", "Chinese (Simplified)", ChineseSimplified),
    l("hi", "हिन्दी", "Hindi", Devanagari),
    l("es", "Español", "Spanish", Latin),
    l("fr", "Français", "French", Latin),
    rtl("ar", "العربية", "Arabic"),
    l("bn", "বাংলা", "Bengali", Bengali),
    l("pt-BR", "Português (Brasil)", "Portuguese (Brazil)", Latin),
    l("ru", "Русский", "Russian", Cyrillic),
    rtl("ur", "اردو", "Urdu"),
    l("id", "Bahasa Indonesia", "Indonesian", Latin),
    l("de", "Deutsch", "German", Latin),
    l("ja", "日本語", "Japanese", Japanese),
    l("tr", "Türkçe", "Turkish", Latin),
    l("ko", "한국어", "Korean", Korean),
    l("vi", "Tiếng Việt", "Vietnamese", Latin),
    l("it", "Italiano", "Italian", Latin),
    l("th", "ไทย", "Thai", Thai),
    rtl("fa", "فارسی", "Persian"),
    l("pl", "Polski", "Polish", Latin),
    l("nl", "Nederlands", "Dutch", Latin),
    l("sv", "Svenska", "Swedish", Latin),
    l("nb", "Norsk bokmål", "Norwegian", Latin),
    l("da", "Dansk", "Danish", Latin),
    l("fi", "Suomi", "Finnish", Latin),
    l("cs", "Čeština", "Czech", Latin),
    l("sk", "Slovenčina", "Slovak", Latin),
    l("hu", "Magyar", "Hungarian", Latin),
    l("ro", "Română", "Romanian", Latin),
    l("el", "Ελληνικά", "Greek", Greek),
    l("uk", "Українська", "Ukrainian", Cyrillic),
    l("bg", "Български", "Bulgarian", Cyrillic),
    l("hr", "Hrvatski", "Croatian", Latin),
    l("sr", "Српски", "Serbian", Cyrillic),
    l("sl", "Slovenščina", "Slovenian", Latin),
    l("lt", "Lietuvių", "Lithuanian", Latin),
    l("lv", "Latviešu", "Latvian", Latin),
    l("et", "Eesti", "Estonian", Latin),
    l("ca", "Català", "Catalan", Latin),
    l("pt-PT", "Português (Portugal)", "Portuguese (Portugal)", Latin),
    l("zh-TW", "繁體中文", "Chinese (Traditional)", ChineseTraditional),
];

pub fn lang(code: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.code.eq_ignore_ascii_case(code))
}

/// The translation file of a language (built in).
pub fn json(code: &str) -> Option<&'static str> {
    Some(match code {
        "zh-CN" => include_str!("../i18n/zh-CN.json"),
        "hi" => include_str!("../i18n/hi.json"),
        "es" => include_str!("../i18n/es.json"),
        "fr" => include_str!("../i18n/fr.json"),
        "ar" => include_str!("../i18n/ar.json"),
        "bn" => include_str!("../i18n/bn.json"),
        "pt-BR" => include_str!("../i18n/pt-BR.json"),
        "ru" => include_str!("../i18n/ru.json"),
        "ur" => include_str!("../i18n/ur.json"),
        "id" => include_str!("../i18n/id.json"),
        "de" => include_str!("../i18n/de.json"),
        "ja" => include_str!("../i18n/ja.json"),
        "tr" => include_str!("../i18n/tr.json"),
        "ko" => include_str!("../i18n/ko.json"),
        "vi" => include_str!("../i18n/vi.json"),
        "it" => include_str!("../i18n/it.json"),
        "th" => include_str!("../i18n/th.json"),
        "fa" => include_str!("../i18n/fa.json"),
        "pl" => include_str!("../i18n/pl.json"),
        "nl" => include_str!("../i18n/nl.json"),
        "sv" => include_str!("../i18n/sv.json"),
        "nb" => include_str!("../i18n/nb.json"),
        "da" => include_str!("../i18n/da.json"),
        "fi" => include_str!("../i18n/fi.json"),
        "cs" => include_str!("../i18n/cs.json"),
        "sk" => include_str!("../i18n/sk.json"),
        "hu" => include_str!("../i18n/hu.json"),
        "ro" => include_str!("../i18n/ro.json"),
        "el" => include_str!("../i18n/el.json"),
        "uk" => include_str!("../i18n/uk.json"),
        "bg" => include_str!("../i18n/bg.json"),
        "hr" => include_str!("../i18n/hr.json"),
        "sr" => include_str!("../i18n/sr.json"),
        "sl" => include_str!("../i18n/sl.json"),
        "lt" => include_str!("../i18n/lt.json"),
        "lv" => include_str!("../i18n/lv.json"),
        "et" => include_str!("../i18n/et.json"),
        "ca" => include_str!("../i18n/ca.json"),
        "pt-PT" => include_str!("../i18n/pt-PT.json"),
        "zh-TW" => include_str!("../i18n/zh-TW.json"),
        _ => return None,
    })
}

/// The language for a system locale like `de-AT`, `pt_BR`, `zh-Hant-TW` or `nn`.
pub fn from_locale(locale: &str) -> &'static str {
    let loc = locale.trim().replace('_', "-").to_ascii_lowercase();
    let loc = loc.split(['.', '@']).next().unwrap_or_default().to_string();
    if loc.starts_with("zh") {
        return if loc.contains("hant") || loc.ends_with("-tw") || loc.ends_with("-hk") || loc.ends_with("-mo") { "zh-TW" } else { "zh-CN" };
    }
    if loc.starts_with("pt") {
        return if loc.ends_with("-pt") { "pt-PT" } else { "pt-BR" };
    }
    let base = loc.split('-').next().unwrap_or_default();
    let base = match base {
        "no" | "nn" => "nb",
        "in" => "id",
        b => b,
    };
    LANGS.iter().find(|l| l.code == base).map(|l| l.code).unwrap_or("en")
}

/// English text -> translation (only the texts that have one).
pub fn parse(json: &str) -> HashMap<String, String> {
    serde_json::from_str::<HashMap<String, String>>(json).unwrap_or_default().into_iter().filter(|(k, v)| !v.trim().is_empty() && k != v && same_placeholders(k, v)).collect()
}

/// A translation must keep the `{}` / `{name}` of its text (else the program's values
/// would be lost).
pub fn same_placeholders(english: &str, translated: &str) -> bool {
    fn marks(s: &str) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        let mut rest = s;
        while let Some(i) = rest.find('{') {
            match rest[i..].find('}') {
                Some(j) => {
                    v.push(&rest[i..i + j + 1]);
                    rest = &rest[i + j + 1..];
                }
                None => break,
            }
        }
        v.sort_unstable();
        v
    }
    marks(english) == marks(translated)
}

/// A (translated) text with its `{}` filled in from `positional`, in order, and each
/// `{name}` from `named`.
pub fn fill(text: &str, positional: &[&dyn std::fmt::Display], named: &[(&str, &dyn std::fmt::Display)]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut pos = positional.iter();
    let mut rest = text;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let key = &rest[i + 1..i + j];
        let value: Option<&dyn std::fmt::Display> = if key.is_empty() { pos.next().copied() } else { named.iter().find(|(k, _)| *k == key).map(|(_, v)| *v) };
        match value {
            Some(v) => out.push_str(&v.to_string()),
            None => out.push_str(&rest[i..i + j + 1]),
        }
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    displayed(text, out)
}

/// A filled-in text prepared for showing, when its template had placeholders.
fn displayed(template: &str, out: String) -> String {
    match DISPLAY.read().ok().and_then(|d| *d) {
        Some(f) if template.contains('{') => f(&out),
        _ => out,
    }
}

type Table = HashMap<String, &'static str>;

static CURRENT: RwLock<Option<(&'static str, Table)>> = RwLock::new(None);

/// How texts are prepared for showing (see `set_language`).
static DISPLAY: RwLock<Option<fn(&str) -> String>> = RwLock::new(None);

/// Switches the language. `display`: how texts are prepared for showing (right-to-left
/// text is reshaped for apps that draw text themselves; the web page does not need it).
/// Texts with `{}` / `{name}` are prepared after they are filled in (`fill`), so the
/// reordering cannot turn their braces around.
pub fn set_language(code: &str, display: Option<fn(&str) -> String>) {
    let code = lang(code).map(|l| l.code).unwrap_or("en");
    if let Ok(mut d) = DISPLAY.write() {
        *d = display;
    }
    let table: Table = json(code)
        .map(parse)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| {
            let v = match display {
                Some(f) if !v.contains('{') => f(&v),
                _ => v,
            };
            // Leaked once per language switch: texts live for the rest of the run.
            (k, &*Box::leak(v.into_boxed_str()))
        })
        .collect();
    if let Ok(mut c) = CURRENT.write() {
        *c = Some((code, table));
    }
}

/// The language in use.
pub fn current() -> &'static str {
    CURRENT.read().ok().and_then(|c| c.as_ref().map(|(code, _)| *code)).unwrap_or("en")
}

/// The translation of an English text (the text itself if there is none).
pub fn tr(english: &'static str) -> &'static str {
    CURRENT.read().ok().and_then(|c| c.as_ref().and_then(|(_, t)| t.get(english).copied())).unwrap_or(english)
}

/// The translation of a text known only at run time (e.g. a message from the server).
pub fn tr_str(english: &str) -> Option<&'static str> {
    CURRENT.read().ok().and_then(|c| c.as_ref().and_then(|(_, t)| t.get(english).copied()))
}

/// A translated text with its `{}` filled in, in order.
pub fn trf(english: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut parts = tr(english).split("{}").peekable();
    while let Some(p) = parts.next() {
        out.push_str(p);
        if parts.peek().is_some() {
            if let Some(a) = args.next() {
                out.push_str(&a.to_string());
            }
        }
    }
    displayed(english, out)
}

/// A font file of the system with the letters of a script (Chinese, Japanese, Korean,
/// Arabic, Devanagari, Bengali, Thai): (bytes, index in a font collection). None for the
/// scripts the built-in fonts already have.
pub fn system_font(script: Script) -> Option<(Vec<u8>, u32)> {
    #[cfg(windows)]
    let candidates: &[(&str, u32)] = match script {
        ChineseSimplified => &[("C:\\Windows\\Fonts\\msyh.ttc", 0), ("C:\\Windows\\Fonts\\simsun.ttc", 0)],
        ChineseTraditional => &[("C:\\Windows\\Fonts\\msjh.ttc", 0), ("C:\\Windows\\Fonts\\mingliub.ttc", 0)],
        Japanese => &[("C:\\Windows\\Fonts\\YuGothM.ttc", 0), ("C:\\Windows\\Fonts\\meiryo.ttc", 0), ("C:\\Windows\\Fonts\\msgothic.ttc", 0)],
        Korean => &[("C:\\Windows\\Fonts\\malgun.ttf", 0), ("C:\\Windows\\Fonts\\gulim.ttc", 0)],
        Arabic => &[("C:\\Windows\\Fonts\\arial.ttf", 0), ("C:\\Windows\\Fonts\\tahoma.ttf", 0)],
        Devanagari | Bengali => &[("C:\\Windows\\Fonts\\Nirmala.ttc", 0), ("C:\\Windows\\Fonts\\Nirmala.ttf", 0)],
        Thai => &[("C:\\Windows\\Fonts\\LeelawUI.ttf", 0), ("C:\\Windows\\Fonts\\tahoma.ttf", 0)],
        _ => &[],
    };
    #[cfg(target_os = "android")]
    let candidates: &[(&str, u32)] = match script {
        // NotoSansCJK collections: Japanese, Korean, Simplified, Traditional Chinese.
        ChineseSimplified => &[("/system/fonts/NotoSansCJK-Regular.ttc", 2), ("/system/fonts/NotoSansSC-Regular.otf", 0)],
        ChineseTraditional => &[("/system/fonts/NotoSansCJK-Regular.ttc", 3), ("/system/fonts/NotoSansTC-Regular.otf", 0)],
        Japanese => &[("/system/fonts/NotoSansCJK-Regular.ttc", 0), ("/system/fonts/NotoSansJP-Regular.otf", 0)],
        Korean => &[("/system/fonts/NotoSansCJK-Regular.ttc", 1), ("/system/fonts/NotoSansKR-Regular.otf", 0)],
        Arabic => &[("/system/fonts/NotoNaskhArabic-Regular.ttf", 0), ("/system/fonts/NotoSansArabic-Regular.ttf", 0), ("/system/fonts/DroidNaskh-Regular.ttf", 0)],
        Devanagari => &[("/system/fonts/NotoSansDevanagari-Regular.otf", 0), ("/system/fonts/NotoSansDevanagari-Regular.ttf", 0), ("/system/fonts/NotoSansDevanagari-VF.ttf", 0)],
        Bengali => &[("/system/fonts/NotoSansBengali-Regular.otf", 0), ("/system/fonts/NotoSansBengali-Regular.ttf", 0), ("/system/fonts/NotoSansBengali-VF.ttf", 0)],
        Thai => &[("/system/fonts/NotoSansThai-Regular.ttf", 0), ("/system/fonts/NotoSansThai-Regular.otf", 0)],
        _ => &[],
    };
    #[cfg(not(any(windows, target_os = "android")))]
    let candidates: &[(&str, u32)] = {
        let _ = script;
        &[]
    };
    candidates.iter().find_map(|(p, i)| std::fs::read(p).ok().map(|b| (b, *i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales() {
        assert_eq!(from_locale("de-AT"), "de");
        assert_eq!(from_locale("pt_BR.UTF-8"), "pt-BR");
        assert_eq!(from_locale("pt-PT"), "pt-PT");
        assert_eq!(from_locale("zh-Hant-TW"), "zh-TW");
        assert_eq!(from_locale("zh-CN"), "zh-CN");
        assert_eq!(from_locale("nn-NO"), "nb");
        assert_eq!(from_locale("xx"), "en");
    }

    #[test]
    fn every_language_has_a_file() {
        for l in LANGS.iter().skip(1) {
            let j = json(l.code).unwrap_or_else(|| panic!("no file for {}", l.code));
            assert!(serde_json::from_str::<HashMap<String, String>>(j).is_ok(), "{} is not valid JSON", l.code);
        }
    }

    #[test]
    fn placeholders_kept() {
        let t = parse(r#"{"Hello {}": "Hallo {}", "Bad {}": "Schlecht", "Same": "Same", "Empty": " "}"#);
        assert_eq!(t.len(), 1);
        set_language("en", None);
        assert_eq!(trf("Hello {} and {}", &[&1, &"x"]), "Hello 1 and x");
        assert_eq!(fill("{b} then {} {a}", &[&1], &[("a", &"A"), ("b", &2)]), "2 then 1 A");
        assert!(same_placeholders("{name} is {}", "{} ist {name}"));
        assert!(!same_placeholders("{name} is", "{nom} est"));
    }
}
