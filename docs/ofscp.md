# OFSCP: groups, channels and private messages

**Status: proposed (2026-11-26), waiting on Joe's decisions (the end of this file). No code yet.**

Postrinch becomes a client of OFSCP, the Open Federated Social Communications Protocol
(`~/dev/ofscp`, v0.1): forums, blogs, group chat and private messages, with one account usable
from any client. It sits beside mail and calendar as a third kind of account, local-first like
the rest: what Postrinch has seen is kept on the machine, searchable, and reachable by agents.

Mockups: [design/mockups/ofscp-add-account.html](../design/mockups/ofscp-add-account.html) and
[design/mockups/ofscp-social.html](../design/mockups/ofscp-social.html).
Spec questions found on the way: [docs/research/ofscp-spec-questions.md](research/ofscp-spec-questions.md).

## OFSCP in one page, as a client sees it

- **An account is a handle on a provider**, `joe@forumall.app`. The part after the @ *is* the
  provider's address; discovery is `GET https://{domain}/.well-known/ofscp-provider`.
- **There is no OAuth and there are no access tokens.** The client sends the handle and password
  to the home provider once (`/api/auth/login`), gets a one-use token valid for 300 s, and with it
  registers an Ed25519 public key it made for this device (`POST /api/auth/device-keys`, answer
  `key_id`). From then on **every request to every provider is signed with that key** (custom
  `X-OFSCP-*` headers over a fixed seven-line string). Keys never expire; signing out is revoking
  one's own key; a password reset revokes them all. Requests more than ±300 s off the provider's
  clock are refused; a `503` is retried with a fresh signature.
- **Federation is client-driven fan-out.** The home provider never relays. The client talks
  straight to whichever provider holds a thing, with the same key: a group's channels on the
  group's provider (REST and its own WebSocket), a private message to the *recipient's* provider
  (then a pointer at home). Remote providers check signatures by fetching the user's keys from
  the user's home.
- **Real time** is one WebSocket per provider (`wss://{domain}/api/ws`), signed in by answering a
  challenge. Channel events come by subscription; on reconnect the client re-subscribes with each
  channel's last cursor and the provider replays what was missed, or says `truncated` and the
  client backfills over REST. Delivery is at-least-once (dedupe by id). Private messages,
  notifications and read markers come on the home socket without subscribing and are never
  replayed: re-read them over REST after a reconnect.
- **Things:** groups (join policy, tier, roles, an action→roles permission map), channels (only
  `text` or `call`), messages (`message` = chat line, `memo` = short post, `article` = long post,
  in `text/plain`, `text/markdown` or html), one level of replies (`reference.id`), edits,
  deletes as tombstones, reactions (no server counts), attachments (uploaded first to
  `/api/media`), private conversations between two people (`dmId` = hash of both handles),
  contacts, follows (pointers only: **the home feed is built by the client**), notifications and
  read markers (both only for channels on the home provider), presence, privacy settings.
  Forum and blog are not channel types: a "blog" is a text channel where only some roles may post
  articles; a "forum" one where members may only reply.
- **Calls** are WebRTC peer to peer with the provider only coordinating, and the spec does not yet
  define how offers and answers reach the other participants (spec question 2).
- **No end-to-end encryption** in v0.1; the spec asks clients to say so.

Forumall (`~/dev/forumall`) is the reference provider and web client. It shows the wire mechanics
well but was finished in June and predates several spec changes (DM pointers, read markers,
notifications, mentions), so the spec decides behaviour and Forumall is for interop testing.

## Where it fits in Postrinch

### Accounts and sign-in

- **An OFSCP account is its own kind, not a mail `Account`.** It gets its own store type,
  `SocialAccount`, keyed by the canonical handle (`joe@forumall.app`, lower-cased). Mail's
  `Account` is keyed by `EmailAddress` and every place that matches on `Provider` assumes labels,
  folders and an outbox of mail; a third `Provider` arm would be "not applicable" almost
  everywhere. Settings › Accounts lists both kinds together; the agents file names either.
- **Adding one** is a fourth choice in Add account, "Groups and messages" (mockup states 1–5):
  the handle, discovery shown before the password, the password sent once, a key made and
  registered as "Postrinch on {hostname}", then the first sync. Nothing is kept until every step
  passes. **The password is never stored**; the private key is, in the OS keychain
  (`Keychain::OFSCP`, service `postrinch.ofscp`, keyed by handle), with the `key_id` in the store.
- **Needs to sign in** (key revoked: WebSocket close 4003, or 401 on a fresh signature) is a state
  of the account, shown on it, as for mail. Signing in again makes a new key.
- **Clock skew** is detected from the provider's `Date` header and kept per provider as an
  offset used when signing; if it is past the limit anyway, the account says so plainly
  (mockup "Clock is off").
- **Making an account** happens on the provider's site, in the browser (registration modes,
  invites and terms differ per provider).
- **Sign out and remove** revokes this computer's key at the provider, then deletes the keychain
  entry and everything stored for the account.

### Crates

| Crate | Holds | New? |
|---|---|---|
| `postrinch-sync-core` | What every sync engine shares, moved out of `postrinch-sync` unchanged: `SecretStore`, `Keychain`, `MemorySecrets`, the Linux default-keyring step, `Backoff`. | new (extracted) |
| `postrinch-ofscp` | The protocol, with no store and no Postrinch types. `wire/`: serde types for every schema used; `sign/`: the canonical string, Ed25519 signing, WS-auth string, dmId; `http/`: a signed reqwest client per provider (discovery, auth bootstrap, device keys, the endpoints, problem-details errors, 503 retry, clock offset); `ws/`: tokio-tungstenite socket (challenge auth, ping/pong, close codes, subscribe/resume, correlation of commands to answers). Usable outside Postrinch. | new |
| `postrinch-ofscp-fake` | A fake OFSCP provider on loopback for tests, like `postrinch-mailtest`: REST and WS for what the client uses, several providers at once for federation, and switchable faults (`truncated`, duplicates, 503, 4003, clock skew, dropped sockets). | new, tests only |
| `postrinch-social` | The OFSCP sync engine: accounts and sign-in, one session per (account, provider), initial sync, live events, resume and backfill, the outbox of changes, private-message merging, mention detection. Writes the store; reports `SocialEvent`s. | new |
| `postrinch-store` | New types (below), a format bump, and the reads and writes. | changed |
| `postrinch-render` | `social.rs`: message content to rinch DOM. Plain text linkified (`quotes::linked`); `text/markdown` through `pulldown-cmark` to HTML; HTML through the same `rinch_html::clean` and picture policy as mail (`notes.rs` path). | changed |
| `postrinch-commands` | `social/`: places, pages, threads, feed, search, post/reply/edit/delete/react (into the outbox), read markers, follows. | changed |
| `postrinch-agents`, `postrinch-mcp` | `social_tools.rs` router and wire calls (below). | changed |
| `postrinch` | The Social mode, its sidebar and panes, add-account steps, settings page, notices. | changed |
| `postrinch-cli` | `add-ofscp`, `ofscp sync`, `ofscp post` for development. | changed |

Why a separate engine crate rather than a module of `postrinch-sync`: that crate is mail-shaped
(`MailSync`, passes on a timer, IMAP IDLE) and already large, while OFSCP is long-lived sockets
to many providers. They share only the keychain and backoff, hence `postrinch-sync-core`. The app
starts `Social::keep_connected` on the same sync thread and tokio runtime as `keep_in_sync`.

### Storage (rhypedb)

Following the calendar's precedent: indexed fields for what is filtered and sorted on, the
protocol object kept whole as JSON, keys `"<account>\u001f<id>"`, cascades on account delete.

| Type | Key | Indexed / linked | JSON |
|---|---|---|---|
| `SocialAccount` | handle | state | profile, settings, `key_id` |
| `SocialProvider` | domain | — | discovery document, capabilities, clock offset, fetched at |
| `SocialGroup` | acct + group URI | account link, provider domain | group, my role, how found (joined here, a follow, an invite) |
| `SocialPlace` | acct + channel URI or dmId | account, group (optional), kind (`chat`/`posts`/`call`/`dm`), followed, last activity | channel or conversation, resume cursors (two for a DM, one per provider), read marker, notify mode |
| `SocialMessage` | acct + place + message id | place link, author, type, created, parent id, deleted, `mentions_me`, unread | the message whole (attachments, metadata), aggregated reactions |
| `SocialMessageContent` | as message | `search_text @fulltext(english)` | — |
| `SocialNotification` | acct + id | account, created, read | the notification |
| `SocialOutbox` | acct + client id | account, place, seq | the change (post, reply, edit, delete, react, unreact, read marker, follow, DM pointer), attempts, last error |

- A place is a channel or a private conversation, so messages, the stream, the composer and
  search are one code path. A channel's **kind** is derived for display: `posts` where the
  channel's permissions allow memos/articles and not chat, or most of its messages are posts;
  `call` for call channels; otherwise `chat`. The person can switch a channel's display.
- Message stays small (the mail rule, guarded by a test the same way); content lives apart.
- **History depth:** a place keeps what has been fetched: the newest page (50) when first seen,
  everything live after that, older pages on scroll. No full-history download.
- Presence and typing are kept in memory only.
- The pending outbox is laid over every read, as mail's outbox is, so a message being sent shows
  in the stream (faded) and sync never undoes a pending change.

### Sync engine (`postrinch-social`)

- **Sessions.** One per (account, provider) that hosts something the account keeps: the home
  provider always, plus each provider with a joined group or a followed channel. A session owns
  the signed HTTP client and the socket. Reconnect backs off 0.5 s doubling to 30 s with jitter;
  4003 stops the session and marks the account Needs to sign in; 4001 retries.
- **After every (re)connect:** subscribe each place on that provider with its stored cursor;
  `truncated` → REST backfill from the cursor forward; the home session also re-reads private
  conversations, notifications, read markers and follows over REST, since those are not replayed.
- **Writes are idempotent** (upsert by id), because delivery is at-least-once.
- **The outbox** sends in order per place, over the socket (`message.create` with a
  `clientMessageId`, edits, deletes, reactions) or REST, and survives disconnects and restarts
  (Forumall's client drops commands while disconnected; Postrinch retries them). A sent message
  is matched to the provider's copy by `clientMessageId`, else by the command's correlation id.
- **Private messages:** sending posts to the recipient's provider, then puts the conversation
  pointer at home; reading merges both providers' pages by `createdAt`, deduped by id, with a
  cursor per side.
- **Mentions:** notifications from the home provider; for channels elsewhere, which produce no
  notification (spec question 8), Postrinch scans incoming messages for `@handle` and
  `@handle@domain` itself.
- **Unread:** read markers where the home provider keeps them; local markers for remote channels
  (they cannot sync between devices; spec question 18).
- **Groups on other providers** are remembered locally. On a new computer they are found again
  only through follows and invites, until the spec adds membership pointers (spec question 3).
- `SocialEvent` (started, synced, failed, needs sign-in, message arrived, sent, send failed)
  feeds the window as `SyncEvent` does: sidebar status lines, the `revision` bump, notices.

### The window

- **A third mode, Social, beside Mail and Calendar** (mockup ofscp-social). Its sidebar: Home
  (the feed), Mentions, Messages (private), then each group with its provider's domain and its
  channels (`#` chat, `▤` posts, `◖` call), then "Join a group…". Counts are unread; badges are
  mentions and private messages.
- **Chat places** take the list and reader's width as one stream with the composer at the
  bottom; "N replies" opens the thread beside it. Hover tools: react, reply in thread, edit
  (own, until `editUntil`), more (delete, copy link, open in browser).
- **Posts places** read like mail: the list of posts, then the post and its replies in the
  reader; ✎ writes a new post where the channel allows it.
- **Home** lists articles and memos from every followed channel, newest first, opening in the
  same reader. Follow and unfollow are on the channel's header (one affordance, on the object).
- **Messages** lists conversations, newest activity first; a conversation is a chat place with
  the encryption banner the spec asks for and the other person's presence.
- **Call channels** show who is in a call and say plainly that Postrinch can't join (v1).
- **Composer:** plain text with Markdown; Enter sends, Shift+Enter breaks a line; spelling as in
  mail; attachments when the spec lets a message carry them (spec question 1).
- **Notices** (`notify.rs`): mentions, replies to me, private messages, per place's notify mode
  (all / mentions / none; the provider's setting where it keeps one).
- **Settings › Accounts** shows the account (mockup state 5): provider, groups, connected
  providers, the key's name, presence, Profile…, Privacy…, Sign out and remove.

### Agents (MCP)

New router `social_tools.rs`, one call each through the running app, everything a third party
wrote under `untrusted`:

- Reads, at once: `list_groups`, `list_places`, `read_place` (a page, newest first, `before`),
  `read_social_thread`, `search_social`, `list_conversations`, `list_mentions`.
- Changes that stay with the person, at once: `mark_read`, `follow` / `unfollow` (a follow is a
  private pointer).
- **Everything others would see is a proposal** the person sends or not, as mail is:
  `propose_post` (a post, a reply, a private message; opens in the composer with the agent's
  reason), `propose_reaction`, and edits and deletes of the person's own messages.
- The agents file's reach names social accounts as it names mail accounts.

## Build order: one PR each

Each is a reviewable unit with its tests; independent review and green CI before merging.

| # | PR | Needs |
|---|---|---|
| 0 | This design: `docs/ofscp.md`, spec questions, mockups, DESIGN.md section | — |
| 1 | Extract `postrinch-sync-core` from `postrinch-sync` (no behaviour change) | — |
| 2 | `postrinch-ofscp` core: wire types, signing, WS-auth string, dmId; golden vectors | — |
| 3 | `postrinch-ofscp-fake`: the fake provider, several at once, faults | 2 |
| 4 | `postrinch-ofscp` transport: signed HTTP client, discovery, sign-in and device keys, WebSocket client | 2, 3 |
| 5 | Store: the social types, format bump, reads and writes | — |
| 6 | Render: social content (plain, Markdown via `pulldown-cmark`, HTML), mentions | — |
| 7 | `postrinch-social` I: add, sign in again, sign out; first sync (me, groups, places, follows); CLI `add-ofscp`, `ofscp sync` | 1, 4, 5 |
| 8 | `postrinch-social` II: live sessions, subscribe/resume/backfill, read markers, notifications, mentions, presence | 7 |
| 9 | `postrinch-social` III: the outbox (post, reply, edit, delete, react, read, follow) | 8 |
| 10 | `postrinch-social` IV: private messages (send to their provider + pointer, merged reading) | 9 |
| 11 | Commands: `social/` | 5, 6 |
| 12 | Window: add account and the settings page | 7, 11 |
| 13 | Window: Social mode, sidebar, chat stream, composer, thread pane | 9, 11 |
| 14 | Window: posts places and Home | 13 |
| 15 | Window: private messages | 10, 13 |
| 16 | Notices and unread counts | 8, 13 |
| 17 | Agents: social tools and proposals | 11 |
| 18 | Joining: invite links and join requests (if in v1) | 9, 13 |
| 19 | Demo seed, screenshots, DESIGN.md brought up to date | 15 |

Can run side by side: 1, 2, 5 and 6 from the start; 3 after 2; 11 after 5 and 6; 17 after 11;
14, 15 and 16 after 13.

## Tests

- **Protocol:** the spec's signing vectors (`~/dev/ofscp/tests/`, never run upstream until now)
  and Forumall's golden vectors (`packages/shared`), copied as fixtures with their source named;
  the dmId example from the spec; every wire type round-trips the spec's example JSON.
- **Transport and engine against the fake provider:** sign-in and key registration; signatures
  checked by the fake exactly as the spec says; reconnect and resume; `truncated` → backfill;
  duplicate events; 503 with a fresh signature; 4003 → Needs to sign in; skew; two providers
  for a remote group and a cross-provider private conversation; outbox retried after a dropped
  socket and a restart; pending matched to confirmed.
- **Store and commands:** as the mail tests do (tempdir store, seeded accounts), including the
  "message stays small" guard and cascades on removal.
- **Window:** the demo seed gains social accounts; rinch visual tests for the stream, posts and
  messages panes.
- **Interop with Forumall:** `#[ignore]`d tests run by a CI job like the GreenMail one (bun, two
  Forumall instances on loopback for federation, users registered by the test). Forumall lags
  the spec, so these cover sign-in, signing, groups, chat and resume, not DM pointers, read
  markers or notifications.

## What waits on the spec

Postrinch works around none of these; the feature waits for the spec (Joe contributes there).

- **Posting anything but a plain chat line** (memos, articles, replies, attachments): the
  WebSocket `message.create` has no `type`, `reference` or `attachments`, and there is no REST
  create (question 1). Forumall's client sends extra fields the spec doesn't define.
- **Calls:** signaling is undefined (question 2).
- **Finding remote groups on a new computer** (question 3).
- **Seeing reactions and edits others make to my private messages live** (question 6).
- **Mentions on other providers** come from Postrinch's own scan until the spec defines them
  (question 8).

## Decisions for Joe

1. **v1 scope.** Proposed in: sign in to an existing account; groups and channels on any
   provider; reading everything (chat, memos, articles, replies, reactions, attachments);
   sending chat messages, edits, deletes and reactions; private messages; follows and Home;
   mentions, notifications, unread counts, presence; agents' tools; joining by invite link.
   Proposed out: calls; making an account in the app; running groups (creating groups and
   channels, roles, invites, approving join requests); contacts; editing privacy settings beyond
   presence; guest accounts.
2. **Video calls: out of v1** (recommended). The spec can't be implemented for calls yet
   (question 2), and they need a full WebRTC stack in Rust with a peer connection to every other
   participant. Call channels still show who's in a call.
3. **Posting memos, articles, replies and attachments:** wait for the spec to define them in
   `message.create` (recommended, and Joe can drive the fix), or send Forumall's undefined
   extra fields now.
4. **The third mode, "Social"**, beside Mail and Calendar, rather than groups mixed into the
   mail sidebar. The name is open ("Social", "Groups", "Chat").
5. **A separate account type** (`SocialAccount`) rather than a third `Provider` on mail's
   `Account`.
6. **Agents:** reads, read markers and follows at once; posts, replies, private messages,
   reactions, edits and deletes as proposals.
7. **Interop CI against Forumall** with bun in a separate job, knowing Forumall lags the spec.
8. **History depth:** newest page plus everything live, older on scroll, rather than
   downloading channels' whole history.
