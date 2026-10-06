use std::{fmt::Write, fs, path::Path, process::Command};

const CORE_PROPERTIES: &str = include_str!("fixtures/unicode/DerivedCoreProperties.txt");
const GRAPHEME_PROPERTIES: &str = include_str!("fixtures/unicode/GraphemeBreakProperty.txt");

// UAX #29 rev. 49 GB9c: Linker Extend* × Consonant, without a prior consonant.
fn single_graphemes() -> [&'static str; 8] {
    [
        "\u{94d}\u{915}",
        "\u{1b44}\u{1b13}",
        "\u{94d}\u{301}\u{915}",
        "\u{1b44}\u{301}\u{1b13}",
        "\u{94d}\u{301}\u{200d}\u{915}",
        "\u{1b44}\u{301}\u{200d}\u{1b13}",
        "\u{915}\u{94d}\u{915}",
        "\u{1b13}\u{1b44}\u{1b13}",
    ]
}

fn multiple_graphemes() -> Vec<Vec<&'static str>> {
    vec![
        vec!["\u{94d}", "a", "\u{915}"],
        vec!["\u{1b44}", "a", "\u{1b13}"],
        vec!["\u{94d}\u{200c}", "\u{915}"],
        vec!["\u{1b44}\u{200c}", "\u{1b13}"],
        vec!["\u{94d}\u{903}", "\u{915}"],
        vec!["\u{1b44}\u{903}", "\u{1b13}"],
        vec!["\u{94d}", "\r\n", "\u{915}"],
        vec!["\u{1b44}", "\t", "\u{1b13}"],
        vec!["\u{301}", "\u{915}"],
        vec!["\u{200d}", "\u{1b13}"],
        vec!["\u{94d}\u{915}", "\u{915}"],
        vec!["\u{1b44}\u{1b13}", "\u{1b13}"],
    ]
}

fn fixture_property<'a>(source: &'a str, codepoint: u32, prefix: &str) -> Option<&'a str> {
    source.lines().find_map(|line| {
        let record = line.split('#').next().unwrap();
        let (range, property) = record.split_once(';')?;
        let property = property.trim().strip_prefix(prefix)?.trim();
        let range = range.trim();
        let (start, end) = range.split_once("..").unwrap_or((range, range));
        let start = u32::from_str_radix(start, 16).unwrap();
        let end = u32::from_str_radix(end, 16).unwrap();
        (start <= codepoint && codepoint <= end).then_some(property)
    })
}

#[test]
fn unicode18_official_fixtures_identify_linkers_extends_and_barriers() {
    assert!(CORE_PROPERTIES.starts_with("# DerivedCoreProperties-18.0.0.txt"));
    assert!(GRAPHEME_PROPERTIES.starts_with("# GraphemeBreakProperty-18.0.0.txt"));
    for (codepoint, indic, grapheme) in [
        (0x094d, Some("Linker"), Some("Extend")),
        (0x0915, Some("Consonant"), None),
        (0x1b44, Some("Linker"), Some("Extend")),
        (0x1b13, Some("Consonant"), None),
        (0x0301, Some("Extend"), Some("Extend")),
        (0x200d, Some("Extend"), Some("ZWJ")),
        (0x200c, None, Some("Extend")),
        (0x0903, None, Some("SpacingMark")),
        (0x0061, None, None),
        (0x000d, None, Some("CR")),
        (0x000a, None, Some("LF")),
        (0x0009, None, Some("Control")),
    ] {
        assert_eq!(
            fixture_property(CORE_PROPERTIES, codepoint, "InCB;"),
            indic,
            "InCB U+{codepoint:04X}"
        );
        assert_eq!(
            fixture_property(GRAPHEME_PROPERTIES, codepoint, ""),
            grapheme,
            "GCB U+{codepoint:04X}"
        );
    }
}

fn escaped(text: &str) -> String {
    let mut result = String::new();
    for scalar in text.chars() {
        write!(result, "\\u{{{:x}}}", u32::from(scalar)).unwrap();
    }
    result
}

#[test]
fn linker_consonant_character_literals_accept_without_a_prior_consonant() {
    let path = Path::new("unicode_version.nc");
    for text in single_graphemes() {
        for spelling in [text.to_owned(), escaped(text)] {
            let source =
                format!("// original source\nchar value = '{spelling}'\n@println(value)\n");
            for release in [false, true] {
                ncc::compile_source_with_options(&source, path, release)
                    .unwrap_or_else(|error| panic!("release={release}, {text:?}: {error}"));
            }
        }
    }
}

#[test]
fn multiple_grapheme_character_literals_reject_at_the_original_literal() {
    let path = Path::new("unicode_version_negative.nc");
    for elements in multiple_graphemes() {
        let text = elements.concat();
        for spelling in [text.clone(), escaped(&text)] {
            let literal = format!("'{spelling}'");
            // A multibyte prefix distinguishes source byte offsets from decoded offsets.
            let prefix = "// \u{1b13} original source\nstr before = \"ok\"\nchar value = ";
            let source = format!("{prefix}{literal}\n@println(value)\n");
            for release in [false, true] {
                let error = ncc::compile_source_with_options(&source, path, release).unwrap_err();
                assert_eq!(error.0.len(), 1, "release={release}: {error}");
                let diagnostic = &error.0[0];
                assert_eq!(
                    diagnostic.message, "a char literal must contain one Unicode grapheme cluster",
                    "release={release}, {text:?}: {error}"
                );
                assert_eq!(diagnostic.span, prefix.len()..prefix.len() + literal.len());
                assert_eq!(&source[diagnostic.span.clone()], literal);
                assert!(
                    error
                        .render(&source, path)
                        .contains("unicode_version_negative.nc:3:14"),
                    "release={release}: {}",
                    error.render(&source, path)
                );
            }
        }
    }
}

#[test]
fn runtime_argument_strings_match_literal_length_indexing_and_iteration() {
    let checks = r"
    assert literal.len == expected.len and text.len == expected.len
    assert text == literal
    assert text[0] == expected[0] and text[$] == expected[$]
    char[] elements = @as(char[], text)
    assert elements == expected
    mut uint visits = 0
    mut byte[] flattened = []
    for index in text {
        assert index == visits
        assert text[index] == expected[index] and text[index] == literal[index]
        flattened = flattened <> @as(byte[], text[index])
        visits = visits + 1
    }
    assert visits == expected.len
    assert flattened == @as(byte[], literal)
";
    let mut source = String::new();
    let cases: Vec<_> = single_graphemes()
        .into_iter()
        .map(|text| vec![text])
        .chain(multiple_graphemes())
        .collect();
    let arguments: Vec<_> = cases.iter().map(|elements| elements.concat()).collect();
    for (index, elements) in cases.iter().enumerate() {
        let literal = escaped(&arguments[index]);
        let expected = elements
            .iter()
            .map(|element| format!("'{}'", escaped(element)))
            .collect::<Vec<_>>()
            .join(", ");
        for (kind, input) in [
            ("literal", format!("\"{literal}\"")),
            ("runtime", format!("@args()[{}]", index + 1)),
        ] {
            writeln!(
                source,
                "test \"Unicode 18 {kind} case {index}\" {{\nstr text = {input}\nstr literal = \"{literal}\"\nchar[] expected = [{expected}]\n{checks}\n}}"
            )
            .unwrap();
        }
    }
    run_runtime_cases(&source, &arguments);
}

fn run_runtime_cases(source: &str, arguments: &[String]) {
    let directory = ncc::temp::Directory::new().unwrap();
    let input = directory.path().join("unicode_version.nc");
    fs::write(&input, source).unwrap();
    for (mode, release) in [("-d", false), ("-r", true)] {
        let c = ncc::compile_test_source_with_options(source, &input, release).unwrap();
        // @args supplies raw UTF-8, so release must retain runtime segmentation.
        assert!(
            c.contains("nc_grapheme_next"),
            "{mode}: missing segmentation"
        );
        assert!(
            c.contains("nc_unicode_ranges"),
            "{mode}: missing Unicode tables"
        );
        let output = Command::new(env!("CARGO_BIN_EXE_ncc"))
            .args(["test", mode])
            .arg(&input)
            .arg("--")
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "{mode}: {:?}", output.stdout);
        assert!(
            output.stderr.is_empty(),
            "{mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
