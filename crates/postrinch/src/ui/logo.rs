//! The Postrinch mark, a paper plane with a wrench for a tail: the same
//! drawing as design/logo/plane.svg, for the title bar. Its outline takes
//! the theme's `--logo-line`, so it reads on light and dark title bars.

use rinch::prelude::*;

/// The mark at `height` 22 px, as the title bar shows it.
#[component]
pub fn logo() -> NodeHandle {
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            viewBox: "10 13 42 38",
            width: "26",
            height: "22",
            role: "img",
            aria-label: "Postrinch",
            stroke-linejoin: "round",
            stroke-linecap: "round",
            g { transform: "translate(16.7 43.6) rotate(-38) scale(1.15)",
                g { class: "logo-outline", stroke-width: "3",
                    rect { x: "2", y: "-1.15", width: "9.5", height: "2.3", rx: "1.15" }
                    path { d: "M-2.95 -1.7 A3.4 3.4 0 1 1 -2.95 1.7 L-0.3 1.7 L-0.3 -1.7 Z" }
                }
                g { fill: "#bcd4f6",
                    rect { x: "2", y: "-1.15", width: "9.5", height: "2.3", rx: "1.15" }
                    path { d: "M-2.95 -1.7 A3.4 3.4 0 1 1 -2.95 1.7 L-0.3 1.7 L-0.3 -1.7 Z" }
                }
            }
            g { class: "logo-line", stroke-width: "2",
                path { d: "M48 18.5 L15.5 29 L24 32.8 Z", fill: "#bcd4f6" }
                path { d: "M48 18.5 L24 32.8 L25.4 36.8 Z", fill: "#2456b8" }
                path { d: "M48 18.5 L25.4 36.8 L31.6 44 Z", fill: "#bcd4f6" }
            }
        }
    }
}
