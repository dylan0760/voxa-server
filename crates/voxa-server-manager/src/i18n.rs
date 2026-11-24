//! Voxa Server Manager in other languages (1.30.0+): `t!("English text")` is the text in
//! the chosen language (🌐 in the header, else Windows' language). The translations are
//! Voxa's (voxa-proto `i18n/<code>.json`).

use std::sync::Arc;

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

/// The language to use: the one chosen in the Manager, or Windows'.
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

/// Right-to-left text (Arabic, Persian, Urdu) prepared for egui's left-to-right run layout:
/// `unicode-bidi` puts runs in visual left-to-right order, and within each RTL run words
/// are ordered right-to-left while keeping each Arabic word's characters in logical order
/// so egui's HarfBuzz shaper (`harfrust`) joins letters and renders each word RTL.
pub fn shape(s: &str) -> String {
    s.split('\n').map(shape_line).collect::<Vec<_>>().join("\n")
}

fn is_arabic_script(c: char) -> bool {
    matches!(c, '\u{0600}'..='\u{06FF}' | '\u{0750}'..='\u{077F}' | '\u{08A0}'..='\u{08FF}')
}

fn mirror_bracket(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '<' => '>',
        '>' => '<',
        _ => c,
    }
}

fn reverse_rtl_chunks(run: &str) -> String {
    let mut chunks: Vec<String> = Vec::new();
    let mut chars = run.chars().peekable();
    while let Some(c) = chars.next() {
        if is_arabic_script(c) {
            let mut word = String::new();
            word.push(c);
            while let Some(&n) = chars.peek() {
                if is_arabic_script(n) {
                    word.push(chars.next().unwrap());
                } else {
                    break;
                }
            }
            chunks.push(word);
        } else {
            let mut item = String::new();
            item.push(mirror_bracket(c));
            while let Some(&n) = chars.peek() {
                if matches!(n, '\u{FE00}'..='\u{FE0F}' | '\u{200D}' | '\u{0300}'..='\u{036F}') {
                    item.push(chars.next().unwrap());
                    if n == '\u{200D}' {
                        if let Some(next) = chars.next() {
                            item.push(next);
                        }
                    }
                } else {
                    break;
                }
            }
            chunks.push(item);
        }
    }
    chunks.reverse();
    chunks.concat()
}

fn shape_line(line: &str) -> String {
    let info = unicode_bidi::BidiInfo::new(line, Some(unicode_bidi::Level::rtl()));
    let Some(para) = info.paragraphs.first() else {
        return line.to_string();
    };
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut out = String::with_capacity(line.len());
    for run in runs {
        let slice = &line[run.clone()];
        if levels[run.start].is_rtl() {
            out.push_str(&reverse_rtl_chunks(slice));
        } else {
            out.push_str(slice);
        }
    }
    out
}

/// On-demand system font provider for egui so the Language list shows every script cleanly.
pub fn script_font_provider(families: Vec<egui::FontFamily>) -> Arc<dyn egui::FontProvider> {
    use egui::epaint::text::{FallbackRequest, FontInsert, FontPriority, InsertFontFamily};
    Arc::new(move |req: &FallbackRequest<'_>| {
        let c = req.cluster.chars().next()?;
        let (name, bytes, index) = voxa_proto::i18n::fallback_font_for_char(c)?;
        let fams = families.iter().cloned().map(|family| InsertFontFamily { family, priority: FontPriority::Lowest }).collect();
        Some(FontInsert::new(name, egui::FontData::from_blob(bytes, index), fams))
    })
}

/// Paints a small 3:2 flag icon for a language code inside `rect`.
pub fn paint_flag(painter: &egui::Painter, rect: egui::Rect, code: &str) {
    use egui::{Color32, CornerRadius, Pos2, Rect, Shape, Stroke};
    use voxa_proto::flags::{flag, star_points, FlagShape};

    let p = painter.with_clip_rect(rect);
    let pt = |x: f32, y: f32| Pos2::new(rect.left() + x * rect.width(), rect.top() + y * rect.height());
    let col = |[r, g, b]: [u8; 3]| Color32::from_rgb(r, g, b);
    let aspect = if rect.height() > 0.0 { rect.width() / rect.height() } else { 1.5 };

    for shape in flag(code) {
        match *shape {
            FlagShape::Rect(x, y, w, h, rgb) => {
                p.rect_filled(Rect::from_min_max(pt(x, y), pt(x + w, y + h)), CornerRadius::ZERO, col(rgb));
            }
            FlagShape::Poly(pts, rgb) => {
                let mapped: Vec<Pos2> = pts.iter().map(|&(x, y)| pt(x, y)).collect();
                p.add(Shape::convex_polygon(mapped, col(rgb), Stroke::NONE));
            }
            FlagShape::Circle(cx, cy, r, rgb) => {
                p.circle_filled(pt(cx, cy), r * rect.height(), col(rgb));
            }
            FlagShape::Star(cx, cy, r, rgb) => {
                let c = col(rgb);
                let center = pt(cx, cy);
                let v = star_points(cx, cy, r, aspect);
                for k in 0..5 {
                    let kite = vec![center, pt(v[(2 * k + 9) % 10].0, v[(2 * k + 9) % 10].1), pt(v[2 * k].0, v[2 * k].1), pt(v[2 * k + 1].0, v[2 * k + 1].1)];
                    p.add(Shape::convex_polygon(kite, c, Stroke::NONE));
                }
            }
        }
    }
    painter.rect_stroke(rect, CornerRadius::same(1), Stroke::new(1.0, Color32::from_rgba_unmultiplied(128, 128, 128, 110)), egui::StrokeKind::Inside);
}

/// An egui `Atom` that draws the flag icon for `code`.
pub fn flag_atom(code: &'static str) -> egui::Atom<'static> {
    egui::Atom::paint(egui::vec2(21.0, 14.0), move |ui, args| {
        paint_flag(ui.painter(), args.rect, code);
    })
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

#[cfg(not(windows))]
pub fn system_locale() -> String {
    std::env::var("LANG").unwrap_or_else(|_| "en".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtl_reorders_words_and_keeps_harfbuzz_logical_words() {
        assert_eq!(shape("العربية"), "العربية");
        assert_eq!(shape("إظهار الطوابع الزمنية"), "الزمنية الطوابع إظهار");
        assert_eq!(shape("مرحباً بك في Voxa Server"), "Voxa Server في بك مرحباً");
        assert_eq!(shape("Voxa 1.38"), "Voxa 1.38");
    }
}
