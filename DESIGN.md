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
| `postrinch-cli` | `postrinch`: add accounts, sync, search, show a message. For development and scripts until the window exists. |
| `postrinch-render` | MIME to displayable parts; HTML parse and sanitize; remote content policy; handing a document to rinch. |
| `postrinch-commands` | The command layer: every action, its arguments, its result, and whether it needs the person's confirmation. |
| `postrinch-mcp` | The MCP server, generated from the command layer. |
| `postrinch` | The rinch app. |

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

### Storage and search

- **rhypedb in process** (`rhypedb-engine`), not a server. Messages, threads, people and labels are objects with links between them; bodies are `@fulltext`; each message (or each chunk of a long one) has an embedding from `rhypedb-embed` (fastembed/ONNX, local, computed in the background).
- **Saved views are queries** in rhypedb's query language, stored as objects, so the window and an agent create and read them the same way. A view must read `Message` and only filter, match, rank or page it (no traversal, no writes); the store refuses anything else when it is saved. That makes the `Message` type in `crates/postrinch-store/src/schema.rhype` a public contract: changing its fields breaks saved views.
- **The schema** (`schema.rhype`) has `Account`, `Label`, `Thread`, `Message`, `Person` and `SavedView`. An object a provider names within an account has a unique `key` of the account's address and the provider's id. People are shared across accounts, one per address, so "everything from this person" spans accounts. Deleting an account cascades to its labels, threads and messages.
- **Writes repeat safely.** rhypedb commits one object at a time, so storing a message is several commits (the message, then its label and people links). Every store write is an upsert that also repairs links, and the sync position is written only after what came before it is stored, so a crash means repeated work, never lost or half-linked mail.
- **Raw messages are files, not rows**: a content-addressed blob directory beside the database (`blobs/ab/cdef…`, written to a temporary file, synced and renamed), because a message with attachments can be megabytes. Garbage collection removes blobs no message refers to, sparing recent ones a writer may be about to refer to.
- **Full text** is one `@fulltext(english)` field per message: subject, names and addresses, and body. Whether subject and people need their own weight is for the search step to measure.

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
2. **How does the MCP server reach agents?** stdio for an agent the app starts, streamable HTTP on localhost for one that is already running, or both; and how a connecting agent is authorised (a per-agent token the person issues in the window?).
3. **What does an embedding cover?** A whole message, chunks of a long one, the quoted history or not, attachments' text or not. Decides index size and search quality; measure on a real mailbox.
4. **Threading for IMAP accounts.** `References`/`In-Reply-To` with subject fallback (JWZ), and how a thread spanning two accounts shows in the combined inbox.
5. **Offline sending and the outbox.** Queue, retry, and what "sent" means before the server has it.
6. **How much mail to keep locally.** Everything, or a window plus headers, for a 15-year Gmail account.
7. **License.** rinch and rhypedb are MIT/Apache-2.0; Postrinch probably follows.

## First steps

Planning and progress live in Overboard, project "Postrinch". There is no `.eml`/mbox importer: real mail comes in through Gmail sync.

1. **Gmail read-only sync into rhypedb**, one account, then many: the model, the schema, sign-in, full sync, incremental sync.
2. **Rendering on real mail.** Run the synced mail through the pipeline and rinch with #2079/#2080; this decides whether "open in browser" is a fallback or a crutch.
3. **Search** over that store: full text, then semantic, measured on a real mailbox.
4. **Calendar read-only sync** for the same Google accounts.
5. **The command layer and the MCP server**, read-only commands first.
6. **The window**: designed first (mockups), then built: accounts, combined inbox, a thread, search, the calendar.
7. **Writing**: drafts, send through the outbox, confirmations; creating and answering events.
8. **IMAP/SMTP.**
