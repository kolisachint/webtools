//! Decoding response bodies that are not UTF-8.
//!
//! Most of the web is UTF-8, and `String::from_utf8_lossy` handles it. The
//! common exception by a wide margin is the single-byte Western European
//! family — `windows-1252`, and `ISO-8859-1`/`latin1`, which browsers treat as
//! windows-1252 anyway. Those are a 128-entry table, decoded here directly.
//!
//! Multi-byte legacy encodings (Shift_JIS, GBK, Big5, EUC-KR) need real tables,
//! and pulling in a full encoding library would add roughly a megabyte to a
//! binary whose whole pitch is being small. Those are decoded lossily and the
//! declared charset is reported on the result instead, so a caller can see why
//! the text looks wrong rather than guessing.

/// Upper half of windows-1252 (0x80-0x9F); 0xA0-0xFF matches Latin-1, which
/// matches the Unicode code points of the same value.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
    '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
];

/// What decoding a body needs, given its declared charset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Charset {
    /// UTF-8 (or nothing declared): decode lossily as UTF-8.
    Utf8,
    /// windows-1252 / ISO-8859-1: decoded here, exactly.
    Cp1252,
    /// Something else. Decoded as UTF-8, which will mangle it; the label is
    /// carried on the result so the caller knows.
    Unsupported(String),
}

/// Classify a charset label.
pub fn classify(label: &str) -> Charset {
    let normalized = label
        .trim()
        .trim_matches('"')
        .to_ascii_lowercase()
        .replace('_', "-");
    match normalized.as_str() {
        "" | "utf-8" | "utf8" | "us-ascii" | "ascii" => Charset::Utf8,
        // Browsers decode every one of these as windows-1252, which is a strict
        // superset of ISO-8859-1 over the bytes that matter.
        "windows-1252" | "cp1252" | "iso-8859-1" | "latin1" | "latin-1" | "iso8859-1"
        | "iso-latin-1" | "ansi-x3.4-1968" => Charset::Cp1252,
        _ => Charset::Unsupported(label.trim().trim_matches('"').to_string()),
    }
}

/// Decode a body according to its declared charset.
///
/// Returns the text, plus the charset label to report when the result may be
/// garbled (`None` when the decoding is exact).
pub fn decode(bytes: &[u8], label: Option<&str>) -> (String, Option<String>) {
    match label.map(classify).unwrap_or(Charset::Utf8) {
        Charset::Utf8 => (String::from_utf8_lossy(bytes).into_owned(), None),
        Charset::Cp1252 => (decode_cp1252(bytes), None),
        Charset::Unsupported(name) => (String::from_utf8_lossy(bytes).into_owned(), Some(name)),
    }
}

/// Decode windows-1252. Every byte maps to exactly one character, so this
/// cannot fail.
pub fn decode_cp1252(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            0x00..=0x7F => out.push(b as char),
            0x80..=0x9F => out.push(CP1252_HIGH[(b - 0x80) as usize]),
            _ => out.push(b as char), // 0xA0-0xFF: Latin-1 == Unicode
        }
    }
    out
}

/// Pull a `charset=` value out of a `Content-Type` header.
pub fn from_content_type(header: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let part = part.trim();
        let rest = part.strip_prefix("charset=").or_else(|| {
            part.to_ascii_lowercase()
                .starts_with("charset=")
                .then(|| &part["charset=".len()..])
        })?;
        let value = rest.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_labels_need_no_special_handling() {
        assert_eq!(classify("utf-8"), Charset::Utf8);
        assert_eq!(classify("UTF-8"), Charset::Utf8);
        assert_eq!(classify(" \"utf8\" "), Charset::Utf8);
        assert_eq!(classify("us-ascii"), Charset::Utf8);
    }

    #[test]
    fn latin_labels_all_decode_as_cp1252() {
        for label in [
            "ISO-8859-1",
            "iso_8859-1",
            "latin1",
            "windows-1252",
            "CP1252",
        ] {
            assert_eq!(classify(label), Charset::Cp1252, "{label}");
        }
    }

    #[test]
    fn a_latin1_body_decodes_correctly() {
        // "Café — naïve" in windows-1252: é=0xE9, em dash=0x97, ï=0xEF.
        let bytes = b"Caf\xe9 \x97 na\xefve";
        let (text, reported) = decode(bytes, Some("ISO-8859-1"));
        assert_eq!(text, "Café — naïve");
        assert_eq!(reported, None, "an exact decode reports no problem");
    }

    #[test]
    fn cp1252_smart_quotes_survive() {
        let bytes = b"\x93quoted\x94 \x85";
        assert_eq!(decode_cp1252(bytes), "“quoted” …");
    }

    #[test]
    fn an_unsupported_charset_is_reported_rather_than_hidden() {
        let (_, reported) = decode(b"\x82\xa0", Some("Shift_JIS"));
        assert_eq!(reported.as_deref(), Some("Shift_JIS"));
    }

    #[test]
    fn utf8_bodies_round_trip() {
        let (text, reported) = decode("Café — naïve".as_bytes(), Some("utf-8"));
        assert_eq!(text, "Café — naïve");
        assert!(reported.is_none());
    }

    #[test]
    fn charset_is_read_out_of_a_content_type() {
        assert_eq!(
            from_content_type("text/html; charset=ISO-8859-1").as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            from_content_type("text/html;charset=\"utf-8\"").as_deref(),
            Some("utf-8")
        );
        assert_eq!(from_content_type("text/html").as_deref(), None);
    }
}
