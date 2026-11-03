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
    --rinch-titlebar-hover: rgba(231, 233, 238, 0.08);
    --rinch-titlebar-active: rgba(231, 233, 238, 0.14);
}
"#;
