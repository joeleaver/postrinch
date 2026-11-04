//! The allow-list: ammonia (html5ever underneath) keeps the tags and
//! attributes mail lays itself out with, and drops everything that runs,
//! submits, frames or styles the page beyond its own elements. Inline
//! styles and picture sources pass through the filters below.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Options, Rendered};

/// Tags kept beyond ammonia's defaults: the legacy ones mail still uses.
const EXTRA_TAGS: &[&str] = &["font", "center", "big", "strike"];
/// Attributes kept on any tag: layout and the legacy presentational ones
/// rinch understands (`bgcolor`, `align`, `cellpadding`…).
const LAYOUT_ATTRIBUTES: &[&str] = &[
    "style",
    "align",
    "valign",
    "bgcolor",
    "width",
    "height",
    "border",
    "cellpadding",
    "cellspacing",
    "color",
    "face",
    "size",
    "dir",
    "colspan",
    "rowspan",
    "nowrap",
];
/// Inline style properties dropped whatever their value.
const DROPPED_PROPERTIES: &[&str] = &["behavior", "-moz-binding", "cursor", "pointer-events"];

/// Sanitize an HTML document (or fragment) into the inside of its body.
/// `inline` maps Content-IDs to `data:` URLs for the pictures the message
/// carries.
pub fn sanitize(html: &str, inline: &[(String, String)], options: Options) -> Rendered {
    let hidden = Arc::new(AtomicUsize::new(0));
    let inline: Arc<HashMap<String, String>> = Arc::new(inline.iter().cloned().collect());
    let remote = options.remote;

    let mut builder = ammonia::Builder::default();
    builder
        .add_tags(EXTRA_TAGS)
        .add_generic_attributes(LAYOUT_ATTRIBUTES)
        .add_clean_content_tags(["title"])
        .url_schemes(HashSet::from(["http", "https", "mailto", "data", "cid"]))
        .link_rel(Some("noopener noreferrer"));
    let counted = hidden.clone();
    builder.attribute_filter(
        move |element, attribute, value| match (element, attribute) {
            (_, "style") => Some(clean_style(value, remote, &counted).into()),
            ("img", "src") => picture(value, remote, &inline, &counted).map(Into::into),
            _ => Some(value.into()),
        },
    );
    let body_style = body_style(body_tag(html).unwrap_or_default(), remote, &hidden);
    let html = builder.clean(html).to_string();
    Rendered {
        html,
        body_style,
        from_html: true,
        remote_hidden: hidden.load(Ordering::Relaxed),
    }
}

/// A picture's source: the message's own (`cid:`), inline data, or the
/// web's when the person asked for it.
fn picture(
    src: &str,
    remote: bool,
    inline: &HashMap<String, String>,
    hidden: &AtomicUsize,
) -> Option<String> {
    let lower = src.trim().to_ascii_lowercase();
    if let Some(id) = src
        .trim()
        .strip_prefix("cid:")
        .or_else(|| src.trim().strip_prefix("CID:"))
    {
        return inline.get(id).cloned();
    }
    if lower.starts_with("data:image/") {
        return Some(src.to_owned());
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        if remote {
            return Some(src.to_owned());
        }
        hidden.fetch_add(1, Ordering::Relaxed);
    }
    None
}

/// An inline style without what could cover the window, run code or fetch
/// from the web unasked.
fn clean_style(style: &str, remote: bool, hidden: &AtomicUsize) -> String {
    let mut kept = Vec::new();
    for declaration in style.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim();
        let lower = value.to_ascii_lowercase();
        if DROPPED_PROPERTIES.contains(&property.as_str())
            || lower.contains("expression(")
            || lower.contains("javascript:")
        {
            continue;
        }
        // Mail cannot lift itself out of its own box onto the window.
        if property == "position" && !matches!(lower.as_str(), "static" | "relative") {
            continue;
        }
        if property == "z-index" {
            continue;
        }
        if lower.contains("url(") {
            let web = lower.contains("http:") || lower.contains("https:") || lower.contains("//");
            if web && !remote {
                hidden.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            if !web && !lower.contains("data:image/") {
                continue;
            }
        }
        kept.push(format!("{property}: {value}"));
    }
    kept.join("; ")
}

/// The mail's `<body ...>` start tag, from its source: ammonia keeps what is
/// inside the body, not the body's own attributes.
fn body_tag(source: &str) -> Option<&str> {
    let lower = source.to_ascii_lowercase();
    let start = lower.find("<body")?;
    let end = lower[start..].find('>')? + start;
    Some(&source[start..end])
}

/// The style for the element that stands for the mail's body.
fn body_style(tag: &str, remote: bool, hidden: &AtomicUsize) -> String {
    let style = attribute(tag, "style").unwrap_or_default();
    let mut out = clean_style(&style, remote, hidden);
    if let Some(color) = attribute(tag, "bgcolor") {
        let color: String = color
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '#')
            .collect();
        if !color.is_empty() {
            if !out.is_empty() {
                out.push_str("; ");
            }
            out = format!("background-color: {color}; {out}");
        }
    }
    out.trim_end_matches("; ").to_owned()
}

/// One attribute's value in a start tag's text.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        let before = lower[..start].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let rest = tag[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        return Some(match rest.chars().next() {
            Some(q @ ('"' | '\'')) => rest[1..].split(q).next().unwrap_or_default().to_owned(),
            _ => rest.split([' ', '>']).next().unwrap_or_default().to_owned(),
        });
    }
    None
}
