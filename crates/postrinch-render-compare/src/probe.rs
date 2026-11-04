//! What makes the probe draw as the window does: the window's fonts, and
//! its loader for pictures from the web (the window's pictures.rs).

use rinch::prelude::*;

/// The window's fonts, for the probe.
pub const FONTS: &[AppFont] = &[
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

/// Pictures from the web, as the window loads them when the person allows
/// it (postrinch's pictures.rs).
pub fn fetch_web_pictures() {
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
