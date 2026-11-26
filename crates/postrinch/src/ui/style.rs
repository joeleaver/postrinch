//! The window's stylesheet, after the mockups in the design canvas.

pub const CSS: &str = r#"
.rinch-borderlesswindow__titlebar { border-bottom: 1px solid var(--line); }

html, body { margin: 0; height: 100%; background: transparent; }

.app {
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    color: var(--ink);
    font-family: 'IBM Plex Sans', sans-serif;
    font-size: 14px;
}

.brand { display: flex; align-items: center; gap: 7px; padding: 0 6px 0 8px; font-size: 15px; font-weight: 600; color: var(--accent); }
.logo-line { stroke: var(--logo-line); }
.logo-outline { stroke: var(--logo-line); fill: var(--logo-line); }
.title-status { display: flex; align-items: center; gap: 12px; padding-right: 8px; }
/* Search, centred on the window whatever is either side. */
.rinch-borderlesswindow__titlebar { position: relative; }
.title-parts { display: flex; align-items: center; }
.title-center {
    position: absolute; top: 0; bottom: 0; left: 50%; width: 360px; margin-left: -180px;
    display: flex; align-items: center; gap: 8px;
}
.title-search { flex: 1; min-width: 0; }
.title-search .rinch-text-input__input {
    height: 26px;
    min-height: 26px;
    padding: 0 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--panel);
    font-size: 13px;
}
.title-search .rinch-text-input__input[data-focused="true"] { background: var(--surface); border-color: var(--accent); }
mark { background: var(--mark); color: inherit; padding: 0 1px; border-radius: 2px; }
.header-error { color: var(--danger); font-size: 13px; }

.window-body { flex: 1; display: flex; min-height: 0; }
.panes { flex: 1; display: flex; min-width: 0; min-height: 0; }
/* A wide grip over the panes' 1px border, without taking room. */
.splitter {
    width: 7px;
    margin: 0 -3px;
    flex-shrink: 0;
    position: relative;
    z-index: 5;
    cursor: col-resize;
}
.splitter:hover { background: var(--accent); opacity: 0.35; }

.dot { display: inline-block; width: 8px; height: 8px; border-radius: 2px; flex-shrink: 0; }

/* Sidebar */
.sidebar {
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
/* The views scroll; the status line at the foot stays put. */
.folders { flex: 1; min-height: 0; overflow-y: auto; }
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
.folder:hover { background: var(--hover); }
.view-line { display: flex; align-items: center; gap: 6px; height: 22px; margin: 4px 4px; }
.view-rule { flex: 1; height: 1px; background: var(--line); }
.view-add {
    width: 20px; height: 20px; padding: 0;
    border: 0; border-radius: 5px;
    background: transparent; color: var(--muted);
    font: inherit; font-size: 15px; line-height: 1;
    cursor: pointer;
}
.view-add:hover { background: var(--hover); color: var(--ink); }
.folder.hidden-view .name-text { color: var(--muted); font-style: italic; }
.view-row { padding: 0 4px 0 2px; gap: 2px; }
.row-chevron { height: 100%; display: flex; align-items: center; justify-content: center; border: 0; padding: 0; background: none; font-family: inherit; font-size: 11px; line-height: 1; color: var(--muted); cursor: pointer; }
.row-chevron:hover { color: var(--ink); }
.row-open { flex: 1; min-width: 0; height: 100%; display: flex; align-items: center; justify-content: space-between; gap: 6px; }
.view-row .count { padding-right: 4px; }
.folder.unsaved .name-text { font-style: italic; }
.row-close {
    width: 24px; height: 24px; flex-shrink: 0; padding: 0;
    display: none; align-items: center; justify-content: center;
    border: 0; border-radius: 5px; background: transparent; color: var(--ink-2);
    font: inherit; font-size: 15px; cursor: pointer;
}
.folder.unsaved .row-close { display: flex; }
/* ✎ on a view's row: unseen at rest and taking no room; while the row
   is hovered, or ✎ has the focus, it stands over the count's place.
   Transparent rather than sizeless, since rinch leaves a sizeless
   button out of the Tab order. */
.view-row { position: relative; }
.row-edit {
    position: absolute; right: 4px; top: 4px;
    width: 24px; height: 24px; padding: 0;
    display: flex; align-items: center; justify-content: center;
    opacity: 0;
    border: 0; border-radius: 5px; background: transparent; color: var(--muted);
    cursor: pointer;
}
.row-edit svg { width: 14px; height: 14px; }
.view-row:hover .row-edit { opacity: 1; }
.view-row:hover .count { visibility: hidden; }
/* Room for ✎ while it shows, so a long name ends in an ellipsis rather
   than under it. */
.view-row:hover .row-open { padding-right: 26px; }
.folder.chosen .row-edit { color: var(--accent-ink); }
/* Focused without the pointer: a solid face over the count. */
.view-row .row-edit:focus { opacity: 1; background: var(--field); color: var(--ink); }
.view-row .row-edit:hover { background: var(--field); color: var(--ink); }
/* Open in the panel: wins over the chosen row's colour and the focus face. */
.view-row .row-edit.pressed { background: var(--accent); color: var(--surface); }
/* The search's row keeps × at its end: ✎ stands just before it. */
.folder.unsaved .row-edit { right: 30px; }
.row-close:hover { background: var(--field); }
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
/* The name itself, which a flex row cannot cut with an ellipsis. */
.name-text { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
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

/* Thread list */
.list {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--line);
}
.rows { flex: 1; overflow-y: auto; border-top: 1px solid var(--line-soft); }
.row {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 12px 20px;
    border-bottom: 1px solid var(--line-soft);
    cursor: pointer;
}
.row:hover { background: var(--row-hover); }
.row.chosen { background: var(--accent-tint); box-shadow: inset 3px 0 0 var(--accent); }
.row-top { display: flex; align-items: center; gap: 8px; }
.who { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.row.unread .who, .row.unread .subject { font-weight: 600; }
/* You answered last: done with it for now, so it steps back. */
.row.mine .who, .row.mine .subject { color: var(--muted); font-weight: 400; }
.row.mine .snippet { color: var(--faint); }
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
    background: var(--surface);
    color: var(--ink);
    font: inherit;
    cursor: pointer;
}

/* Reader */
.reader {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--panel);
}
/* Each conversation scrolls in a box of its own, so it opens at the top. */
.reader-frame { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.reader-scroll { flex: 1; min-height: 0; overflow-y: auto; }
.reader-empty { padding: 40px; color: var(--muted); }
.thread { padding: 14px 28px 24px; display: flex; flex-direction: column; gap: 12px; }
.thread-title { display: flex; align-items: center; gap: 10px; min-width: 0; font-size: 13px; }
.thread-subject {
    flex: 1; min-width: 0; margin: 0;
    font-size: 18px; font-weight: 600;
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}
.thread-title .open-all { flex-shrink: 0; color: var(--accent); cursor: pointer; }
.message {
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    padding: 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
}
.message-head { display: flex; align-items: center; gap: 8px; }
.message-from { font-weight: 600; }
.message-address { color: var(--muted); font-size: 13px; }
.message-when { margin-left: auto; color: var(--muted); font-size: 13px; }
.message-to { color: var(--muted); font-size: 13px; margin-top: -6px; }
.message-body { white-space: pre-wrap; overflow-wrap: anywhere; line-height: 1.6; color: var(--ink); }
.message-note { color: var(--ink-2); font-style: italic; }
/* A message as its sender drew it: on white, as mail is written for it,
   in dark mode too. */
.mail { display: flex; flex-direction: column; gap: 10px; }
/* .mail-html, the card a message is drawn in: postrinch_render::HOST_CSS. */
/* Mail with no colours of its own reads in the window's colours. */
.mail-html.plain { background: var(--surface); color: var(--ink); }
.pictures-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    padding: 8px 12px;
    border-radius: 8px;
    background: var(--panel);
    font-size: 13px;
    color: var(--ink-2);
}
.pictures-line { flex: 1; min-width: 240px; }
.message-folded {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 18px;
    border: 1px solid var(--line);
    border-radius: 10px;
    background: var(--surface);
    cursor: pointer;
}
.message-folded:hover { background: var(--row-hover); }
.folded-from { width: 160px; flex-shrink: 0; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.folded-snippet { flex: 1; min-width: 0; color: var(--muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.quote-toggle {
    align-self: flex-start;
    padding: 2px 10px;
    border: 1px solid var(--field);
    border-radius: 6px;
    background: var(--surface);
    color: var(--muted);
    font-size: 12px;
    cursor: pointer;
}
.quote-toggle:hover { background: var(--hover); }
/* Reply, Reply all and Forward: in a message's header, and on a list row
   under the pointer, where they take the place of its date. */
.answer-icons { display: flex; gap: 2px; margin: -6px -6px -6px 0; }
.answer-icon {
    width: 30px; height: 30px;
    display: flex; align-items: center; justify-content: center;
    border: 0; border-radius: 6px;
    background: transparent;
    color: var(--muted);
    cursor: pointer;
}
.answer-icon:hover { background: var(--hover); color: var(--ink); }
.row .answer-icons { display: none; margin: -7px -8px -7px 0; }
.row:hover .answer-icons { display: flex; }
.row:hover .when { display: none; }
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
    border: 2px solid var(--surface);
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
.account-row:hover { background: var(--row-hover); }
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
.account-actions { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; padding-top: 10px; border-top: 1px solid var(--line-soft); }
.account-actions .spacer { flex: 1; }
.settings-hint { margin: 0; font-size: 13px; color: var(--muted); line-height: 1.5; }
.notice {
    padding: 10px 14px;
    border: 1px solid var(--notice-line);
    border-radius: 8px;
    background: var(--accent-tint);
    color: var(--accent-ink);
}
.notice.trouble { border-color: var(--warn-line); background: var(--warn-bg); color: var(--trouble); }
.settings-page { flex: 1; display: flex; min-width: 0; }
.settings-heading { margin: 0; font-size: 22px; font-weight: 600; }
.settings-subheading { margin: 8px 0 0; font-size: 15px; font-weight: 600; }
.choices { display: flex; flex-direction: column; gap: 8px; max-width: 520px; }
.choice {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 14px;
    border: 1px solid var(--line);
    border-radius: 10px;
    cursor: pointer;
}
.choice:hover { background: var(--row-hover); }
.choice.chosen { border-color: var(--accent); background: var(--accent-tint); }
.choice-name { font-weight: 600; }
.choice-about { font-size: 13px; color: var(--muted); }
/* Too narrow for the accounts and the chosen one side by side: the list
   above, the account under it, the page scrolling as one. */
@media (max-width: 1000px) {
    .settings-page { flex-direction: column; min-height: 0; overflow-y: auto; }
    .settings-accounts { width: auto; padding: 24px 20px 16px; border-right: none; border-bottom: 1px solid var(--line-soft); }
    .settings-main { flex: none; overflow-y: visible; padding: 20px; }
}
@media (max-width: 720px) {
    .settings-nav { width: 160px; }
    .facts { grid-template-columns: 110px minmax(0, 1fr); }
}
"#;
