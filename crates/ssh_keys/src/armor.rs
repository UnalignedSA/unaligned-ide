//! The `-----BEGIN …-----` / `-----END …-----` wrapping that OpenSSH and PEM
//! keys share, read the way people actually paste it: with text around it,
//! Windows line ends, lost or changed line breaks.

use std::ops::RangeInclusive;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use zeroize::Zeroizing;

use crate::{KeyError, is_padding};

const BEGIN: &str = "-----BEGIN ";
const DASHES: &str = "-----";

/// One armored block: the label of its BEGIN line and what lies between the
/// BEGIN and END lines.
pub(crate) struct Armor<'a> {
    pub(crate) label: &'a str,
    /// `Proc-Type:` and `DEK-Info:` of an encrypted PEM key, kept as they are.
    pub(crate) headers: Vec<&'a str>,
    /// The base64 lines as given, without the spaces around them.
    lines: Vec<&'a str>,
}

/// Finds the private key block of a text. `Ok(None)` when the text has no
/// BEGIN line at all; an error when it has blocks but none is a private key.
pub(crate) fn find_private_key(text: &str) -> Result<Option<Armor<'_>>, KeyError> {
    let mut rest = text;
    let mut other_label = None;
    while let Some(start) = rest.find(BEGIN) {
        let after_begin = &rest[start + BEGIN.len()..];
        let label_end = after_begin.find(DASHES).filter(|end| {
            let label = &after_begin[..*end];
            !label.is_empty() && label.len() <= 64 && !label.contains(['\n', '\r'])
        });
        let Some(label_end) = label_end else {
            return Err(KeyError::Damaged(
                "The first line of this key is damaged: copy the whole key again, from \"-----BEGIN\" to the last dash."
                    .to_string(),
            ));
        };
        let label = &after_begin[..label_end];
        let body_start = &after_begin[label_end + DASHES.len()..];
        // The dashes that close the END line are not required: they are the
        // first thing a careless selection loses.
        let end_marker = format!("-----END {label}");
        let is_private = label.ends_with("PRIVATE KEY");
        match body_start.find(&end_marker) {
            Some(body_end) if is_private => {
                return Ok(Some(Armor::new(label, &body_start[..body_end])));
            }
            Some(body_end) => {
                other_label.get_or_insert(label);
                rest = &body_start[body_end + end_marker.len()..];
            }
            None if is_private => {
                return Err(KeyError::Damaged(
                    "This key is cut short, its last line (\"-----END …\") is missing: copy the whole key again."
                        .to_string(),
                ));
            }
            None => {
                other_label.get_or_insert(label);
                rest = body_start;
            }
        }
    }
    match other_label {
        None => Ok(None),
        Some(label) if label.contains("PUBLIC KEY") => Err(KeyError::PublicKey),
        Some(_) => Err(KeyError::NotAKey),
    }
}

impl<'a> Armor<'a> {
    fn new(label: &'a str, body: &'a str) -> Self {
        let mut headers = Vec::new();
        let mut lines = Vec::new();
        for line in body.split(['\n', '\r']) {
            let line = line.trim_matches(is_padding);
            if line.is_empty() {
                continue;
            }
            // Base64 has no colon, a header line always has one, and headers
            // come before the body.
            if lines.is_empty() && line.contains(':') {
                headers.push(line);
            } else {
                lines.push(line);
            }
        }
        Self {
            label,
            headers,
            lines,
        }
    }

    /// The base64 body as one line, without any of the spaces or line breaks
    /// it was given with.
    fn joined(&self) -> Zeroizing<String> {
        let mut joined = Zeroizing::new(String::with_capacity(
            self.lines.iter().map(|line| line.len()).sum(),
        ));
        for line in &self.lines {
            joined.extend(line.chars().filter(|character| !is_padding(*character)));
        }
        joined
    }

    /// The bytes the base64 body stands for.
    pub(crate) fn decode(&self) -> Result<Zeroizing<Vec<u8>>, KeyError> {
        let joined = self.joined();
        if joined.is_empty() {
            return Err(KeyError::Damaged(
                "This key is empty between its first and last lines: copy the whole key again."
                    .to_string(),
            ));
        }
        STANDARD.decode(joined.as_bytes()).map(Zeroizing::new).map_err(|_| {
            KeyError::Damaged(
                "This key is damaged, some of its characters are missing or were changed: copy the whole key again."
                    .to_string(),
            )
        })
    }

    /// The block as a key file: LF line ends, nothing around it. The body
    /// lines are kept when they are already regular and their length is one
    /// of `kept_widths`; otherwise the body is cut again every `width`
    /// characters, as the tools that write these files do.
    ///
    /// Call it after `decode` has accepted the body.
    pub(crate) fn to_file_text(&self, kept_widths: RangeInclusive<usize>, width: usize) -> String {
        let joined;
        let lines: Vec<&str> = if self.is_regular(&kept_widths) {
            self.lines.clone()
        } else {
            joined = self.joined();
            // Base64 is ASCII, so every chunk of bytes is valid text.
            joined
                .as_bytes()
                .chunks(width)
                .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
                .collect()
        };
        let mut text = String::with_capacity(
            2 * (BEGIN.len() + self.label.len() + DASHES.len() + 1)
                + self
                    .headers
                    .iter()
                    .map(|header| header.len() + 1)
                    .sum::<usize>()
                + 1
                + lines.iter().map(|line| line.len() + 1).sum::<usize>(),
        );
        text.push_str(BEGIN);
        text.push_str(self.label);
        text.push_str(DASHES);
        text.push('\n');
        for header in &self.headers {
            text.push_str(header);
            text.push('\n');
        }
        if !self.headers.is_empty() {
            text.push('\n');
        }
        for line in lines {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("-----END ");
        text.push_str(self.label);
        text.push_str(DASHES);
        text.push('\n');
        text
    }

    /// Whether the body lines look like a tool wrote them: all of one length
    /// (a whole number of base64 groups, and one of `widths`), the last one
    /// no longer.
    fn is_regular(&self, widths: &RangeInclusive<usize>) -> bool {
        let Some((last, full)) = self.lines.split_last() else {
            return false;
        };
        let shape_is_regular = match full.first() {
            None => last.len() <= *widths.end(),
            Some(first) => {
                let width = first.len();
                widths.contains(&width)
                    && width % 4 == 0
                    && last.len() <= width
                    && full.iter().all(|line| line.len() == width)
            }
        };
        shape_is_regular && self.lines.iter().all(|line| !line.contains(is_padding))
    }
}
