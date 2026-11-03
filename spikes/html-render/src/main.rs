//! Renders each fixture through rinch's software painter to out/<mode>/<name>.png.
//! Modes: raw = fixtures/ as-is, what rinch does today; norm = out/normalized/ from normalize.py.
use rinch::app::RinchApp;
use rinch::prelude::*;
use rinch_core::dom::DomDocument;
use std::path::Path;
use std::time::Instant;

const W: u32 = 700;
const H: u32 = 1100;

#[component]
fn shell() -> NodeHandle {
    rsx! { div { id: "mail" } }
}

/// Lowercased-tag search for an element's [start of tag, after '>'] span.
fn find_tag(lower: &str, tag: &str, from: usize) -> Option<(usize, usize)> {
    let s = lower[from..].find(&format!("<{tag}"))? + from;
    let e = lower[s..].find('>')? + s + 1;
    Some((s, e))
}

/// The naive split an app would do: pull out <style> blocks and the <body> contents.
pub fn split_raw(html: &str) -> (String, String, String) {
    let lower = html.to_ascii_lowercase();
    let (mut css, mut i) = (String::new(), 0);
    let mut cuts = vec![];
    while let Some((s, e)) = find_tag(&lower, "style", i) {
        let Some(c) = lower[e..].find("</style>") else { break };
        css.push_str(&html[e..e + c].replace("<!--", "").replace("-->", ""));
        css.push('\n');
        cuts.push((s, e + c + 8));
        i = e + c + 8;
    }
    let mut body = String::new();
    let mut body_style = String::new();
    let (bs, be) = find_tag(&lower, "body", 0).unwrap_or((0, 0));
    if be > 0 {
        let tag = &html[bs..be];
        if let Some(p) = tag.find("style=\"") {
            body_style = tag[p + 7..].split('"').next().unwrap_or("").to_string();
        }
    }
    let end = lower.rfind("</body>").unwrap_or(html.len());
    let mut pos = be;
    for (s, e) in cuts.iter().filter(|c| c.0 >= be) {
        body.push_str(&html[pos..*s]);
        pos = *e;
    }
    body.push_str(&html[pos..end]);
    (css, body, body_style)
}

fn render(name: &str, css: &str, body: &str, body_style: &str, out: &Path) -> (f64, f64) {
    let mut app = RinchApp::new(shell);
    app.mount_component(W as f32, H as f32);
    let t = Instant::now();
    {
        let doc = app.doc().expect("mounted");
        let mut doc = doc.borrow_mut();
        doc.load_css("body{margin:8px}");
        doc.load_css(css);
        let b = doc.body();
        let mail = doc.get_children(b)[0];
        // The mail's own `<body style>` belongs on the real body (its margin
        // replaces the default 8px, as in a browser), not on the wrapper.
        doc.set_attribute(b, "style", body_style);
        doc.set_inner_html(mail, body);
    }
    let parse = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    app.resize_layout(W, H);
    app.mark_scene_dirty();
    let (px, w, h) = app.build_pixels(1.0, (W, H), false);
    let paint = t.elapsed().as_secs_f64() * 1e3;
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(px).unwrap();
    }
    std::fs::write(out.join(format!("{name}.png")), bytes).unwrap();
    (parse, paint)
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or("raw".into());
    let only = std::env::args().nth(2);
    let out = Path::new("out").join(&mode);
    std::fs::create_dir_all(&out).unwrap();
    let mut files: Vec<_> = std::fs::read_dir("fixtures").unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    for f in files {
        let name = f.file_stem().unwrap().to_str().unwrap().to_string();
        if only.as_ref().is_some_and(|o| !name.contains(o.as_str())) {
            continue;
        }
        let path = match mode.as_str() { "norm" => Path::new("out/normalized").join(f.file_name().unwrap()), "parsed" => Path::new("out/parsed").join(f.file_name().unwrap()), _ => f.clone() };
        let html = std::fs::read_to_string(&path).unwrap();
        let (css, body, bs) = split_raw(&html);
        let (p, q) = render(&name, &css, &body, &bs, &out);
        println!("{name:32} dom {p:7.1}ms  layout+paint {q:7.1}ms  html {}B", html.len());
    }
}
