//! Nickname uniqueness (1.26.0+). Each name belongs to one identity on the server, also
//! while its owner is offline, so nobody can take or imitate someone else's name.
//! Names are compared by a "skeleton": case, accents, spacing and separators are ignored
//! and look-alike characters (Cyrillic and Greek letters, I/l/1, 0/O, full-width forms)
//! count as the same letter, so "DyIan", "Dylаn" (Cyrillic а) and "dy_lan" all clash
//! with "Dylan".

use std::collections::HashMap;

fn fold(c: char) -> Option<char> {
    // Full-width ASCII (U+FF01..U+FF5E) to ASCII.
    let c = match c as u32 {
        0xFF01..=0xFF5E => char::from_u32(c as u32 - 0xFF01 + 0x21).unwrap_or(c),
        _ => c,
    };
    // Combining accents are dropped (precomposed letters are mapped below).
    if matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF) {
        return None;
    }
    // Separators and spacing do not make a name different.
    if c.is_whitespace() || matches!(c, '_' | '-' | '.' | '\'' | '`' | '´' | '·' | '•' | '~' | ',' | ':' | ';') {
        return None;
    }
    let mapped = match c {
        // Letters that look like l / I.
        'I' | 'l' | '1' | '|' | '!' | 'ǀ' | 'Ι' | 'І' | 'Ӏ' | 'ı' | 'ℓ' | 'ł' | 'Ł' => 'l',
        '0' | 'O' | 'o' | 'Ο' | 'ο' | 'О' | 'о' | 'Օ' | 'օ' | 'ø' | 'Ø' | 'ö' | 'Ö' | 'ó' | 'Ó' | 'ò' | 'Ò' | 'ô' | 'Ô' | 'õ' | 'Õ' => 'o',
        // Cyrillic and Greek look-alikes.
        'А' | 'а' | 'Α' | 'α' => 'a',
        'В' | 'в' | 'Β' | 'β' => 'b',
        'С' | 'с' | 'Ϲ' | 'ϲ' => 'c',
        'ԁ' => 'd',
        'Е' | 'е' | 'Ε' | 'ε' | 'Ё' | 'ё' => 'e',
        'Н' | 'н' | 'Η' => 'h',
        'і' | 'ї' | 'Ї' | 'ι' => 'i',
        'Ј' | 'ј' => 'j',
        'К' | 'к' | 'Κ' | 'κ' => 'k',
        'М' | 'м' | 'Μ' => 'm',
        'Ν' | 'ν' | 'п' => 'n',
        'Р' | 'р' | 'Ρ' | 'ρ' => 'p',
        'Ѕ' | 'ѕ' => 's',
        'Т' | 'т' | 'Τ' | 'τ' => 't',
        'υ' | 'ս' => 'u',
        'ѵ' | 'Ѵ' => 'v',
        'ԝ' | 'ѡ' | 'ω' => 'w',
        'Х' | 'х' | 'Χ' | 'χ' => 'x',
        'У' | 'у' | 'Υ' | 'ү' | 'γ' => 'y',
        'Ζ' => 'z',
        // Common accented Latin letters.
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'a',
        'ç' | 'Ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'i',
        'ñ' | 'Ñ' => 'n',
        'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'u',
        'ý' | 'ÿ' | 'Ý' => 'y',
        c => c,
    };
    Some(mapped)
}

/// The form two names are compared in: equal skeletons mean "the same name".
pub fn skeleton(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().filter_map(fold) {
        out.extend(c.to_lowercase());
    }
    if out.is_empty() {
        // Only separators: compare as written (lower case) so it still clashes with itself.
        name.trim().to_lowercase()
    } else {
        out
    }
}

/// Which identity holds each name (by skeleton), including people who are offline.
#[derive(Default)]
pub struct NameIndex {
    owner: HashMap<String, String>,
    by_uid: HashMap<String, String>,
}

impl NameIndex {
    /// Built from the saved people. Where old data has two identities with the same
    /// name, the one seen most recently keeps it.
    pub fn build<'a>(people: impl IntoIterator<Item = (&'a str, &'a str, u64)>) -> NameIndex {
        let mut list: Vec<(&str, &str, u64)> = people.into_iter().collect();
        list.sort_by_key(|(_, _, seen)| std::cmp::Reverse(*seen));
        let mut idx = NameIndex::default();
        for (uid, nick, _) in list {
            let sk = skeleton(nick);
            if !nick.trim().is_empty() && !idx.owner.contains_key(&sk) {
                idx.set(uid, nick);
            }
        }
        idx
    }

    /// The identity that owns `nick`, if it is not `uid`.
    pub fn owner_other_than(&self, nick: &str, uid: &str) -> Option<&str> {
        self.owner.get(&skeleton(nick)).map(|s| s.as_str()).filter(|o| *o != uid)
    }

    /// `uid` now uses `nick` (its previous name is released).
    pub fn set(&mut self, uid: &str, nick: &str) {
        self.remove(uid);
        let sk = skeleton(nick);
        self.owner.insert(sk.clone(), uid.to_string());
        self.by_uid.insert(uid.to_string(), sk);
    }

    /// The person was removed from the server: their name is free again.
    pub fn remove(&mut self, uid: &str) {
        if let Some(old) = self.by_uid.remove(uid) {
            if self.owner.get(&old).map(|o| o == uid).unwrap_or(false) {
                self.owner.remove(&old);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_alikes_clash() {
        let d = skeleton("Dylan");
        for n in ["dylan", "DYLAN", "DyIan", "Dy1an", "Dylаn", "Dyl an", "dy_lan", "Dylán", "Ｄｙｌａｎ", " Dylan "] {
            assert_eq!(skeleton(n), d, "{n}");
        }
        assert_ne!(skeleton("Dylan"), skeleton("Dyla"));
        assert_ne!(skeleton("Anna"), skeleton("Hanna"));
        assert_eq!(skeleton("B0b"), skeleton("Bob"));
    }

    #[test]
    fn index_reserves_names_for_offline_people() {
        let mut idx = NameIndex::build([("u1", "Dylan", 10), ("u2", "Sam", 5), ("u3", "dylan", 1)]);
        assert_eq!(idx.owner_other_than("DyIan", "u9"), Some("u1"), "most recently seen keeps the name");
        assert_eq!(idx.owner_other_than("Dylan", "u1"), None, "your own name is not taken");
        idx.set("u1", "Dylan2");
        assert_eq!(idx.owner_other_than("Dylan", "u9"), None, "old name released on rename");
        assert_eq!(idx.owner_other_than("dylan2", "u9"), Some("u1"));
        idx.remove("u2");
        assert_eq!(idx.owner_other_than("Sam", "u9"), None, "removed people free their name");
    }
}
