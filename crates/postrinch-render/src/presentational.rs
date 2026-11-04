//! Mail's legacy presentational attributes as inline CSS. HTML email still
//! lays itself out with `bgcolor`, `width`, `align`, `valign`,
//! `cellpadding`, `border` and `<font>`; rinch maps none of them yet
//! (rinch#1419), while it does draw the CSS they stand for. A browser treats
//! them as hints below the author's own styles, so each becomes a
//! declaration put *before* the element's own `style`, which still wins.
//!
//! Runs on the sanitizer's output, and lets through only values that read as
//! what they should be (a number, a colour, a keyword, a font name), so
//! nothing can be smuggled into a style this way.

use std::cell::RefCell;
use std::rc::Rc;

use lol_html::html_content::Element;
use lol_html::{RewriteStrSettings, element, rewrite_str};

/// The table attributes that reach its cells.
#[derive(Debug, Clone, Copy, Default)]
struct TableHints {
    padding: Option<u32>,
    border: Option<u32>,
}

/// `html` with presentational attributes turned into inline styles.
pub fn to_css(html: &str) -> String {
    // The open tables' hints, innermost last, for their cells.
    let tables: Rc<RefCell<Vec<TableHints>>> = Rc::new(RefCell::new(Vec::new()));
    let (opened, for_cells) = (tables.clone(), tables.clone());
    let mut settings = RewriteStrSettings::new();
    settings = settings.append_element_content_handler(element!("table", move |el| {
        let hints = TableHints {
            padding: el.get_attribute("cellpadding").and_then(|v| pixels(&v)),
            border: el.get_attribute("border").and_then(|v| pixels(&v)),
        };
        let mut css = Vec::new();
        common(el, &mut css);
        if let Some(n) = hints.border.filter(|n| *n > 0) {
            css.push(format!("border: {n}px outset #808080"));
        }
        if let Some(n) = el.get_attribute("cellspacing").and_then(|v| pixels(&v)) {
            css.push(format!("border-spacing: {n}px"));
        }
        if el
            .get_attribute("align")
            .is_some_and(|a| a.eq_ignore_ascii_case("center"))
        {
            css.push("margin-left: auto; margin-right: auto".into());
        }
        for name in ["cellpadding", "cellspacing", "border", "align"] {
            el.remove_attribute(name);
        }
        prepend(el, &css);
        opened.borrow_mut().push(hints);
        let closed = opened.clone();
        if let Some(handlers) = el.end_tag_handlers() {
            handlers.push(Box::new(move |_end| {
                closed.borrow_mut().pop();
                Ok(())
            }));
        }
        Ok(())
    }));
    settings = settings.append_element_content_handler(element!("td, th", move |el| {
        let mut css = Vec::new();
        let table = for_cells.borrow().last().copied().unwrap_or_default();
        if let Some(n) = table.padding {
            css.push(format!("padding: {n}px"));
        }
        if table.border.is_some_and(|n| n > 0) {
            css.push("border: 1px inset #808080".into());
        }
        common(el, &mut css);
        aligned(el, &mut css);
        prepend(el, &css);
        Ok(())
    }));
    settings = settings.append_element_content_handler(element!("tr", |el| {
        let mut css = Vec::new();
        common(el, &mut css);
        aligned(el, &mut css);
        prepend(el, &css);
        Ok(())
    }));
    settings =
        settings.append_element_content_handler(element!("div, p, h1, h2, h3, h4, h5, h6", |el| {
            let mut css = Vec::new();
            if let Some(color) = el.get_attribute("bgcolor").and_then(|v| colour(&v)) {
                css.push(format!("background-color: {color}"));
                el.remove_attribute("bgcolor");
            }
            if let Some(align) = el.get_attribute("align").and_then(|v| keyword(&v, ALIGN)) {
                css.push(format!("text-align: {align}"));
                el.remove_attribute("align");
            }
            prepend(el, &css);
            Ok(())
        }));
    settings = settings.append_element_content_handler(element!("center", |el| {
        el.set_tag_name("div")?;
        prepend(el, &["text-align: center".to_owned()]);
        Ok(())
    }));
    settings = settings.append_element_content_handler(element!("font", |el| {
        let mut css = Vec::new();
        if let Some(color) = el.get_attribute("color").and_then(|v| colour(&v)) {
            css.push(format!("color: {color}"));
        }
        if let Some(face) = el.get_attribute("face").and_then(|v| font_names(&v)) {
            css.push(format!("font-family: {face}"));
        }
        if let Some(size) = el.get_attribute("size").and_then(|v| font_size(&v)) {
            css.push(format!("font-size: {size}"));
        }
        for name in ["color", "face", "size"] {
            el.remove_attribute(name);
        }
        el.set_tag_name("span")?;
        prepend(el, &css);
        Ok(())
    }));
    rewrite_str(html, settings).unwrap_or_else(|_| html.to_owned())
}

const ALIGN: &[&str] = &["left", "right", "center", "justify"];
const VALIGN: &[&str] = &["top", "middle", "bottom", "baseline"];

/// `bgcolor`, `width` and `height`, which tables, rows and cells share.
fn common(el: &mut Element, css: &mut Vec<String>) {
    if let Some(color) = el.get_attribute("bgcolor").and_then(|v| colour(&v)) {
        css.push(format!("background-color: {color}"));
    }
    for name in ["width", "height"] {
        if let Some(length) = el.get_attribute(name).and_then(|v| length(&v)) {
            css.push(format!("{name}: {length}"));
        }
    }
    for name in ["bgcolor", "width", "height"] {
        el.remove_attribute(name);
    }
}

/// A row's or cell's `align` and `valign`.
fn aligned(el: &mut Element, css: &mut Vec<String>) {
    if let Some(align) = el.get_attribute("align").and_then(|v| keyword(&v, ALIGN)) {
        css.push(format!("text-align: {align}"));
    }
    if let Some(valign) = el.get_attribute("valign").and_then(|v| keyword(&v, VALIGN)) {
        css.push(format!("vertical-align: {valign}"));
    }
    if el.has_attribute("nowrap") {
        css.push("white-space: nowrap".into());
    }
    for name in ["align", "valign", "nowrap"] {
        el.remove_attribute(name);
    }
}

/// Put `css` before the element's own style.
fn prepend(el: &mut Element, css: &[String]) {
    if css.is_empty() {
        return;
    }
    let own = el.get_attribute("style").unwrap_or_default();
    let mut style = css.join("; ");
    if !own.trim().is_empty() {
        style.push_str("; ");
        style.push_str(own.trim());
    }
    // A failure here only leaves the attribute as it was.
    let _ = el.set_attribute("style", &style);
}

/// A whole number of pixels: `20`, `20px`.
fn pixels(value: &str) -> Option<u32> {
    let v = value.trim().trim_end_matches("px");
    v.parse::<u32>().ok().filter(|n| *n <= 10_000)
}

/// `600` as `600px`, `100%` as itself.
fn length(value: &str) -> Option<String> {
    let v = value.trim();
    if let Some(percent) = v.strip_suffix('%') {
        let n: f32 = percent.trim().parse().ok()?;
        return (0.0..=100.0).contains(&n).then(|| format!("{n}%"));
    }
    pixels(v).map(|n| format!("{n}px"))
}

/// A colour: `#rgb`, `#rrggbb`, the same without `#`, or a name.
fn colour(value: &str) -> Option<String> {
    let v = value.trim();
    let hex = v.strip_prefix('#').unwrap_or(v);
    if matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Some(format!("#{hex}"));
    }
    (!v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| v.to_ascii_lowercase())
}

/// One of `allowed`, any case.
fn keyword(value: &str, allowed: &[&str]) -> Option<&'static str> {
    let v = value.trim().to_ascii_lowercase();
    ALIGN
        .iter()
        .chain(VALIGN)
        .find(|k| **k == v && allowed.contains(k))
        .copied()
}

/// `<font face>`: font names, each quoted.
fn font_names(value: &str) -> Option<String> {
    let names: Vec<String> = value
        .split(',')
        .map(|n| n.trim().trim_matches(['"', '\'']).trim())
        .filter(|n| !n.is_empty())
        .filter(|n| {
            n.chars()
                .all(|c| c.is_alphanumeric() || c == ' ' || c == '-')
        })
        .map(|n| format!("'{n}'"))
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// `<font size>`: 1 to 7, or relative to 3 (`+1`, `-2`).
fn font_size(value: &str) -> Option<&'static str> {
    let v = value.trim();
    let n: i32 = match v.strip_prefix('+') {
        Some(up) => 3 + up.parse::<i32>().ok()?,
        None if v.starts_with('-') => 3 + v.parse::<i32>().ok()?,
        None => v.parse().ok()?,
    };
    Some(match n.clamp(1, 7) {
        1 => "x-small",
        2 => "small",
        3 => "medium",
        4 => "large",
        5 => "x-large",
        6 => "xx-large",
        _ => "xxx-large",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_attributes_become_styles_before_the_authors() {
        let html = r##"<table width="600" bgcolor="FAF8F5" cellpadding="10" align="center"><tbody><tr><td bgcolor="#384124" valign="top" align="left" style="padding: 2px">x</td></tr></tbody></table>"##;
        let out = to_css(html);
        assert!(out.contains(r##"<table style="background-color: #FAF8F5; width: 600px; margin-left: auto; margin-right: auto">"##), "{out}");
        assert!(out.contains(r##"<td style="padding: 10px; background-color: #384124; text-align: left; vertical-align: top; padding: 2px">"##), "{out}");
    }

    #[test]
    fn nested_tables_keep_their_own_padding() {
        let html = r#"<table cellpadding="8"><tr><td><table><tr><td>in</td></tr></table></td><td>out</td></tr></table>"#;
        let out = to_css(html);
        assert_eq!(out.matches("padding: 8px").count(), 2, "{out}");
    }

    #[test]
    fn values_that_do_not_read_are_left_out() {
        let out = to_css(r#"<td bgcolor="red; position: fixed" width="100vh" align="evil">x</td>"#);
        assert!(!out.contains("position"), "{out}");
        assert!(!out.contains("style="), "{out}");
    }

    #[test]
    fn font_and_center_become_spans_and_divs() {
        let out = to_css(
            r##"<center><font color="#ff0000" face="Arial, 'Helvetica Neue'" size="+1">x</font></center>"##,
        );
        assert_eq!(
            out,
            r##"<div style="text-align: center"><span style="color: #ff0000; font-family: 'Arial', 'Helvetica Neue'; font-size: large">x</span></div>"##
        );
    }
}
