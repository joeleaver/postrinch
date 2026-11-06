//! Showing a message as its sender drew it, and the pictures from the web
//! in it: hidden until the person asks, for one message or always for one
//! sender, because loading them tells the sender the mail was opened.

use std::collections::{HashMap, HashSet};

use postrinch_commands::{Commands, Rendered, Thread};
use postrinch_model::Message;

use crate::appearance::Settings;
use crate::state::State;

/// Whose pictures from the web load.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pictures {
    /// Messages the person asked to see them for, by id.
    pub messages: HashSet<String>,
    /// Senders whose pictures always load, by address key.
    pub senders: HashSet<String>,
}

impl Pictures {
    fn load_for(&self, message: &Message) -> bool {
        self.messages.contains(&message.id.0)
            || sender(message).is_some_and(|s| self.senders.contains(&s))
    }
}

/// A message's sender, as pictures are allowed by.
pub fn sender(message: &Message) -> Option<String> {
    message.headers.from.first().map(|m| m.address.key())
}

/// The thread's downloaded messages as they show, by message id. Run on the
/// loader thread: reading and parsing a message takes milliseconds.
pub fn render(
    commands: &Commands,
    thread: &Thread,
    pictures: &Pictures,
) -> HashMap<String, Rendered> {
    let account = &thread.summary.account;
    let mut out = HashMap::new();
    for message in thread.messages.iter().filter(|m| m.downloaded) {
        match commands.render_message(account, &message.id, pictures.load_for(message)) {
            Ok(Some(rendered)) => {
                out.insert(message.id.0.clone(), rendered);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("message {} does not render: {e}", message.id.0),
        }
    }
    out
}

impl State {
    /// Load this message's pictures from the web, this time.
    pub fn show_pictures(self, message: &Message) {
        let id = message.id.0.clone();
        self.pictures_shown.update(|shown| {
            shown.insert(id);
        });
    }

    /// Always load pictures from this message's sender, and remember it.
    pub fn always_show_pictures(self, message: &Message) {
        let Some(sender) = sender(message) else {
            return;
        };
        self.pictures_from.update(|senders| {
            senders.insert(sender.clone());
        });
        let mut settings = Settings::load();
        if !settings.pictures_from.contains(&sender) {
            settings.pictures_from.push(sender);
            settings.pictures_from.sort();
        }
        if let Err(e) = settings.save() {
            self.error.set(Some(format!("That was not saved: {e}")));
        }
    }
}

/// Open a message, as Postrinch drew it, in the system's browser: from a
/// file of its own in the temporary folder.
pub fn open_in_browser(message: &Message, rendered: &Rendered) {
    let subject = message.headers.subject.clone().unwrap_or_default();
    let title = subject.replace('&', "&amp;").replace('<', "&lt;");
    let page = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"{}\">{}</body></html>",
        rendered.body_style.replace('"', "&quot;"),
        rendered.html
    );
    let name: String = message
        .id
        .0
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    let path = std::env::temp_dir().join(format!("postrinch-{name}.html"));
    if let Err(e) = std::fs::write(&path, page).and_then(|()| open::that(&path)) {
        tracing::warn!("opening the message in a browser failed: {e}");
    }
}

/// The most a picture from the web may weigh.
const MOST_BYTES: u64 = 15 * 1024 * 1024;

/// Fetch pictures from the web for mail, as the person allowed them: the
/// sanitizer lets an `http(s)` source through only then. Without cookies or
/// a referrer, with a time limit and a size limit.
pub fn fetch_web_pictures() {
    struct Web(reqwest::blocking::Client);
    impl rinch::image::ImageLoader for Web {
        fn load(&self, src: &str) -> rinch::image::ImageLoadResult {
            use rinch::image::ImageLoadResult::{Failed, Loaded};
            use std::io::Read;
            tracing::debug!("fetching picture {src}");
            let response = match self.0.get(src).send().and_then(|r| r.error_for_status()) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!("picture {src} failed: {e}");
                    return Failed(e.to_string());
                }
            };
            let mut bytes = Vec::new();
            match response.take(MOST_BYTES + 1).read_to_end(&mut bytes) {
                Ok(_) if bytes.len() as u64 > MOST_BYTES => Failed("too large".into()),
                Ok(_) => Loaded(bytes),
                Err(e) => Failed(e.to_string()),
            }
        }
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(postrinch_render::PICTURE_USER_AGENT)
        .build();
    match client {
        Ok(client) => {
            let web = std::sync::Arc::new(Web(client));
            rinch::image::register_image_scheme_arc("https", web.clone());
            rinch::image::register_image_scheme_arc("http", web);
        }
        Err(e) => tracing::warn!("pictures from the web cannot load: {e}"),
    }
}
