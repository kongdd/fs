//! Everything-style expression filtering over byte-safe backend candidates.
use crate::{
    config::Config,
    native::Query,
    search::{SearchOptions, visit_paths_until},
};
use anyhow::{Result, bail};
use std::{collections::HashSet, ffi::OsString, os::unix::ffi::OsStringExt};

#[derive(Debug, PartialEq)]
enum Token {
    Word(Vec<u8>, bool),
    Or,
    Not,
    Open,
    Close,
}

fn tokens(args: &[OsString]) -> Result<Vec<Token>> {
    use std::os::unix::ffi::OsStrExt;
    let input = args
        .iter()
        .map(|s| s.as_bytes())
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
    // Each OR branch contributes mandatory positive literals. NOT and extension
    // predicates never supply an unsafe anchor. Bound DNF expansion explicitly.
    fn branches(&self) -> Result<Vec<Vec<(OsString, bool)>>> {
        match self {
            Self::Term(term) => Ok(vec![term.anchor.iter().cloned().collect()]),
            Self::Extension(_) | Self::Not(_) => Ok(vec![vec![]]),
            Self::Or(a, b) => {
                let mut left = a.branches()?;
                left.extend(b.branches()?);
                if left.len() > 64 {
                    bail!("too many OR branches (maximum 64)");
                }
                Ok(left)
            }
            Self::And(a, b) => {
                let left = a.branches()?;
                let right = b.branches()?;
                if left.len() * right.len() > 64 {
                    bail!("too many OR branches (maximum 64)");
                }
                Ok(left
                    .into_iter()
                    .flat_map(|l| {
                        right.iter().map(move |r| {
                            let mut terms = l.clone();
                            terms.extend(r.iter().cloned());
                            terms
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
        let mut basename = !self.options.match_path && !word.contains(&b'/');
        if !literal {
            if let Some(value) = word.strip_prefix(b"ext:") {
                let extensions = value
                    .split(|b| *b == b';')
                    .map(|s| s.strip_prefix(b".").unwrap_or(s).to_vec())
                    .collect::<Vec<_>>();
                if extensions.iter().any(|s| {
                    s.is_empty() || s.iter().any(|b| matches!(b, b'/' | b'\\' | b'*' | b'?'))
                }) {
                    bail!("ext: requires extensions separated by semicolons, e.g. ext:nc;tif");
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
            anchor(&word).map(|bytes| (OsString::from_vec(bytes), basename))
        };
        let query = Query::new(&SearchOptions {
            patterns: vec![OsString::from_vec(word)],
            basename,
            ignore_case: !self.options.case_sensitive,
            regex,
            ..Default::default()
        })?;
        Ok(Expr::Term(Term { query, anchor }))
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
    let mut anchors = Vec::new();
    for branch in branches {
        let Some(best) = branch.into_iter().max_by_key(|(s, _)| s.len()) else {
            anchors = vec![(OsString::from("*"), false)];
            break;
        };
        if !anchors.contains(&best) {
            anchors.push(best);
        }
    }
    let dedup = anchors.len() > 1;
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
            locate: false,
            ..options.clone()
        };
        visit_paths_until(cfg, &candidate, |path| {
            if !expr.matches(path) || (dedup && !seen.insert(path.to_vec())) {
                return Ok(true);
            }
            if skipped < options.offset {
                skipped += 1;
                return Ok(true);
            }
            visitor(path)?;
            written += 1;
            Ok(options.limit.is_none_or(|limit| written < limit))
        })?;
        if options.limit.is_some_and(|limit| written >= limit) {
            break;
        }
    }
    Ok(())
}
