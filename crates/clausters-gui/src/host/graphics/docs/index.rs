//! **Searching the documentation**: every section of every document, and
//! the two questions asked of them.
//!
//! - **A search across every document** (Ctrl+Shift+F): the sections that
//!   hold every word of a query, a word matching a word of the section that
//!   is it, starts with it or is a letter or two from it, a heading weighing
//!   three times its text, the most relevant first -- the full-text index
//!   the current editor builds with MiniSearch, written here once.
//! - **Help on a name** (Ctrl+D): the sections that talk about it, the one
//!   whose heading *is* the name first -- an API reference's entry for it --
//!   then by how much they say it, in code more than in prose, as the
//!   current editor's search for help reads its books.

use std::collections::HashMap;

/// A section of a document: the text from one heading to the next.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub path: String,
    pub heading: Option<String>,
    pub text: String,
}

/// A section that matches a search.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    pub heading: Option<String>,
    /// The section's first line that holds a word that matched.
    pub line: String,
    /// The words of the section that matched, as the index holds them.
    pub terms: Vec<String>,
}

/// Whether `line` opens or closes a fenced code block.
fn fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~")
}

/// A Markdown heading's title, when `line` is one.
fn heading(line: &str) -> Option<&str> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &line[hashes..];
    rest.starts_with([' ', '\t']).then(|| rest.trim())
}

/// **A document split into its sections**; a `#` inside a code block is not
/// a heading.
pub fn sections(path: &str, source: &str) -> Vec<Section> {
    let mut out = Vec::new();
    let mut current = Section {
        path: path.to_string(),
        heading: None,
        text: String::new(),
    };
    let mut in_fence = false;
    for line in source.lines() {
        if fence(line) {
            in_fence = !in_fence;
        }
        match (!in_fence).then(|| heading(line)).flatten() {
            Some(title) => {
                if !current.text.trim().is_empty() || current.heading.is_some() {
                    out.push(current);
                }
                current = Section {
                    path: path.to_string(),
                    heading: Some(title.replace('`', "").trim().to_string()),
                    text: String::new(),
                };
            }
            None => {
                current.text.push_str(line);
                current.text.push('\n');
            }
        }
    }
    if !current.text.trim().is_empty() || current.heading.is_some() {
        out.push(current);
    }
    out
}

/// The words of `text`, lower case: what lies between spaces and
/// punctuation.
fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// How many edits make `a` into `b`, or more than `most` when that many do
/// not.
fn distance(a: &str, b: &str, most: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > most {
        return most + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut row = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            row[j + 1] = (prev[j] + usize::from(ca != cb))
                .min(prev[j + 1] + 1)
                .min(row[j] + 1);
        }
        if row.iter().min().copied().unwrap_or(0) > most {
            return most + 1;
        }
        prev = row;
    }
    prev[b.len()]
}

/// One field of a section: its words counted, and how many there are.
#[derive(Clone, Debug, Default)]
struct Field {
    counts: HashMap<String, u32>,
    len: u32,
}

impl Field {
    fn of(text: &str) -> Field {
        let mut field = Field::default();
        for w in words(text) {
            *field.counts.entry(w).or_default() += 1;
            field.len += 1;
        }
        field
    }
}

/// **The index**: every section, its two fields counted, and every word the
/// sections hold.
#[derive(Clone, Debug, Default)]
pub struct Index {
    sections: Vec<Section>,
    fields: Vec<[Field; 2]>,
    /// Each word, and the sections that hold it.
    vocabulary: HashMap<String, Vec<usize>>,
}

/// A heading weighs this many times its text.
const HEADING_BOOST: f32 = 3.0;
/// A word that starts with the one sought weighs this much of one that is it.
const PREFIX_WEIGHT: f32 = 0.375;
/// A word a few letters from the one sought weighs this much of one that is.
const FUZZY_WEIGHT: f32 = 0.45;
/// How far a word may be from the one sought, for each of its letters.
const FUZZY: f32 = 0.2;

impl Index {
    /// **Every document indexed**, as `(path, source)` pairs.
    pub fn build<'a>(docs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Index {
        let mut index = Index::default();
        for (path, source) in docs {
            for section in sections(path, source) {
                let at = index.sections.len();
                let fields = [
                    Field::of(section.heading.as_deref().unwrap_or("")),
                    Field::of(&section.text),
                ];
                for field in &fields {
                    for w in field.counts.keys() {
                        let holders = index.vocabulary.entry(w.clone()).or_default();
                        if holders.last() != Some(&at) {
                            holders.push(at);
                        }
                    }
                }
                index.sections.push(section);
                index.fields.push(fields);
            }
        }
        index
    }

    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The words of the index a word sought matches, each with its weight.
    fn matches(&self, sought: &str) -> Vec<(&str, f32)> {
        let len = sought.chars().count();
        let most = ((len as f32 * FUZZY).round() as usize).min(6);
        self.vocabulary
            .keys()
            .filter_map(|w| {
                if w == sought {
                    Some((w.as_str(), 1.0))
                } else if w.starts_with(sought) {
                    let extra = (w.chars().count() - len) as f32;
                    Some((
                        w.as_str(),
                        PREFIX_WEIGHT * len as f32 / (len as f32 + 0.3 * extra),
                    ))
                } else if most > 0 {
                    let d = distance(sought, w, most);
                    (d <= most).then(|| {
                        (
                            w.as_str(),
                            FUZZY_WEIGHT * len as f32 / (len as f32 + d as f32),
                        )
                    })
                } else {
                    None
                }
            })
            .collect()
    }

    /// **The sections that hold every word of `query`**, the most relevant
    /// first, at most `limit` of them.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let sought: Vec<String> = words(query).collect();
        if sought.is_empty() || self.sections.is_empty() {
            return Vec::new();
        }
        let n = self.sections.len() as f32;
        let average = |f: usize| self.fields.iter().map(|fs| fs[f].len as f32).sum::<f32>() / n;
        let averages = [average(0).max(1.0), average(1).max(1.0)];
        // every word sought must be held, by an exact, a prefix or a fuzzy
        // match; the scores of the words add up
        let mut total: Option<HashMap<usize, (f32, Vec<String>)>> = None;
        for s in &sought {
            let mut scores: HashMap<usize, (f32, Vec<String>)> = HashMap::new();
            for (word, weight) in self.matches(s) {
                let holders = &self.vocabulary[word];
                let idf =
                    ((n - holders.len() as f32 + 0.5) / (holders.len() as f32 + 0.5) + 1.0).ln();
                for &at in holders {
                    let mut score = 0.0;
                    for (f, boost) in [(0, HEADING_BOOST), (1, 1.0)] {
                        let field = &self.fields[at][f];
                        let tf = *field.counts.get(word).unwrap_or(&0) as f32;
                        if tf > 0.0 {
                            let (k, b, d) = (1.2, 0.7, 0.5);
                            let norm = 1.0 - b + b * field.len as f32 / averages[f];
                            score += boost * idf * (d + tf * (k + 1.0) / (tf + k * norm));
                        }
                    }
                    let entry = scores.entry(at).or_default();
                    entry.0 += score * weight;
                    if !entry.1.iter().any(|t| t == word) {
                        entry.1.push(word.to_string());
                    }
                }
            }
            total = Some(match total {
                None => scores,
                Some(before) => before
                    .into_iter()
                    .filter_map(|(at, (score, mut terms))| {
                        let (more, also) = scores.get(&at)?;
                        terms.extend(also.iter().cloned());
                        Some((at, (score + more, terms)))
                    })
                    .collect(),
            });
        }
        let mut found: Vec<(usize, f32, Vec<String>)> = total
            .unwrap_or_default()
            .into_iter()
            .map(|(at, (score, terms))| (at, score, terms))
            .collect();
        found.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        found
            .into_iter()
            .take(limit)
            .map(|(at, _, terms)| {
                let s = &self.sections[at];
                Hit {
                    path: s.path.clone(),
                    heading: s.heading.clone(),
                    line: line_with(&s.text, &terms),
                    terms,
                }
            })
            .collect()
    }
}

/// The first line of `text` that holds one of `terms`, skipping the
/// anchors an API page writes.
fn line_with(text: &str, terms: &[String]) -> String {
    for line in text.lines() {
        let lower = line.to_lowercase();
        if !line.starts_with("<a id=") && terms.iter().any(|t| lower.contains(t.as_str())) {
            return line.trim().to_string();
        }
    }
    String::new()
}

/// A section that talks about a name, for help on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Named {
    pub path: String,
    /// The section's heading; none before the first.
    pub heading: Option<String>,
    pub score: u32,
    /// Whether the heading *is* the name: an API reference's entry for it.
    pub named: bool,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Where `term` stands in `text` as a whole word.
fn word_matches<'a>(text: &'a str, term: &'a str) -> impl Iterator<Item = usize> + 'a {
    text.match_indices(term).filter_map(move |(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + term.len()..].chars().next();
        (!before.is_some_and(is_word) && !after.is_some_and(is_word)).then_some(i)
    })
}

/// `text` without Markdown's backslash escapes (`free\_all` is `free_all`).
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match chars.peek() {
            Some(&next) if c == '\\' && next.is_ascii_punctuation() => {
                out.push(next);
                chars.next();
            }
            _ => out.push(c),
        }
    }
    out
}

/// **The sections of `docs` that talk about `term`**, the best first: a
/// heading that is the name (`## Server`, or one that ends in `.Server`),
/// then by score -- a heading that holds it as a word weighs more than
/// code, and code more than prose; one section a document, at most ten.
pub fn talking_about<'a>(
    docs: impl IntoIterator<Item = (&'a str, &'a str)>,
    term: &str,
) -> Vec<Named> {
    let term = term.trim();
    if term.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (path, text) in docs {
        let mut sections: Vec<(Option<String>, u32, bool)> = vec![(None, 0, false)];
        let mut in_fence = false;
        for line in text.lines() {
            if fence(line) {
                in_fence = !in_fence;
                continue;
            }
            let trimmed = line.trim_start();
            if !in_fence && trimmed.starts_with('#') {
                let title = trimmed.trim_start_matches('#').trim().to_string();
                let plain = unescape(&title.replace('`', ""));
                let score = if plain == term || plain.ends_with(&format!(".{term}")) {
                    300
                } else if word_matches(&plain, term).next().is_some() {
                    100
                } else {
                    0
                };
                sections.push((Some(title), score, score >= 300));
                continue;
            }
            let section = sections.last_mut().expect("a first section");
            for i in word_matches(line, term) {
                let in_code = line[..i].matches('`').count() % 2 == 1;
                section.1 += if in_fence || in_code { 10 } else { 1 };
            }
        }
        if let Some((heading, score, named)) = sections
            .into_iter()
            .rev()
            .max_by_key(|(_, score, named)| (*named, *score))
            && score > 0
        {
            hits.push(Named {
                path: path.to_string(),
                heading,
                score,
                named,
            });
        }
    }
    hits.sort_by(|a, b| {
        (b.named, b.score)
            .cmp(&(a.named, a.score))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(10);
    hits
}
