//! Spellcheck: rinch marks misspelled words in the fields people write in
//! and offers suggestions and "Add to dictionary" on a right press. Which
//! fields are checked is each field's `spellcheck` (see DESIGN.md
//! "Spellcheck"); this module keeps the user's own words.
//!
//! The words added with "Add to dictionary" live in `dictionary.txt` in
//! Postrinch's config directory, one word per line, saved after each change.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use rinch::spellcheck::{PersonalDictionary, SpellcheckConfig};

/// The file in the config directory that holds the user's own words.
const FILE: &str = "dictionary.txt";

/// Where the user's words are kept: `<config dir>/postrinch/dictionary.txt`.
fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("postrinch").join(FILE))
}

/// The spellcheck settings for the window: the system's dictionaries for
/// the user's languages, and the user's own words, loaded from and saved
/// to the config directory. With no config directory the words last only
/// as long as the window.
pub fn config() -> SpellcheckConfig {
    let dictionary = match path() {
        Some(path) => personal_dictionary(path),
        None => Arc::new(PersonalDictionary::new()),
    };
    SpellcheckConfig::new().personal_dictionary(dictionary)
}

/// The words saved at `path`, in a dictionary that writes them back there
/// whenever one is added or removed.
fn personal_dictionary(path: PathBuf) -> Arc<PersonalDictionary> {
    let dictionary = Arc::new(PersonalDictionary::with_words(load(&path)));
    // Weak, so the dictionary does not own itself through its listener.
    let weak: Weak<PersonalDictionary> = Arc::downgrade(&dictionary);
    dictionary.add_listener(move |_| {
        let Some(dictionary) = weak.upgrade() else {
            return;
        };
        if let Err(e) = save(&path, &dictionary.words()) {
            tracing::warn!("cannot save the dictionary to {}: {e}", path.display());
        }
    });
    dictionary
}

/// The words in the file at `path`: one per line, blank lines and
/// surrounding spaces ignored. Empty when there is no file or it cannot be
/// read.
fn load(path: &Path) -> Vec<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            tracing::warn!("cannot read the dictionary at {}: {e}", path.display());
            Vec::new()
        }
    }
}

fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Writes `words` to `path`, one per line, through a file beside it that
/// replaces the old one whole, so a crash never leaves half a list.
fn save(path: &Path, words: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = words.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let partial = path.with_extension("txt.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_an_empty_dictionary() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(&dir.path().join(FILE)).is_empty());
    }

    #[test]
    fn lines_are_words_and_blanks_are_skipped() {
        assert_eq!(
            parse("Postrinch\n\n  rinch \r\nGmail"),
            ["Postrinch", "rinch", "Gmail"]
        );
    }

    #[test]
    fn an_added_word_is_saved_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("postrinch").join(FILE);
        let dictionary = personal_dictionary(path.clone());
        assert!(dictionary.add("Postrinch"));
        assert!(dictionary.add("rhypedb"));
        assert_eq!(load(&path), ["Postrinch", "rhypedb"]);

        let again = personal_dictionary(path.clone());
        assert!(again.contains("Postrinch") && again.contains("RHYPEDB"));
    }

    #[test]
    fn a_removed_word_leaves_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        save(&path, &["Postrinch".into(), "rinch".into()]).unwrap();
        let dictionary = personal_dictionary(path.clone());
        assert!(dictionary.remove("rinch"));
        assert_eq!(load(&path), ["Postrinch"]);
        assert!(!path.with_extension("txt.partial").exists());
    }
}
