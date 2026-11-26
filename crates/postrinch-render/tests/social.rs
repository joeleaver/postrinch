//! Rendering OFSCP message bodies, hostile ones included: what runs, loads,
//! covers or pretends is gone, whatever the format.

use postrinch_render::social::{Attachment, Body, Options, Rendered, render};

const DOMAIN: &str = "a.example";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn body<'a>(mime: &'a str, text: &'a str, attachments: &'a [Attachment<'a>]) -> Body<'a> {
    Body {
        mime,
        text,
        attachments,
        mentions: None,
        domain: DOMAIN,
    }
}

fn shown(mime: &str, text: &str) -> Rendered {
    render(&body(mime, text, &[]), Options::default())
}

fn picture<'a>(id: &'a str, source: Option<&'a str>) -> Attachment<'a> {
    Attachment {
        id,
        mime: "image/png",
        name: Some("photo.png"),
        picture: source,
    }
}

const DATA_PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

#[test]
fn hostile_html_loses_what_runs_submits_frames_covers_or_styles() {
    let out = shown("text/html", &fixture("11_hostile.html")).html;
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
        "style=",
        "position",
        "<div",
        "class=",
        "id=",
    ] {
        assert!(!lower.contains(gone), "{gone} survived:\n{out}");
    }
}

#[test]
fn html_keeps_only_the_spec_list() {
    let out = shown(
        "text/html",
        r#"<div class="card" style="background: red"><h2 id="t">Title</h2>
           <p><font color="red">red</font> <b>b</b> <u>u</u> <sub>2</sub></p>
           <table><tr><td colspan="2" width="9" bgcolor="red">cell</td></tr></table>
           <details><summary>more</summary>hidden?</details></div>"#,
    )
    .html;
    for kept in [
        "<h2>Title</h2>",
        "<b>b</b>",
        "<u>u</u>",
        "<sub>2</sub>",
        r#"<td colspan="2">cell</td>"#,
    ] {
        assert!(out.contains(kept), "{kept} missing:\n{out}");
    }
    for gone in [
        "<div", "<font", "<details", "<summary", "class=", "style=", "width=", "bgcolor=", "id=",
    ] {
        assert!(!out.contains(gone), "{gone} survived:\n{out}");
    }
    assert!(out.contains("red") && out.contains("hidden?"), "{out}");
}

#[test]
fn what_hides_in_a_removed_element_goes_with_it() {
    let out = shown(
        "text/html",
        "<p>a</p><noscript><p>Enable scripts!</p></noscript><object data=\"x.swf\"><p>fallback</p></object>\
         <iframe>frame</iframe><template><p>template</p></template><embed src=\"x\"><p>b</p>",
    )
    .html;
    assert_eq!(out, "<p>a</p><p>b</p>");
}

#[test]
fn nothing_named_by_address_is_loaded() {
    let html = shown(
        "text/html",
        r#"<p style="background: url(https://track.example/bg.png)">hi</p>
           <img src="https://track.example/open.gif" alt="tracker"><img src="data:image/gif;base64,R0lG">"#,
    );
    assert!(!html.html.contains("track.example"), "{}", html.html);
    assert!(!html.html.contains("<img"), "{}", html.html);
    // A picture that places no attachment is its caption.
    assert!(html.html.contains("tracker"), "{}", html.html);
    assert_eq!(html.pictures_held, 0);

    let md = shown(
        "text/markdown",
        "![tracker](https://track.example/open.gif) ![](http://track.example/x.png)",
    );
    assert!(!md.html.contains("<img"), "{}", md.html);
    assert!(
        md.html
            .contains(r#"<a href="https://track.example/open.gif">tracker</a>"#),
        "{}",
        md.html
    );
}

#[test]
fn links_only_for_the_schemes_the_spec_allows() {
    for (mime, text) in [
        (
            "text/html",
            r#"<a href="javascript:alert(1)">j</a> <a href="data:text/html,x">d</a>
               <a href="/relative">r</a> <a href="//evil.example/">p</a> <a href="file:///etc">f</a>
               <a href="https://ok.example/">ok</a> <a href="mailto:bob@ok.example">mail</a>"#,
        ),
        (
            "text/markdown",
            "[j](javascript:alert(1)) [d](data:text/html,x) [r](/relative) [p](//evil.example/) \
             [f](file:///etc) [ok](https://ok.example/) [mail](mailto:bob@ok.example)",
        ),
    ] {
        let out = shown(mime, text).html;
        let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(out.matches("<a ").count(), 2, "{mime}: {out}");
        assert!(
            out.contains(r#"<a href="https://ok.example/">ok</a>"#),
            "{mime}: {out}"
        );
        assert!(
            out.contains(r#"<a href="mailto:bob@ok.example">mail</a>"#),
            "{mime}: {out}"
        );
        for word in ["j", "d", "r", "p", "f"] {
            assert!(
                out.contains(&format!("{word} ")),
                "{mime}: {word} lost:\n{out}"
            );
        }
    }
}

#[test]
fn raw_html_in_markdown_is_text() {
    let out = shown(
        "text/markdown",
        "<div onclick=\"x()\">\n<script>alert(1)</script>\n</div>\n\nhi <img src=x onerror=alert(1)>",
    )
    .html;
    assert!(
        !out.contains("<script") && !out.contains("<img") && !out.contains("<div"),
        "{out}"
    );
    assert!(
        out.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
        "{out}"
    );
    assert!(out.contains("&lt;img src=x onerror=alert(1)&gt;"), "{out}");
}

#[test]
fn plain_text_is_escaped_and_its_web_addresses_linked() {
    let out = shown(
        "text/plain",
        "<script>alert(1)</script>\r\n  see https://e.example/x and javascript:alert(2)",
    )
    .html;
    assert!(
        out.starts_with("<div style=\"white-space: pre-wrap"),
        "{out}"
    );
    assert!(
        out.contains("&lt;script&gt;alert(1)&lt;/script&gt;\n  see "),
        "{out}"
    );
    assert!(
        out.contains(r#"<a href="https://e.example/x">https://e.example/x</a>"#),
        "{out}"
    );
    assert_eq!(out.matches("<a ").count(), 1, "{out}");
}

#[test]
fn an_unknown_format_is_shown_as_plain_text() {
    let out = shown("application/x-evil", "<b>bold</b>").html;
    assert!(out.contains("&lt;b&gt;bold&lt;/b&gt;"), "{out}");
}

#[test]
fn attachments_are_placed_where_the_body_names_them() {
    let attachments = [
        picture("att_1", Some(DATA_PNG)),
        picture("att_2", Some("https://cdn.b.example/m/abc/photo.png")),
        Attachment {
            id: "att_3",
            mime: "video/mp4",
            name: Some("clip.mp4"),
            picture: None,
        },
        picture("att_4", None),
    ];
    for (mime, text) in [
        (
            "text/html",
            r#"<p><img data-attachment="att_1" alt="one" src="https://track.example/x.gif">
               <img data-attachment="att_2" alt="two"><img data-attachment="att_3">
               <img data-attachment="nope" alt="missing"><a href="attachment:nope">gone</a></p>"#,
        ),
        (
            "text/markdown",
            "![one](attachment:att_1) ![two](attachment:att_2) ![](attachment:att_3) \
             ![missing](attachment:nope) [gone](attachment:nope)",
        ),
    ] {
        let held = render(&body(mime, text, &attachments), Options::default());
        let html = &held.html;
        assert!(
            html.contains(&format!(r#"<img src="{DATA_PNG}" alt="one">"#)),
            "{mime}: {html}"
        );
        assert!(
            !html.contains("track.example") && !html.contains("cdn.b.example"),
            "{mime}: {html}"
        );
        assert!(
            html.contains(r#"<a class="ofscp-attachment" href="attachment:att_2">two</a>"#),
            "{mime}: {html}"
        );
        assert!(
            html.contains(r#"<a class="ofscp-attachment" href="attachment:att_3">clip.mp4</a>"#),
            "{mime}: {html}"
        );
        assert!(
            html.contains("missing") && html.contains("gone"),
            "{mime}: {html}"
        );
        assert!(!html.contains("nope"), "{mime}: {html}");
        assert_eq!(held.placed, ["att_1", "att_2", "att_3"], "{mime}");
        assert_eq!(held.pictures_held, 1, "{mime}");

        let loaded = render(&body(mime, text, &attachments), Options { pictures: true });
        assert!(
            loaded
                .html
                .contains(r#"<img src="https://cdn.b.example/m/abc/photo.png" alt="two">"#),
            "{mime}: {}",
            loaded.html
        );
        assert_eq!(loaded.pictures_held, 0, "{mime}");
    }
}

#[test]
fn an_attachment_id_cannot_reach_out_of_its_selector() {
    let id = r#"x"], img, p[a="#;
    let attachments = [
        picture(id, Some(DATA_PNG)),
        picture("att_2", Some(DATA_PNG)),
    ];
    let text = r#"<p><img data-attachment="x&quot;], img, p[a=" alt="mine"><img data-attachment="other" alt="not"></p>"#;
    let out = render(&body("text/html", text, &attachments), Options::default());
    assert_eq!(out.placed, [id]);
    assert_eq!(out.html.matches("<img").count(), 1, "{}", out.html);
    assert!(out.html.contains("not"), "{}", out.html);
}

#[test]
fn mentions_are_marked_in_text_only() {
    let out = shown(
        "text/html",
        r#"<p>hi @bob and @carol@b.example</p><p><a href="https://e.example/">@dave</a>
           <code>@eve</code></p><p>bob@a.example</p>"#,
    );
    assert_eq!(out.mentions, ["bob@a.example", "carol@b.example"]);
    assert!(
        out.html.contains(
            r#"<span class="ofscp-mention" data-actor="carol@b.example">@carol@b.example</span>"#
        ),
        "{}",
        out.html
    );
    let md = shown(
        "text/markdown",
        "hi @bob, `@eve` and [@dave](https://e.example/)",
    );
    assert_eq!(md.mentions, ["bob@a.example"]);
}

#[test]
fn a_sender_cannot_forge_a_mention_or_an_attachment_link() {
    let out = shown(
        "text/html",
        r#"<p><span class="ofscp-mention" data-actor="admin@a.example">@admin</span> says hi
           <a class="ofscp-attachment" href="https://evil.example/">x</a></p>"#,
    );
    // The text names @admin, so it is marked: by Postrinch, from the text.
    assert_eq!(out.mentions, ["admin@a.example"]);
    assert_eq!(out.html.matches("ofscp-mention").count(), 1, "{}", out.html);
    assert!(!out.html.contains("ofscp-attachment"), "{}", out.html);
}

#[test]
fn the_messages_own_list_decides_who_is_mentioned() {
    let listed = ["carol@b.example".to_owned()];
    let message = Body {
        mentions: Some(&listed),
        ..body("text/plain", "@bob @carol@b.example", &[])
    };
    let out = render(&message, Options::default());
    assert_eq!(out.mentions, ["carol@b.example"]);
    assert_eq!(out.html.matches("ofscp-mention").count(), 1, "{}", out.html);
}

#[test]
fn very_deep_nesting_is_cut_in_every_format() {
    let html = "<blockquote>".repeat(100_000) + "deep" + &"</blockquote>".repeat(100_000);
    let out = shown("text/html", &html).html;
    assert!(out.matches("<blockquote>").count() <= 512);

    let unclosed = "<span>".repeat(200_000) + "x";
    let out = shown("text/html", &unclosed).html;
    assert!(out.matches("<span>").count() <= 512);

    let quotes = ">".repeat(100_000) + " deep";
    let out = shown("text/markdown", &quotes).html;
    assert!(
        out.matches("<blockquote>").count() <= 512,
        "{} levels",
        out.matches("<blockquote>").count()
    );

    // Each level indented two more: 2,000 levels is 4 MB.
    let lists = (0..2_000)
        .map(|i| format!("{}- x\n", "  ".repeat(i)))
        .collect::<String>();
    let out = shown("text/markdown", &lists).html;
    assert!(out.matches("<ul>").count() <= 512);
}

#[test]
fn very_large_bodies_render() {
    let text = "@bob https://e.example/ <b>&amp;</b> ".repeat(100_000);
    for mime in ["text/plain", "text/markdown", "text/html"] {
        let out = shown(mime, &text);
        assert_eq!(out.mentions, ["bob@a.example"], "{mime}");
        assert!(out.html.len() > text.len() / 2, "{mime}");
    }
    let emphasis = "*a ".repeat(100_000) + &"b* ".repeat(100_000);
    assert!(!shown("text/markdown", &emphasis).html.is_empty());
    let empty_links = "[](javascript:x)".repeat(100_000);
    assert!(
        !shown("text/markdown", &empty_links)
            .html
            .contains("javascript")
    );
}

#[test]
fn many_attachments_and_pictures() {
    let ids: Vec<String> = (0..1000).map(|i| format!("att_{i}")).collect();
    let attachments: Vec<Attachment> = ids.iter().map(|id| picture(id, Some(DATA_PNG))).collect();
    let text: String = ids
        .iter()
        .map(|id| format!("<p><img data-attachment=\"{id}\" alt=\"{id}\"></p>"))
        .collect();
    let out = render(&body("text/html", &text, &attachments), Options::default());
    assert_eq!(out.placed.len(), 1000);
    assert_eq!(out.html.matches("<img").count(), 1000);
}
