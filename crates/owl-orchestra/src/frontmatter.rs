//! Front-matter parser for `*.md` spec files.
//!
//! Format (Hugo / Zola convention, TOML-flavoured):
//!
//! ```text
//! +++
//! schema_version = 1
//! id            = "grep_callsites"
//! …
//! +++
//!
//! Body markdown follows…
//! ```
//!
//! We use TOML (not YAML) so a single parser handles every spec file in the
//! crate.  Both LF and CRLF line endings are accepted.

use serde::de::DeserializeOwned;

use crate::error::FrontmatterErrorKind;

/// Parsed front-matter + remaining markdown body.
pub struct Parsed<T> {
    pub front: T,
    pub body:  String,
}

/// Split `+++\n…\n+++\n…body…` into TOML-deserialised front-matter + body.
///
/// Returns [`FrontmatterErrorKind::MissingOpening`] if the input does not
/// begin with `+++`, [`FrontmatterErrorKind::MissingClosing`] if no closing
/// `+++` line is found, or [`FrontmatterErrorKind::Toml`] on a parse error.
pub fn parse<T: DeserializeOwned>(input: &str) -> Result<Parsed<T>, FrontmatterErrorKind> {
    let mut lines = input.lines();

    // Opening delimiter (skip a single optional BOM line if present)
    let first = lines.next().ok_or(FrontmatterErrorKind::MissingOpening)?;
    if first.trim() != "+++" {
        return Err(FrontmatterErrorKind::MissingOpening);
    }

    // Collect front-matter lines until the closing `+++` line.
    let mut fm = String::new();
    let mut found_close = false;
    let mut body_lines: Vec<&str> = Vec::new();
    for line in lines {
        if !found_close {
            if line.trim() == "+++" {
                found_close = true;
                continue;
            }
            fm.push_str(line);
            fm.push('\n');
        } else {
            body_lines.push(line);
        }
    }

    if !found_close {
        return Err(FrontmatterErrorKind::MissingClosing);
    }

    let front: T = toml::from_str(&fm).map_err(FrontmatterErrorKind::Toml)?;
    // Trim a single leading blank line — common after the `+++` delimiter.
    let body = body_lines.join("\n");
    let body = body.strip_prefix('\n').unwrap_or(&body).to_string();
    Ok(Parsed { front, body })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct Demo {
        id:    String,
        count: u32,
    }

    #[test]
    fn parses_basic_frontmatter() {
        // `lines()` strips line terminators; we deliberately don't re-add a
        // trailing newline.  Body content semantically preserved, but the
        // final `\n` after the last line is treated as terminator, not data.
        let input = "+++\nid = \"foo\"\ncount = 3\n+++\n\nhello world\n";
        let p: Parsed<Demo> = parse(input).expect("must parse");
        assert_eq!(p.front, Demo { id: "foo".into(), count: 3 });
        assert_eq!(p.body, "hello world");
    }

    #[test]
    fn rejects_missing_opening() {
        let input = "no frontmatter here";
        assert!(matches!(
            parse::<Demo>(input),
            Err(FrontmatterErrorKind::MissingOpening)
        ));
    }

    #[test]
    fn rejects_missing_closing() {
        let input = "+++\nid = \"foo\"\ncount = 3\n";
        assert!(matches!(
            parse::<Demo>(input),
            Err(FrontmatterErrorKind::MissingClosing)
        ));
    }

    #[test]
    fn handles_crlf_line_endings() {
        let input = "+++\r\nid = \"foo\"\r\ncount = 9\r\n+++\r\n\r\nbody\r\n";
        let p: Parsed<Demo> = parse(input).expect("crlf must parse");
        assert_eq!(p.front.count, 9);
    }

    #[test]
    fn body_can_contain_triple_plus_inside() {
        // Only standalone `+++` lines act as delimiters.
        let input = "+++\nid = \"x\"\ncount = 0\n+++\nfoo\n+++not-a-delim\nbar\n";
        let p: Parsed<Demo> = parse(input).expect("must parse");
        assert!(p.body.contains("+++not-a-delim"));
    }
}
