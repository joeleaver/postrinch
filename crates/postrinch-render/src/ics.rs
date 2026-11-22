//! iCalendar (RFC 5545) text as components of properties: lines unfolded,
//! parameters split out, quoted values kept whole. What the properties mean
//! is read elsewhere (invitation.rs).

/// One `NAME;PARAM=value:value` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Property {
    /// Uppercase.
    pub name: String,
    /// (uppercase name, value without quotes).
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Property {
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The value as text, with its escapes undone (`\n`, `\,`, `\;`, `\\`).
    pub fn text(&self) -> String {
        unescape(&self.value)
    }
}

/// A `BEGIN:NAME` … `END:NAME` block: its own properties and the blocks
/// inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Component {
    /// Uppercase.
    pub name: String,
    pub properties: Vec<Property>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn get(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|p| p.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> {
        self.properties.iter().filter(move |p| p.name == name)
    }

    pub fn children<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Component> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// The calendar objects in `text` (usually one `VCALENDAR`). Lines that
/// are not properties are skipped; a block left open ends with the text.
pub(crate) fn parse(text: &str) -> Vec<Component> {
    let mut done = Vec::new();
    let mut open: Vec<Component> = Vec::new();
    for line in unfold(text) {
        let Some(property) = property(&line) else {
            continue;
        };
        match property.name.as_str() {
            "BEGIN" => open.push(Component {
                name: property.value.trim().to_ascii_uppercase(),
                ..Default::default()
            }),
            "END" => {
                if let Some(component) = open.pop() {
                    match open.last_mut() {
                        Some(parent) => parent.children.push(component),
                        None => done.push(component),
                    }
                }
            }
            _ => {
                if let Some(component) = open.last_mut() {
                    component.properties.push(property);
                }
            }
        }
    }
    while let Some(component) = open.pop() {
        match open.last_mut() {
            Some(parent) => parent.children.push(component),
            None => done.push(component),
        }
    }
    done
}

/// Lines with their folds joined: a line that starts with a space or a
/// tab continues the one before, less that one character.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ if raw.is_empty() => {}
            _ => lines.push(raw.to_owned()),
        }
    }
    lines
}

/// One unfolded line as a property; `None` when it has no name or colon.
fn property(line: &str) -> Option<Property> {
    // The name and parameters end at the first colon outside quotes.
    let mut quoted = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let mut parts = split_unquoted(head, ';').into_iter();
    let name = parts.next()?.trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let params = parts
        .filter_map(|p| {
            let (n, v) = p.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(v);
            Some((n.trim().to_ascii_uppercase(), v.to_owned()))
        })
        .collect();
    Some(Property {
        name,
        params,
        value: value.to_owned(),
    })
}

/// `component` as iCalendar text: CRLF lines folded at 75 octets,
/// parameter values quoted where they hold `:`, `;` or `,`.
pub(crate) fn write(component: &Component) -> String {
    let mut out = String::new();
    write_into(component, &mut out);
    out
}

fn write_into(component: &Component, out: &mut String) {
    fold(&format!("BEGIN:{}", component.name), out);
    for p in &component.properties {
        let mut line = p.name.clone();
        for (name, value) in &p.params {
            let quoted = value.contains([':', ';', ',']);
            if quoted {
                line.push_str(&format!(";{name}=\"{value}\""));
            } else {
                line.push_str(&format!(";{name}={value}"));
            }
        }
        line.push(':');
        line.push_str(&p.value);
        fold(&line, out);
    }
    for child in &component.children {
        write_into(child, out);
    }
    fold(&format!("END:{}", component.name), out);
}

/// One content line, folded so no line passes 75 octets.
fn fold(line: &str, out: &mut String) {
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out.push_str("\r\n");
}

fn split_unquoted(text: &str, at: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut quoted, mut start) = (false, 0);
    for (i, c) in text.char_indices() {
        if c == '"' {
            quoted = !quoted;
        } else if c == at && !quoted {
            parts.push(&text[start..i]);
            start = i + c.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_folded_lines_parameters_and_nesting() {
        let text = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n\
                    SUMMARY:Lunch\\, then a\r\n  walk\r\n\
                    ORGANIZER;CN=\"Ada; L\":mailto:ada@example.org\r\n\
                    BEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM\r\n\
                    END:VEVENT\r\nEND:VCALENDAR\r\n";
        let all = parse(text);
        assert_eq!(all.len(), 1);
        let cal = &all[0];
        assert_eq!(cal.get("METHOD").unwrap().value, "REQUEST");
        let event = cal.children("VEVENT").next().unwrap();
        assert_eq!(event.get("SUMMARY").unwrap().text(), "Lunch, then a walk");
        let organizer = event.get("ORGANIZER").unwrap();
        assert_eq!(organizer.param("CN"), Some("Ada; L"));
        assert_eq!(organizer.value, "mailto:ada@example.org");
        assert_eq!(event.children("VALARM").count(), 1);
    }

    #[test]
    fn writes_what_it_reads() {
        let text = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nORGANIZER;CN=\"Ada; L\":mailto:ada@example.org\r\n\
                    SUMMARY:x\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let back = write(&parse(text)[0]);
        assert_eq!(back, text);
        let long = Component {
            name: "VEVENT".into(),
            properties: vec![Property {
                name: "SUMMARY".into(),
                params: Vec::new(),
                value: "é".repeat(60),
            }],
            children: Vec::new(),
        };
        let written = write(&long);
        assert!(written.split("\r\n").all(|l| l.len() <= 75));
        assert_eq!(parse(&written)[0], long);
    }

    #[test]
    fn unescapes_newlines_and_backslashes() {
        assert_eq!(unescape("a\\nb\\\\c\\;d"), "a\nb\\c;d");
    }

    #[test]
    fn a_block_left_open_still_counts() {
        let all = parse("BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\n");
        assert_eq!(
            all[0]
                .children("VEVENT")
                .next()
                .unwrap()
                .get("UID")
                .unwrap()
                .value,
            "x"
        );
    }
}
