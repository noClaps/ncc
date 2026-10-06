//! Extended grapheme boundaries (Unicode 18.0.0 / UAX #29 rev. 49), in a forward scan.
use crate::unicode_data::RANGES;

const EXTENDED_PICTOGRAPHIC: u8 = 16;
const INCB_CONSONANT: u8 = 32;
const INCB_LINKER: u8 = 64;
const INCB_EXTEND: u8 = 128;

#[must_use]
pub fn property(c: u32) -> u8 {
    let i = RANGES.partition_point(|(_, hi, _)| *hi < c);
    let flags = RANGES
        .get(i)
        .filter(|(lo, _, _)| *lo <= c)
        .map_or(0, |(_, _, v)| *v);
    if (0xac00..=0xd7a3).contains(&c) {
        flags
            | if (c - 0xac00).is_multiple_of(28) {
                8
            } else {
                9
            }
    } else {
        flags
    }
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
    Linked,
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
                    || (property & INCB_CONSONANT != 0 && self.indic == Indic::Linked)
                    || (property & EXTENDED_PICTOGRAPHIC != 0 && self.emoji == Emoji::AfterZwj)
                    || (previous == 11 && current == 11 && self.regional % 2 == 1))
            }
        };
        self.regional = if current == 11 { self.regional + 1 } else { 0 };
        self.emoji = if property & EXTENDED_PICTOGRAPHIC != 0 {
            Emoji::Pictograph
        } else {
            match current {
                3 if self.emoji == Emoji::Pictograph => Emoji::Pictograph,
                15 if self.emoji == Emoji::Pictograph => Emoji::AfterZwj,
                _ => Emoji::None,
            }
        };
        // GB9c needs a linker followed only by InCB extends, not a prior consonant.
        if property & INCB_LINKER != 0 {
            self.indic = Indic::Linked;
        } else if property & INCB_EXTEND == 0 {
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
    let mut result = String::from("/* Unicode 18.0.0; Unicode, Inc. Unicode License V3. */\n");
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
    use std::{fmt::Write, fs, process::Command};

    struct Case {
        line: usize,
        text: String,
        ends: Vec<usize>,
    }

    // Expected boundaries come directly from the official vectors, not our tables
    // or another implementation of the segmentation algorithm.
    fn official_cases() -> Vec<Case> {
        let source = include_str!("../tests/fixtures/unicode/GraphemeBreakTest.txt");
        assert!(source.starts_with("# GraphemeBreakTest-18.0.0.txt"));
        source
            .lines()
            .enumerate()
            .filter_map(|(line, record)| {
                let record = record.split('#').next().unwrap().trim();
                if record.is_empty() {
                    return None;
                }
                let mut text = String::new();
                let mut ends = Vec::new();
                for token in record.split_whitespace() {
                    match token {
                        "÷" if !text.is_empty() => ends.push(text.len()),
                        "÷" | "×" => (),
                        _ => text
                            .push(char::from_u32(u32::from_str_radix(token, 16).unwrap()).unwrap()),
                    }
                }
                assert_eq!(ends.last(), Some(&text.len()));
                Some(Case {
                    line: line + 1,
                    text,
                    ends,
                })
            })
            .collect()
    }

    #[test]
    fn unicode18_official_rust_boundaries() {
        let cases = official_cases();
        assert!(cases.len() > 500);
        assert_eq!(boundaries(""), [] as [usize; 0]);
        for case in cases {
            let starts: Vec<_> = std::iter::once(0)
                .chain(case.ends.iter().copied().take(case.ends.len() - 1))
                .collect();
            assert_eq!(
                boundaries(&case.text),
                starts,
                "official line {}",
                case.line
            );
        }
    }

    #[test]
    fn unicode18_linkers_and_overlapping_properties() {
        assert_eq!(property(0x94d), 3 | INCB_LINKER);
        assert_eq!(property(0x915), INCB_CONSONANT);
        assert_eq!(property(0x200d), 15 | INCB_EXTEND);
        assert_eq!(property(0x1f469), EXTENDED_PICTOGRAPHIC);
        for text in ["\u{94d}\u{915}", "\u{94d}\u{301}\u{200d}\u{915}"] {
            assert_eq!(boundaries(text), [0], "{text:?}");
        }
        assert_eq!(boundaries("\u{94d}a\u{915}"), [0, 3, 4]);
        assert_eq!(boundaries("\u{94d}\r\u{915}"), [0, 3, 4]);
    }

    fn official_properties() -> Vec<u8> {
        let mut values = vec![0; 0x0011_0000];
        for source in [
            include_str!("../tests/fixtures/unicode/GraphemeBreakProperty.txt"),
            include_str!("../tests/fixtures/unicode/DerivedCoreProperties.txt"),
            include_str!("../tests/fixtures/unicode/emoji-data.txt"),
        ] {
            for line in source.lines() {
                let fields: Vec<_> = line
                    .split('#')
                    .next()
                    .unwrap()
                    .split(';')
                    .map(str::trim)
                    .collect();
                if fields.len() < 2 {
                    continue;
                }
                let value = match fields[1..] {
                    ["CR"] => 1,
                    ["Control"] => 2,
                    ["Extend"] => 3,
                    ["L"] => 6,
                    ["LF"] => 7,
                    ["LV"] => 8,
                    ["LVT"] => 9,
                    ["Prepend"] => 10,
                    ["Regional_Indicator"] => 11,
                    ["SpacingMark"] => 12,
                    ["T"] => 13,
                    ["V"] => 14,
                    ["ZWJ"] => 15,
                    ["Extended_Pictographic"] => 16,
                    ["InCB", "Consonant"] => 32,
                    ["InCB", "Linker"] => 64,
                    ["InCB", "Extend"] => 128,
                    _ => continue,
                };
                let (lo, hi) = fields[0].split_once("..").unwrap_or((fields[0], fields[0]));
                let lo = usize::from_str_radix(lo, 16).unwrap();
                let hi = usize::from_str_radix(hi, 16).unwrap();
                for entry in &mut values[lo..=hi] {
                    *entry |= value;
                }
            }
        }
        values
    }

    #[test]
    fn unicode18_tables_match_all_official_properties() {
        for (c, expected) in (0..=0x0010_ffff).zip(official_properties()) {
            assert_eq!(property(c), expected, "U+{c:04X}");
        }
        for pair in RANGES.windows(2) {
            assert!(pair[0].1 < pair[1].0, "ranges must be ordered and disjoint");
        }
    }

    fn c_case(source: &mut String, case: &Case) {
        source.push_str("{static const char text[]=\"");
        for byte in case.text.bytes() {
            write!(source, "\\x{byte:02x}").unwrap();
        }
        source.push_str("\"; static const size_t ends[]={");
        for end in &case.ends {
            write!(source, "{end},").unwrap();
        }
        writeln!(
            source,
            "}}; if(check(text,sizeof(text)-1,ends,sizeof(ends)/sizeof(*ends),{})) return 1;}}",
            case.line
        )
        .unwrap();
    }

    fn c_property_oracle(source: &mut String) {
        let properties = official_properties();
        source.push_str("static int check_properties(void) {\nstatic const struct {uint32_t lo,hi; unsigned char value;} expected[]={\n");
        let mut lo = 0;
        while lo < properties.len() {
            let mut hi = lo;
            while hi + 1 < properties.len() && properties[hi + 1] == properties[lo] {
                hi += 1;
            }
            writeln!(source, "{{{lo},{hi},{}}},", properties[lo]).unwrap();
            lo = hi + 1;
        }
        source.push_str(
            r#"};
    for(size_t i=0; i<sizeof(expected)/sizeof(*expected); ++i) {
        for(uint32_t c=expected[i].lo; c<=expected[i].hi; ++c) {
            if(nc_property(c)!=expected[i].value) {
                fprintf(stderr,"property mismatch U+%04X\n",(unsigned)c);
                return 1;
            }
        }
    }
    return 0;
}
"#,
        );
    }

    fn c_oracle_source(cases: &[Case]) -> String {
        let mut source = String::from(
            "#include <stdint.h>\n#include <stddef.h>\n#include <stdio.h>\n#include <stdlib.h>\n",
        );
        source.push_str(&c_tables());
        // Compile the actual emitted decoder and scanner, excluding unrelated
        // string-container helpers that need the rest of the compiler runtime.
        source.push_str(
            include_str!("runtime_unicode.h")
                .split("static uint64_t nc_str_len")
                .next()
                .unwrap(),
        );
        c_property_oracle(&mut source);
        source.push_str(
            r#"
static void nc_panic(const char *message) { fputs(message,stderr); exit(2); }
static int check(const char *text, size_t bytes, const size_t *ends, size_t count, size_t line) {
    const char *cursor=text, *end=text+bytes;
    for (size_t i=0; i<count; ++i) {
        cursor=nc_grapheme_next(cursor,end);
        if ((size_t)(cursor-text)!=ends[i]) {
            fprintf(stderr,"official line %zu, cluster %zu: got %zu, expected %zu\n",
                    line,i,(size_t)(cursor-text),ends[i]);
            return 1;
        }
    }
    return cursor!=end;
}
int main(void) {
    if(check_properties()) return 1;
    static const char empty[]="";
    if(nc_grapheme_next(empty,empty)!=empty) return 1;
"#,
        );
        for case in cases {
            c_case(&mut source, case);
        }
        source.push_str("return 0;\n}\n");
        source
    }

    #[test]
    fn unicode18_official_c_boundaries() {
        let directory = crate::temp::Directory::new().unwrap();
        let input = directory.path().join("unicode.c");
        let executable = directory.path().join("unicode");
        fs::write(&input, c_oracle_source(&official_cases())).unwrap();
        for optimization in ["-O0", "-O2"] {
            let output = Command::new("cc")
                .args(["-std=c11", optimization])
                .arg(&input)
                .arg("-o")
                .arg(&executable)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let output = Command::new(&executable).output().unwrap();
            assert!(
                output.status.success(),
                "{optimization}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
