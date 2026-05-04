pub fn clean(raw: &str) -> String {
    let text = raw.trim();
    if text.is_empty() {
        return String::new();
    }

    let (wake_word, rest) = split_wake_word(text);
    let corrected = apply_self_corrections(rest);
    let cleaned = remove_fillers(&corrected);
    let cleaned = normalize_screen_phrase(&cleaned);

    let assembled = match wake_word {
        Some(w) => {
            let rest = cleaned.trim();
            if rest.is_empty() {
                w.to_string()
            } else {
                format!("{}, {}", w, rest)
            }
        }
        None => cleaned,
    };

    finalize(&assembled)
}

// ── Wake word ──────────────────────────────────────────────────────────

fn split_wake_word(s: &str) -> (Option<&'static str>, &str) {
    let lower = s.to_lowercase();
    // Longest variants first to avoid short prefix shadowing
    let variants: &[&str] = &[
        "hey, anna", "hej, anna", "hey anna", "hej anna",
        "hey, anne", "hej, anne", "hey anne", "hej anne",
    ];
    for variant in variants {
        if lower.starts_with(variant) {
            let rest = s[variant.len()..].trim_start_matches([',', '.', ' ']);
            return (Some("Hey Anna"), rest);
        }
    }
    (None, s)
}

// ── Self-corrections ───────────────────────────────────────────────────
//
// When the speaker catches a mistake they say a trigger phrase like "hov nej"
// and then restart. We discard everything up to and including the trigger,
// then repeat until no more triggers remain (handles cascading corrections).

fn apply_self_corrections(s: &str) -> String {
    const TRIGGERS: &[&str] = &["hov nej", "nej vent", "jeg mener", "nej faktisk", "bare glem det"];

    let mut result = s.to_string();
    loop {
        let lower = result.to_lowercase();
        let found = TRIGGERS
            .iter()
            .filter_map(|t| lower.find(t).map(|pos| (pos, pos + t.len())))
            .min_by_key(|(pos, _)| *pos);

        match found {
            Some((_, end)) => {
                result = result[end..].trim_start_matches([',', '.', ' ']).to_string();
            }
            None => break,
        }
    }
    result
}

// ── Filler words ───────────────────────────────────────────────────────

fn remove_fillers(s: &str) -> String {
    const FILLERS: &[&str] = &["øh", "øhh", "øhhh", "øhm", "øhmm", "hmm", "hm", "mmh"];

    s.split_whitespace()
        .filter(|w| {
            let stripped = w.trim_matches(|c: char| !c.is_alphabetic());
            let lower = stripped.to_lowercase();
            !FILLERS.contains(&lower.as_str())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Screen-look phrase normalization ───────────────────────────────────
//
// Phrases meaning "look at the screen" are normalized to "se på skærmen"
// so downstream `needs_screenshot()` detection is reliable regardless of
// which phrasing Whisper transcribed.

fn normalize_screen_phrase(s: &str) -> String {
    // Longer variants first to prevent partial shadowing
    const VARIANTS: &[&str] = &[
        "kig på min skærm",
        "se på min skærm",
        "vis min skærm",
        "kig på skærmen",
        "kig på skærm",
        "vis skærmen",
        "se på skærm",
    ];

    let lower = s.to_lowercase();
    for variant in VARIANTS {
        if let Some(pos) = lower.find(variant) {
            let end = pos + variant.len();
            let before = s[..pos].trim_end();
            let after = s[end..].trim_start_matches([',', '.', ' ']);
            return match (before.is_empty(), after.is_empty()) {
                (true, true) => "se på skærmen".to_string(),
                (true, false) => format!("se på skærmen {}", after),
                (false, true) => format!("{} se på skærmen", before),
                (false, false) => format!("{} se på skærmen {}", before, after),
            };
        }
    }
    s.to_string()
}

// ── Capitalization + terminal punctuation ──────────────────────────────

fn finalize(s: &str) -> String {
    let s = s.trim();
    if s.is_empty() {
        return String::new();
    }
    let mut chars = s.chars();
    let first = chars.next().unwrap();
    let capitalized = format!("{}{}", first.to_uppercase(), chars.as_str());

    if capitalized.ends_with(['.', '!', '?', '…']) {
        capitalized
    } else {
        format!("{}.", capitalized)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_example() {
        let input = "hej anne øhh kig lige på skærmen hov nej jeg mener brug skærmen og udfyld øh feltet med kundenavn Petersen";
        assert_eq!(
            clean(input),
            "Hey Anna, brug skærmen og udfyld feltet med kundenavn Petersen."
        );
    }

    #[test]
    fn test_filler_removal() {
        assert_eq!(clean("øh hej"), "Hej.");
        assert_eq!(clean("skriv øhm noget"), "Skriv noget.");
        assert_eq!(clean("hmm lad mig tænke"), "Lad mig tænke.");
        assert_eq!(clean("øhhh hvad skal jeg gøre"), "Hvad skal jeg gøre.");
    }

    #[test]
    fn test_wake_word_normalization() {
        assert_eq!(clean("hej anna hvad er klokken"), "Hey Anna, hvad er klokken.");
        assert_eq!(clean("hey anne brug skærmen"), "Hey Anna, brug skærmen.");
        assert_eq!(clean("hej, anna hvad sker der"), "Hey Anna, hvad sker der.");
        assert_eq!(clean("hey, anna"), "Hey Anna.");
    }

    #[test]
    fn test_self_correction_single() {
        assert_eq!(clean("skriv rapport hov nej skriv mail"), "Skriv mail.");
        assert_eq!(clean("åbn filen nej vent luk vinduet"), "Luk vinduet.");
    }

    #[test]
    fn test_self_correction_cascading() {
        // Multiple corrections: only the last segment survives
        assert_eq!(
            clean("skriv rapport hov nej skriv mail jeg mener send besked"),
            "Send besked."
        );
    }

    #[test]
    fn test_screen_phrase_normalization() {
        assert_eq!(clean("kig på skærmen og beskriv"), "Se på skærmen og beskriv.");
        assert_eq!(clean("vis skærmen"), "Se på skærmen.");
        assert_eq!(clean("kig på min skærm og forklar"), "Se på skærmen og forklar.");
    }

    #[test]
    fn test_capitalization_and_punctuation() {
        assert_eq!(clean("skriv rapport"), "Skriv rapport.");
        assert_eq!(clean("hvad er klokken?"), "Hvad er klokken?");
        assert_eq!(clean("yes!"), "Yes!");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(clean(""), "");
        assert_eq!(clean("   "), "");
    }

    #[test]
    fn test_filler_only_input() {
        // After removing all fillers nothing remains — return empty string
        assert_eq!(clean("øh øhm hmm"), "");
    }
}
