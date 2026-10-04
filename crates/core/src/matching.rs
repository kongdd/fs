//! Exact byte-safe matching for Everything expressions; no database construction.
use crate::query::SearchOptions;
use anyhow::{Context, Result, bail};
use regex::bytes::{Regex, RegexBuilder};

pub struct Query {
    matchers: Vec<Matcher>,
    basename: bool,
    ignore_case: bool,
}
enum Matcher {
    Literal(Vec<u8>),
    Prefix(Vec<u8>),
    Suffix(Vec<u8>),
    Regex(Regex),
}
impl Query {
    pub fn new(options: &SearchOptions) -> Result<Self> {
        let mut matchers = Vec::new();
        for pattern in &options.patterns {
            let raw = pattern.as_encoded_bytes();
            let bytes = if options.regex {
                std::borrow::Cow::Borrowed(raw)
            } else {
                crate::platform::normalize(raw)
            };
            if !options.regex
                && let Some(matcher) = simple_glob(&bytes)
            {
                matchers.push(matcher);
            } else if options.regex || bytes.iter().any(|b| matches!(b, b'*' | b'?' | b'[')) {
                let expression = if options.regex {
                    pattern
                        .to_str()
                        .context("regex patterns must be UTF-8")?
                        .to_owned()
                } else {
                    glob_regex(&bytes)?
                };
                matchers.push(Matcher::Regex(
                    RegexBuilder::new(&expression)
                        .unicode(false)
                        .case_insensitive(options.ignore_case)
                        .build()
                        .context("invalid pattern")?,
                ));
            } else {
                matchers.push(Matcher::Literal(bytes.to_vec()));
            }
        }
        Ok(Self {
            matchers,
            basename: options.basename,
            ignore_case: options.ignore_case,
        })
    }
    pub fn matches(&self, path: &[u8]) -> bool {
        let value = if self.basename {
            path.rsplit(|b| *b == b'/').next().unwrap_or(path)
        } else {
            path
        };
        self.matchers.iter().all(|matcher| match matcher {
            Matcher::Literal(needle) => contains(value, needle, self.ignore_case),
            Matcher::Prefix(needle) => value.get(..needle.len()).is_some_and(|prefix| {
                if self.ignore_case {
                    prefix.eq_ignore_ascii_case(needle)
                } else {
                    prefix == needle
                }
            }),
            Matcher::Suffix(needle) => value.len().checked_sub(needle.len()).is_some_and(|start| {
                let suffix = &value[start..];
                if self.ignore_case {
                    suffix.eq_ignore_ascii_case(needle)
                } else {
                    suffix == needle
                }
            }),
            Matcher::Regex(regex) => regex.is_match(value),
        })
    }
}

// Skip impossible starting positions with memchr's byte-search fast path,
// rather than performing an ASCII comparison at every byte in the filename.
fn contains(value: &[u8], needle: &[u8], ignore_case: bool) -> bool {
    let Some(&first) = needle.first() else {
        return true;
    };
    let Some(last_start) = value.len().checked_sub(needle.len()) else {
        return false;
    };
    if !ignore_case {
        return memchr::memmem::find(value, needle).is_some();
    }
    memchr::memchr2_iter(
        first.to_ascii_lowercase(),
        first.to_ascii_uppercase(),
        &value[..=last_start],
    )
    .any(|start| value[start..start + needle.len()].eq_ignore_ascii_case(needle))
}

// Only leading/trailing stars: byte comparisons exactly implement the glob.
// Internal stars, '?' and character classes keep the general regex matcher.
fn simple_glob(pattern: &[u8]) -> Option<Matcher> {
    if pattern.iter().any(|b| matches!(b, b'?' | b'[')) || !pattern.contains(&b'*') {
        return None;
    }
    let start = pattern
        .iter()
        .position(|&b| b != b'*')
        .unwrap_or(pattern.len());
    let end = pattern
        .iter()
        .rposition(|&b| b != b'*')
        .map_or(start, |i| i + 1);
    let literal = &pattern[start..end];
    if literal.contains(&b'*') {
        return None;
    }
    let needle = literal.to_vec();
    Some(
        if start > 0 && (end < pattern.len() || literal.is_empty()) {
            Matcher::Literal(needle)
        } else if start > 0 {
            Matcher::Suffix(needle)
        } else {
            Matcher::Prefix(needle)
        },
    )
}

// Glob matching is anchored like plocate; non-glob text is substring matching.

fn glob_regex(pattern: &[u8]) -> Result<String> {
    let mut result = String::from("(?s)^");
    let mut i = 0;
    while i < pattern.len() {
        match pattern[i] {
            b'*' => result.push_str(".*"),
            b'?' => result.push('.'),
            b'[' => {
                result.push('[');
                i += 1;
                if i == pattern.len() {
                    bail!("unclosed glob character class");
                }
                if matches!(pattern[i], b'!' | b'^') {
                    result.push('^');
                    i += 1;
                }
                let start = i;
                while i < pattern.len() && pattern[i] != b']' {
                    if pattern[i] == b'-' {
                        result.push('-');
                    } else {
                        result.push_str(&format!("\\x{:02x}", pattern[i]));
                    }
                    i += 1;
                }
                if i == pattern.len() || i == start {
                    bail!("invalid glob character class");
                }
                result.push(']');
            }
            byte => result.push_str(&format!("\\x{byte:02x}")),
        }
        i += 1;
    }
    result.push('$');
    Ok(result)
}

#[cfg(test)]
#[path = "../../../tests/unit/core/matching.rs"]
mod tests;
