//! Everything-style expression filtering over byte-safe backend candidates.
use crate::platform::{normalize, os_string};
use crate::{
    config::Config,
    matching::Query,
    search::{SearchOptions, visit_paths_until_filtered},
};
use anyhow::{Result, bail};
use fs_core::index_store::grams;
use std::{collections::HashSet, ffi::OsString};

#[derive(Debug, PartialEq)]
enum Token {
    Word(Vec<u8>, bool),
    Or,
    Not,
    Open,
    Close,
}

fn tokens(args: &[OsString]) -> Result<Vec<Token>> {
    let input = args
        .iter()
        .map(|s| s.as_encoded_bytes())
        .collect::<Vec<_>>()
        .join(&b' ');
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b' ' | b'\t' | b'\n' | b'\r' => {
                i += 1;
                continue;
            }
            b'|' => {
                out.push(Token::Or);
                i += 1;
            }
            b'!' => {
                out.push(Token::Not);
                i += 1;
            }
            b'<' => {
                out.push(Token::Open);
                i += 1;
            }
            b'>' => {
                out.push(Token::Close);
                i += 1;
            }
            _ => {
                let literal_prefix = input[i] == b'"';
                let mut word = Vec::new();
                let mut quoted = false;
                while i < input.len() {
                    let b = input[i];
                    if b == b'"' {
                        quoted = !quoted;
                        i += 1;
                        continue;
                    }
                    if !quoted
                        && (b.is_ascii_whitespace() || matches!(b, b'|' | b'!' | b'<' | b'>'))
                    {
                        break;
                    }
                    word.push(b);
                    i += 1;
                }
                if quoted {
                    bail!("unclosed quote in search expression");
                }
                if word.is_empty() {
                    bail!("empty search term");
                }
                out.push(Token::Word(word, literal_prefix));
            }
        }
        if out.len() > 256 {
            bail!("search expression is too complex (maximum 256 tokens)");
        }
    }
    Ok(out)
}

struct Term {
    query: Query,
    anchor: Option<(OsString, bool)>,
    basename: bool,
}
enum Expr {
    Term(Term),
    Extension(Vec<Vec<u8>>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
}
impl Expr {
    fn matches(&self, path: &[u8]) -> bool {
        match self {
            Self::Term(term) => term.query.matches(path),
            Self::Extension(extensions) => {
                let name = path.rsplit(|b| *b == b'/').next().unwrap_or(path);
                name.iter().rposition(|b| *b == b'.').is_some_and(|dot| {
                    extensions
                        .iter()
                        .any(|ext| name[dot + 1..].eq_ignore_ascii_case(ext))
                })
            }
            Self::And(a, b) => a.matches(path) && b.matches(path),
            Self::Or(a, b) => a.matches(path) || b.matches(path),
            Self::Not(a) => !a.matches(path),
        }
    }
    // Path terms cannot be decided from a basename. In particular NOT of
    // a mixed/path expression must not negate a conservative approximation.
    fn basename_only(&self) -> bool {
        match self {
            Self::Term(term) => term.basename,
            Self::Extension(_) => true,
            Self::And(a, b) | Self::Or(a, b) => a.basename_only() && b.basename_only(),
            Self::Not(a) => a.basename_only(),
        }
    }

    fn may_match_basename(&self, name: &[u8]) -> bool {
        match self {
            Self::Term(term) => !term.basename || term.query.matches(name),
            Self::Extension(_) => self.matches(name),
            Self::And(a, b) => a.may_match_basename(name) && b.may_match_basename(name),
            Self::Or(a, b) => a.may_match_basename(name) || b.may_match_basename(name),
            Self::Not(a) => !a.basename_only() || !a.matches(name),
        }
    }

    // Only positive AND terms are mandatory. Leave OR/NOT planning unchanged.
    fn required_grams(&self) -> Vec<u32> {
        match self {
            Self::Term(term) => term
                .anchor
                .as_ref()
                .map_or_else(Vec::new, |(pattern, _)| grams(pattern.as_encoded_bytes())),
            Self::Extension(ext) if ext.len() == 1 => {
                let literal = [b".".as_slice(), &ext[0]].concat();
                anchor(&literal).map_or_else(Vec::new, |run| grams(&run))
            }
            Self::And(a, b) => {
                let mut grams = a.required_grams();
                grams.extend(b.required_grams());
                grams
            }
            _ => Vec::new(),
        }
    }

    // Keep only the strongest necessary anchor per OR branch; the search
    // uses one anchor, not a list of terms. NOT supplies none. Bound expansion.
    fn branches(&self) -> Result<Vec<Option<(OsString, bool)>>> {
        match self {
            Self::Term(term) => Ok(vec![term.anchor.clone()]),
            Self::Extension(extensions) => {
                if extensions.len() > 64 {
                    return Ok(vec![None]); // Preserve large extension-list support.
                }
                extensions
                    .iter()
                    .map(|extension| {
                        let mut literal = Vec::with_capacity(extension.len() + 1);
                        literal.push(b'.');
                        literal.extend_from_slice(extension);
                        // '[' is legal in an extension, but candidate patterns
                        // interpret it as glob syntax. Keep only a safe literal
                        // run; exact extension matching remains authoritative.
                        anchor(&literal)
                            .map(|bytes| os_string(bytes).map(|pattern| (pattern, true)))
                            .transpose()
                    })
                    .collect()
            }
            Self::Not(_) => Ok(vec![None]),
            Self::Or(a, b) => {
                let mut left = a.branches()?;
                left.extend(b.branches()?);
                if left.len() > 64 {
                    return Ok(vec![None]); // Bound planning, not expression semantics.
                }
                Ok(left)
            }
            Self::And(a, b) => {
                let left = a.branches()?;
                let right = b.branches()?;
                if left.len() * right.len() > 64 {
                    return Ok(vec![None]);
                }
                Ok(left
                    .iter()
                    .flat_map(|l| {
                        right.iter().map(move |r| {
                            l.iter()
                                .chain(r.iter())
                                .max_by_key(|(s, _)| s.len())
                                .cloned()
                        })
                    })
                    .collect())
            }
        }
    }
}

struct Parser<'a> {
    tokens: Vec<Token>,
    position: usize,
    options: &'a SearchOptions,
}
impl Parser<'_> {
    fn expression(&mut self) -> Result<Expr> {
        let mut expr = self.and()?;
        while self.tokens.get(self.position) == Some(&Token::Or) {
            self.position += 1;
            expr = Expr::Or(Box::new(expr), Box::new(self.and()?));
        }
        Ok(expr)
    }
    fn and(&mut self) -> Result<Expr> {
        let mut expr = self.unary()?;
        while matches!(
            self.tokens.get(self.position),
            Some(Token::Word(..) | Token::Not | Token::Open)
        ) {
            expr = Expr::And(Box::new(expr), Box::new(self.unary()?));
        }
        Ok(expr)
    }
    fn unary(&mut self) -> Result<Expr> {
        match self.tokens.get(self.position) {
            Some(Token::Not) => {
                self.position += 1;
                Ok(Expr::Not(Box::new(self.unary()?)))
            }
            Some(Token::Open) => {
                self.position += 1;
                let expr = self.expression()?;
                if self.tokens.get(self.position) != Some(&Token::Close) {
                    bail!("unclosed <...> search group");
                }
                self.position += 1;
                Ok(expr)
            }
            Some(Token::Word(word, literal)) => {
                let word = word.clone();
                let literal = *literal;
                self.position += 1;
                self.term(word, literal)
            }
            _ => bail!("expected a search term or <...> group"),
        }
    }
    fn term(&self, mut word: Vec<u8>, literal: bool) -> Result<Expr> {
        let mut regex = false;
        let mut basename = !self.options.include_path
            && !word
                .iter()
                .any(|&b| b == b'/' || (cfg!(windows) && b == b'\\'));
        if !literal {
            if let Some(value) = word
                .strip_prefix(b"exts:")
                .or_else(|| word.strip_prefix(b"ext:"))
            {
                let extensions = value
                    .split(|b| matches!(b, b',' | b';'))
                    .map(|s| s.strip_prefix(b".").unwrap_or(s).to_vec())
                    .collect::<Vec<_>>();
                if extensions.iter().any(|s| {
                    s.is_empty() || s.iter().any(|b| matches!(b, b'/' | b'\\' | b'*' | b'?'))
                }) {
                    bail!("exts: requires comma-separated extensions, e.g. exts:docx,pdf");
                }
                return Ok(Expr::Extension(extensions));
            }
            if word.starts_with(b"file:") || word.starts_with(b"folder:") {
                bail!(
                    "file:/folder: require type metadata unavailable in plocate indexes; not supported yet"
                );
            }
            if let Some(value) = word.strip_prefix(b"path:") {
                basename = false;
                word = value.to_vec();
            } else if let Some(value) = word.strip_prefix(b"regex:") {
                regex = true;
                word = value.to_vec();
            }
        }
        if word.is_empty() {
            bail!("empty search term after modifier");
        }
        let anchor = if regex {
            None
        } else {
            word = normalize(&word).into_owned();
            anchor(&word)
                .map(|bytes| os_string(bytes).map(|pattern| (pattern, basename)))
                .transpose()?
        };
        let query = Query::new(&SearchOptions {
            patterns: vec![os_string(word)?],
            basename,
            ignore_case: !self.options.case_sensitive,
            regex,
            ..Default::default()
        })?;
        Ok(Expr::Term(Term {
            query,
            anchor,
            basename,
        }))
    }
}

// A mandatory literal run of a glob is a safe substring candidate. Do not
// treat character-class alternatives as mandatory characters.
fn anchor(word: &[u8]) -> Option<Vec<u8>> {
    let mut longest = Vec::new();
    let mut run = Vec::new();
    let mut class = false;
    for &byte in word {
        if byte == b'[' {
            class = true;
        }
        if class || matches!(byte, b'*' | b'?') {
            if run.len() > longest.len() {
                longest = std::mem::take(&mut run);
            }
            run.clear();
        } else {
            run.push(byte);
        }
        if byte == b']' {
            class = false;
        }
    }
    if run.len() > longest.len() {
        longest = run;
    }
    (!longest.is_empty()).then_some(longest)
}

fn parse(options: &SearchOptions) -> Result<Expr> {
    let mut parser = Parser {
        tokens: tokens(&options.patterns)?,
        position: 0,
        options,
    };
    let expr = parser.expression()?;
    if parser.position != parser.tokens.len() {
        bail!("unexpected search operator or closing group");
    }
    Ok(expr)
}

pub fn visit(
    cfg: &Config,
    options: &SearchOptions,
    mut visitor: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let expr = parse(options)?;
    let branches = expr.branches()?;
    let required_grams = if matches!(expr, Expr::And(..)) {
        expr.required_grams()
    } else {
        Vec::new()
    };
    let mut anchors = Vec::new();
    for branch in branches {
        let Some(best) = branch else {
            anchors = vec![(OsString::from("*"), false)];
            break;
        };
        if !anchors.contains(&best) {
            anchors.push(best);
        }
    }
    let dedup = anchors.len() > 1;
    let basename_only = expr.basename_only();
    let mut seen = HashSet::new();
    let mut skipped = 0;
    let mut written = 0;
    for (anchor, basename) in anchors {
        let candidate = SearchOptions {
            patterns: vec![anchor],
            ignore_case: true,
            basename,
            limit: None,
            offset: 0,
            ..options.clone()
        };
        visit_paths_until_filtered(
            cfg,
            &candidate,
            &required_grams,
            |name| expr.may_match_basename(name),
            |path, native_filtered| {
                if (!(native_filtered && basename_only) && !expr.matches(path))
                    || (dedup && !seen.insert(path.to_vec()))
                {
                    return Ok(true);
                }
                if skipped < options.offset {
                    skipped += 1;
                    return Ok(true);
                }
                visitor(path)?;
                written += 1;
                Ok(options.limit.is_none_or(|limit| written < limit))
            },
        )?;
        if options.limit.is_some_and(|limit| written >= limit) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/unit/locate/everything.rs"]
mod tests;
