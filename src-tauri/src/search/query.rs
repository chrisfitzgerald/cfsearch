//! dtSearch-style query syntax -> Tantivy queries.
//!
//! On top of what Tantivy's `QueryParser` already understands (boolean,
//! phrases, proximity slop) this layer adds three term forms:
//!
//! * **Wildcards** -- `invoic*`, `organi?e` (`*` = any run, `?` = one char)
//! * **Regex**     -- `/inv.*ce/` (slashes delimit a raw regex)
//! * **Fuzzy**     -- `term~` or `term~2` (edit distance, max 2)
//!
//! ## Model
//!
//! The query is split on top-level `OR` into groups; within a group terms
//! are combined with `AND` (every term required) unless negated with a
//! leading `-` or a preceding `NOT`. So `a b OR c -d` means
//! `(a AND b) OR (c AND NOT d)`. A bare `AND` is accepted but redundant.
//!
//! Normal terms and phrases are delegated to the provided `QueryParser`
//! (so tokenization/lowercasing and the content+filename default fields are
//! reused). Wildcard/regex/fuzzy terms are matched against the `content`
//! field's indexed (lowercased) terms.

use anyhow::{anyhow, Result};
use tantivy::query::{
    AllQuery, BooleanQuery, FuzzyTermQuery, Occur, Query, QueryParser, RegexQuery,
};
use tantivy::schema::Field;
use tantivy::Term;

/// Maximum edit distance we allow for fuzzy queries (Tantivy's automaton is
/// only practical for small distances).
const MAX_FUZZY_DISTANCE: u8 = 2;

/// Translates cfSearch query strings into Tantivy queries.
pub struct QueryBuilder<'a> {
    parser: &'a QueryParser,
    content: Field,
}

/// Whether a term is required or excluded within its group.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Sign {
    /// Required (the default within a group).
    Positive,
    /// Excluded (`-term` or `NOT term`).
    Negative,
}

/// A single lexed term (a word or a quoted phrase) plus its sign.
#[derive(Clone, Debug)]
enum TermTok {
    Word { sign: Sign, text: String },
    Phrase { sign: Sign, text: String, slop: u32 },
}

/// Top-level tokens produced by the lexer.
#[derive(Clone, Debug)]
enum Tok {
    Or,
    And,
    Term(TermTok),
}

impl<'a> QueryBuilder<'a> {
    pub fn new(parser: &'a QueryParser, content: Field) -> Self {
        Self { parser, content }
    }

    /// Build a Tantivy query from a cfSearch query string.
    pub fn build(&self, input: &str) -> Result<Box<dyn Query>> {
        let toks = lex(input);
        let groups = split_or(toks);
        if groups.is_empty() {
            return Err(anyhow!("empty query"));
        }

        // Single group -> just that AND-group; multiple -> OR them together.
        if groups.len() == 1 {
            return self.build_group(&groups[0]);
        }
        let mut shoulds: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(groups.len());
        for group in &groups {
            shoulds.push((Occur::Should, self.build_group(group)?));
        }
        Ok(Box::new(BooleanQuery::new(shoulds)))
    }

    /// Build one AND-group: positive terms are `Must`, negatives `MustNot`.
    fn build_group(&self, terms: &[TermTok]) -> Result<Box<dyn Query>> {
        let mut clauses: Vec<(Occur, Box<dyn Query>)> = Vec::with_capacity(terms.len());
        let mut has_positive = false;
        for term in terms {
            let (occur, query) = self.build_term(term)?;
            if occur == Occur::Must {
                has_positive = true;
            }
            clauses.push((occur, query));
        }
        // A group of only exclusions (e.g. "-draft") needs something to
        // subtract from, so match everything first.
        if !has_positive {
            clauses.insert(0, (Occur::Must, Box::new(AllQuery)));
        }
        Ok(Box::new(BooleanQuery::new(clauses)))
    }

    /// Build the query for a single term and resolve its occurrence.
    fn build_term(&self, term: &TermTok) -> Result<(Occur, Box<dyn Query>)> {
        match term {
            TermTok::Phrase { sign, text, slop } => {
                let qstr = if *slop > 0 {
                    format!("\"{text}\"~{slop}")
                } else {
                    format!("\"{text}\"")
                };
                let query = self.parser.parse_query(&qstr)?;
                Ok((occur_for(*sign), query))
            }
            TermTok::Word { sign, text } => {
                let query = self.build_word(text)?;
                Ok((occur_for(*sign), query))
            }
        }
    }

    /// Classify a word as regex / wildcard / fuzzy / plain and build it.
    fn build_word(&self, word: &str) -> Result<Box<dyn Query>> {
        // /regex/
        if word.len() >= 2 && word.starts_with('/') && word.ends_with('/') {
            let pattern = word[1..word.len() - 1].to_lowercase();
            return self.regex_query(&pattern);
        }
        // wildcard (* or ?)
        if word.contains('*') || word.contains('?') {
            let pattern = glob_to_regex(&word.to_lowercase());
            return self.regex_query(&pattern);
        }
        // fuzzy (term~ or term~N)
        if let Some((stem, rest)) = word.split_once('~') {
            if !stem.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                let distance = parse_fuzzy_distance(rest);
                let term = Term::from_field_text(self.content, &stem.to_lowercase());
                return Ok(Box::new(FuzzyTermQuery::new(term, distance, true)));
            }
        }
        // plain term -> delegate (reuses tokenizer + default fields)
        Ok(self.parser.parse_query(word)?)
    }

    fn regex_query(&self, pattern: &str) -> Result<Box<dyn Query>> {
        let query = RegexQuery::from_pattern(pattern, self.content)
            .map_err(|e| anyhow!("invalid pattern {pattern:?}: {e}"))?;
        Ok(Box::new(query))
    }
}

fn occur_for(sign: Sign) -> Occur {
    match sign {
        Sign::Positive => Occur::Must,
        Sign::Negative => Occur::MustNot,
    }
}

/// Parse the digits after `~`, defaulting to 1 and capping at the max.
fn parse_fuzzy_distance(digits: &str) -> u8 {
    if digits.is_empty() {
        return 1;
    }
    digits
        .parse::<u8>()
        .unwrap_or(1)
        .clamp(1, MAX_FUZZY_DISTANCE)
}

/// Convert a shell-style glob to an (anchored-by-Tantivy) regex.
fn glob_to_regex(glob: &str) -> String {
    let mut re = String::with_capacity(glob.len() + 4);
    for c in glob.chars() {
        match c {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            // Escape regex metacharacters that may appear in a term.
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => {
                re.push('\\');
                re.push(c);
            }
            other => re.push(other),
        }
    }
    re
}

/// Split top-level tokens into OR-separated groups, dropping `AND`/empties.
fn split_or(toks: Vec<Tok>) -> Vec<Vec<TermTok>> {
    let mut groups = Vec::new();
    let mut current = Vec::new();
    for tok in toks {
        match tok {
            Tok::Or => {
                if !current.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
            }
            Tok::And => {} // redundant; AND is the in-group default
            Tok::Term(t) => current.push(t),
        }
    }
    if !current.is_empty() {
        groups.push(current);
    }
    groups
}

/// Lex a query string into top-level tokens.
///
/// Whitespace separates tokens; double quotes delimit phrases (with an
/// optional trailing `~N` slop); `+`/`-` and the words `AND`/`OR`/`NOT` are
/// operators. `NOT` negates the next term.
fn lex(input: &str) -> Vec<Tok> {
    let chars: Vec<char> = input.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut pending_neg = false;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        // Leading sign.
        let mut sign = Sign::Positive;
        match chars[i] {
            '+' => {
                i += 1;
            }
            '-' => {
                sign = Sign::Negative;
                i += 1;
            }
            _ => {}
        }
        if i >= chars.len() {
            break;
        }

        if chars[i] == '"' {
            // Phrase: read until the closing quote.
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            if i < chars.len() {
                i += 1; // consume closing quote
            }
            // Optional ~N slop.
            let mut slop = 0u32;
            if i < chars.len() && chars[i] == '~' {
                i += 1;
                let ds = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                slop = chars[ds..i].iter().collect::<String>().parse().unwrap_or(0);
            }
            let sign = apply_pending_neg(&mut pending_neg, sign);
            if !text.trim().is_empty() {
                toks.push(Tok::Term(TermTok::Phrase { sign, text, slop }));
            }
        } else {
            // Word: read until whitespace.
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();

            // Operator keywords are only recognized unsigned (no +/-).
            if sign == Sign::Positive {
                match word.as_str() {
                    "AND" => {
                        toks.push(Tok::And);
                        continue;
                    }
                    "OR" => {
                        toks.push(Tok::Or);
                        continue;
                    }
                    "NOT" => {
                        pending_neg = true;
                        continue;
                    }
                    _ => {}
                }
            }
            let sign = apply_pending_neg(&mut pending_neg, sign);
            toks.push(Tok::Term(TermTok::Word { sign, text: word }));
        }
    }
    toks
}

/// Fold a pending `NOT` into the next term's sign, then clear it.
fn apply_pending_neg(pending: &mut bool, sign: Sign) -> Sign {
    if *pending {
        *pending = false;
        Sign::Negative
    } else {
        sign
    }
}
