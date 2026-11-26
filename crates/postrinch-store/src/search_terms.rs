//! The words of a search as full-text search reads them, and as a mail
//! server's substring search can ask for them without missing any.
//!
//! Full-text search (`MessageContent.search_text` in `schema.rhype`) reads
//! text with rhypedb's `english` analyzer: words split at punctuation
//! (`invoice-4471` is `invoice` and `4471`), folded (case, diacritics),
//! stop words dropped, and each word stemmed (Porter2: `shipping`,
//! `shipped` and `ships` are all `ship`). [`search_tokens`] gives those
//! terms, so a query can be checked term by term.
//!
//! An IMAP server's `TEXT` key matches a substring instead. To find at
//! least what full-text search finds, a term is asked for by text that
//! every word with that stem contains ([`server_texts`]): the stem, less
//! the endings the stemmer can write in place of a word's own (a `y`
//! made `i`, an `e` added, `bility` made `ble`), and the few words
//! Porter2 stems by a list rather than by rule (`dying` is `die`).

use rhypedb_engine::fulltext::Analyzer;
use unicode_segmentation::UnicodeSegmentation;

/// The analyzer of `search_text` (`@fulltext(analyzer: "english")`).
const ANALYZER: Analyzer = Analyzer::English;

/// Shortest text worth asking a server for: one letter is in nearly every
/// message, so asking for it narrows nothing.
const SHORTEST: usize = 2;

/// Words Porter2 stems by its list of exceptions, by the stem they get,
/// that do not contain what [`server_text`] asks for that stem.
const LISTED: &[(&str, &str)] = &[
    ("die", "dying"),
    ("lie", "lying"),
    ("tie", "tying"),
    ("sky", "skies"),
];

/// One term of a search's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchToken {
    /// The term as full-text search holds it (`ship`).
    pub term: String,
    /// The word as it was written, lowercased (`shipping`).
    pub word: String,
}

/// The terms full-text search reads in `text`, in order: stop words and
/// punctuation are not terms.
pub fn search_tokens(text: &str) -> Vec<SearchToken> {
    let words: Vec<&str> = text.unicode_words().collect();
    ANALYZER
        .analyze(text)
        .into_iter()
        .filter_map(|token| {
            let word = words.get(token.position as usize)?;
            Some(SearchToken {
                term: token.term,
                word: word.to_lowercase(),
            })
        })
        .collect()
}

/// Texts to ask a server for so that every message full-text search finds
/// by `token` is found (a server's `TEXT` matches substrings, without case
/// in ASCII): a message with a word of the token's stem contains one of
/// them. Empty when nothing short of every message would do (the stem is
/// a letter, or the word as typed shares less with it).
///
/// Diacritics are the server's to match: full-text search folds them
/// (`cafe` finds `café`), and a server may not. What is asked keeps to
/// the letters typed, so mail written as the word was typed is found.
pub fn server_texts(token: &SearchToken) -> Vec<String> {
    let mut texts = Vec::new();
    let mut every = false;
    let mut ask = |text: String| {
        if text.chars().count() < SHORTEST {
            every = true;
        } else if !texts.contains(&text) {
            texts.push(text);
        }
    };
    let typed_as_folded = token.word.is_ascii();
    let stem = server_text(&token.term);
    // A word typed with diacritics: only what it shares with its folded
    // stem is surely in mail written as it was typed.
    ask(if typed_as_folded {
        stem.to_owned()
    } else {
        common_prefix(stem, &token.word).to_owned()
    });
    for (listed_stem, word) in LISTED {
        if *listed_stem == token.term {
            ask((*word).to_owned());
        }
    }
    if every { Vec::new() } else { texts }
}

/// What every word stemmed to `stem` contains: the stem without the
/// endings Porter2 writes rather than keeps. Its step 1c makes a final
/// `y` an `i` (`happy` is `happi`, but so is `happiness`), steps 1a and
/// 1b add an `e` (`hoping` is `hope`), and step 2 makes `bility` `ble`,
/// whose `e` step 5 then drops (`probability`, `probably` and `probable`
/// are all `probabl`). The other endings it writes (`ate`, `ize`, `ence`,
/// `ive`, `al`, `ic`, `ous`, `ful`, `less`, `og`, `tion`) are left of
/// letters the word had, so they end in at most an `e` it may not have.
fn server_text(stem: &str) -> &str {
    let stem = stem
        .strip_suffix('e')
        .or_else(|| stem.strip_suffix('i'))
        .unwrap_or(stem);
    match stem.strip_suffix("bl") {
        Some(rest) => &stem[..rest.len() + 1],
        None => stem,
    }
}

/// The longest start `a` and `b` share.
fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let end = a
        .char_indices()
        .zip(b.chars())
        .find(|((_, x), y)| x != y)
        .map_or(a.len().min(b.len()), |((i, _), _)| i);
    &a[..end]
}

#[cfg(test)]
#[path = "search_terms_tests.rs"]
mod tests;
