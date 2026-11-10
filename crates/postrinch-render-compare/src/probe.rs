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
/// it (postrinch's pictures.rs), each fetched once into `cache` and read from
/// there after: a redraw, or a bisect, never asks the senders' servers again.
/// A fetch that failed is kept as failed, so it is not retried either.
pub fn fetch_web_pictures(cache: &std::path::Path) {
    use std::hash::{Hash, Hasher};
    use std::io::Read;
    use rinch::image::ImageLoadResult::{self, Failed, Loaded};

    struct Web {
        client: reqwest::blocking::Client,
        cache: std::path::PathBuf,
    }
    impl Web {
        fn fetch(&self, src: &str) -> Result<Vec<u8>, String> {
            let response = self
                .client
                .get(src)
                .send()
                .and_then(|r| r.error_for_status())
                .map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            response
                .take(15 * 1024 * 1024)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            Ok(bytes)
        }
    }
    impl rinch::image::ImageLoader for Web {
        fn load(&self, src: &str) -> ImageLoadResult {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            src.hash(&mut hasher);
            let key = format!("{:016x}", hasher.finish());
            let kept = self.cache.join(&key);
            let failed = self.cache.join(format!("{key}.failed"));
            if let Ok(bytes) = std::fs::read(&kept) {
                return Loaded(bytes);
            }
            if let Ok(why) = std::fs::read_to_string(&failed) {
                return Failed(why);
            }
            match self.fetch(src) {
                Ok(bytes) => {
                    let _ = std::fs::write(&kept, &bytes);
                    Loaded(bytes)
                }
                Err(why) => {
                    let _ = std::fs::write(&failed, &why);
                    Failed(why)
                }
            }
        }
    }
    let _ = std::fs::create_dir_all(cache);
    if let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .user_agent(postrinch_render::PICTURE_USER_AGENT)
        .build()
    {
        let web = std::sync::Arc::new(Web {
            client,
            cache: cache.to_path_buf(),
        });
        rinch::image::register_image_scheme_arc("https", web.clone());
        rinch::image::register_image_scheme_arc("http", web);
    }
}
