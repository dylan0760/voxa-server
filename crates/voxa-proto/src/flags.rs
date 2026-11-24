//! Small flag icons for the 41 languages in `i18n::LANGS` (Voxa 1.38.1+, Voxa Server
//! 1.30.1+): pure vector shapes in normalized 0..=1 coordinates (3:2 aspect), drawn by
//! Voxa (Windows and Android), Voxa Server Manager, and the web admin page (SVG).

/// A single filled shape in a 3:2 flag (`x`, `y` in `0.0..=1.0`, top-left origin).
#[derive(Clone, Copy, Debug)]
pub enum FlagShape {
    /// Filled rectangle: x, y, width, height, RGB.
    Rect(f32, f32, f32, f32, [u8; 3]),
    /// Filled convex polygon (vertices in `0.0..=1.0`), RGB.
    Poly(&'static [(f32, f32)], [u8; 3]),
    /// Filled circle: center `(cx, cy)` in `0.0..=1.0`, radius as a fraction of flag height, RGB.
    Circle(f32, f32, f32, [u8; 3]),
    /// Filled 5-pointed star: center `(cx, cy)` in `0.0..=1.0`, outer radius as a fraction of
    /// flag height, RGB.
    Star(f32, f32, f32, [u8; 3]),
}

use FlagShape::*;

const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];
const BLACK: [u8; 3] = [0x14, 0x14, 0x18];
const RED: [u8; 3] = [0xD6, 0x26, 0x26];
const DARK_RED: [u8; 3] = [0xA4, 0x1E, 0x2F];
const CRIMSON: [u8; 3] = [0xBC, 0x00, 0x2D];
const BLUE: [u8; 3] = [0x00, 0x52, 0xB4];
const NAVY: [u8; 3] = [0x01, 0x21, 0x69];
const SKY: [u8; 3] = [0x00, 0x72, 0xCE];
const DARK_GREEN: [u8; 3] = [0x00, 0x6A, 0x4E];
const YELLOW: [u8; 3] = [0xFF, 0xD5, 0x00];
const GOLD: [u8; 3] = [0xFF, 0xCC, 0x00];
const SAFFRON: [u8; 3] = [0xFF, 0x99, 0x33];

// Helper geometry for complex flags.
const UK_DIAG_W1: &[(f32, f32)] = &[(0.0, 0.0), (0.14, 0.0), (1.0, 0.86), (1.0, 1.0), (0.86, 1.0), (0.0, 0.14)];
const UK_DIAG_W2: &[(f32, f32)] = &[(0.86, 0.0), (1.0, 0.0), (1.0, 0.14), (0.14, 1.0), (0.0, 1.0), (0.0, 0.86)];
const UK_DIAG_R1: &[(f32, f32)] = &[(0.0, 0.0), (0.07, 0.0), (1.0, 0.93), (1.0, 1.0), (0.93, 1.0), (0.0, 0.07)];
const UK_DIAG_R2: &[(f32, f32)] = &[(0.93, 0.0), (1.0, 0.0), (1.0, 0.07), (0.07, 1.0), (0.0, 1.0), (0.0, 0.93)];

const BR_DIAMOND: &[(f32, f32)] = &[(0.5, 0.13), (0.88, 0.5), (0.5, 0.87), (0.12, 0.5)];
const CZ_TRIANGLE: &[(f32, f32)] = &[(0.0, 0.0), (0.48, 0.5), (0.0, 1.0)];

// South Korea: upper semicircle (convex polygon, 3:2 aspect, r_y = 0.25 -> r_x = 0.1667) + corner bars.
const KR_TOP: &[(f32, f32)] = &[
    (0.333, 0.500),
    (0.346, 0.404),
    (0.382, 0.323),
    (0.436, 0.269),
    (0.500, 0.250),
    (0.564, 0.269),
    (0.618, 0.323),
    (0.654, 0.404),
    (0.667, 0.500),
];
const KR_TL: &[(f32, f32)] = &[(0.14, 0.30), (0.25, 0.13), (0.31, 0.18), (0.20, 0.35)];
const KR_TR: &[(f32, f32)] = &[(0.75, 0.13), (0.86, 0.30), (0.80, 0.35), (0.69, 0.18)];
const KR_BL: &[(f32, f32)] = &[(0.14, 0.70), (0.20, 0.65), (0.31, 0.82), (0.25, 0.87)];
const KR_BR: &[(f32, f32)] = &[(0.69, 0.82), (0.80, 0.65), (0.86, 0.70), (0.75, 0.87)];

const EN: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, NAVY),
    Poly(UK_DIAG_W1, WHITE),
    Poly(UK_DIAG_W2, WHITE),
    Poly(UK_DIAG_R1, [0xC8, 0x10, 0x2E]),
    Poly(UK_DIAG_R2, [0xC8, 0x10, 0x2E]),
    Rect(0.41, 0.0, 0.18, 1.0, WHITE),
    Rect(0.0, 0.37, 1.0, 0.26, WHITE),
    Rect(0.445, 0.0, 0.11, 1.0, [0xC8, 0x10, 0x2E]),
    Rect(0.0, 0.42, 1.0, 0.16, [0xC8, 0x10, 0x2E]),
];

const ZH_CN: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xDE, 0x29, 0x10]),
    Star(0.18, 0.28, 0.16, [0xFF, 0xDE, 0x00]),
    Circle(0.33, 0.13, 0.045, [0xFF, 0xDE, 0x00]),
    Circle(0.38, 0.23, 0.045, [0xFF, 0xDE, 0x00]),
    Circle(0.38, 0.36, 0.045, [0xFF, 0xDE, 0x00]),
    Circle(0.33, 0.46, 0.045, [0xFF, 0xDE, 0x00]),
];

const HI: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, SAFFRON),
    Rect(0.0, 0.33, 1.0, 0.34, WHITE),
    Rect(0.0, 0.66, 1.0, 0.34, [0x13, 0x88, 0x08]),
    Circle(0.5, 0.5, 0.13, NAVY),
    Circle(0.5, 0.5, 0.085, WHITE),
    Circle(0.5, 0.5, 0.035, NAVY),
];

const ES: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.26, [0xAA, 0x15, 0x1B]),
    Rect(0.0, 0.25, 1.0, 0.50, [0xF1, 0xBF, 0x00]),
    Rect(0.0, 0.74, 1.0, 0.26, [0xAA, 0x15, 0x1B]),
    Rect(0.24, 0.36, 0.11, 0.27, [0xAA, 0x15, 0x1B]),
    Rect(0.26, 0.39, 0.07, 0.20, [0xF1, 0xBF, 0x00]),
    Rect(0.275, 0.42, 0.04, 0.14, [0xAA, 0x15, 0x1B]),
];

const FR: &[FlagShape] = &[
    Rect(0.0, 0.0, 0.34, 1.0, [0x00, 0x26, 0x74]),
    Rect(0.33, 0.0, 0.34, 1.0, WHITE),
    Rect(0.66, 0.0, 0.34, 1.0, [0xED, 0x29, 0x39]),
];

const AR: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0x00, 0x6C, 0x35]),
    Rect(0.22, 0.33, 0.56, 0.13, WHITE),
    Rect(0.28, 0.47, 0.44, 0.07, WHITE),
    Rect(0.22, 0.64, 0.56, 0.06, WHITE),
    Rect(0.68, 0.59, 0.05, 0.15, WHITE),
];

const BN: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, DARK_GREEN),
    Circle(0.45, 0.5, 0.29, [0xF4, 0x2A, 0x41]),
];

const PT_BR: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0x00, 0x9C, 0x3B]),
    Poly(BR_DIAMOND, [0xFF, 0xDF, 0x00]),
    Circle(0.5, 0.5, 0.22, [0x00, 0x27, 0x76]),
    Rect(0.37, 0.47, 0.26, 0.06, WHITE),
];

const RU: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, WHITE),
    Rect(0.0, 0.33, 1.0, 0.34, [0x00, 0x39, 0xA6]),
    Rect(0.0, 0.66, 1.0, 0.34, [0xD5, 0x2B, 0x1E]),
];

const UR: &[FlagShape] = &[
    Rect(0.0, 0.0, 0.26, 1.0, WHITE),
    Rect(0.25, 0.0, 0.75, 1.0, [0x01, 0x41, 0x1C]),
    Circle(0.61, 0.50, 0.26, WHITE),
    Circle(0.66, 0.46, 0.21, [0x01, 0x41, 0x1C]),
    Star(0.71, 0.38, 0.10, WHITE),
];

const ID: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.51, [0xCE, 0x11, 0x26]),
    Rect(0.0, 0.50, 1.0, 0.50, WHITE),
];

const DE: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, BLACK),
    Rect(0.0, 0.33, 1.0, 0.34, [0xDD, 0x00, 0x00]),
    Rect(0.0, 0.66, 1.0, 0.34, [0xFF, 0xCE, 0x00]),
];

const JA: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, WHITE),
    Circle(0.5, 0.5, 0.29, CRIMSON),
];

const TR: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xE3, 0x0A, 0x17]),
    Circle(0.41, 0.50, 0.25, WHITE),
    Circle(0.46, 0.50, 0.20, [0xE3, 0x0A, 0x17]),
    Star(0.64, 0.50, 0.13, WHITE),
];

const KO: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, WHITE),
    Poly(KR_TL, BLACK),
    Poly(KR_TR, BLACK),
    Poly(KR_BL, BLACK),
    Poly(KR_BR, BLACK),
    Circle(0.5, 0.5, 0.25, [0x00, 0x47, 0xA0]),
    Poly(KR_TOP, [0xCD, 0x2E, 0x3A]),
    Circle(0.417, 0.5, 0.125, [0xCD, 0x2E, 0x3A]),
    Circle(0.583, 0.5, 0.125, [0x00, 0x47, 0xA0]),
];

const VI: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xDA, 0x25, 0x1D]),
    Star(0.5, 0.5, 0.29, [0xFF, 0xFF, 0x00]),
];

const IT: &[FlagShape] = &[
    Rect(0.0, 0.0, 0.34, 1.0, [0x00, 0x92, 0x46]),
    Rect(0.33, 0.0, 0.34, 1.0, WHITE),
    Rect(0.66, 0.0, 0.34, 1.0, [0xCE, 0x2B, 0x37]),
];

const TH: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.175, [0xA5, 0x19, 0x31]),
    Rect(0.0, 0.167, 1.0, 0.175, WHITE),
    Rect(0.0, 0.333, 1.0, 0.334, [0x2D, 0x2A, 0x4A]),
    Rect(0.0, 0.666, 1.0, 0.175, WHITE),
    Rect(0.0, 0.833, 1.0, 0.167, [0xA5, 0x19, 0x31]),
];

const FA: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0x23, 0x9F, 0x40]),
    Rect(0.0, 0.33, 1.0, 0.34, WHITE),
    Rect(0.0, 0.66, 1.0, 0.34, [0xDA, 0x00, 0x00]),
    Circle(0.5, 0.5, 0.11, [0xDA, 0x00, 0x00]),
    Circle(0.5, 0.47, 0.06, WHITE),
    Rect(0.485, 0.39, 0.03, 0.22, [0xDA, 0x00, 0x00]),
];

const PL: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.51, WHITE),
    Rect(0.0, 0.50, 1.0, 0.50, [0xDC, 0x14, 0x3C]),
];

const NL: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0xAE, 0x1C, 0x28]),
    Rect(0.0, 0.33, 1.0, 0.34, WHITE),
    Rect(0.0, 0.66, 1.0, 0.34, [0x21, 0x46, 0x8B]),
];

const SV: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0x00, 0x6A, 0xA7]),
    Rect(0.28, 0.0, 0.14, 1.0, [0xFE, 0xCC, 0x02]),
    Rect(0.0, 0.40, 1.0, 0.20, [0xFE, 0xCC, 0x02]),
];

const NB: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xBA, 0x0C, 0x2F]),
    Rect(0.25, 0.0, 0.20, 1.0, WHITE),
    Rect(0.0, 0.36, 1.0, 0.28, WHITE),
    Rect(0.29, 0.0, 0.12, 1.0, [0x00, 0x20, 0x5B]),
    Rect(0.0, 0.42, 1.0, 0.16, [0x00, 0x20, 0x5B]),
];

const DA: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xC8, 0x10, 0x2E]),
    Rect(0.28, 0.0, 0.13, 1.0, WHITE),
    Rect(0.0, 0.41, 1.0, 0.18, WHITE),
];

const FI: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, WHITE),
    Rect(0.27, 0.0, 0.16, 1.0, [0x00, 0x35, 0x80]),
    Rect(0.0, 0.38, 1.0, 0.24, [0x00, 0x35, 0x80]),
];

const CS: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.51, WHITE),
    Rect(0.0, 0.50, 1.0, 0.50, [0xD7, 0x14, 0x1A]),
    Poly(CZ_TRIANGLE, [0x11, 0x45, 0x7E]),
];

const SK: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, WHITE),
    Rect(0.0, 0.33, 1.0, 0.34, [0x0B, 0x4E, 0xA2]),
    Rect(0.0, 0.66, 1.0, 0.34, [0xEE, 0x1C, 0x25]),
    Rect(0.20, 0.27, 0.18, 0.44, WHITE),
    Rect(0.22, 0.30, 0.14, 0.38, [0xEE, 0x1C, 0x25]),
    Rect(0.275, 0.35, 0.03, 0.24, WHITE),
    Rect(0.24, 0.42, 0.10, 0.05, WHITE),
];

const HU: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0xCE, 0x29, 0x39]),
    Rect(0.0, 0.33, 1.0, 0.34, WHITE),
    Rect(0.0, 0.66, 1.0, 0.34, [0x47, 0x70, 0x50]),
];

const RO: &[FlagShape] = &[
    Rect(0.0, 0.0, 0.34, 1.0, [0x00, 0x2B, 0x7F]),
    Rect(0.33, 0.0, 0.34, 1.0, [0xFC, 0xD1, 0x16]),
    Rect(0.66, 0.0, 0.34, 1.0, [0xCE, 0x11, 0x26]),
];

const EL: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0x0D, 0x5E, 0xAF]),
    Rect(0.0, 1.0 / 9.0, 1.0, 1.0 / 9.0, WHITE),
    Rect(0.0, 3.0 / 9.0, 1.0, 1.0 / 9.0, WHITE),
    Rect(0.0, 5.0 / 9.0, 1.0, 1.0 / 9.0, WHITE),
    Rect(0.0, 7.0 / 9.0, 1.0, 1.0 / 9.0, WHITE),
    Rect(0.0, 0.0, 0.37, 5.0 / 9.0, [0x0D, 0x5E, 0xAF]),
    Rect(0.145, 0.0, 0.08, 5.0 / 9.0, WHITE),
    Rect(0.0, 2.0 / 9.0, 0.37, 1.0 / 9.0, WHITE),
];

const UK: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.51, [0x00, 0x57, 0xB7]),
    Rect(0.0, 0.50, 1.0, 0.50, YELLOW),
];

const BG: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, WHITE),
    Rect(0.0, 0.33, 1.0, 0.34, [0x00, 0x96, 0x6E]),
    Rect(0.0, 0.66, 1.0, 0.34, RED),
];

const HR: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0xFF, 0x00, 0x00]),
    Rect(0.0, 0.33, 1.0, 0.34, WHITE),
    Rect(0.0, 0.66, 1.0, 0.34, [0x17, 0x17, 0x96]),
    Rect(0.41, 0.30, 0.18, 0.40, WHITE),
    Rect(0.42, 0.32, 0.08, 0.18, [0xFF, 0x00, 0x00]),
    Rect(0.50, 0.50, 0.08, 0.18, [0xFF, 0x00, 0x00]),
];

const SR: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0xC6, 0x36, 0x3C]),
    Rect(0.0, 0.33, 1.0, 0.34, [0x0C, 0x40, 0x76]),
    Rect(0.0, 0.66, 1.0, 0.34, WHITE),
    Rect(0.24, 0.25, 0.15, 0.46, [0xC6, 0x36, 0x3C]),
    Rect(0.27, 0.34, 0.09, 0.26, GOLD),
];

const SL: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, WHITE),
    Rect(0.0, 0.33, 1.0, 0.34, [0x00, 0x5D, 0xA4]),
    Rect(0.0, 0.66, 1.0, 0.34, [0xED, 0x1C, 0x24]),
    Rect(0.18, 0.18, 0.14, 0.30, [0x00, 0x5D, 0xA4]),
    Rect(0.21, 0.30, 0.08, 0.12, WHITE),
];

const LT: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, [0xFD, 0xB9, 0x13]),
    Rect(0.0, 0.33, 1.0, 0.34, [0x00, 0x6A, 0x44]),
    Rect(0.0, 0.66, 1.0, 0.34, [0xC1, 0x27, 0x2D]),
];

const LV: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.41, DARK_RED),
    Rect(0.0, 0.40, 1.0, 0.20, WHITE),
    Rect(0.0, 0.59, 1.0, 0.41, DARK_RED),
];

const ET: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 0.34, SKY),
    Rect(0.0, 0.33, 1.0, 0.34, BLACK),
    Rect(0.0, 0.66, 1.0, 0.34, WHITE),
];

const CA: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xFC, 0xDD, 0x09]),
    Rect(0.0, 1.0 / 9.0, 1.0, 1.0 / 9.0, [0xDA, 0x12, 0x1A]),
    Rect(0.0, 3.0 / 9.0, 1.0, 1.0 / 9.0, [0xDA, 0x12, 0x1A]),
    Rect(0.0, 5.0 / 9.0, 1.0, 1.0 / 9.0, [0xDA, 0x12, 0x1A]),
    Rect(0.0, 7.0 / 9.0, 1.0, 1.0 / 9.0, [0xDA, 0x12, 0x1A]),
];

const PT_PT: &[FlagShape] = &[
    Rect(0.0, 0.0, 0.41, 1.0, [0x00, 0x66, 0x00]),
    Rect(0.40, 0.0, 0.60, 1.0, [0xFF, 0x00, 0x00]),
    Circle(0.40, 0.50, 0.19, GOLD),
    Rect(0.35, 0.39, 0.10, 0.21, [0xFF, 0x00, 0x00]),
    Rect(0.37, 0.43, 0.06, 0.13, WHITE),
];

const ZH_TW: &[FlagShape] = &[
    Rect(0.0, 0.0, 1.0, 1.0, [0xFE, 0x00, 0x00]),
    Rect(0.0, 0.0, 0.50, 0.50, [0x00, 0x00, 0x95]),
    Circle(0.25, 0.25, 0.16, WHITE),
    Circle(0.25, 0.25, 0.11, [0x00, 0x00, 0x95]),
    Circle(0.25, 0.25, 0.09, WHITE),
];

const FALLBACK: &[FlagShape] = &[Rect(0.0, 0.0, 1.0, 1.0, BLUE), Circle(0.5, 0.5, 0.28, WHITE)];

/// The shapes that draw the flag for a language code in `i18n::LANGS`.
pub fn flag(code: &str) -> &'static [FlagShape] {
    match code {
        "en" => EN,
        "zh-CN" => ZH_CN,
        "hi" => HI,
        "es" => ES,
        "fr" => FR,
        "ar" => AR,
        "bn" => BN,
        "pt-BR" => PT_BR,
        "ru" => RU,
        "ur" => UR,
        "id" => ID,
        "de" => DE,
        "ja" => JA,
        "tr" => TR,
        "ko" => KO,
        "vi" => VI,
        "it" => IT,
        "th" => TH,
        "fa" => FA,
        "pl" => PL,
        "nl" => NL,
        "sv" => SV,
        "nb" => NB,
        "da" => DA,
        "fi" => FI,
        "cs" => CS,
        "sk" => SK,
        "hu" => HU,
        "ro" => RO,
        "el" => EL,
        "uk" => UK,
        "bg" => BG,
        "hr" => HR,
        "sr" => SR,
        "sl" => SL,
        "lt" => LT,
        "lv" => LV,
        "et" => ET,
        "ca" => CA,
        "pt-PT" => PT_PT,
        "zh-TW" => ZH_TW,
        _ => FALLBACK,
    }
}

/// The 10 alternating outer/inner vertices of a 5-pointed star in normalized `0..=1` coords,
/// where `r_y` is the outer radius as a fraction of height and `aspect` is `width / height`.
pub fn star_points(cx: f32, cy: f32, r_y: f32, aspect: f32) -> [(f32, f32); 10] {
    let mut pts = [(0.0, 0.0); 10];
    let r_x = r_y / aspect;
    for (i, p) in pts.iter_mut().enumerate() {
        let a = -std::f32::consts::FRAC_PI_2 + (i as f32) * std::f32::consts::PI / 5.0;
        let s = if i % 2 == 0 { 1.0 } else { 0.40 };
        *p = (cx + a.cos() * r_x * s, cy + a.sin() * r_y * s);
    }
    pts
}

/// Compact JSON description of a flag for the web admin page (`/i18n/langs.json`).
pub fn flag_json(code: &str) -> serde_json::Value {
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02x}{g:02x}{b:02x}");
    let r3 = |v: f32| (v * 1000.0).round() / 1000.0;
    let items: Vec<serde_json::Value> = flag(code)
        .iter()
        .map(|s| match *s {
            Rect(x, y, w, h, c) => serde_json::json!(["r", r3(x), r3(y), r3(w), r3(h), hex(c)]),
            Circle(cx, cy, r, c) => serde_json::json!(["c", r3(cx), r3(cy), r3(r), hex(c)]),
            Poly(pts, c) => {
                let flat: Vec<f32> = pts.iter().flat_map(|&(x, y)| [r3(x), r3(y)]).collect();
                serde_json::json!(["p", flat, hex(c)])
            }
            Star(cx, cy, r, c) => {
                let flat: Vec<f32> = star_points(cx, cy, r, 1.5).iter().flat_map(|&(x, y)| [r3(x), r3(y)]).collect();
                serde_json::json!(["p", flat, hex(c)])
            }
        })
        .collect();
    serde_json::Value::from(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_has_its_own_flag() {
        for l in crate::i18n::LANGS {
            let f = flag(l.code);
            assert!(!f.is_empty(), "empty flag for {}", l.code);
            assert_ne!(f.as_ptr(), FALLBACK.as_ptr(), "missing flag for {}", l.code);
        }
    }
}
