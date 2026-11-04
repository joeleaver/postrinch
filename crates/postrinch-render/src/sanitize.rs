//! Mail through rinch's cleaner (`rinch_html::clean`: the allow-list,
//! style sheets applied inline, inline styles filtered, links limited to
//! http, https and mailto), with mail's own rule for pictures: the ones the
//! message carries (`cid:`) inline, `data:` pictures kept, and pictures on the
//! web only when the person asked for them, counting the ones held back.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rinch_html::{Policy, clean};

use crate::{Options, Rendered};

/// Sanitize an HTML document (or fragment) into the inside of its body.
/// `inline` maps Content-IDs to `data:` URLs for the pictures the message
/// carries.
pub fn sanitize(html: &str, inline: &[(String, String)], options: Options) -> Rendered {
    let hidden = Arc::new(AtomicUsize::new(0));
    let inline: HashMap<String, String> = inline.iter().cloned().collect();
    let counted = hidden.clone();
    let policy = Policy {
        picture: Arc::new(move |src| {
            let src = src.trim();
            let lower = src.to_ascii_lowercase();
            if lower.starts_with("cid:") {
                return inline.get(&src[4..]).cloned();
            }
            if lower.starts_with("data:image/") {
                return Some(src.to_owned());
            }
            if lower.starts_with("http://") || lower.starts_with("https://") {
                if options.remote {
                    return Some(src.to_owned());
                }
                counted.fetch_add(1, Ordering::Relaxed);
            }
            None
        }),
        ..Policy::default()
    };
    let cleaned = clean(html, &policy);
    Rendered {
        html: cleaned.html,
        body_style: cleaned.body_style,
        from_html: true,
        remote_hidden: hidden.load(Ordering::Relaxed),
    }
}
