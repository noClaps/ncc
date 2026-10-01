//! Extended grapheme boundaries (Unicode 16 / UAX #29), in a forward scan.
use crate::unicode_data::RANGES;
#[must_use]
pub fn property(c: u32) -> u8 {
    if (0xac00..=0xd7a3).contains(&c) {
        return if (c - 0xac00).is_multiple_of(28) {
            8
        } else {
            9
        };
    }
    let i = RANGES.partition_point(|(_, hi, _)| *hi < c);
    RANGES
        .get(i)
        .filter(|(lo, _, _)| *lo <= c)
        .map_or(0, |(_, _, v)| *v)
}
#[derive(Default, PartialEq, Eq)]
enum Emoji {
    #[default]
    None,
    Pictograph,
    AfterZwj,
}
#[derive(Default, PartialEq, Eq)]
enum Indic {
    #[default]
    None,
    Consonant,
    LinkedConsonant,
}
// UAX #29 rules require independent, overlapping state for these properties.
#[derive(Default)]
struct State {
    previous: Option<u8>,
    regional: usize,
    emoji: Emoji,
    indic: Indic,
}
impl State {
    fn push(&mut self, c: u32) -> bool {
        let property = property(c);
        let current = property & 15;
        let boundary = match self.previous {
            None => true,
            Some(1) if current == 7 => false,
            Some(previous) if matches!(previous, 1 | 2 | 7) || matches!(current, 1 | 2 | 7) => true,
            Some(previous) => {
                !((previous == 6 && matches!(current, 6 | 8 | 9 | 14))
                    || (matches!(previous, 8 | 14) && matches!(current, 13 | 14))
                    || (matches!(previous, 9 | 13) && current == 13)
                    || matches!(current, 3 | 12 | 15)
                    || previous == 10
                    || (current == 5 && self.indic == Indic::LinkedConsonant)
                    || (previous == 15 && current == 4 && self.emoji == Emoji::AfterZwj)
                    || (previous == 11 && current == 11 && self.regional % 2 == 1))
            }
        };
        self.regional = if current == 11 { self.regional + 1 } else { 0 };
        self.emoji = match current {
            4 => Emoji::Pictograph,
            3 if self.emoji == Emoji::Pictograph => Emoji::Pictograph,
            15 if self.emoji == Emoji::Pictograph => Emoji::AfterZwj,
            _ => Emoji::None,
        };
        if current == 5 {
            self.indic = Indic::Consonant;
        } else if matches!(c, 0x94d | 0x9cd | 0xacd | 0xb4d | 0xc4d | 0xd4d) {
            if self.indic != Indic::None {
                self.indic = Indic::LinkedConsonant;
            }
        } else if property & 16 == 0 {
            self.indic = Indic::None;
        }
        self.previous = Some(current);
        boundary
    }
}
#[must_use]
pub fn boundaries(text: &str) -> Vec<usize> {
    let mut state = State::default();
    text.char_indices()
        .filter_map(|(i, c)| state.push(c as u32).then_some(i))
        .collect()
}
#[must_use]
pub fn c_tables() -> String {
    use std::fmt::Write;
    let mut result = String::from("/* Unicode 16.0.0; Rust Project Developers, MIT. */\n");
    result.push_str("/*\n");
    result.push_str(include_str!("../UNICODE-LICENSE"));
    result.push_str("\n*/\n");
    result.push_str("static const struct { uint32_t lo, hi; unsigned char property; } nc_unicode_ranges[] = {\n");
    for (lo, hi, value) in RANGES {
        writeln!(result, "{{{lo},{hi},{value}}},").unwrap();
    }
    result.push_str("};\n");
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;
    fn assert_reference_boundaries(text: &str) {
        let expected: Vec<_> = text.grapheme_indices(true).map(|(i, _)| i).collect();
        assert_eq!(boundaries(text), expected, "{text:?}");
    }

    #[test]
    fn emoji_state_transitions_match_reference() {
        let contexts = [
            "",
            "👩",
            "👩\u{301}\u{308}",
            "👩\u{200d}",
            "👩\u{200d}\u{200d}",
            "👩\u{200d}\u{301}",
            "👩\u{200d}👩",
            "👩a",
            "👩\r\n",
            "👩\u{301}a\u{301}",
            "\u{200d}",
        ];
        for prefix in contexts {
            for suffix in ["👩", "\u{301}👩", "\u{200d}👩", "\u{200d}\u{200d}👩"] {
                assert_reference_boundaries(&format!("{prefix}{suffix}"));
            }
        }
    }

    #[test]
    fn indic_state_transitions_match_reference() {
        for linker in [
            '\u{94d}', '\u{9cd}', '\u{acd}', '\u{b4d}', '\u{c4d}', '\u{d4d}',
        ] {
            for text in [
                format!("{linker}क"),
                format!("{linker}\u{301}{linker}क"),
                format!("क\u{301}क{linker}क"),
                format!("क{linker}कक"),
                format!("क{linker}क{linker}क"),
                format!("क{linker}{linker}क"),
                format!("क\u{301}{linker}\u{301}क"),
                format!("क{linker}\u{200d}क"),
                format!("क{linker}\u{200d}\u{200d}क"),
                format!("क{linker}a\u{301}{linker}क"),
                format!("क{linker}\r\n{linker}क"),
                format!("क{linker}👩\u{200d}👩क"),
            ] {
                assert_reference_boundaries(&text);
            }
        }
    }

    #[test]
    fn regional_indicator_transitions_match_reference() {
        for count in 0..=7 {
            let indicators = "🇮".repeat(count);
            for separator in ["", "a", "\r\n", "\u{301}", "\u{200d}", "👩\u{200d}"] {
                assert_reference_boundaries(&format!("{indicators}{separator}🇮🇳🇮🇳"));
            }
        }
    }

    // Unicode escapes preserve decomposed text when testing grapheme boundaries.
    #[test]
    fn matches_reference_boundaries() {
        let contexts = [
            "",
            "a",
            "\r",
            "\u{600}",
            "🇮",
            "👩\u{301}\u{200d}",
            "क्",
            "\u{1100}\u{1161}",
        ];
        for &(lo, hi, _) in RANGES {
            for c in [lo, hi] {
                let c = char::from_u32(c).unwrap();
                for prefix in contexts {
                    let text = format!("{prefix}{c}a{c}\u{301}👩\u{200d}👩");
                    assert_reference_boundaries(&text);
                }
            }
        }
    }
}
