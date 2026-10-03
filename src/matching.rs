//! Exact byte-safe matching for Everything expressions; no database construction.
use crate::search::SearchOptions;
use anyhow::{Context, Result, bail};
use regex::bytes::{Regex, RegexBuilder};
use std::os::unix::ffi::OsStrExt;

pub struct Query {
    matchers: Vec<Matcher>,
    basename: bool,
    ignore_case: bool,
}
enum Matcher {
    Literal(Vec<u8>),
    Regex(Regex),
}
impl Query {
    pub fn new(options: &SearchOptions) -> Result<Self> {
        let mut matchers = Vec::new();
        for pattern in &options.patterns {
            let bytes = pattern.as_bytes();
            if options.regex || bytes.iter().any(|b| matches!(b, b'*' | b'?' | b'[')) {
                let expression = if options.regex {
                    pattern
                        .to_str()
                        .context("regex patterns must be UTF-8")?
                        .to_owned()
                } else {
                    glob_regex(bytes)?
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
    pub(crate) fn matches(&self, path: &[u8]) -> bool {
        let value = if self.basename {
            path.rsplit(|b| *b == b'/').next().unwrap_or(path)
        } else {
            path
        };
        self.matchers.iter().all(|matcher| match matcher {
            Matcher::Literal(needle) => {
                needle.is_empty()
                    || value.windows(needle.len()).any(|window| {
                        if self.ignore_case {
                            window.eq_ignore_ascii_case(needle)
                        } else {
                            window == needle
                        }
                    })
            }
            Matcher::Regex(regex) => regex.is_match(value),
        })
    }
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
#[path = "../tests/unit/matching.rs"]
mod tests;
