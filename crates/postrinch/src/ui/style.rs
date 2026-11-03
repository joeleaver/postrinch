//! The window's stylesheet, after the mockups in the design canvas.

pub const CSS: &str = r#"
:root {
    --rinch-primary-color: #2456b8;
    --ink: #16181d;
    --ink-2: #3a3f4a;
    --muted: #5b6170;
    --line: #dfe2e7;
    --line-soft: #eceef1;
    --field: #d4d8de;
    --panel: #f4f5f7;
    --accent: #2456b8;
    --accent-tint: #e6eefb;
    --accent-ink: #173d84;
    --trouble: #7a3f06;
    /* A white title bar, like the mockups, not rinch's blue one. */
    --rinch-titlebar-bg: #ffffff;
    --rinch-titlebar-text: #16181d;
    --rinch-titlebar-icon: #3a3f4a;
    --rinch-titlebar-hover: rgba(22, 24, 29, 0.06);
    --rinch-titlebar-active: rgba(22, 24, 29, 0.1);
}

.rinch-borderlesswindow__titlebar { border-bottom: 1px solid var(--line); }

html, body { margin: 0; height: 100%; }

.app {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: #ffffff;
    color: var(--ink);
    font-family: 'IBM Plex Sans', sans-serif;
    font-size: 14px;
}

.brand { padding: 0 6px 0 10px; font-size: 15px; font-weight: 600; color: var(--accent); }
.title-status { display: flex; align-items: center; gap: 12px; padding-right: 8px; }
.title-search { width: 360px; margin-right: 8px; }
.title-search .rinch-text-input__input {
    height: 26px;
    min-height: 26px;
    padding: 0 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--panel);
    font-size: 13px;
}
.title-search .rinch-text-input__input[data-focused="true"] { background: #ffffff; border-color: var(--accent); }
mark { background: #fbe7b0; color: inherit; padding: 0 1px; border-radius: 2px; }
.header-error { color: #a3271d; font-size: 13px; }

.panes { flex: 1; display: flex; min-height: 0; }

.dot { display: inline-block; width: 8px; height: 8px; border-radius: 2px; flex-shrink: 0; }

/* Sidebar */
.sidebar {
    width: 240px;
    flex-shrink: 0;
    box-sizing: border-box;
    padding: 12px 8px;
    border-right: 1px solid var(--line);
    background: var(--panel);
    display: flex;
    flex-direction: column;
    gap: 18px;
    min-height: 0;
}
/* The labels scroll; the folders above and sync below stay put. */
.labels { flex: 1; min-height: 0; overflow-y: auto; }
.folder-group { display: flex; flex-direction: column; gap: 2px; }
.section-title {
    padding: 0 10px 4px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
}
.folder {
    height: 32px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    padding: 0 10px;
    border-radius: 6px;
    cursor: pointer;
}
.folder:hover { background: #eaecef; }
.folder.indented { padding-left: 22px; }
.folder.chosen { background: var(--accent-tint); color: var(--accent-ink); font-weight: 600; }
.folder-name {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}
.count { color: var(--muted); font-weight: 500; }
.chevron {
    width: 14px;
    flex-shrink: 0;
    color: var(--muted);
    font-size: 11px;
    text-align: center;
}
.folder .folder-name { flex: 1; }
.folder.heading .folder-name, .label-account {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--muted);
}
.folder.heading { margin-top: 10px; height: 28px; }
.folder.from-app .folder-name { color: var(--ink-2); }
.show-all {
    padding: 8px 10px 8px 22px;
    font-size: 12px;
    color: var(--accent);
    cursor: pointer;
}
.folder.chosen .count { color: var(--accent-ink); }
.label-account {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 10px 2px;
    font-size: 12px;
    color: var(--muted);
}
.sync {
    flex-shrink: 0;
    padding: 12px 10px 0;
    border-top: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 10px;
}
.sync .section-title { padding: 0; }
.sync-account { display: flex; flex-direction: column; gap: 4px; }
.sync-address { font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sync-line { font-size: 12px; color: var(--muted); }
.sync-line.trouble { color: var(--trouble); }
.progress { height: 6px; border-radius: 3px; background: var(--line); overflow: hidden; }
.progress-fill { height: 100%; background: var(--accent); }

/* Thread list */
.list {
    width: 420px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
}
.list-head {
    flex-shrink: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 16px 20px 10px;
}
.back {
    width: 30px;
    height: 30px;
    border: 0;
    border-radius: 8px;
    background: transparent;
    color: var(--ink);
    font-size: 18px;
    cursor: pointer;
}
.back:hover { background: #eaecef; }
.search-head { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.list-head h1 { margin: 0; font-size: 18px; font-weight: 600; }
.list-count { color: var(--muted); font-size: 13px; }
.rows { flex: 1; overflow-y: auto; border-top: 1px solid var(--line-soft); }
.row {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 12px 20px;
    border-bottom: 1px solid var(--line-soft);
    cursor: pointer;
}
.row:hover { background: #f8f9fa; }
.row.chosen { background: var(--accent-tint); box-shadow: inset 3px 0 0 var(--accent); }
.row-top { display: flex; align-items: center; gap: 8px; }
.who { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row.unread .who, .row.unread .subject { font-weight: 600; }
.thread-count { font-size: 12px; color: var(--muted); }
.attachment { font-size: 12px; }
.when { flex-shrink: 0; font-size: 12px; color: var(--muted); }
.subject { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row-bottom { display: flex; align-items: center; gap: 6px; min-width: 0; }
.label-chip {
    flex-shrink: 0;
    padding: 0 6px;
    border: 1px solid var(--field);
    border-radius: 4px;
    font-size: 11px;
    line-height: 16px;
    color: var(--ink-2);
}
.tabs {
    flex-shrink: 0;
    display: flex;
    padding: 0 13px;
}
.tab {
    height: 32px;
    padding: 0 7px;
    border: 0;
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--muted);
    font: inherit;
    font-size: 13px;
    cursor: pointer;
}
.tab.chosen { color: var(--accent-ink); border-bottom-color: var(--accent); font-weight: 600; }
.snippet {
    flex: 1;
    min-width: 0;
    color: var(--muted);
    font-size: 13px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
}
.empty { padding: 24px 20px; color: var(--muted); }
.more {
    margin: 12px 20px 20px;
    height: 34px;
    border: 1px solid var(--field);
    border-radius: 8px;
    background: #ffffff;
    color: var(--ink);
    font: inherit;
    cursor: pointer;
}

/* Reader */
.reader {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    background: var(--panel);
}
.reader-empty { padding: 40px; color: var(--muted); }
.thread { padding: 24px 28px; display: flex; flex-direction: column; gap: 14px; }
.thread-subject { margin: 0; font-size: 20px; font-weight: 600; }
.thread-meta { font-size: 13px; color: var(--muted); }
.message {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: #ffffff;
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
}
.message-head { display: flex; align-items: baseline; gap: 8px; }
.message-from { font-weight: 600; }
.message-address { color: var(--muted); font-size: 13px; }
.message-when { margin-left: auto; color: var(--muted); font-size: 13px; }
.message-to { color: var(--muted); font-size: 13px; margin-top: -6px; }
.message-body { white-space: pre-wrap; line-height: 1.6; color: var(--ink); }
.message-note { color: var(--ink-2); font-style: italic; }
.attachments { display: flex; flex-wrap: wrap; gap: 6px; }
.chip {
    padding: 4px 10px;
    border: 1px solid var(--line);
    border-radius: 999px;
    font-size: 12px;
    color: var(--ink-2);
}

/* Label dialog */
.dialog { display: flex; flex-direction: column; gap: 12px; }
.dialog-title { margin: 0; font-size: 16px; font-weight: 600; }
.dialog-hint { margin: 0; font-size: 13px; color: var(--muted); }
.dialog-actions { display: flex; justify-content: flex-end; gap: 8px; }
.swatches { display: flex; flex-wrap: wrap; gap: 10px; }
.swatch {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    border: 2px solid #ffffff;
    box-shadow: 0 0 0 1px var(--field);
    cursor: pointer;
}
.swatch.chosen { box-shadow: 0 0 0 2px var(--ink); }

/* Settings */
.settings { flex: 1; display: flex; min-height: 0; height: 100%; }
.settings-nav {
    width: 220px;
    flex-shrink: 0;
    box-sizing: border-box;
    padding: 18px 10px;
    background: var(--panel);
    border-right: 1px solid var(--line);
    display: flex;
    flex-direction: column;
    gap: 2px;
}
.settings-title { padding: 0 10px 12px; font-size: 16px; font-weight: 600; }
.settings-back { margin-top: auto; padding: 8px 10px; color: var(--accent); cursor: pointer; font-size: 13px; }
.settings-accounts {
    width: 320px;
    flex-shrink: 0;
    box-sizing: border-box;
    padding: 28px 16px 20px 24px;
    border-right: 1px solid var(--line-soft);
    display: flex;
    flex-direction: column;
    gap: 6px;
}
.settings-accounts-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; }
.settings-accounts-head h1 { margin: 0; font-size: 22px; font-weight: 600; }
.account-row {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px;
    border-radius: 8px;
    cursor: pointer;
}
.account-row:hover { background: #f4f5f7; }
.account-row.chosen { background: var(--accent-tint); }
.account-row .dot { margin-top: 5px; width: 10px; height: 10px; border-radius: 3px; }
.account-row-text { min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.account-address { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.account-state { font-size: 12px; color: var(--muted); }
.account-state.trouble { color: var(--trouble); }
.settings-main { flex: 1; min-width: 0; padding: 28px; display: flex; flex-direction: column; gap: 18px; overflow-y: auto; }
.account-detail { display: flex; flex-direction: column; gap: 14px; max-width: 640px; }
.account-detail h2 { margin: 0; font-size: 18px; font-weight: 600; }
.facts { display: grid; grid-template-columns: 140px minmax(0, 1fr); row-gap: 10px; column-gap: 16px; }
.fact-name { color: var(--muted); }
.account-actions { display: flex; align-items: center; gap: 10px; padding-top: 10px; border-top: 1px solid var(--line-soft); }
.account-actions .spacer { flex: 1; }
.settings-hint { margin: 0; font-size: 13px; color: var(--muted); line-height: 1.5; }
.notice {
    padding: 10px 14px;
    border: 1px solid #c9d8f3;
    border-radius: 8px;
    background: var(--accent-tint);
    color: var(--accent-ink);
}
.notice.trouble { border-color: #f0cf96; background: #fdf4e3; color: var(--trouble); }
"#;
