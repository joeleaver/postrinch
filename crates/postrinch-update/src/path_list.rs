//! A Windows `Path` value as text: entries split by `;`, compared the way
//! Windows finds programs (case aside, a trailing `\` aside, environment
//! variables expanded). Plain text in, plain text out, so it is tested on
//! every system; `win::path` reads and writes the registry around it.

/// Is `dir` one of `path`'s entries? `expand` expands `%VAR%`s the way
/// the system does.
pub(crate) fn has(path: &str, dir: &str, expand: &dyn Fn(&str) -> String) -> bool {
    let want = normal(dir, expand);
    entries(path).any(|entry| normal(entry, expand) == want)
}

/// `path` with `dir` added at the end, keeping every entry as it was
/// written (an unexpanded `%VAR%` stays one).
pub(crate) fn with(path: &str, dir: &str) -> String {
    let kept = path.trim_end_matches(';');
    if kept.trim().is_empty() {
        dir.to_owned()
    } else {
        format!("{kept};{dir}")
    }
}

/// `path` without the entries that are `dir`, the rest as they were.
pub(crate) fn without(path: &str, dir: &str, expand: &dyn Fn(&str) -> String) -> String {
    let want = normal(dir, expand);
    path.split(';')
        .filter(|entry| entry.trim().is_empty() || normal(entry, expand) != want)
        .collect::<Vec<_>>()
        .join(";")
        .trim_end_matches(';')
        .to_owned()
}

fn entries(path: &str) -> impl Iterator<Item = &str> {
    path.split(';').filter(|entry| !entry.trim().is_empty())
}

fn normal(entry: &str, expand: &dyn Fn(&str) -> String) -> String {
    let entry = entry.trim().trim_matches('"');
    let expanded = if entry.contains('%') {
        expand(entry)
    } else {
        entry.to_owned()
    };
    expanded
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIR: &str = r"C:\Users\a\AppData\Local\Programs\Postrinch";

    fn expand(s: &str) -> String {
        s.replace("%LOCALAPPDATA%", r"C:\Users\a\AppData\Local")
    }

    #[test]
    fn an_entry_is_found_however_it_was_written() {
        assert!(has(&format!(r"C:\Windows;{DIR}"), DIR, &expand));
        assert!(has(
            r"c:\users\A\appdata\local\programs\postrinch\;C:\x",
            DIR,
            &expand
        ));
        assert!(has(r#""%LOCALAPPDATA%\Programs\Postrinch""#, DIR, &expand));
        assert!(has(
            "C:/Users/a/AppData/Local/Programs/Postrinch",
            DIR,
            &expand
        ));
        assert!(!has(
            r"C:\Windows;C:\Users\a\AppData\Local\Programs",
            DIR,
            &expand
        ));
        assert!(!has("", DIR, &expand));
    }

    #[test]
    fn adding_appends_one_entry() {
        assert_eq!(with("", DIR), DIR);
        assert_eq!(with(r"C:\x;", DIR), format!(r"C:\x;{DIR}"));
        assert_eq!(
            with(r"%USERPROFILE%\bin", DIR),
            format!(r"%USERPROFILE%\bin;{DIR}")
        );
    }

    #[test]
    fn removing_takes_only_that_entry() {
        let path = format!(r"C:\x;;{DIR}\;%USERPROFILE%\bin;{DIR}");
        assert_eq!(without(&path, DIR, &expand), r"C:\x;;%USERPROFILE%\bin");
        assert_eq!(without(DIR, DIR, &expand), "");
        assert_eq!(without(r"C:\x", DIR, &expand), r"C:\x");
    }
}
