# Rules: what a rule can say, and where each part runs

Step 1 of the rules work (Overboard: "Design: mail rules that run in the
cloud where possible, locally where not"). Written 2026-11-18. The goal, as
decided: one set of rules per account; each rule runs on the mail server
where the server can run it, in Postrinch where it cannot, split between the
two when it has to be; the person never has to notice or care.

This page is the contract the compiler, the editor and the local runner are
built against. Rows marked **(test)** are what the Gmail documentation
leaves unclear; each is settled by trying it on a test account before the
compiler relies on it.

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
- The editor never shows where a rule runs, as decided. The rule's details
  pane may say, in small type, "runs while Postrinch is open" for a rule that
  is entirely local: that is a fact about reliability the person may want,
  not a choice they make. **(Joe to confirm.)**

## 1. Conditions

A rule's conditions are **all of**, **any of**, or **none of** a list; a
list may hold a group, one level deep (the editor need not offer deeper).

### Arrival conditions: what the message says

| Condition | Match | Gmail filter | Sieve | Notes |
|---|---|---|---|---|
| From | is address | `from` = `"ann@example.com"` | `address :is "from"` | exact |
| From | domain is | `from:(*@example.com)` **(test)** | `address :domain :is "from"` | Gmail's `from:example.com` also matches `notexample.com`-style tokens; only the form the test proves exact goes to the server |
| From | name contains word | `from` = `Ann` | `header :contains "from"` (substring) | Gmail matches words; Sieve substrings. Sieve gets it only as `:regex` with word boundaries, where offered |
| To / Cc / Bcc / any recipient | same three as From | `to:` `cc:` `bcc:` in `query`; the `to` field is to-or-cc | `address :is ["to","cc"]` | Gmail's `to` field means To or Cc: "any recipient" maps there, "To only" goes in `query` as `to:` **(test)** |
| Sent to my alias | address is | `deliveredto:` in `query` | `envelope :is "to"` (needs `envelope`) | for `+tag` and alias addresses |
| Subject | contains words | `subject` | `header :contains "subject"` | words vs substring, as for names |
| Subject / body / anywhere | contains phrase | `"…"` in `subject` or `query` | `body :text :contains` (needs `body`) | |
| Body or anywhere | contains words | `query` | `body :text :contains` | Gmail also searches attachment names here; Postrinch matches that too, so "anywhere" is defined as Gmail's meaning |
| Mailing list | is | `list:` in `query` | `header :contains "list-id"` | |
| Has an attachment | — | `hasAttachment` | none in base Sieve: local | Sieve has no portable MIME test (`mime` ext, RFC 5703, is rare) |
| Attachment name | contains / ends with | `filename:` in `query` | local | `filename:pdf` matches type too **(test)** |
| Size | larger / smaller than | `size` + `sizeComparison` | `size :over` / `:under` | Gmail measures the whole message; so does Sieve; Postrinch uses the stored raw size |
| Header | any name, contains / is | no: local | `header :contains "x-…"` | |
| Text with wildcards or a pattern | matches | no: local | `:matches`, or `:regex` where offered | Gmail filters have no wildcards or regex |
| From someone I know | — | no: local | no: local | "I've written to them", or a contact once contacts exist |
| Gmail's category | is | local **(test)** | n/a | Gmail chooses the category itself; whether a filter sees it on arrival is not documented |
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
  finds it. **(Open: whether that window matters enough to lock.)**

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
- **Query length** has a limit Google does not publish **(test)**; a query
  past it is split or goes local.
- **Sieve script size** is the server's `MAXSCRIPTSIZE` (ManageSieve
  capability); same treatment.
- **The server refuses** (a scope withdrawn, a forwarding address no longer
  verified, the quota): the rule runs locally and keeps trying the server
  each sync. The rule's details show why, in a sentence. **(Joe to confirm
  that this quiet note is acceptable.)**

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

## Next

1. Run the **(test)** rows on a test account; fix this page from what they
   show.
2. Mockups of the rules list and the rule editor.
3. Build: the vocabulary and compiler with tests, Gmail filter sync and the
   settings message, the local runner with catch-up; Sieve with IMAP
   accounts.
