//! The terms full-text search reads, and what a server is asked for so
//! that it finds every message they find.

use super::*;

fn token(word: &str) -> SearchToken {
    let mut tokens = search_tokens(word);
    assert_eq!(tokens.len(), 1, "{word}: {tokens:?}");
    tokens.remove(0)
}

fn terms(text: &str) -> Vec<String> {
    search_tokens(text).into_iter().map(|t| t.term).collect()
}

#[test]
fn the_terms_are_full_text_searchs() {
    // The schema's analyzer is the one read here.
    assert!(
        include_str!("schema.rhype")
            .contains("search_text: String @fulltext(analyzer: \"english\")")
    );
    assert_eq!(terms("invoice-4471"), ["invoic", "4471"]);
    assert_eq!(terms("Shipping"), ["ship"]);
    // Stop words and punctuation are not terms.
    assert_eq!(terms("state of the art!"), ["state", "art"]);
    assert!(terms("of the").is_empty());
    let shipping = token("Shipping");
    assert_eq!(shipping.word, "shipping");
}

/// Words that share a stem: whichever is typed, what the server is asked
/// for is in every one of them.
const FAMILIES: &[&[&str]] = &[
    &["ship", "ships", "shipped", "shipping"],
    &["happy", "happiness"],
    &["hope", "hoping", "hoped", "hopes"],
    &["probable", "probably", "probability"],
    &["die", "dies", "died", "dying"],
    &["lie", "lies", "lying"],
    &["tie", "ties", "tied", "tying"],
    &["sky", "skies"],
    &["cry", "cries", "cried"],
    &["agree", "agreed", "agreeing"],
    &["relate", "related", "relating"],
    &["sensitive", "sensitivity"],
    &["apology", "apologies"],
    &["invoice", "invoices", "invoiced"],
    &["camera", "cameras", "camera's"],
    &["formal", "formality"],
    &["connect", "connected", "connection", "connections"],
];

#[test]
fn every_word_of_a_stem_has_what_is_asked() {
    let mut wrong = Vec::new();
    for family in FAMILIES {
        let stem = token(family[0]).term;
        for typed in *family {
            let typed = token(typed);
            if typed.term != stem {
                wrong.push(format!("{} is {}, not {stem}", typed.word, typed.term));
                continue;
            }
            let texts = server_texts(&typed);
            for word in *family {
                if !texts.iter().any(|t| word.contains(t.as_str())) {
                    wrong.push(format!("{word} has none of {texts:?} ({})", typed.word));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn what_is_asked_is_the_stem_where_every_word_has_it() {
    assert_eq!(server_texts(&token("shipping")), ["ship"]);
    assert_eq!(server_texts(&token("thursday")), ["thursday"]);
    assert_eq!(server_texts(&token("4471")), ["4471"]);
    assert_eq!(server_texts(&token("happiness")), ["happ"]);
    assert_eq!(server_texts(&token("dying")), ["di", "dying"]);
}

#[test]
fn diacritics_typed_keep_to_the_letters_typed() {
    // Full-text search folds them; what is asked is in mail written as
    // typed, whether or not the server folds.
    assert_eq!(server_texts(&token("café")), ["caf"]);
    let texts = server_texts(&token("Grüße"));
    assert_eq!(texts, ["gr"]);
    // A word with no Latin letters is asked as typed.
    assert_eq!(server_texts(&token("привет")), ["привет"]);
}

#[test]
fn a_letter_asks_for_nothing() {
    // `x` is a term, but one letter is in nearly every message.
    assert!(server_texts(&token("x")).is_empty());
}
