# Rules: what a rule can say, and where each part runs

Step 1 of the rules work (Overboard: "Design: mail rules that run in the
cloud where possible, locally where not"). Written 2026-11-18. The goal, as
decided: one set of rules per account; each rule runs on the mail server
where the server can run it, in Postrinch where it cannot, split between the
two when it has to be; the person never has to notice or care.

This page is the contract the compiler, the editor and the local runner are
built against. What the Gmail documentation left unclear was tried on a real
account on 2026-11-18 (**Tested**, at the end); the tables below already
follow from it.

## The principle that decides every table

**A part runs on the server only if the server matches exactly the same
mail.** "Close enough" is not enough: a rule that archives what Gmail thinks
matches, while the editor promised something narrower, is a rule that hides
mail the person wanted. Where Gmail's meaning is broader or different, that
condition runs in Postrinch, and so does every action that depends on it.

Two consequences:

- The vocabulary below chooses its words so that the common ones compile
  exactly. "From is ann@example.com" and "from contains the word invoice" are
  different conditions because Gmail treats them differently.
- The editor never shows where a rule runs, not even as a note on a rule
  that is entirely local (Joe, 2026-11-18).

## 1. Conditions

A rule's conditions are **all of**, **any of**, or **none of** a list; a
list may hold a group, one level deep (the editor need not offer deeper).

### Arrival conditions: what the message says

| Condition | Match | Gmail filter | Sieve | Notes |
|---|---|---|---|---|
| From | is address | `from` = `"ann@example.com"` | `address :is "from"` | exact |
| From | domain is | no: local | `address :domain :is "from"` | every Gmail form (`from:example.com`, `@example.com`, `*@example.com`) also matches subdomains, `example.com.evil.net`, and "example.com" in the name or local part |
| From | name contains word | `from` = `Ann` | `header :contains "from"` (substring) | Gmail matches words; Sieve substrings. Sieve gets it only as `:regex` with word boundaries, where offered |
| Any recipient | address is | `to` field, or `to:` | `address :is ["to","cc","bcc"]` | Gmail's `to:` matches To, Cc and Bcc |
| Cc / Bcc | address is | `cc:` / `bcc:` | `address :is "cc"` | each matches only its own header |
| To (only) | address is | no: local | `address :is "to"` | Gmail has no To-only operator |
| Sent to my alias | address is | `deliveredto:` | `envelope :is "to"` (needs `envelope`) | reads the Delivered-To header Gmail adds on delivery; to try with a real `+tag` delivery before relying on it |
| Subject | contains words | `subject` | `header :regex` with word boundaries | Gmail matches whole words, no stemming: `invoice` misses "invoices" and "reinvoice" |
| Subject / body / anywhere | contains phrase | `"…"` in `subject` or `query` | `body :text :contains` (needs `body`) | |
| Body or anywhere | contains words | `query` | `body :text :contains` | Gmail also searches attachment names here; Postrinch matches that too, so "anywhere" is defined as Gmail's meaning |
| Mailing list | is | `list:` in `query` | `header :contains "list-id"` | |
| Has an attachment | — | `hasAttachment` | none in base Sieve: local | Sieve has no portable MIME test (`mime` ext, RFC 5703, is rare) |
| Attachment type | is (extension) | `filename:pdf` | local | any case; not names that merely contain "pdf" |
| Attachment name | is | `filename:report.pdf` | local | |
| Attachment name | contains | no: local | local | |
| Size | larger / smaller than | `size` + `sizeComparison` | `size :over` / `:under` | Gmail measures the whole message; so does Sieve; Postrinch uses the stored raw size |
| Header | any name, contains / is | no: local | `header :contains "x-…"` | |
| Text with wildcards or a pattern | matches | no: local | `:matches`, or `:regex` where offered | Gmail filters have no wildcards or regex |
| From someone I know | — | no: local | no: local | "I've written to them", or a contact once contacts exist |
| Gmail's category | is | no: local | n/a | a `category:primary` filter caught mail sent from the account but not arriving mail Gmail had put in Primary |
| Looks like (semantic) | — | local | local | waits for embeddings |

### State conditions: what has happened to it since

Unread, starred, has a label, in the inbox, older than N days, replied to.
These describe mail after it arrived, which a server filter never sees
again, so **they are always local**. They exist for time rules ("archive
newsletters read more than a week ago") and for "apply to existing mail".

### Combining, on Gmail

- **All of** → one filter: Gmail ANDs its fields.
- **Any of** → one filter whose `query` joins the parts with `OR` and
  parentheses, or several filters with the same actions when a part needs a
  field of its own (`hasAttachment`, `size`).
- **None of** → `negatedQuery` for word conditions; a field such as
  `hasAttachment` cannot be negated, so "has no attachment" is local.
- If any condition of a rule is local, the whole match is local. The server
  may still do part of the work: see **Splitting**.

## 2. Actions

| Action | Gmail filter | Sieve | Local | Notes |
|---|---|---|---|---|
| Add a label | `addLabelIds` | `fileinto :copy` (folders) or `addflag` (keyword) | yes | on IMAP, a label is a folder or a keyword: decided with IMAP accounts |
| Skip the inbox (archive) | `removeLabelIds: INBOX` | `fileinto "Archive"` (special-use `\Archive`) | yes | |
| Mark read | `removeLabelIds: UNREAD` | `addflag "\\Seen"` (`imap4flags`) | yes | |
| Star | `addLabelIds: STARRED` | `addflag "\\Flagged"` | yes | |
| Mark important / never important | `IMPORTANT` add / remove | n/a | yes | Gmail only |
| Put in a category | `addLabelIds: CATEGORY_…` | n/a | yes | Gmail only |
| Never send to spam | `removeLabelIds: SPAM` | n/a | no | only works on arrival: server only, Gmail only |
| Move to trash | `addLabelIds: TRASH` | `fileinto "Trash"` | yes | |
| Forward to | `forward` (address must already be verified in Gmail) | `redirect :copy` | sends mail: see Safety | leaves the account |
| Stop processing more rules | see **Order** | `stop` | yes | |
| Remind me / bring back later | no | no | yes | brings it back to the inbox, unread, at a time |
| Archive after N days | no | no | yes | a time action: local by nature |
| Notify me | no | no | yes | on each computer, which is what a notification should do |
| Ask an agent (summarise, draft a reply) | no | no | yes | the agent proposes; it still cannot send |
| Auto-reply | no (the API has no canned responses) | `vacation` | sends mail | later; out of scope for the first build |
| Delete for good | — | `discard` | — | not offered: nothing in Postrinch deletes for good without asking |

**Safety.** Forwarding and auto-reply send mail out of the account, which
Postrinch otherwise never does without asking. Saving a rule with either
asks once, in the editor, naming the address; after that it runs unattended
on the server, as Gmail's own filters do.

## 3. Splitting a rule

Every rule compiles to one of three shapes:

1. **All on the server.** Every condition and action compiles: one or a few
   Gmail filters. Postrinch runs nothing for it on new mail.
2. **All local.** A condition is local: Postrinch matches and acts.
3. **Split.** Every condition compiles but some actions do not ("from the
   school, label School and ask an agent for a summary"). The server does
   its actions and also adds the rule's **marker**; the local runner finds
   the marker, does the remaining actions, and removes the marker.

The marker is a Gmail label per rule, `[Postrinch]/r<id>`, created hidden
(`labelListVisibility: labelHide`, `messageListVisibility: hide`), and left
out of Postrinch's own sidebar and label chips. On IMAP it is a keyword
(`$postrinch-r<id>`) set with `addflag`.

The marker solves three things at once:

- **The same mail.** The local half never re-evaluates conditions, so it
  cannot disagree with the server about what matched.
- **Folders don't matter.** The server half may archive before Postrinch
  sees the mail; the runner looks for markers across all mail, not by
  folder.
- **Done once, across computers.** Removing the marker goes through the
  outbox to Gmail, so another computer running Postrinch sees it gone and
  does not repeat the work. Two computers catching up in the same minute can
  still both act; that is harmless for labels and archiving, and an agent
  action records its own result (a label) before running so the second
  finds it. There is no lock (Joe, 2026-11-18): a rare repeat is cheaper
  than a lock between computers.

A shape-2 rule that is all local has no marker: its actions are idempotent
(labels, flags, archive), or record what they did, as above.

## 4. Order and "stop processing more rules"

Gmail runs every matching filter, in no order; Sieve runs top to bottom.
Postrinch's rules **run in the order the person sets**, and a rule may say
"stop processing more rules". Most rules never need it, because labelling,
starring and archiving don't conflict whatever the order.

On Gmail, "stop" compiles by negation: every filter for a later rule gets
the earlier stopping rule's conditions added to its `negatedQuery`. That only
works when the earlier rule's conditions all compile; when they don't, the
later rules it shadows become local too. The editor says nothing of this;
the compiler does it.

## 5. When Postrinch is closed

- Shape 1 runs anyway, on Gmail.
- Shapes 2 and 3 catch up on launch and after each sync, over mail that
  arrived since the runner last finished on this computer (kept in the
  store), and every message carrying a marker, whatever its age.
- A computer running Postrinch for the first time starts from that moment;
  it never sweeps old mail unless the person asks for "also apply to
  existing mail", which runs locally over the store, both halves, through
  the outbox like any other action.

## 6. Limits and failures

- **Gmail allows 1,000 filters** per account; "any of" and "stop" multiply
  them. The compiler counts before saving; past the limit, the rules that
  cost most run locally.
- **Query length:** Gmail accepted filter queries of up to 10,000
  characters without complaint; no limit was found. Whether a very long
  query still matches as written was not tried; the compiler keeps queries
  under 1,500 characters and splits longer ones into several filters.
- **Sieve script size** is the server's `MAXSCRIPTSIZE` (ManageSieve
  capability); same treatment.
- **The server refuses** (a scope withdrawn, a forwarding address no longer
  verified, the quota): the rule runs locally and keeps trying the server
  each sync. The rule's details show why, in a sentence (Joe agreed,
  2026-11-18).

## 7. What has to change outside the rules code

- **A new Google scope.** Creating filters needs
  `https://www.googleapis.com/auth/gmail.settings.basic`; `mail.google.com`
  only lets Postrinch *read* them. It is a restricted scope, like the one we
  already have. Postrinch asks for it the first time a rule needs Gmail, with
  incremental authorization (`include_granted_scopes=true`), so nobody signs
  in again until they make a rule. A grant without it is not refused, as the
  Gmail scope is: rules simply run locally. DESIGN.md's scope list changes.
- **Forwarding addresses** must be verified in Gmail first; adding one needs
  yet another scope (`gmail.settings.sharing`). The first build only offers
  addresses Gmail already lists as verified, and says how to add one in
  Gmail.
- **Labels:** `[Postrinch]/…` is kept out of the sidebar, chips, the label
  menu and agents' `list_labels`.
- **Where rules live**, as decided on the card: one settings message in the
  account under a hidden label. Its format is designed with the compiler.

## 8. Filters made elsewhere

`filters.list` returns every filter. Those not in Postrinch's record show in
the rules list as "made in Gmail", read-only, each with **Import**: it turns
the filter into a rule, saves that rule's filter, and only then deletes the
old one. A filter whose query uses operators outside this vocabulary keeps
that part as a "Gmail search" condition, which runs on Gmail only.

## Tested

On joe@joeleaver.com, 2026-11-18, with synthetic messages and filters that
carried a nonsense word, all removed afterwards (messages to the trash).

- **Search** (`messages.list` over inserted messages): the domain, recipient,
  attachment and word rows above.
- **Filters on arrival:** `messages.import` runs filters as delivery does,
  so it is how the compiler's integration tests deliver test mail with any
  sender. A `from:(*@example.com)` filter caught `ann@sub.example.com`; a
  filter's `to` field caught a message where the address was only in Cc.
- **Category:** a `category:primary` filter labelled a message the account
  sent itself, but not imported mail Gmail put in Primary
  (`CATEGORY_PERSONAL`).
- **Scope:** the probe asked for `mail.google.com` and
  `gmail.settings.basic` together and Google granted both, with the
  unverified-app warning, as the Testing consent screen shows for any scope.

## Built (2026-11-18)

- **`postrinch-rules`**, no I/O: the rule model, the matcher, the Gmail
  compiler and what is kept (`Settings`, shared through the settings
  message; `Local`, this computer's). As built, a negated condition
  compiles to `-term` in the filter's `query` rather than to
  `negatedQuery`; "has no attachment" stays local. A filter adds one user
  label, so each further label and the marker takes a filter of its own.
- **A rule counts as Gmail's only once all its filters are made**, and
  mail that arrived before they were goes through it here, so a rule just
  saved or refused never misses mail in between.
- **Rules for mail that has been here a while look at the inbox**, hourly:
  the editor says "Mail has been in the inbox a while".
- **Sync** (`postrinch-sync`, `google/sync/rules.rs`) reads and writes the
  settings message under `[Postrinch]/Settings` (never stored as mail),
  makes and deletes filters and markers, records refusals, lists filters
  made in Gmail and verified forwarding addresses. It runs when the rules
  changed here, or every ten minutes; trouble with it never stops mail.
- **Import** turns a Gmail filter into a rule, with parts outside the
  vocabulary as a "Gmail search" condition (Gmail only; here it matches
  nothing), and deletes the old filter once the rule's are made.
- **Ask an agent** starts an agent connected to Claude Code from
  Postrinch, unattended (`claude -p`, allowed only its Postrinch tools),
  one at a time, ten minutes at most. Its answer goes to the log: what it
  can do is label, archive, star or propose a reply. Other apps cannot be
  started without a window.
- **Agents' tools** (`postrinch-mcp`): `list_rules`, `save_rule`,
  `set_rule_on`, `move_rule` and `delete_rule` do what the Rules page does,
  in the accounts the agent may see. Rules travel in a plain shape
  (`postrinch-agents`' `rule_shape.rs`: `{"test": "from_domain", "value":
  …}`, `{"action": "add_label", "value": "Receipts"}`), labels by name. A
  rule that labels, files or flags mail is saved at once. One that
  forwards or asks an agent, new or now matching different mail, is only
  proposed: it waits on this computer (`Local.proposed`), shows in the
  sidebar beside proposed messages, and opens in the editor with the
  agent's reason, for the person to save or discard. As in the editor,
  forwarding goes only to an address Gmail has verified. `run_rule` runs a
  saved rule now over the last three months, or over every message (for a
  cleanup), in the background past a minute. Conditions include
  `category` (also accepted as `label` CATEGORY_*); a waited rule may not
  use `gmail_search`, which only Gmail can run.
- **Run now** on a rule's row in Settings › Rules does what `run_rule`
  does: the last three months (with how many conversations match) or all
  mail, off the window's thread; the row then says how many messages
  changed. Like "also do this to these now", it changes mail only: it
  forwards nothing and asks no agent.
- **Tried live, 2026-11-18,** on joe@joeleaver.com from a test window on a
  copy of the store: Allow (signing in again with `gmail.settings.basic`),
  then a rule Gmail ran (a filter starred the test mail as it arrived) and
  one Postrinch ran (it labelled the mail within a minute), then deleting
  both, which deleted the filter. Learnt on the way: the mail scope lists
  filters, so only the grant's scopes say whether Postrinch may make them;
  Gmail answers an empty body when there are no filters; the settings label
  is now made only when there are rules to keep.
- **Not yet:** Sieve (with IMAP accounts); a split rule (marker) and an agent
  run against real Gmail; forwarding by a rule that
  Postrinch runs (only Gmail forwards); remind me later and notify; trying
  a refused rule again before the rules next change; `deliveredto:` with a
  real `+tag` delivery.
