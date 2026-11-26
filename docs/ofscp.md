# OFSCP: groups, channels, posts and private messages

**Status: planned, 2026-11-26.** Joe's decisions are recorded at the end of this file.

Postrinch becomes a client of OFSCP, the Open Federated Social Communications Protocol
(`~/dev/ofscp`, v0.1): forums, blogs, group chat and private messages, with one account usable
from any client. It sits beside mail and calendar as a third kind of account. It is
local-first like the rest: what Postrinch has seen is kept on the machine, searchable, and
reachable by agents. It is also meant to be **a first-class tool for writing memos and
articles**, with pictures and video from the first wave. Building it is also a way to push the
spec forward: where the spec falls short, the fix goes into the spec (below, "Working with the
spec").

Mockups:
- [ofscp-social.html](../design/mockups/ofscp-social.html): the window, views, media, threads,
  articles, Home, messages, editing a view, a picture at full size.
- [ofscp-authoring.html](../design/mockups/ofscp-authoring.html): the composer with pasted and
  dropped files, dragging files over the window, the article and memo editor, preview, the
  profile picture.
- [ofscp-add-account.html](../design/mockups/ofscp-add-account.html): adding an account.

Spec questions: [docs/research/ofscp-spec-questions.md](research/ofscp-spec-questions.md).

## OFSCP in one page, as a client sees it

- **An account is a handle on a provider**, such as `joe@forumall.app`. The part after the @
  *is* the provider's address. Discovery is `GET https://{domain}/.well-known/ofscp-provider`.
- **There is no OAuth, and there are no access tokens.**
  - The client sends the handle and password to the home provider once (`/api/auth/login`)
    and gets back a one-use token valid for 300 s.
  - With that token it registers an Ed25519 public key it made for this device
    (`POST /api/auth/device-keys`), and the provider answers with a `key_id`.
  - From then on, **every request to every provider is signed with that key**, in custom
    `X-OFSCP-*` headers over a fixed seven-line string.
  - Keys never expire. Signing out means revoking your own key; a password reset revokes them
    all.
  - Providers refuse requests more than ±300 s off their clock. A `503` is retried with a
    fresh signature.
- **Federation is driven by the client.** The home provider never relays anything.
  - The client talks straight to whichever provider holds a thing, with the same key: a
    group's channels on the group's provider (REST, plus that provider's own WebSocket); a
    private message on the *recipient's* provider, followed by a pointer at home.
  - Remote providers check a signature by fetching the user's keys from the user's home
    provider.
- **Real time** is one WebSocket per provider (`wss://{domain}/api/ws`), signed in by answering
  a challenge.
  - Channel events come by subscription. On reconnect the client subscribes again with each
    channel's last cursor. The provider replays what was missed, or answers `truncated`, and
    then the client backfills over REST.
  - Delivery is at-least-once, so the client deduplicates by id.
  - Private messages, notifications and read markers arrive on the home socket without a
    subscription. Private messages can still be subscribed by `dmId` with a `since` cursor
    (§7.4) and resume like channels; notifications and read markers have no cursor, so after
    a reconnect the client reads them again over REST.
- **Things in the protocol:**
  - Groups, each with a join policy, a tier, roles and a map of which roles may do which
    actions.
  - Channels, only `text` or `call`.
  - Messages of three types: `message` (a chat line), `memo` (a short post) and `article`
    (a long post). Content is `text/plain`, `text/markdown` or HTML.
  - One level of replies, edits, deletes left as tombstones, and reactions (the server keeps
    no counts).
  - Attachments, uploaded first to `/api/media`.
  - Private conversations between two people. A conversation's `dmId` is a hash of both
    handles.
  - Contacts, and follows. A follow is only a pointer, so the client builds the feed itself.
  - Notifications and read markers, both for channels on the home provider only.
  - Presence and profiles with avatars.
- **Forum and blog are not channel types.** They come from the message types and the
  channel's permissions: a blog is a channel where only some roles may post articles.
- **No end-to-end encryption** in v0.1, and the spec asks clients to say so.

There are two other implementations. Forumall (`~/dev/forumall`) is the reference provider
and web client, but it predates several spec changes. re-ofscp (`~/dev/re-ofscp`) is Joe's
Rust provider, on rhypedb, still being built. In both cases the spec decides behaviour.

## Working with the spec

- **The OFSCP oracle session owns the spec.** It works in `~/dev/ofscp` and writes the spec
  PRs; how to find it is in re-ofscp's CLAUDE.md, "Protocol questions". When a protocol
  question isn't answered by the spec plus its open PRs, ask the oracle. Each question gives
  the section, the concrete case, and the default Postrinch would pick. **Postrinch never
  ships a private guess.** Code that depends on an open spec PR names it in a comment, or, until the PR exists, the
  decision's entry in the questions file ("Oracle answers").
- **The spec work runs alongside the build.** These features are in the first wave, and each
  needs the spec to change first (numbers refer to the questions file):
  - posting memos, articles, replies and attachments (1, 5);
  - content format and sanitizing (14);
  - media upload and access (16);
  - finding remote groups on a new device (3);
  - live reactions and edits on your own private messages (6);
  - the echo of `clientMessageId` (28).
- **Where each question stands** is at the end of the questions file, "Oracle answers".
  Notifications for mentions in remote channels are deferred past v0.1 by the spec (§10.1,
  §13), so Postrinch marks them locally only.
- **Interop:** Forumall to start. re-ofscp, Joe's own provider, as it grows. And the spec
  itself, through shared vectors.

## Where it fits in Postrinch

### The window: a modes rail

The whole window gets **a narrow rail at the far left**: Mail, Calendar and Social, each an
icon with its name under it and a badge of what's unread, with Settings at its foot. It
replaces the Mail | Calendar switch at the top of the sidebar. It is its own PR, done early,
and it changes mail and calendar too.

### Accounts and sign-in

- **An OFSCP account is its own kind** (decided): a `SocialAccount`, keyed by the lower-cased
  handle. It is not a third `Provider` on mail's `Account`. Settings › Accounts lists both
  kinds together, and the agents file names either kind.
- **Adding one** is a fourth choice in Add account, "Groups and messages":
  1. Enter the handle.
  2. Postrinch shows what discovery found before it asks for the password.
  3. The password is sent once.
  4. Postrinch makes a key and registers it as "Postrinch on {hostname}".
  5. The first sync runs.

  Nothing is kept until every step passes. **The password is never stored.** The private key
  is, in the OS keychain (service `postrinch.ofscp`, keyed by handle), and the `key_id` goes in
  the store.
- **Needs to sign in** (the key was revoked: WebSocket close 4003, or a 401 on a fresh
  signature) is a state of the account, as it is for mail. Signing in again makes a new key.
- **Clock skew:** Postrinch learns each provider's offset from its `Date` header and signs
  with it. If the offset is past the limit anyway, the account says so plainly.
- **Making an account** happens on the provider's site, in the browser.
- **The profile** has a name, an about line and an avatar. The picture can be chosen,
  dropped or pasted, and is then cropped and uploaded.
- **Sign out and remove** revokes this computer's key, then deletes the keychain entry and
  everything stored for the account.

### Organizing: views, not providers

Places are organized the way mail is: **by the person's own views, across every account and
provider**, not by provider or group.

- **A place** is a channel or a private conversation.
- **A view** is a saved selection of places, with optional filters:
  - kinds (chat, memo, article);
  - words, `from:` and `tag:`;
  - how it reads (a stream, or a list with a reader);
  - its count.

  Notices are not a view setting: they belong to the channel or group (everything, mentions
  and replies, or nothing), as the spec keeps them (§10.2, synced with the home provider), so
  a place shown in two views can't disagree with itself. The view panel shows and changes its
  places' notices. Private conversations are outside §10.2: they always notify, unless muted
  locally.

  A group in a view means all of that group's channels, including ones made later.
- **Built-in views:** Home (posts from followed channels), Mentions, Messages (private
  conversations), Drafts, and, at the foot, All channels (every place Postrinch knows, to
  browse, join and follow from). Built-in views can be edited like any other.
- **Your views** come next, in your order. ✎ on a row opens the view's one panel, as in Mail.
- **Joining a group offers a view for it.** The person can keep it, or add the group to a view
  they already have.
- **A view with several places** shows them as tabs in its header, with "All" merging them.
  In "All", each message says where it comes from, and the composer says where it posts: the
  last place posted to, or the place of the message being answered. ▾ changes it.
- **One view type for both modes?** Not yet. Mail's `SavedView` is a query over mail; a social
  view is a selection of places plus filters. They share the panel's design and the sidebar
  behaviour, not the type. Merging them is a later question, once both are used.

### Reading

- **Streams** (chat), loosely after Slack and Discord without copying them:
  - messages grouped by author, with avatars;
  - a toolbar on hover: react, reply in a thread, edit (your own, until `editUntil`), and
    more;
  - "N replies" opens the thread beside the stream;
  - your own message shows faded until the provider confirms it.
- **Lists and a reader** (posts) read like mail. An article has its cover, its title,
  pictures and videos inline, and its replies below.
- **Media:**
  - Pictures show large, and open full size over the window.
  - Videos play in place through `rinch-video` (libmpv).
  - Other files show as cards.
  - Media is fetched by its URL, unsigned (attachment URLs are unguessable capability URLs,
    possibly on a CDN or object store, not the provider), and kept in the content-addressed
    blob store, as mail attachments are. A URL that stops working is fetched again from the
    message, since providers may hand out short-lived URLs or revoke a deleted message's
    files.
- **Private messages** show the encryption banner the spec asks for, and presence on each
  avatar.
- **Call channels** show who is in a call, and say that Postrinch can't join yet (calls come
  in a later wave).

### Writing

- **The composer:**
  - Markdown, with buttons for the common marks.
  - Pictures pasted with Ctrl+V (`rinch-clipboard`), files dropped onto the window
    (`ondropfiles`) or picked with ＋ all go to a tray above the text, each with ✕. A pasted
    bitmap arrives as raw RGBA and is encoded to PNG before upload. Pasting copied *files*
    (a video copied in a file manager) needs `rinch-clipboard` to offer file lists, which it
    doesn't yet ([rinch#3092](https://github.com/joeleaver/rinch/issues/3092)).
  - Uploads start at once, to the account's home provider, and show their progress; Send
    waits for them. The provider's `maxUploadBytes` is checked before uploading.
  - Enter sends; Shift+Enter starts a new line.
- **The article and memo editor** opens in the main pane:
  - Article or Memo, and where it goes (▾ lists the places that allow that kind).
  - Article: a cover, a title, tags, and the body in rinch's rich-text editor
    (`rinch-editor-core` already reads and writes Markdown), with pictures and videos placed
    at the caret. The editor has an image node but no video node, and Markdown has no video
    syntax: the node is [rinch#3091](https://github.com/joeleaver/rinch/issues/3091), and how inline media is referenced in a body goes into
    spec question 14.
  - Memo: a title, plain text and pictures.
  - Drafts save as you type and are listed in Drafts. Preview shows the post exactly as a
    reader will see it.
  - Publish is the rightmost button.
- **Editing a published post** uses the same editor.

### Crates

| Crate | Holds | New? |
|---|---|---|
| `postrinch-sync-core` | What every sync engine shares, moved out of `postrinch-sync` unchanged: `SecretStore`, `Keychain`, `MemorySecrets`, the Linux default-keyring step, `Backoff`. | new (extracted) |
| `postrinch-ofscp` | The protocol, with no store and no Postrinch types. `wire/`: serde types; `sign/`: the canonical string, Ed25519 signing, the WS-auth string, dmId; `http/`: a signed reqwest client per provider (discovery, sign-in and device keys, endpoints, media upload with progress, problem-details errors, 503 retry, clock offset); `ws/`: the socket (challenge auth, ping, close codes, subscribe/resume, correlation). | new |
| `postrinch-ofscp-fake` | A fake provider on loopback for tests, like `postrinch-mailtest`: several providers at once, signatures checked as the spec says, switchable faults. | new, tests only |
| `postrinch-social` | The sync engine: accounts and sign-in, a session per (account, provider), first sync, live events, resume and backfill, the outbox, media upload and fetch, private-message merging, mention scanning. | new |
| `postrinch-store` | New types (below), a format bump, and the reads and writes. | changed |
| `postrinch-render` | `social.rs`: plain text linkified; Markdown through `pulldown-cmark`; HTML through `rinch_html::clean` and the picture policy, as mail is; mentions marked. | changed |
| `postrinch-commands` | `social/`: views, places, pages, threads, search, drafts, posting and changes (into the outbox), read markers, follows, the profile. | changed |
| `postrinch-agents`, `postrinch-mcp` | `social_tools.rs` and its wire calls. | changed |
| `postrinch` | The modes rail, the Social mode, the composer and editor, the add-account steps, the settings page, notices. | changed |
| `postrinch-cli` | `add-ofscp`, `ofscp sync`, `ofscp post`, for development. | changed |

- **Why a separate engine crate:** `postrinch-sync` is shaped by mail (passes on a timer,
  IMAP IDLE) and is already large, while OFSCP keeps sockets open to many providers. The two
  share only the keychain and backoff, hence `postrinch-sync-core`. The app starts
  `Social::keep_connected` on the same sync thread and tokio runtime as `keep_in_sync`.
- **Shared with re-ofscp:** re-ofscp already has `re-ofscp-types` (wire types, no I/O, builds
  for wasm). One Rust crate for the wire types, signing and dmId, used by the server and the
  client, would keep them honest with each other. Proposed: build `postrinch-ofscp`'s I/O-free
  core so that it can move into that shared crate, and agree it with re-ofscp's session
  before PR 2.

### Storage (rhypedb)

The same pattern as the calendar: indexed fields for whatever is filtered or sorted on, the
protocol object kept whole as JSON, keys of the form `"<account>\u001f<id>"`, and cascades
when an account is deleted.

| Type | Key | Indexed / linked | JSON |
|---|---|---|---|
| `SocialAccount` | handle | state | profile, settings, `key_id` |
| `SocialProvider` | domain | — | discovery document, capabilities, clock offset |
| `SocialGroup` | acct + group URI | account, provider | group, my role, how it was found, notices |
| `SocialPlace` | acct + channel URI or dmId | account, group (optional), kind, followed, last activity | channel or conversation, resume cursors, read marker, notices (channels) |
| `SocialView` | view id | order | name, places and groups, filters, read as, count |
| `SocialMessage` | acct + place + message id | place, author, type, created, parent, deleted, `mentions_me`, unread | the message whole, reactions summed |
| `SocialMessageContent` | as message | `search_text @fulltext` | — |
| `SocialMedia` | acct + provider + attachment id | URL, blob hash, fetched | mime, size, width, height, duration, poster |
| `SocialDraft` | draft id | account, place, updated | kind, title, Markdown body, tags, cover, attachments (local files and uploaded ones) |
| `SocialNotification` | acct + id | account, created, read | the notification |
| `SocialOutbox` | acct + client id | account, place, seq | the change (post, reply, edit, delete, react, read marker, follow, DM pointer, profile), attempts, last error |

- Messages stay small (mail's rule, and the same guard test). Media bytes live in the blob
  store.
- **History:** a place keeps the newest page when first seen, everything live after that, and
  older pages when you scroll (decided).
- Presence and typing indicators are kept in memory only.
- The pending outbox is laid over every read, so a message being sent shows up, and sync never
  undoes a pending change.

### Sync engine (`postrinch-social`)

- **Sessions:** one per (account, provider) that hosts something the account keeps. The home
  provider always has one, and so does every provider with a joined group or a followed
  channel.
  - Reconnects back off from 0.5 s, doubling to 30 s, with jitter.
  - Close code 4003 marks the account as needing to sign in. Close code 4001 retries.
- **After every reconnect:** subscribe each place with its stored cursor. A `truncated`
  answer means a REST backfill. The home session also reads private conversations,
  notifications, read markers and follows again.
- **Writes are idempotent** (upserts by id).
- **The outbox:**
  - Sends in order, per place.
  - Survives dropped sockets and restarts.
  - Matches a sent message to the provider's copy by `clientMessageId`.
  - Media is uploaded before the message that carries it.
- **Private messages:** sending goes to the recipient's provider, then the pointer is written
  at home. Reading merges both providers by `createdAt`, deduplicated by id.
- **Mentions:** the home provider's notifications cover home channels. For every place,
  Postrinch also marks `mentions_me` from the messages it syncs, using the spec's mention
  syntax (§10.1). That is the design, not a stopgap: it is local and needs nothing from the
  provider. What the spec lacks (question 8) is a notification for a remote mention that
  reaches the person's other clients.
- **Unread:** the provider's read markers at home; local markers elsewhere.
- **Groups on other providers** are remembered locally. On a new computer they come back
  through follows and invites, until the spec adds membership pointers (questions 3 and 29,
  in progress with the oracle).

### Agents (MCP)

**Every action in Social is an agent tool (Joe: "everything"):**
- Reads: views, places, pages, threads, search, conversations, mentions, drafts.
- Writing: drafts, posts, replies, private messages, reactions, edits, deletes.
- Read markers, follows, joining, and the views themselves.

Everything a third party wrote goes under `untrusted`.

**Anything that speaks for the person to others is a proposal** (decided), as with mail's
`propose_message`: posts, replies, private messages, reactions, edits and deletes of
published messages, joining, and follows. Each opens in Postrinch with the agent's reason
and goes out only when the person presses its button. Local changes apply at once: drafts,
read markers, and the person's views.

## Build order: one PR each

Each PR is a reviewable unit with its tests, and merges after an independent review and green
CI. Some PRs (marked *spec*) build on spec decisions Joe has made (2026-11-26) that are
still on their way into the spec; their code names the spec PR once it exists.

| # | PR | Needs |
|---|---|---|
| 1 | Modes rail for the whole window; the sidebar's Mail \| Calendar switch removed | — |
| 2 | `postrinch-sync-core` extracted (no behaviour change) | — |
| 3 | `postrinch-ofscp` core: wire types, signing, the WS-auth string, dmId; golden vectors (agree sharing with re-ofscp first) | — |
| 4 | `postrinch-ofscp-fake` | 3 |
| 5 | `postrinch-ofscp` transport: signed HTTP, discovery, sign-in and device keys, media upload (*spec*, question 16), WebSocket | 3, 4 |
| 6 | Store: the social types, a format bump | — |
| 7 | Render: social content (plain, Markdown, HTML), mentions (*spec*, question 14) | — |
| 8 | Engine I: add, sign in again, sign out; first sync; CLI | 2, 5, 6 |
| 9 | Engine II: live sessions, resume and backfill, read markers, notifications, mentions, presence | 8 |
| 10 | Engine III: the outbox (chat messages, edits, deletes, reactions, read, follow) (*spec*, question 28) | 9 |
| 11 | Engine IV: media, both ways (upload with progress, fetch and cache, avatars) (*spec*, question 16) | 10 |
| 12 | Engine V: private messages (live reactions and edits on sent ones: *spec*, question 6) | 10 |
| 13 | Commands: `social/` (views, places, pages, threads, search, drafts, changes, profile) | 6, 7 |
| 14 | Window: add account, settings page, profile and avatar | 8, 11, 13 |
| 15 | Window: Social mode, views in the sidebar, the view panel, tabs | 1, 13 |
| 16 | Window: streams, threads, the composer (paste, drop, tray, uploads) (uploads: *spec*, question 16) | 10, 11, 15 |
| 17 | Window: pictures and video (inline, full size, rinch-video), file cards | 11, 16 |
| 18 | Window: lists and the reader, Home | 15, 17 |
| 19 | Window: private messages | 12, 16 |
| 20 | Editor: articles and memos, drafts, preview, publishing (*spec*, question 1) | 13, 17 |
| 21 | Replies and attachments on posts sent (*spec*, question 1) | 16 |
| 22 | Notices and unread counts, on the rail too | 9, 15 |
| 23 | Agents: the social tools (proposals open in the composer, editor and private messages) | 13, 16, 19, 20 |
| 24 | Joining: invite links, join requests, All channels | 10, 15 |
| 25 | Demo seed, visual tests, interop CI, DESIGN.md | 17, 19, 20 |

- **In parallel from the start:** 1, 2, 3, 6 and 7, plus the spec work with the oracle.
- **After 15:** 22 and 24 (once 9 and 10 are in); 18 once 17 is in. 23 comes after the
  composer, editor and private messages (16, 19, 20).
- **Packaging:** `rinch-video` needs libmpv in every release (Linux, Windows, macOS). This is
  checked with the release work before PR 17.
- **Rinch issues** (rinch is a git dependency, never edited here): a video node in
  `rinch-editor-core`, and file lists in `rinch-clipboard`.

## Tests

- **Protocol:** the spec's signing vectors, Forumall's golden vectors, and the spec's dmId
  example, as fixtures that name their source. Every wire type round-trips the spec's example
  JSON.
- **Against the fake provider:**
  - sign-in and key registration, with signatures checked by the fake;
  - reconnect and resume; `truncated` followed by a backfill; duplicate events;
  - a `503` retried with a fresh signature; close 4003; clock skew;
  - two providers, for a remote group and for a private conversation;
  - the outbox retried after a dropped socket and after a restart;
  - an upload interrupted part-way; media fetched once and then served from the cache.
- **Store and commands:** like the mail tests, including the guard that keeps messages small,
  cascades, and drafts surviving a restart.
- **Window:** social accounts in the demo seed, and rinch visual tests for the rail, streams,
  the editor and media.
- **Interop:** `#[ignore]`d tests run by a CI job, like the GreenMail job, against Forumall (two
  instances, for federation), and against re-ofscp once it serves the protocol.

## Decisions (Joe, 2026-11-26)

1. **v1 scope:** as proposed, but organized by the person's views across providers, like
   mail, not by provider or group. Also in the first wave: full multimedia (pictures, videos,
   drag and drop, pasting pictures and videos, avatars), and writing memos, articles, replies
   and attachments, pushed through the spec with the oracle.
2. **Calls:** a later wave (rinch already has WebRTC: `rinch-rtc`, `rinch-av`).
3. **Postrinch is a first-class tool for writing memos and articles.** Replies and attachments
   too. The spec gets fixed for them; the client doesn't work around it.
4. **The mode is called Social.** A modes rail at the far left replaces the Mail | Calendar
   switch.
5. **A separate account type.**
6. **Agents: everything**, with anything that speaks for the person to others as a
   proposal the person sends, as mail does.
7. **Forumall is a start, not the interop target;** the spec is pushed as Postrinch is built.
8. **History:** the newest page, then live, with older pages on scroll.

Slack and Discord are the inspiration for the window, not something to copy.
