//! Decoding response bodies that are not UTF-8.
//!
//! Most of the web is UTF-8 and takes the fast path here. The rest is decoded
//! through `encoding_rs`, the same implementation Firefox uses, which covers
//! the whole WHATWG Encoding Standard: the single-byte Western family
//! (`windows-1252`, `ISO-8859-*`), the CJK multi-byte encodings (Shift_JIS,
//! GBK, GB18030, Big5, EUC-KR, EUC-JP, ISO-2022-JP), KOI8, and UTF-16.
//!
//! An earlier version hand-rolled a windows-1252 table and reported everything
//! else as undecodable, on the grounds that conversion tables would bloat a
//! binary that advertises being small. Measured, the tables cost about 0.2 MB —
//! against handing back mojibake for every Japanese, Chinese and Korean page,
//! which is most of the non-Latin web.
//!
//! Label lookup follows the WHATWG rules, so the aliases real pages use
//! (`latin1`, `sjis`, `x-gbk`, `ms949`, …) all resolve.

/// What decoding a body needs, given its declared charset.
///
/// Non-exhaustive: the set of recognized encodings is `encoding_rs`', not ours,
/// and this should be able to grow without breaking callers again.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Charset {
    /// UTF-8 (or nothing declared): decoded as UTF-8, lossily.
    Utf8,
    /// A label we can decode exactly. Carries the encoding's canonical name.
    Supported(String),
    /// A label no known encoding matches. Decoded as UTF-8, which will mangle
    /// it, and reported so the caller can see why.
    Unknown(String),
}

/// Is this label UTF-8 (or absent), and therefore the fast path?
fn is_utf8_label(normalized: &str) -> bool {
    matches!(
        normalized,
        "" | "utf-8" | "utf8" | "unicode-1-1-utf-8" | "us-ascii" | "ascii"
    )
}

/// Classify a charset label.
pub fn classify(label: &str) -> Charset {
    let trimmed = label.trim().trim_matches('"');
    if is_utf8_label(&trimmed.to_ascii_lowercase()) {
        return Charset::Utf8;
    }
    match encoding_rs::Encoding::for_label(trimmed.as_bytes()) {
        // `for_label` maps the UTF-8 aliases too; keep them on the fast path.
        Some(encoding) if encoding == encoding_rs::UTF_8 => Charset::Utf8,
        Some(encoding) => Charset::Supported(encoding.name().to_string()),
        None => Charset::Unknown(trimmed.to_string()),
    }
}

/// Decode a body according to its declared charset.
///
/// Returns the text, plus the charset label to report when the result may be
/// garbled — `None` whenever the decoding was exact.
pub fn decode(bytes: &[u8], label: Option<&str>) -> (String, Option<String>) {
    let Some(label) = label else {
        return (String::from_utf8_lossy(bytes).into_owned(), None);
    };
    let trimmed = label.trim().trim_matches('"');
    if is_utf8_label(&trimmed.to_ascii_lowercase()) {
        return (String::from_utf8_lossy(bytes).into_owned(), None);
    }

    match encoding_rs::Encoding::for_label(trimmed.as_bytes()) {
        Some(encoding) => {
            // `decode` strips a BOM when present and substitutes replacement
            // characters for malformed sequences rather than failing.
            let (text, _, _) = encoding.decode(bytes);
            (text.into_owned(), None)
        }
        None => (
            String::from_utf8_lossy(bytes).into_owned(),
            Some(trimmed.to_string()),
        ),
    }
}

/// Find `<meta charset=…>` in the head of a body whose header declared nothing.
///
/// Only the first 2 KiB are searched: the declaration is required to appear
/// early, and scanning a whole 5 MiB body for it would be wasted work.
pub fn sniff_meta(raw: &[u8]) -> Option<String> {
    const WINDOW: usize = 2048;
    let head = &raw[..raw.len().min(WINDOW)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    let at = text.find("charset")? + "charset".len();
    let rest = text[at..].trim_start().strip_prefix('=')?.trim_start();
    let value: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!value.is_empty()).then_some(value)
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
    fn utf8_labels_take_the_fast_path() {
        for label in ["utf-8", "UTF-8", " \"utf8\" ", "us-ascii", ""] {
            assert_eq!(classify(label), Charset::Utf8, "{label}");
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
        let (text, _) = decode(b"\x93quoted\x94 \x85", Some("windows-1252"));
        assert_eq!(text, "“quoted” …");
    }

    /// The case the hand-rolled table could not handle: CJK pages came back as
    /// replacement characters.
    #[test]
    fn shift_jis_decodes() {
        // "こんにちは" in Shift_JIS.
        let bytes = b"\x82\xb1\x82\xf1\x82\xc9\x82\xbf\x82\xcd";
        let (text, reported) = decode(bytes, Some("Shift_JIS"));
        assert_eq!(text, "こんにちは");
        assert!(reported.is_none());
    }

    #[test]
    fn gbk_decodes() {
        // "中文" in GBK.
        let (text, reported) = decode(b"\xd6\xd0\xce\xc4", Some("GBK"));
        assert_eq!(text, "中文");
        assert!(reported.is_none());
    }

    #[test]
    fn big5_decodes() {
        // "中文" in Big5.
        let (text, reported) = decode(b"\xa4\xa4\xa4\xe5", Some("Big5"));
        assert_eq!(text, "中文");
        assert!(reported.is_none());
    }

    #[test]
    fn euc_kr_decodes() {
        // "한국" in EUC-KR.
        let (text, reported) = decode(b"\xc7\xd1\xb1\xb9", Some("EUC-KR"));
        assert_eq!(text, "한국");
        assert!(reported.is_none());
    }

    /// Real pages declare aliases, not canonical names.
    #[test]
    fn whatwg_aliases_resolve() {
        for (label, canonical) in [
            ("latin1", "windows-1252"),
            ("sjis", "Shift_JIS"),
            ("x-gbk", "GBK"),
            ("windows-949", "EUC-KR"),
            ("korean", "EUC-KR"),
            ("iso-2022-jp", "ISO-2022-JP"),
        ] {
            assert_eq!(
                classify(label),
                Charset::Supported(canonical.to_string()),
                "{label}"
            );
        }
    }

    #[test]
    fn an_unrecognized_label_is_reported_rather_than_hidden() {
        assert_eq!(
            classify("x-not-a-real-encoding"),
            Charset::Unknown("x-not-a-real-encoding".into())
        );
        let (_, reported) = decode(b"bytes", Some("x-not-a-real-encoding"));
        assert_eq!(reported.as_deref(), Some("x-not-a-real-encoding"));
    }

    #[test]
    fn utf8_bodies_round_trip() {
        let (text, reported) = decode("Café — naïve".as_bytes(), Some("utf-8"));
        assert_eq!(text, "Café — naïve");
        assert!(reported.is_none());
    }

    #[test]
    fn a_utf16_body_decodes_including_its_bom() {
        // "hi" in UTF-16LE with a BOM.
        let (text, reported) = decode(b"\xff\xfeh\x00i\x00", Some("utf-16"));
        assert_eq!(text, "hi");
        assert!(reported.is_none());
    }

    #[test]
    fn meta_charset_is_sniffed_from_the_head() {
        assert_eq!(
            sniff_meta(br#"<html><head><meta charset="Shift_JIS"></head>"#).as_deref(),
            Some("shift_jis")
        );
        assert_eq!(
            sniff_meta(
                b"<html><head><meta http-equiv=content-type content='text/html; charset=gbk'>"
            )
            .as_deref(),
            Some("gbk")
        );
        assert_eq!(sniff_meta(b"<html><head></head>").as_deref(), None);
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
