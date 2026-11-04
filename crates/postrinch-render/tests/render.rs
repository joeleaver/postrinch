//! Rendering real and hostile mail.

use postrinch_render::{Options, render, sanitize};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn hostile_mail_loses_what_runs_submits_frames_or_covers() {
    let out = sanitize(&fixture("11_hostile.html"), &[], Options::default()).html;
    let lower = out.to_ascii_lowercase();
    for gone in [
        "<script",
        "<iframe",
        "<form",
        "<input",
        "onload=",
        "onclick=",
        "onerror=",
        "javascript:",
        "<style",
        "position: fixed",
        "position:fixed",
        "position: absolute",
    ] {
        assert!(!lower.contains(gone), "{gone} survived:\n{out}");
    }
}

#[test]
fn legacy_layout_attributes_stay() {
    let out = sanitize(&fixture("03_receipt_attrs.html"), &[], Options::default());
    assert!(out.html.contains("bgcolor="), "{}", out.html);
    assert!(out.html.contains("cellpadding="), "{}", out.html);
    assert!(out.html.contains("<table"));
}

#[test]
fn pictures_from_the_web_wait_until_asked() {
    let mail = r##"<html><body bgcolor="#f0f0f0" style="margin: 0; position: fixed">
        <img src="https://track.example/open.gif" width="1" height="1">
        <div style="background: url(https://track.example/bg.png); color: red">hi</div>
        <a href="https://shop.example/sale" onclick="steal()">Sale</a>
        </body></html>"##;
    let hidden = sanitize(mail, &[], Options::default());
    assert_eq!(hidden.remote_hidden, 2, "{}", hidden.html);
    assert!(!hidden.html.contains("track.example"), "{}", hidden.html);
    assert!(hidden.html.contains("color: red"));
    assert!(hidden.html.contains(r#"href="https://shop.example/sale""#));
    assert!(hidden.html.contains("noopener"));
    assert!(!hidden.html.contains("onclick"));
    assert_eq!(hidden.body_style, "background-color: #f0f0f0; margin: 0");

    let shown = sanitize(mail, &[], Options { remote: true });
    assert_eq!(shown.remote_hidden, 0);
    assert!(shown.html.contains("https://track.example/open.gif"));
}

#[test]
fn pictures_the_message_carries_are_inlined() {
    let raw = "From: a@example.org\r\nTo: b@example.org\r\nSubject: logo\r\nMIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"b1\"\r\n\r\n\
--b1\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
<p>Hello</p><img src=\"cid:logo@x\" alt=\"Logo\"><img src=\"cid:missing@x\">\r\n\
--b1\r\nContent-Type: image/png\r\nContent-ID: <logo@x>\r\nContent-Transfer-Encoding: base64\r\n\r\n\
iVBORw0KGgo=\r\n--b1--\r\n";
    let out = render(raw.as_bytes(), Options::default()).unwrap();
    assert!(out.from_html);
    assert!(
        out.html
            .contains("src=\"data:image/png;base64,iVBORw0KGgo=\""),
        "{}",
        out.html
    );
    assert!(!out.html.contains("cid:"), "{}", out.html);
}

#[test]
fn text_mail_is_escaped() {
    let raw = "From: a@example.org\r\nSubject: hi\r\n\r\nUse <b> & keep\r\n  spacing\r\n";
    let out = render(raw.as_bytes(), Options::default()).unwrap();
    assert!(!out.from_html);
    assert!(
        out.html.contains("Use &lt;b&gt; &amp; keep"),
        "{}",
        out.html
    );
    assert!(out.html.contains("pre-wrap"));
}
