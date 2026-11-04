//! The probe: a rinch window that draws one message's cleaned HTML as the
//! reader does (the same card, CSS, fonts and picture loading), one after
//! another. The capture step drives it through rinch's debug server: it
//! presses Next, waits, and screenshots the card a page at a time (Down
//! moves the card up by [`STEP`]), as a window is no taller than the screen.

use std::path::PathBuf;

use rinch::prelude::*;

use crate::WIDTH;
use crate::export::Cleaned;

/// How far Down moves the card, in px: less than any window is tall.
pub const STEP: u32 = 800;
/// Room to the right of the card for the probe's buttons.
pub const BAR: u32 = 120;

const FONTS: &[AppFont] = &[
    AppFont::sans_serif(include_bytes!(
        "../../postrinch/assets/fonts/IBMPlexSans-Regular.ttf"
    )),
    AppFont::sans_serif(include_bytes!(
        "../../postrinch/assets/fonts/IBMPlexSans-Medium.ttf"
    )),
    AppFont::sans_serif(include_bytes!(
        "../../postrinch/assets/fonts/IBMPlexSans-SemiBold.ttf"
    )),
    AppFont::sans_serif(include_bytes!(
        "../../postrinch/assets/fonts/IBMPlexSans-Italic.ttf"
    )),
    AppFont::monospace(include_bytes!(
        "../../postrinch/assets/fonts/IBMPlexMono-Regular.ttf"
    )),
];

/// Run the probe over the message folders `dirs`, starting before the
/// first (Next shows the first).
pub fn run(dirs: Vec<PathBuf>) {
    let theme = ThemeProviderProps {
        primary_color: Some("blue".into()),
        font_family: Some("'IBM Plex Sans', sans-serif".into()),
        font_family_monospace: Some("'IBM Plex Mono', monospace".into()),
        ..Default::default()
    };
    let window = WindowProps {
        title: "Postrinch render probe".into(),
        width: WIDTH + BAR,
        height: 1200,
        ..Default::default()
    };
    App::new(move |scope| probe(scope, dirs.clone()))
        .window_props(window)
        .fonts(FONTS)
        .theme(theme)
        .run();
}

#[component]
fn probe(dirs: Vec<PathBuf>) -> NodeHandle {
    // On the UI thread, as the window does (ui/mod.rs).
    fetch_web_pictures();
    let shown: Signal<Option<usize>> = Signal::new(None);
    let page = Signal::new(0u32);
    let count = dirs.len();
    rsx! {
        div { class: "probe",
            style { {postrinch_render::HOST_CSS} }
            style { {CSS} }
            div { class: "probe-bar",
                button {
                    class: "probe-next",
                    onclick: move || {
                        page.set(0);
                        shown.update(|s| *s = Some(s.map_or(0, |i| (i + 1).min(count))));
                    },
                    "Next"
                }
                button { class: "probe-down", onclick: move || page.update(|p| *p += 1), "Down" }
                span { class: "probe-at", {move || shown.get().map_or("-".to_owned(), |i| i.to_string())} }
            }
            // Keyed on the message shown, so each is drawn afresh
            // (rinch#2105: a loop, not `if let`).
            div {
                class: "probe-sheet",
                style: {move || format!("margin-top: -{}px", page.get() * STEP)},
                for i in shown.get().filter(|&i| i < count) {
                    div { key: i.to_string(), {card(__scope, dirs[i].clone())} }
                }
            }
        }
    }
}

#[component]
fn card(dir: PathBuf) -> NodeHandle {
    let cleaned: Cleaned = std::fs::read(dir.join("cleaned.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Cleaned {
            html: "<p>cleaned.json is missing</p>".into(),
            body_style: String::new(),
        });
    let body = rsx! { div { class: "mail-body", style: {cleaned.body_style.clone()} } };
    body.set_inner_html(&cleaned.html);
    rsx! { div { class: "mail-html", {body} } }
}

const CSS: &str = r#"
body { margin: 0; background: #ffffff; }
.probe { position: relative; display: flex; flex-direction: column; }
.probe-bar { position: absolute; top: 0; left: 724px; width: 110px; z-index: 10; display: flex; flex-direction: column; gap: 6px; padding: 6px; background: #eeeeee; }
.probe-sheet { width: 720px; }
.mail-html { width: 720px; box-sizing: border-box; border-radius: 0; }
"#;

/// Pictures from the web, as the window loads them when the person allows
/// it (postrinch's pictures.rs).
fn fetch_web_pictures() {
    struct Web(reqwest::blocking::Client);
    impl rinch::image::ImageLoader for Web {
        fn load(&self, src: &str) -> rinch::image::ImageLoadResult {
            use rinch::image::ImageLoadResult::{Failed, Loaded};
            use std::io::Read;
            let response = match self.0.get(src).send().and_then(|r| r.error_for_status()) {
                Ok(r) => r,
                Err(e) => return Failed(e.to_string()),
            };
            let mut bytes = Vec::new();
            match response.take(15 * 1024 * 1024).read_to_end(&mut bytes) {
                Ok(_) => Loaded(bytes),
                Err(e) => Failed(e.to_string()),
            }
        }
    }
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (compatible; Postrinch)")
        .build()
    {
        let web = std::sync::Arc::new(Web(client));
        rinch::image::register_image_scheme_arc("https", web.clone());
        rinch::image::register_image_scheme_arc("http", web);
    }
}
