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
}

html, body { margin: 0; height: 100%; }

.app {
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: #ffffff;
    color: var(--ink);
    font-family: 'IBM Plex Sans', sans-serif;
    font-size: 14px;
}

.header {
    height: 52px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 16px;
    border-bottom: 1px solid var(--line);
}
.brand { width: 208px; font-size: 18px; font-weight: 600; color: var(--accent); }
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
    height: 34px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
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
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--line-soft);
}
.list-head h1 { margin: 0; font-size: 18px; font-weight: 600; }
.list-count { color: var(--muted); font-size: 13px; }
.rows { flex: 1; overflow-y: auto; }
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
.snippet {
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
"#;
