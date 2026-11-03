# Postrinch design

Canonical. When a decision changes, change it here and say why in the commit.

## What it is

A desktop mail client that is built for an agent first and a person second, without being worse for the person.

- **Many accounts.** Any number of Gmail accounts and any number of IMAP accounts, side by side.
- **Calendar alongside mail.** Each Google account's calendars, through the Calendar API: see, create and answer events, and act on the invitations that arrive as mail.
- **One inbox or many.** A combined inbox across accounts, and filtered views that are saved queries ("everything from my bank, any account", "unanswered threads older than a week").
- **Search that understands.** Exact search (words, phrases, senders, dates) and semantic search ("the email where someone offered to cover my shift") over all mail, on the machine, through [rhypedb](https://github.com/joeleaver/rhypedb).
- **Agent-first.** Every action the window offers is a command, and every command is an MCP tool. The window is one client of the command layer; an agent is another. Nothing the window can do is out of an agent's reach, and nothing an agent can do skips the person's say on what matters (see **Safety**).
- **Local-first.** Mail, the index and the tokens live on the user's machine. There is no Postrinch server.

## Non-goals (for now)

- A web or mobile client.
- A hosted service, sync between machines, or shared mailboxes.
- Contacts and tasks, beyond reading what is in a message.
- Calendars other than Google's (CalDAV, iCloud, Exchange).
- Exchange/EWS. Outlook.com over IMAP with OAuth may come later.

## Shape

```
┌─────────────────────────────────────────────────────────────────┐
│ postrinch (the app)                                             │
│                                                                 │
│   rinch UI ──┐                       ┌── MCP server (local)     │
│              ▼                       ▼                          │
│        ┌───────────── command layer ─────────────┐              │
│        │ search · read · draft · send · label ·  │              │
│        │ archive · views · accounts · rules       │              │
│        └──────┬──────────────┬───────────┬───────┘              │
│               ▼              ▼           ▼                      │
│          store (rhypedb)   sync        render                   │
│          objects, links,   Gmail API,  parse, sanitize,         │
│          full text,        IMAP/SMTP   map to rinch DOM         │
│          vectors                                                │
└─────────────────────────────────────────────────────────────────┘
        tokens in the OS keychain · mail and index on local disk
```

Planned crates (a Cargo workspace, names provisional):

| Crate | Holds |
|---|---|
| `postrinch-model` | Accounts, messages, threads, labels/folders, views; no I/O. |
| `postrinch-store` | The rhypedb schema and every read and write of it; full-text and vector indexing; embeddings in the background. |
| `postrinch-sync` | Google sign-in and Gmail sync (built); IMAP/SMTP and the outbox to come. A provider trait arrives with the second provider, shaped by both. |
| `postrinch-cli` | `postrinch-cli`: add accounts, sync, search, show a message. For development and scripts. |
| `postrinch-render` | MIME to displayable parts; HTML parse and sanitize; remote content policy; handing a document to rinch. |
| `postrinch-commands` | The command layer: every action, its arguments, its result, and whether it needs the person's confirmation. |
| `postrinch-agents` | Agents: the agents file (keys and reach), the wire format between the bridge and the app, the app's listener, the bridge's client, and what each call answers. |
| `postrinch-mcp` | The MCP server agents launch over stdio (rmcp); each tool is one call to the running app. |
| `postrinch` | The rinch app: the window over the command layer, with sync running in the background (`GmailSync::keep_in_sync`). Its `debug` feature turns on rinch's debug server for tools; it is never on in a build for use, because it shows the window, mail included, to any local process. |

## Decisions so far

### Accounts and sign-in

- **Gmail signs in with OAuth 2.0**, the installed-app flow: authorization code with PKCE, the system browser, a loopback redirect. Each account added runs the flow once; its refresh token is kept in the OS keychain (`keyring`), keyed by the account's address. `prompt=select_account` when adding, `login_hint` when an account must sign in again. Detail and Google's rules: [docs/research/gmail-auth.md](docs/research/gmail-auth.md).
- **Every account can need to sign in again** at any time (revoked, password changed, unused for six months, an admin policy). That is a state of the account, shown on it, not an error for the app.
- **Gmail over the Gmail API, everything else over IMAP/SMTP.** The API gives Gmail's labels and threads as they are, incremental sync from a `historyId`, and Gmail's own search to fall back on. One OAuth scope, `https://mail.google.com/`, also allows IMAP/SMTP, which keeps that route open.
- **Scopes:** `openid`, `email`, `https://mail.google.com/` (restricted) and `https://www.googleapis.com/auth/calendar` (sensitive), asked for together at sign-in.
- **The Google Cloud project** is `postrinch` (organization joeleaver.com), with the Gmail and Calendar APIs enabled, an External consent screen and one "Desktop app" OAuth client. The client's JSON is kept outside the repository (`~/.config/postrinch/google-oauth-client.json` while developing); it never goes into git. The consent screen is in Testing until it has a home page, a privacy policy and an authorized domain, so for now only listed test users can sign in and their refresh tokens last 7 days.
- **App passwords** are a manual fallback for IMAP accounts only, never the main path.
- **OAuth is written on reqwest, not the `oauth2` crate**, which would pull in a second reqwest. A grant that leaves out the Gmail or Calendar scope is refused, as is a token the keychain cannot keep. **Revoking a Google refresh token withdraws the whole grant** for that account and client, every token under it included, so Postrinch revokes only when it holds no other token for the account (a refused first sign-in) and on sign-out; a new sign-in simply replaces the stored token. Google retires an account's oldest token past 100 per client.
- **The keychain, per platform** (the `keyring` crate): Keychain Services on macOS; Credential Manager on Windows, with local persistence so the token does not roam with a domain profile; the Secret Service on Linux (GNOME Keyring or KDE Wallet). When a Linux Secret Service has no default keyring (GNOME Keyring outside a GNOME session has only an in-memory one), Postrinch asks it to create one, which prompts the person for its password.

### Gmail sync

- **Gmail's quota sets the speed.** Each user of a project gets 6,000 units a minute; `messages.get` costs 60 with `format=raw` and 20 with `metadata` or `minimal`. So one account downloads at most about 100 whole messages a minute (measured: 99), whatever the client does; batching does not change the units. Above 80 million units a day per project, Google has said charges will apply. Every request draws its cost from a per-account token bucket that starts a little under Gmail's limit, halves on a rate-limit answer and recovers while Gmail is quiet. (IMAP for bulk download was considered and set aside for Google accounts.)
- **Headers first, bodies after.** A full sync lists every message id (spam and trash included), stores each new one from its headers alone (`format=metadata`; measured 274 a minute), and removes stored ones Gmail no longer has. The mailbox is then listed, threaded and searchable by sender and subject. **Backfill** downloads the bodies (`format=raw`) newest first at background priority: it spends only quota foreground work leaves and keeps a reserve, so a sync or an opened message never waits behind it. New mail found by incremental sync is downloaded whole at once. A message records whether it is downloaded; headers never overwrite a downloaded body. **Incremental sync** reads `history.list` from the cursor and applies it: deletions, new messages, label changes (`format=minimal` for those).
- **The cursor** is `FullSince{historyId}` while a full sync runs, taken before listing, so an interrupted one resumes and then catches up from that point; and `History{historyId}` after. When Gmail's history has expired (404), a full sync runs that also refreshes stored messages' labels.
- **Labels:** `UNREAD` and `STARRED` become flags; Gmail system labels the model has no place for (`CHAT`, the coloured stars) are not stored.
- **Requests** go over HTTP/1.1 (h2 0.4.16+ drops Gmail's connections as a "flood" of small DATA frames, and reqwest does not expose the limit). They retry dropped connections and 5xx with exponential backoff (honouring `Retry-After`), and a 401 once with a fresh token. The store is blocking, so sync runs every store call on a blocking thread.

### Changing mail

- **Local first, through an outbox.** An action (read, unread, star, archive, back to the inbox, trash, restore, add or remove a label) names threads, as the window and an agent see mail (`Commands::act`, `ThreadAction`). It becomes one `MessageChange` to the messages it would change (unread and star go to the latest message, as in Gmail), which the store applies at once and records as an `OutboxEntry` in the same database. Nothing waits on the network.
- **Labels by id; trash is a label.** A change adds or removes provider label ids and sets read or starred. Archiving removes the inbox label; trashing adds the trash label. Every mailbox but the trash and spam leaves out trashed and spam mail, and so does search, as Gmail's do, so this holds whatever labels Gmail itself strips on trashing.
- **Sync never undoes a change on its way out.** Every store write of a message's labels and flags applies the account's outbox entries over it, in order. A change only adds, removes or sets, so applying it again is harmless. An entry stays after it is sent, until a sync that began after it finishes: that sync's report already includes it.
- **Sending** (`GmailSync::keep_sending`) runs beside sync, so a first sync of hours never holds a change back, and is woken when a change is made. Oldest first: the trash through `messages.trash`/`untrash`, the rest through `messages.batchModify`. A change Gmail refuses for good (400, 404) is dropped and its messages read again; any other failure keeps it, and the ones after it wait behind it, so Gmail gets them in the order they were made. Entries survive restarts and going offline.
- **None of these asks for confirmation**: each can be undone and stays in the account. Deleting for good, sending and anything that leaves the account will (see **Safety**).
- **In the window**, an action shows at once: it is kept as pending and applied over the rows and the reader until a list loaded after it arrives. Settings › Reading says when an opened conversation is marked read (on open, the default; after 3 seconds; or only when asked) and what opens after the open one is archived or trashed (the next, the default; or nothing).

### Agents

- **An agent is an app the person let in**, with a name, a key and the accounts it sees (every account, those added later included, or a list). The agents file (`<config>/postrinch/agents.json`, on this computer only, owner-only) keeps each key's SHA-256; the key itself (`postrinch_agent_<id>_<secret>`) is shown once. **Settings › Agents** makes, changes and removes them (`postrinch-cli agent add/list/remove` does the same from a terminal).
- **Connecting an app writes one entry into its own settings**: Claude Desktop (`<config>/Claude/claude_desktop_config.json`), Claude Code (`~/.claude.json`), Cursor (`~/.cursor/mcp.json`), Codex (`$CODEX_HOME/config.toml` or `~/.codex/config.toml`). The file is backed up beside itself first (`.postrinch-backup`), the rest of it is kept as it was (key order, comments, permissions), and a file that does not read is left alone: the person gets the key and the entry to paste instead, as for any app set up by hand. The agents file records where the entry went, so removing the agent takes it out again. An app counts as found when its settings file or folder exists.
- **The app answers; the bridge only carries.** `postrinch-mcp` holds no mail and opens no store: the store takes one opener, and the window is it. It sends each tool call over loopback (`127.0.0.1:7465`, or `POSTRINCH_APP_ADDR`) as one JSON line, after a hello carrying the key from `POSTRINCH_AGENT_KEY`. The app checks the key, reads the agents file again on every call (so removing or narrowing an agent applies to an open connection), and runs the call through the command layer limited to the agent's accounts (`Commands::limited_to`, which never widens). To an agent, the other accounts do not exist. With the app closed, every tool says so in a sentence; starting the app for an agent is for later.
- **Tools:** `list_accounts`, `list_folders`, `list_threads`, `read_thread`, `search`, `list_labels` and `change_threads` (read, star, archive, trash, labels, each undone by its opposite). A change an agent makes shows in the window at once and wakes sync's sender, as the window's own do.
- **Mail's words are kept apart.** Whatever a sender wrote (subjects, names, addresses, snippets, bodies, attachment names) is under an `untrusted` key in every result, and the server's instructions say to read it as data. Text hidden by rendering is not flagged yet: that waits for the renderer (see **Safety**).

### Storage and search

- **rhypedb in process** (`rhypedb-engine`), not a server. Messages, threads, people and labels are objects with links between them; bodies are `@fulltext`; each message (or each chunk of a long one) has an embedding from `rhypedb-embed` (fastembed/ONNX, local, computed in the background).
- **Saved views are queries** in rhypedb's query language, stored as objects, so the window and an agent create and read them the same way. A view must read `Message` and only filter, match, rank or page it (no traversal, no writes); the store refuses anything else when it is saved. That makes the `Message` type in `crates/postrinch-store/src/schema.rhype` a public contract: changing its fields breaks saved views.
- **The schema** (`schema.rhype`) has `Account`, `Label`, `Thread`, `Message`, `MessageContent`, `Person`, `SavedView` and `StoreInfo`. An object a provider names within an account has a unique `key` of the account's address and the provider's id. People are shared across accounts, one per address, so "everything from this person" spans accounts. Deleting an account cascades to its labels and threads, a thread to its messages, a message to its content.
- **Messages stay small.** rhypedb copies an object's whole scalars into the reverse entry of each of its links when it has another to-one relation, and into the index entry of each `@indexed` field (joeleaver/rhypedb#26). With the body and headers on `Message` that made the database 432 MB for 3,700 messages, more than the raw mail. So `Message` holds only small fields and one to-one relation (`thread`; its account is reached through the thread), and what a message says (search text, body, headers, attachments) is in its `MessageContent`, which has one relation and no indexed field and is copied nowhere. Tests guard both rules.
- **Store format, during development:** the store records a format version; a store of another format (or one whose data the schema no longer has) is refused, with the instruction to delete it and add the accounts again (raw messages stay). Upgrades in place come with the first release.
- **Writes repeat safely.** rhypedb commits one object at a time, so storing a message is several commits (the message, then its label and people links). Every store write is an upsert that also repairs links, and the sync position is written only after what came before it is stored, so a crash means repeated work, never lost or half-linked mail.
- **Raw messages are files, not rows**: a content-addressed blob directory beside the database (`blobs/ab/cdef….zst`, written to a temporary file, synced and renamed), because a message with attachments can be megabytes. Each is compressed with zstd at its default level: real mail shrinks by about 37% (446 MB to 282 MB for 3,718 messages), and higher levels save under 2% more. A blob is named by the SHA-256 of its uncompressed bytes, and reading it checks that hash, so a damaged file is an error rather than a wrong message. Garbage collection removes blobs no message refers to, sparing recent ones a writer may be about to refer to.
- **Listing a mailbox stays cheap.** Each thread keeps when its latest message arrived (`Thread.latest`, kept as messages are stored and removed), so a mailbox is put in order from its threads alone, not by reading every message in them. Label counts are kept until the next write, as the sidebar and the thread list ask for them one after the other. A store from before `latest` existed opens as it is: its threads are filled in once, in the background. Measured on an 18,700-thread inbox: a reload went from about 3 s of CPU to 0.55 s.
- **Full text** is one `@fulltext(english)` field per message, on its `MessageContent`: subject, names and addresses, and body. Whether subject and people need their own weight is for the search step to measure.

### Labels

- **Labels are a tree**, split on "/". Gmail's own labels have fixed places (the unified inboxes, Sent, Drafts, and the categories as inbox tabs); the sidebar's tree is the labels people and apps made.
- **Other apps' labels are set apart**, under "From other apps", collapsed: bracketed namespaces (`[Superhuman]/…`, `[Zoom Mail]`, `[Imap]/…`), SaneBox's `@Sane…`, and the folders IMAP clients create (`Deleted Items`, `Notes`, `Sync Issues`…). Anyone who has tried a few mail apps has these; they should not crowd the person's own.
- **What shows by default:** labels Gmail lists, with mail in them; show-if-unread labels while they hold unread mail. "Show all labels" brings back the rest.
- **Folder or tag is read from use:** a label whose mail is nearly all out of the inbox and filed nowhere else works as a folder; any other works as a tag and shows as a chip on thread rows.
- **The person's changes stay on this computer** (`LabelPrefs`: a name, shown or hidden, pinned, a colour). Sync never touches them and nothing is sent to Gmail.

### Rendering

- **HTML mail is parsed and sanitized before rinch sees it**: an HTML5 parser (html5ever), an allow-list sanitizer, `<style>` kept and scoped, scripts, forms, frames and event attributes dropped.
- **rinch does the layout**: real table layout and legacy attributes (rinch#2079) and floats (rinch#2080) remove the need for the spike's table-to-grid rewrite. See [docs/research/html-rendering.md](docs/research/html-rendering.md).
- **Remote content is off by default** (tracking pixels), per sender or per message on request, fetched without cookies.
- **"Open in browser"** stays as the fallback for mail that still renders badly.

### Safety

Mail is hostile input, for the renderer and for the agent.

- **Commands say what they risk.** Reading, searching and drafting run freely. Sending, deleting, unsubscribing, forwarding outside the account's domain and anything else that leaves the machine or cannot be undone asks the person in the window, showing exactly what will happen, before it runs. An agent cannot grant itself that confirmation.
- **Mail reaches an agent as data, marked as such.** Tool results carry message content in fields that say it is untrusted third-party text, and the rendered-hidden parts (white-on-white, 1px, `display:none` text) are flagged rather than silently passed on: the rendering spike's hostile fixture hid an instruction to an AI that a sanitizer does not remove.
- **No secret leaves the keychain** through a tool. An agent never sees a token.

## Open questions

1. **Does a local-only client need Google's yearly CASA assessment?** Google's pages disagree; ask Google's verification team before relying on "no server, no assessment". Until it is answered, Postrinch runs as an unverified "in production" project (100 users, a warning screen), plus a bring-your-own-client-ID setting. See [docs/research/gmail-auth.md](docs/research/gmail-auth.md).
2. ~~How does the MCP server reach agents?~~ **Decided 2026-11-03, after Pimble's agents contract:** each connected app is an *agent* with its own name and key (shown once, stored hashed, on this machine only). Apps launch a small stdio bridge, `postrinch-mcp`, with the key in the environment, never on the command line; the bridge talks to the running app over loopback, and the app enforces the agent's scope: which accounts it can see, and that sending, deleting and anything else that leaves the machine still wait for the person's confirmation. Settings › Agents offers one-click setup that writes the entry into Claude Desktop, Claude Code, Cursor and Codex configs (leaving the rest of the file alone and keeping a backup), and a snippet to paste for any other app. A remote agent (one that cannot launch a process here) is for later.
3. **What does an embedding cover?** A whole message, chunks of a long one, the quoted history or not, attachments' text or not. Decides index size and search quality; measure on a real mailbox.
4. **Threading for IMAP accounts.** `References`/`In-Reply-To` with subject fallback (JWZ), and how a thread spanning two accounts shows in the combined inbox.
5. **Sending mail through the outbox.** Changes to mail go through it now (see **Changing mail**); sending adds what "sent" means before Gmail has it, and the person's confirmation.
6. **How much mail to keep locally.** Everything, or a window plus headers, for a 15-year Gmail account.
7. **License.** rinch and rhypedb are MIT/Apache-2.0; Postrinch probably follows.

## First steps

Planning and progress live in Overboard, project "Postrinch". There is no `.eml`/mbox importer: real mail comes in through Gmail sync.

Status as of 2026-11-03 (evening):

1. ~~**Gmail read-only sync into rhypedb**~~ Done. Several accounts; headers first, page by page as Gmail lists them, the newest 100 whole; bodies backfilled newest first; an opened message downloads at once; quota paced and fair; retried across network drops (`GmailSync::keep_in_sync`).
2. **Rendering on real mail.** Not started: the reader shows `body_text`. Waiting partly on rinch's email-rendering work.
3. **Search**: full text done (`Commands::search`, title-bar search with marked excerpts). Semantic search and the filter popover are not.
4. **Calendar read-only sync.** Not started.
5. **The command layer and the MCP server.** Read commands done (`postrinch-commands`: sidebar, listings, threads, search, accounts, label prefs), and changing mail (`act`, `label_choices`). The MCP server, bridge and agents are built (see **Agents**), with Settings › Agents and one-click setup for Claude Desktop, Claude Code, Cursor and Codex.
6. ~~**The window**~~ Built: borderless, menus, sidebar with the label tree, inbox tabs, list, reader, search, Settings (Accounts, Appearance), light and dark, resizable panes, keyboard moves. Loads run on a loader thread, never the UI thread. Calendar views wait for step 4.
7. **Changing mail, then writing.** Changing mail done: read, star, archive, trash and labels from the reader and Gmail's keys, applied at once and sent through the outbox; mark read on open. Checked live against Gmail on 2026-11-03: each change reached Gmail within seconds and was not undone by the syncs after it. Next: drafts and sending through the same outbox, with confirmations.
8. **IMAP/SMTP.**

Known issues upstream, none blocking: rinch#1544 (glyphs on whole pixels: uneven letter spacing), rinch#2105 (`if let` in rsx keeps its first value; the window uses keyed `for` loops instead), rinch#2116 (menu dropdown scrollbar from rounding), rhypedb#27 (a `@unique` key that prefixes another is missed after reopen, so duplicates are created), rinch#723 (the desktop DOM never frees a node a redraw throws away; until it does, the window's lists are keyed by what a row is, and each row follows its data through a `Memo`, so rows are built once rather than on every change), rhypedb#28 (compaction reads the whole database into memory and rewrites all of it every ~16 MB of writes), rinch#2129 (a keyed `for` puts rows in the wrong order when an update moves several; the thread list keys rows by thread and latest activity, so a thread that moves gets a new row and nothing is ever moved), rhypedb#29 (dropping a database can return before its lock is released, so reopening at once is occasionally refused).
