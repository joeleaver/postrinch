//! The window's colours, light and dark. Everything else in the stylesheet
//! names these variables, never a colour.

/// Light, after the mockups: white surfaces, grey panels, the blue accent.
pub const LIGHT: &str = r#"
:root {
    --surface: #ffffff;
    --panel: #f4f5f7;
    --ink: #16181d;
    --ink-2: #3a3f4a;
    --muted: #5b6170;
    --faint: #8a909c;
    --line: #dfe2e7;
    --line-soft: #eceef1;
    --field: #d4d8de;
    --hover: #eaecef;
    --row-hover: #f8f9fa;
    --accent: #2456b8;
    --accent-tint: #e6eefb;
    --accent-ink: #173d84;
    --logo-line: #173d84;
    --mark: #fbe7b0;
    --danger: #a3271d;
    --trouble: #7a3f06;
    --notice-line: #c9d8f3;
    --warn-bg: #fdf4e3;
    --warn-line: #f0cf96;
    --red-tint: #f9dedc;
    --red-ink: #8c1d18;
    --green-tint: #d7f0e0;
    --green-ink: #1d5b36;
    --rinch-primary-color: #2456b8;
    /* A title bar in the window's own colours, not rinch's blue one. */
    --rinch-titlebar-bg: var(--surface);
    --rinch-titlebar-text: var(--ink);
    --rinch-titlebar-icon: var(--ink-2);
    --rinch-titlebar-hover: rgba(22, 24, 29, 0.06);
    --rinch-titlebar-active: rgba(22, 24, 29, 0.1);
}
"#;

/// Dark: near-black surfaces, the accent lifted so it reads on them.
pub const DARK: &str = r#"
:root {
    --surface: #16181d;
    --panel: #1d2026;
    --ink: #e7e9ee;
    --ink-2: #c4c8d0;
    --muted: #949aa6;
    --faint: #6b717d;
    --line: #2d3139;
    --line-soft: #262a31;
    --field: #3a3f49;
    --hover: #2a2e36;
    --row-hover: #1f2329;
    --accent: #6f9cf0;
    --accent-tint: #1f2d48;
    --accent-ink: #b6cdf7;
    --logo-line: #9fbdf3;
    --mark: #5c4a12;
    --danger: #f0897c;
    --trouble: #f0b46a;
    --notice-line: #2f4670;
    --warn-bg: #3a2c12;
    --warn-line: #6b5120;
    --red-tint: #4a2626;
    --red-ink: #f2b8b5;
    --green-tint: #1f3d2c;
    --green-ink: #a8dcbc;
    --rinch-titlebar-hover: rgba(231, 233, 238, 0.08);
    --rinch-titlebar-active: rgba(231, 233, 238, 0.14);
}
"#;

/// Mail with no design of its own, redrawn for the dark window
/// (postrinch_render::for_dark): its text in `--ink`, its links in
/// `--accent`. Kept with [`DARK`], whose colours these are.
pub const DARK_MAIL: postrinch_render::DarkColours = postrinch_render::DarkColours {
    ink: postrinch_render::Rgba::rgb(0xe7, 0xe9, 0xee),
    link: postrinch_render::Rgba::rgb(0x6f, 0x9c, 0xf0),
};
