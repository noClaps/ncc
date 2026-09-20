//! Extended grapheme boundaries (Unicode 16 / UAX #29), in a forward scan.
use crate::unicode_data::RANGES;
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
#[derive(Default)]
struct State {
    previous: u8,
    started: bool,
    regional: usize,
    emoji: bool,
    zwj: bool,
    consonant: bool,
    linker: bool,
}
impl State {
    fn push(&mut self, c: u32) -> bool {
        let property = property(c);
        let current = property & 15;
        let previous = self.previous;
        let boundary = !self.started
            || if previous == 1 && current == 7 {
                false
            } else if matches!(previous, 1 | 2 | 7) || matches!(current, 1 | 2 | 7) {
                true
            } else {
                !((previous == 6 && matches!(current, 6 | 8 | 9 | 14))
                    || (matches!(previous, 8 | 14) && matches!(current, 13 | 14))
                    || (matches!(previous, 9 | 13) && current == 13)
                    || matches!(current, 3 | 12 | 15)
                    || previous == 10
                    || (current == 5 && self.consonant && self.linker)
                    || (previous == 15 && current == 4 && self.zwj)
                    || (previous == 11 && current == 11 && self.regional % 2 == 1))
            };
        self.regional = if current == 11 { self.regional + 1 } else { 0 };
        self.zwj = current == 15 && self.emoji;
        self.emoji = current == 4 || (current == 3 && self.emoji);
        if current == 5 {
            self.consonant = true;
            self.linker = false;
        } else if matches!(c, 0x94d | 0x9cd | 0xacd | 0xb4d | 0xc4d | 0xd4d) {
            self.linker = true;
        } else if property & 16 == 0 {
            self.consonant = false;
            self.linker = false;
        }
        self.previous = current;
        self.started = true;
        boundary
    }
}
pub fn boundaries(text: &str) -> Vec<usize> {
    let mut state = State::default();
    text.char_indices()
        .filter_map(|(i, c)| state.push(c as u32).then_some(i))
        .collect()
}
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
            "가",
        ];
        for &(lo, hi, _) in RANGES {
            for c in [lo, hi] {
                let c = char::from_u32(c).unwrap();
                for prefix in contexts {
                    let text = format!("{prefix}{c}a{c}\u{301}👩\u{200d}👩");
                    let expected: Vec<_> = text.grapheme_indices(true).map(|(i, _)| i).collect();
                    assert_eq!(boundaries(&text), expected, "{text:?}");
                }
            }
        }
    }
}
