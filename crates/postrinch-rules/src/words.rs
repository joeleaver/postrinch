//! Words, as rules compare them: Gmail matches whole words, any case, with
//! no stemming ("invoice" misses "invoices"), so Postrinch does the same.

/// The words of `text`, lowercased: runs of letters and digits.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `haystack` holds every one of `wanted`, each as a whole word.
pub(crate) fn has_all(haystack: &[String], wanted: &[String]) -> bool {
    wanted.iter().all(|w| haystack.contains(w))
}

/// Whether `haystack` holds `phrase` as consecutive words.
pub(crate) fn has_phrase(haystack: &[String], phrase: &[String]) -> bool {
    !phrase.is_empty() && haystack.windows(phrase.len()).any(|w| w == phrase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_any_case() {
        let have = words("Your INVOICE, re-sent (No. 42)");
        assert_eq!(have, ["your", "invoice", "re", "sent", "no", "42"]);
        assert!(has_all(&have, &words("invoice 42")));
        assert!(!has_all(&have, &words("invoices")));
        assert!(has_phrase(&have, &words("re-sent")));
        assert!(!has_phrase(&have, &words("sent re")));
        assert!(!has_phrase(&have, &[]));
    }
}
