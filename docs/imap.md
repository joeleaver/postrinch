# IMAP and SMTP accounts

How a plain IMAP/SMTP account (Fastmail, iCloud, a work server, any provider) fits beside the Gmail accounts, which keep the Gmail API. DESIGN.md keeps the decisions in short; this is the detail. Written 2026-11-24.

## The goal

A person adds an IMAP/SMTP account from Settings › Accounts, and then reads, searches, files, flags, writes and sends mail in it as they do in a Gmail account. Agents' tools work over it wherever the operation makes sense. Calendars stay Google's: an IMAP account has none.

## One model, two providers

Postrinch's model was written for labels, and Gmail's labels are a superset of folders. So an IMAP account is shown through the same model rather than beside it:

- **A folder is a label.** Each selectable IMAP folder is a `Label` of the account. A message's labels are the folders holding a copy of it.
- **The special folders are the system labels, with the same ids in every account.** `INBOX`, `SENT`, `DRAFT`, `TRASH` and `SPAM` name the inbox, Sent, Drafts, Trash and Junk whatever a provider calls them (`SystemLabel::provider_id`). So "archive is removing `INBOX`", "trash is adding `TRASH`" and the rules' tests hold for every account by contract, not by luck. The folders are found by SPECIAL-USE (RFC 6154) or, failing that, by the names providers use (`Sent Items`, `Deleted Messages`, `Junk E-mail`…), and the person can choose them in the account's settings.
- **A user folder's label id is minted** (`f` and 12 hex digits) when Postrinch first sees it, and kept in the folder's state with its name on the server. Renaming a folder from Postrinch keeps its id, so its messages, prefs and the rules naming it stay attached. A folder renamed elsewhere looks like one deleted and one made.
- **Categories, Important and Gmail's search** don't exist for IMAP accounts. The Inbox view shows an account without categories whole (as its Primary), the category tabs and rule actions are offered only for accounts that have them (already so in the sidebar), and server search speaks IMAP `SEARCH` instead of Gmail's language.
- **Servers that are label stores underneath** (Gmail over IMAP is the one people meet, and the first live test target) show every label as a folder and every message in `\All` ("[Gmail]/All Mail"). There `\All` takes the archive role (archiving is removing the inbox copy, which Gmail turns into removing the label), and the virtual `\Flagged` and `\Important` folders are not synced as labels: they only mirror the star and Gmail's importance, which the flags already carry. Gmail files sent mail itself, so its Sent append is off. An address can be one account only: adding an address Postrinch already has as a Google account is refused with a sentence.
- **The `\Archive` folder** (or one called `Archive`) is where archiving moves mail. It is an ordinary label, so archived mail shows in All mail as in Gmail. An account with none gets one made the first time something is archived.

## A message's identity

Gmail names a message once for its life. IMAP names a copy by `(folder, UIDVALIDITY, UID)`, which changes on every move. Postrinch's message id must not change on a move, or the outbox could not re-apply changes over sync, an open message would vanish from the reader when archived, and rules would see moved mail as new.

- **The provider message id is derived from the `Message-ID` header**: `m` and the first 24 hex digits of SHA-256 of the account key and the header as written. Copies in several folders are one message with several labels, as in Gmail: a message sent to oneself is in Sent and the inbox.
- **A message without a `Message-ID`** (rare: some automated mail) gets `u` and a hash of the folder's UIDVALIDITY, its id and the UID. It is the one case where a move looks like a delete and a new message; nothing is lost.
- **Where each copy is** lives in the store as an `ImapPlace` (account, folder label id, UIDVALIDITY, UID, message id, the copy's flags and MODSEQ), so a change finds the copies to act on, and sync finds what a folder no longer has.

## Threading

Gmail threads mail itself; for IMAP Postrinch does (DESIGN.md open question 4).

- A thread is named by its root: the thread of the nearest stored ancestor in `References` (from the last entry back) or `In-Reply-To`; failing that, the first `References` entry, then `In-Reply-To`, then the message's own `Message-ID`. The thread id is `t` and a hash of that root, so a reply that arrives before what it answers still lands in the right thread once the root arrives (the root's own id hashes to the same thread).
- No subject-only threading: it joins unrelated "Invoice" mails, and Gmail stopped doing it too.
- A thread is per account, as in Gmail. In the combined inbox, two accounts' copies of one conversation are two rows.

## Sync

Per account, one IMAP connection does the passes; a second one IDLEs.

- **Folders first.** `LIST` (with `SPECIAL-USE` where offered) gives the folders; each becomes a label (`put_label`), each gone is removed. `\Noselect` and `\NonExistent` folders are skipped. Names are decoded from modified UTF-7 and shown with `/` between levels whatever the server's delimiter.
- **Each folder** keeps `ImapFolder` state: name on the server, delimiter, label id, role, UIDVALIDITY, UIDNEXT, HIGHESTMODSEQ.
  - **UIDVALIDITY changed** (or first time): every place of that folder is forgotten and the folder is listed again (`UID FETCH 1:* (UID FLAGS RFC822.SIZE INTERNALDATE BODY.PEEK[HEADER])`, newest first, in batches).
  - **New mail**: `UID FETCH <UIDNEXT>:*` the same way. Headers first, bodies after, as for Gmail: the newest 100 whole, the rest by headers, then backfill downloads bodies (`BODY.PEEK[]`) newest first.
  - **Changed flags**: with CONDSTORE, `UID FETCH 1:* (FLAGS) (CHANGEDSINCE <modseq>)`; without it, `UID FETCH 1:* (FLAGS)` for the whole folder (cheap: a few bytes a message).
  - **Gone mail**: with QRESYNC, `VANISHED (EARLIER)` from the same fetch; without it, `UID SEARCH ALL` compared with the places known. A message whose last place is gone is removed from the store.
  - Folders are synced inbox first, then Sent, Drafts, user folders, Archive, Trash and Junk; the position is written per folder only after what came before it is stored, so an interrupted pass repeats work and never skips mail.
- **Flags**: `\Seen` ↔ read, `\Flagged` ↔ starred, `\Draft` and `\Deleted` honoured (a copy marked `\Deleted` counts as gone). A message is unread if any copy lacks `\Seen` and starred if any has `\Flagged`.
- **New mail at once**: IDLE on the inbox (RFC 2177), restarted every 25 minutes, wakes the account's pass; servers without IDLE are polled with the other accounts (every minute). Other folders change by the minute's pass.
- **An opened message not downloaded yet** is fetched at once (`BODY.PEEK[]` from any of its places), as for Gmail.

## Changing mail

The outbox stays as it is: a `MessageChange` per set of messages, applied here at once, sent in order, applied again over whatever sync brings back until a later sync has it. IMAP's sender turns each change into commands per message, from its places:

| Change | IMAP |
|---|---|
| read / unread | `UID STORE ±FLAGS.SILENT (\Seen)` on every copy |
| star / unstar | `UID STORE ±FLAGS.SILENT (\Flagged)` on every copy |
| add a label (a folder) | `UID COPY` one copy into it |
| remove a label, others remain | delete that copy: `\Deleted` then `UID EXPUNGE` (UIDPLUS) |
| remove `INBOX` with nothing added (archive) | `UID MOVE` the inbox copy to the archive folder |
| remove one folder, add another (move) | `UID MOVE` |
| add `TRASH` | `UID MOVE` one copy to Trash, delete the others (Gmail drops a trashed message's other labels too) |
| remove `TRASH` (restore) | `UID MOVE` back to the inbox, or the folders the change adds |
| add/remove `SPAM` | as for Trash, with Junk |

- Without `MOVE` (RFC 6851), a move is `COPY`, `\Deleted`, `UID EXPUNGE`; without UIDPLUS's `UID EXPUNGE`, a plain `EXPUNGE` only when no other copy in that folder is marked `\Deleted` by someone else (otherwise the change waits and says why).
- The places a command makes (COPYUID/APPENDUID from UIDPLUS) are recorded at once; without UIDPLUS the next sync finds them.
- **Refused for good** (a `NO` naming a missing folder or message): dropped and the messages read again, as Gmail's 400/404. Anything else waits and keeps the order.
- **Labels**: making one is `CREATE` (and `SUBSCRIBE`); renaming is `RENAME`; deleting is `DELETE`. A label with `/` in its name is a nested folder, written with the server's delimiter.

## Writing and sending

- **Sending is SMTP** (`lettre`): submission on 587 with STARTTLS or 465 with TLS, authenticated as the account. The queue (`Outgoing`) and its order, refusals and retries stay as they are.
- **Each message goes once.** SMTP gives no way to ask whether a message got through, so the risk is narrowed: a reply of 4xx before the data is accepted means it did not go and is retried; a 5xx means refused, and waits for the person; a connection lost after the data was sent is the one uncertain case: Postrinch looks for the message's `Message-ID` in Sent (a provider that files sent mail itself shows it there), and when it isn't there the mail waits for the person, saying it may have gone, with Send again and Discard. It is never retried blind.
- **Sent**: after a send, the message is `APPEND`ed to Sent (`\Seen`), unless the account's settings say the provider files sent mail itself (Gmail-like servers, Outlook, iCloud: a setting, on by default and turned off when Sent already holds the message a moment later). The place is recorded from APPENDUID.
- **Drafts live in the Drafts folder.** Keep as draft `APPEND`s the message (`\Draft \Seen`); keeping it again appends the new one and deletes the old; sending deletes it.
- Replies thread by `In-Reply-To` and `References`, which compose already writes.

## Credentials

- **OAuth first, passwords only where there is no OAuth.** As DESIGN.md says, app passwords are a fallback, never the main path: they are long-lived, see the whole account, and are typed by hand. Wherever Postrinch has an OAuth client for the provider, the add-account dialog's way in is "Sign in with <provider>" in the browser, and the password is behind "Use an app password instead". A password is the first choice only for providers that offer no OAuth to apps like Postrinch (iCloud; Fastmail, which keeps OAuth for partners; most work and self-hosted servers).
- **Providers with OAuth in this release:** Google over IMAP (the existing Google client: its `https://mail.google.com/` scope covers IMAP and SMTP, so nothing new is registered), and Microsoft (Outlook.com, Hotmail, Live and Microsoft 365: password sign-in is gone there, so OAuth is the only way; a public "Mobile and desktop" client registered in Microsoft Entra, redirect `http://localhost`, delegated `IMAP.AccessAsUser.All`, `SMTP.Send`, `offline_access`, `openid`, `email`; its client id in `~/.config/postrinch/microsoft-oauth-client.json`, kept out of git like Google's). Yahoo and AOL follow once Yahoo approves a developer app for mail; until then they use app passwords. An organization whose admin requires consent for apps shows that as the account's sign-in trouble, in the admin's words.
- **Passwords and app passwords**: kept in the keychain under the service `postrinch.imap`, keyed by the account's address, as JSON (`{"kind":"password","password":…}`), never in the store. One secret for IMAP and SMTP unless the person gives SMTP its own.
- **OAuth 2 (`XOAUTH2`/`OAUTHBEARER`)** for providers that require it: the same loopback flow as Google's, with the provider's endpoints and client id, its refresh token kept as `{"kind":"oauth2","provider":…,"refresh_token":…}`, and access tokens refreshed before they expire and once more on a refused AUTHENTICATE. The flow and SASL are built generically; adding a provider is its endpoints, scopes and client id. Which provider an account uses is found from discovery (the ISPDB's `<authentication>OAuth2</authentication>` and the MX hosting), never asked.
- A refused login (`AUTHENTICATIONFAILED`, SMTP 535, or a refused refresh token) is the account's "needs to sign in" state with a sentence from the server, as for Google; Sign in again opens the browser for OAuth accounts and asks for the password otherwise.

## The account's settings

`ImapSettings`, on the account in the store (no secrets): the IMAP host, port and security (TLS, STARTTLS), the SMTP host, port and security, the user name for each, how it signs in (password or a named OAuth provider), the folders chosen for each role when the person overrides SPECIAL-USE, and whether the provider files sent mail itself. Plain-text connections are not offered.

## Adding an account

From Settings › Accounts › Add account › Other mail account (the HTML mockup is the design, `design/mockups/add-imap-account.html`):

1. **Address and name.** The address alone starts discovery.
2. **Discovery** (`crates/postrinch-discover`), in this order, first answer wins: the domain's own autoconfig (`https://autoconfig.<domain>/mail/config-v1.1.xml`, then `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`), Mozilla's ISPDB (`https://autoconfig.thunderbird.net/v1.1/<domain>`), then the MX host's domain in the ISPDB (mail hosted by Google Workspace or Microsoft 365 is told so: Google's is better added as a Google account), then DNS SRV (`_imaps._tcp`, `_imap._tcp`, `_submissions._tcp`, `_submission._tcp`, RFC 6186/8314), then guesses (`imap.`/`mail.` and `smtp.`/`mail.` with the usual ports), each probed before it is offered. Everything over HTTPS; a plain-HTTP answer is refused. SRV records are not authenticated (RFC 6186 §6), so servers they name outside the address's domain are shown for the person to confirm before any password is sent (`Discovery::confirm_servers`).
3. **Sign-in**: "Sign in with <provider>" in the browser where Postrinch has an OAuth client for it (an app password stays possible behind a link); otherwise the password, or the app password with the provider's page for making one.
4. **Check**: Postrinch logs in to IMAP and SMTP (EHLO and AUTH, no mail sent) and lists the folders. Each failure says which server and what it said, and opens the manual settings with what was found filled in.
5. **Manual settings**, reachable at any time: both servers' host, port, security and user name.
6. The account is stored, its secret kept, and sync starts.

## The provider façade

The app now holds one `GmailSync`; it will hold a `MailSync` (in `postrinch-sync`) that has the Gmail engine when Google is set up and the IMAP engine always, and sends each call to the account's provider: keep in sync, keep sending, download a message, search the server, labels, and Google's calendar and rules (an IMAP account answers those with "IMAP accounts have no calendars" or runs its rules here). A missing Google OAuth client then turns off Google accounts only. The agents crate's `Gmail` trait becomes `Remote`, the same calls, routed by account.

## Testing

A pure-Rust fake IMAP and SMTP server (`crates/postrinch-mailtest`, a dev-dependency only) runs in each test on a loopback port, with in-memory folders, UIDs, MODSEQs and the extensions under test switchable (CONDSTORE, QRESYNC, MOVE, UIDPLUS, SPECIAL-USE, IDLE), plus TLS with a test certificate. No test touches a real mailbox. A real-server smoke test against GreenMail in Docker runs by hand (`tools/imap-greenmail.sh`), not in CI.

## Crates

- **IMAP: `async-imap`** (tokio runtime, used in production by Delta Chat), with `tokio-rustls` and `rustls-platform-verifier`, as reqwest already uses. Raw commands for what it lacks (`UID MOVE` parsing of COPYUID, `ENABLE QRESYNC`).
- **SMTP: `lettre`** without default features: `smtp-transport`, `tokio1-rustls`, `rustls-platform-verifier`, `aws-lc-rs`. The message bytes are still built by `mail-builder` (`build_message`), and sent raw.
- **Mailbox names: `utf7-imap`**. **DNS SRV and MX: `hickory-resolver`**. **Autoconfig XML: `quick-xml`**.
