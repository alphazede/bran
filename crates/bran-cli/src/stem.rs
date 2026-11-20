//! English Snowball ("Porter2") stemmer for lowercase ASCII words.
//!
//! The brief fielded index applies it to query words and document tokens
//! alike, so `studies`, `study` and `studied` meet at `studi`.

const VOWELS: &[u8] = b"aeiouy";

fn is_vowel(c: u8) -> bool {
    VOWELS.contains(&c)
}

/// Irregular forms the algorithm defines outright.
pub const EXCEPTIONS: [(&str, &str); 18] = [
    ("skis", "ski"),
    ("skies", "sky"),
    ("dying", "die"),
    ("lying", "lie"),
    ("tying", "tie"),
    ("idly", "idl"),
    ("gently", "gentl"),
    ("ugly", "ugli"),
    ("early", "earli"),
    ("only", "onli"),
    ("singly", "singl"),
    ("sky", "sky"),
    ("news", "news"),
    ("howe", "howe"),
    ("atlas", "atlas"),
    ("cosmos", "cosmos"),
    ("bias", "bias"),
    ("andes", "andes"),
];

/// Stem of `word`, which must be lowercase ASCII letters. Other input is
/// returned unchanged.
pub fn stem(word: &str) -> String {
    if word.len() <= 2 || !word.bytes().all(|b| b.is_ascii_lowercase()) {
        return word.to_owned();
    }
    if let Some((_, fixed)) = EXCEPTIONS.iter().find(|(w, _)| *w == word) {
        return (*fixed).to_owned();
    }
    // `Y` marks a consonant y: initial, or after a vowel.
    let mut w: Vec<u8> = word.as_bytes().to_vec();
    for i in 0..w.len() {
        if w[i] == b'y' && (i == 0 || is_vowel(w[i - 1])) {
            w[i] = b'Y';
        }
    }
    let (p1, p2) = regions(&w);
    step_1a(&mut w);
    if matches!(
        &w[..],
        b"inning"
            | b"outing"
            | b"canning"
            | b"herring"
            | b"earring"
            | b"proceed"
            | b"exceed"
            | b"succeed"
    ) {
        return finish(w);
    }
    step_1b(&mut w, p1);
    step_1c(&mut w);
    step_2(&mut w, p1);
    step_3(&mut w, p1, p2);
    step_4(&mut w, p2);
    step_5(&mut w, p1, p2);
    finish(w)
}

/// Verb of an agent-noun stem: a stem ending in `-er` after a consonant
/// (`parser`, `builder`, `scanner`) loses that ending and is stemmed again
/// (`pars`, `build`, `scan`), so it meets the stem of its verb. Other stems are
/// returned unchanged.
pub fn agent_verb(stem_form: &str) -> String {
    let b = stem_form.as_bytes();
    let n = b.len();
    if n >= 6
        && stem_form.ends_with("er")
        && b.iter().all(u8::is_ascii_lowercase)
        && !is_vowel(b[n - 3])
    {
        // A doubled final consonant (`scanner`) drops one copy with the `-er`.
        let base = if b[n - 3] == b[n - 4] {
            &stem_form[..n - 3]
        } else {
            &stem_form[..n - 1]
        };
        return stem(base);
    }
    stem_form.to_owned()
}

fn finish(mut w: Vec<u8>) -> String {
    for c in &mut w {
        if *c == b'Y' {
            *c = b'y';
        }
    }
    String::from_utf8(w).unwrap_or_default()
}

/// Start of the region after the first non-vowel that follows a vowel,
/// searching from `from`.
fn region_after(w: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < w.len() && !is_vowel(w[i]) {
        i += 1;
    }
    while i < w.len() && is_vowel(w[i]) {
        i += 1;
    }
    (i + 1).min(w.len())
}

fn regions(w: &[u8]) -> (usize, usize) {
    let p1 = [&b"gener"[..], b"commun", b"arsen"]
        .iter()
        .find(|prefix| w.starts_with(prefix))
        .map_or_else(|| region_after(w, 0), |prefix| prefix.len());
    (p1, region_after(w, p1))
}

/// Short syllable ending at `end` (exclusive): non-vowel, vowel, non-vowel
/// other than w, x or Y; or vowel then non-vowel at the word start.
fn short_syllable_at(w: &[u8], end: usize) -> bool {
    if end == 2 {
        return is_vowel(w[0]) && !is_vowel(w[1]);
    }
    end >= 3
        && !is_vowel(w[end - 3])
        && is_vowel(w[end - 2])
        && !is_vowel(w[end - 1])
        && !matches!(w[end - 1], b'w' | b'x' | b'Y')
}

/// The longest suffix in `list` that `w` ends with.
fn longest<'a>(w: &[u8], list: &[&'a str]) -> Option<&'a str> {
    list.iter()
        .filter(|s| w.ends_with(s.as_bytes()))
        .max_by_key(|s| s.len())
        .copied()
}

fn replace(w: &mut Vec<u8>, suffix: &str, with: &str) {
    w.truncate(w.len() - suffix.len());
    w.extend_from_slice(with.as_bytes());
}

fn step_1a(w: &mut Vec<u8>) {
    match longest(w, &["sses", "ied", "ies", "s", "us", "ss"]) {
        Some("sses") => replace(w, "sses", "ss"),
        Some(s @ ("ied" | "ies")) => {
            let with = if w.len() - 3 > 1 { "i" } else { "ie" };
            replace(w, s, with);
        }
        Some("s") => {
            if w[..w.len() - 2].iter().any(|&c| is_vowel(c)) {
                w.pop();
            }
        }
        _ => {}
    }
}

fn step_1b(w: &mut Vec<u8>, p1: usize) {
    match longest(w, &["eed", "eedly", "ed", "edly", "ing", "ingly"]) {
        Some(s @ ("eed" | "eedly")) => {
            if w.len() - s.len() >= p1 {
                replace(w, s, "ee");
            }
        }
        Some(s) => {
            let stem_len = w.len() - s.len();
            if !w[..stem_len].iter().any(|&c| is_vowel(c)) {
                return;
            }
            w.truncate(stem_len);
            if w.ends_with(b"at") || w.ends_with(b"bl") || w.ends_with(b"iz") {
                w.push(b'e');
            } else if w.len() >= 2
                && w[w.len() - 1] == w[w.len() - 2]
                && matches!(
                    w[w.len() - 1],
                    b'b' | b'd' | b'f' | b'g' | b'm' | b'n' | b'p' | b'r' | b't'
                )
            {
                w.pop();
            } else if p1 >= w.len() && short_syllable_at(w, w.len()) {
                w.push(b'e');
            }
        }
        None => {}
    }
}

fn step_1c(w: &mut [u8]) {
    let n = w.len();
    if n > 2 && matches!(w[n - 1], b'y' | b'Y') && !is_vowel(w[n - 2]) {
        w[n - 1] = b'i';
    }
}

fn step_2(w: &mut Vec<u8>, p1: usize) {
    const LIST: &[(&str, &str)] = &[
        ("tional", "tion"),
        ("enci", "ence"),
        ("anci", "ance"),
        ("abli", "able"),
        ("entli", "ent"),
        ("izer", "ize"),
        ("ization", "ize"),
        ("ational", "ate"),
        ("ation", "ate"),
        ("ator", "ate"),
        ("alism", "al"),
        ("aliti", "al"),
        ("alli", "al"),
        ("fulness", "ful"),
        ("ousli", "ous"),
        ("ousness", "ous"),
        ("iveness", "ive"),
        ("iviti", "ive"),
        ("biliti", "ble"),
        ("bli", "ble"),
        ("ogi", "og"),
        ("fulli", "ful"),
        ("lessli", "less"),
        ("li", ""),
    ];
    let Some(&(suffix, with)) = LIST
        .iter()
        .filter(|(s, _)| w.ends_with(s.as_bytes()))
        .max_by_key(|(s, _)| s.len())
    else {
        return;
    };
    let start = w.len() - suffix.len();
    if start < p1 {
        return;
    }
    match suffix {
        "ogi" if start == 0 || w[start - 1] != b'l' => {}
        "li" if start == 0 || !b"cdeghkmnrt".contains(&w[start - 1]) => {}
        _ => replace(w, suffix, with),
    }
}

fn step_3(w: &mut Vec<u8>, p1: usize, p2: usize) {
    const LIST: &[(&str, &str)] = &[
        ("tional", "tion"),
        ("ational", "ate"),
        ("alize", "al"),
        ("icate", "ic"),
        ("iciti", "ic"),
        ("ical", "ic"),
        ("ful", ""),
        ("ness", ""),
        ("ative", ""),
    ];
    let Some(&(suffix, with)) = LIST
        .iter()
        .filter(|(s, _)| w.ends_with(s.as_bytes()))
        .max_by_key(|(s, _)| s.len())
    else {
        return;
    };
    let start = w.len() - suffix.len();
    if start >= p1 && (suffix != "ative" || start >= p2) {
        replace(w, suffix, with);
    }
}

fn step_4(w: &mut Vec<u8>, p2: usize) {
    let list = [
        "al", "ance", "ence", "er", "ic", "able", "ible", "ant", "ement", "ment", "ent", "ism",
        "ate", "iti", "ous", "ive", "ize", "ion",
    ];
    let Some(suffix) = longest(w, &list) else {
        return;
    };
    let start = w.len() - suffix.len();
    if start < p2 {
        return;
    }
    if suffix == "ion" && (start == 0 || !matches!(w[start - 1], b's' | b't')) {
        return;
    }
    w.truncate(start);
}

fn step_5(w: &mut Vec<u8>, p1: usize, p2: usize) {
    let n = w.len();
    match w.last() {
        Some(b'e') => {
            let start = n - 1;
            if start >= p2 || (start >= p1 && !short_syllable_at(w, start)) {
                w.pop();
            }
        }
        Some(b'l') => {
            if n > p2 && n >= 2 && w[n - 2] == b'l' {
                w.pop();
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::stem;

    #[test]
    fn agent_nouns_meet_their_verbs() {
        use super::agent_verb;
        let verb_of = |w: &str| agent_verb(&stem(w));
        for (noun, verb) in [
            ("parser", "parse"),
            ("parsers", "parse"),
            ("builder", "build"),
            ("browsers", "browse"),
            ("scanner", "scan"),
        ] {
            assert_eq!(verb_of(noun), verb_of(verb), "{noun}");
        }
        // Short words, vowel-led endings and non-agent forms keep their stem.
        for word in ["water", "player", "engineer", "parse", "tier"] {
            assert_eq!(verb_of(word), stem(word), "{word}");
        }
    }

    #[test]
    fn reference_vocabulary() {
        for (word, expected) in [
            ("consign", "consign"),
            ("consigned", "consign"),
            ("consigning", "consign"),
            ("consignment", "consign"),
            ("consist", "consist"),
            ("consisted", "consist"),
            ("consistency", "consist"),
            ("consistent", "consist"),
            ("consistently", "consist"),
            ("consisting", "consist"),
            ("consists", "consist"),
            ("consolation", "consol"),
            ("consolations", "consol"),
            ("consolatory", "consolatori"),
            ("console", "consol"),
            ("consoled", "consol"),
            ("consoles", "consol"),
            ("consolidate", "consolid"),
            ("consolidated", "consolid"),
            ("consolidating", "consolid"),
            ("consoling", "consol"),
            ("consolingly", "consol"),
            ("generate", "generat"),
            ("generously", "generous"),
            ("studies", "studi"),
            ("study", "studi"),
            ("indexed", "index"),
            ("index", "index"),
            ("hoping", "hope"),
            ("hopping", "hop"),
            ("ties", "tie"),
            ("cries", "cri"),
            ("gas", "gas"),
            ("gaps", "gap"),
            ("kiwis", "kiwi"),
            ("by", "by"),
            ("say", "say"),
            ("cry", "cri"),
            ("dying", "die"),
            ("installed", "instal"),
            ("stalled", "stall"),
            ("frequency", "frequenc"),
            ("schedules", "schedul"),
            ("policies", "polici"),
            ("policy", "polici"),
        ] {
            assert_eq!(stem(word), expected, "{word}");
        }
    }
}
