# OFSCP v0.1: spec questions from building a native client

Gaps, ambiguities and contradictions found while designing Postrinch's OFSCP client
([docs/ofscp.md](../ofscp.md)), for the spec's authors. Line numbers are
`~/dev/ofscp/docs/ofscp_spec.md` as of 2026-11-26; schema paths are under
`schemas/v0.1`. Postrinch works around none of these: where one blocks a feature, the
feature waits (docs/ofscp.md, "What waits on the spec").

The schemas' own README says they were generated and not yet reviewed, and the conformance
tests only validate example JSON against them; the two signing vectors are published but
never run.

1. **There is no way to create a channel memo, article, reply or attachment message.**
   - The `message.create` command (`ws/message-create.json`, §7.1 L1494-1503) carries only
     `groupId, channelId, clientMessageId, content`. It has no `type`, `attachments`,
     `reference`, `tags` or `metadata`.
   - Yet §5.2.1 L650 gates `message.create` by `type` (`post:<type>`), §5.8 says attachments go in
     a message, and §7.2 defines replies.
   - REST has PATCH and DELETE for messages but **no POST create** (L1548-1551). The signing
     vector nevertheless signs a `POST .../messages` (tests/signing-vector.json; §4.4.3 L395).
   - Why it matters: a client cannot post anything but plain chat, and a WS-less client cannot
     post at all.
   - Fix: add the fields to `WsMessageCreate` and define `POST .../messages` (with
     `clientMessageId` idempotency).
2. **Call signaling cannot be implemented interoperably** (§9.2 L2103-2112).
   - `offer`, `answer` and `ice` are REST POSTs with no target participant, no sender, and no
     session id.
   - Their responses are unspecified.
   - No WS event delivers relayed SDP or ICE to the other peers; only `call.started`, `call.ended`
     and `call.participant` exist.
   - Mesh versus a single offerer, glare handling, renegotiation, mute/video state updates, and
     what `participants[].role` and `media` mean are all missing.
   - `call.participant` with state `joined` carries no media info.
   - The `call/end` authorization names a "`host`" role that is never defined.
   - Why it matters: calls are unbuildable as written.
3. **The home provider never learns of remote group memberships.**
   - Joins go straight to the group's provider (§8.2 L2000), and there is no membership pointer
     analogous to DM conversation pointers (§7.4 L1748).
   - `GET /api/users/{ref}/groups` (§6.5) lists object URIs, but nothing says how the home
     provider learns about remote groups.
   - Why it matters: a fresh device logging in **cannot rediscover which remote groups the user
     belongs to**. Only follows (§7.6) and DM pointers survive.
   - Fix: add `PUT/DELETE /api/me/groups/{groupRef}` pointers, mirroring §7.4, with a
     `groups.updated` event.
4. **Envelope `ts` is required for client commands, but the spec examples omit it.**
   - `ws/envelope.json` requires `id, type, ts, data`. The examples for `authenticate`,
     `subscribe`, `message.create`, `ping` and others (L1281, L1409, L1495, L1595) have no `ts`.
   - Why it matters: a strict provider rejects spec-faithful clients.
   - Fix: make `ts` optional for commands, or fix the examples.
5. **WS message payloads are partial objects.**
   - `message.created` and `message.updated` define `message` as only `{id, clientMessageId,
     author, createdAt, editedAt, content}` (`ws/message-created.json`). There is no `type`,
     `attachments`, `reference`, `deletedAt`, `permissions.editUntil` or `metadata`.
   - `dm.message` is similar.
   - Why it matters: clients can't render a memo or article, a reply thread, or edit eligibility
     from live events without refetching.
   - Fix: say whether the payload is a full `BaseMessage`. It should be.
6. **The DM real-time model breaks for sent messages and cross-provider pairs.**
   - Reactions and edits on a message *I sent* are stored on the counterparty's provider and
     emitted "to the conversation's local subscribers" there (§7.4 L1810, §8.3 L2014). Implicit
     delivery covers only my own inbox (L1779).
   - So I see the other side's reaction to my message only if I hold a WS to every DM
     counterparty's provider and subscribe there. Doing so is never specified, and I have no
     inbox there. Can a remote actor subscribe to a `dmId` on a foreign provider?
   - Which connection carries `typing.start` for a DM is also unspecified: mine or the
     recipient's provider?
   - Merging history needs two independent cursor spaces (L1770). There is no rule for resuming
     a merged conversation.
7. **DM edit, delete and reaction paths are only examples, and who forwards is unclear.**
   - §8.3 gives `/api/federation/dms/{dmId}/messages/{mid}[/reactions/{key}]` prefixed by "e.g."
     (L2014-2015). §7.4 gives `/api/dms/...` paths (L1812).
   - §7.4 L1834 says a PATCH/DELETE "is forwarded to that message's home provider". Forwarded by
     whom, the client or the home provider? That contradicts "client-driven, no relay".
   - It also mentions "a provider-signed federation ingest variant" that is never defined.
   - Fix: give normative paths and say the client sends.
8. **Notification delivery to native clients is undefined.**
   - §10 webhooks (`type:"webpush"`, `target`) and §10.3 Web Push (`/api/push/subscribe`)
     overlap, and their relationship is unstated.
   - Webhook `events` vocabulary, `resource` shape, retry policy, and whether a webhook target
     may be a third-party URL are all unspecified.
   - The compliance checklist (L2340) makes "notification endpoints" a MUST while §10.1-10.3 are
     OPTIONAL.
   - There is no native push (APNs, FCM-native, UnifiedPush). Web Push needs a browser push
     service.
   - Cross-provider mentions produce no notification at all (§10.1 L2170).
   - Why it matters: a desktop client must keep N sockets open and parse mentions itself.
9. **There is no actor-to-provider indirection.**
   - The actor domain *is* the API host (§2.2, §3.1). There is no WebFinger or `.well-known`
     delegation, so `alice@example.com` can't be served from `social.example.com`, and a provider
     can never move hosts without breaking every identity, `dmId` and signature.
   - For a mail client whose users think "my address = my domain", this is a UX and migration
     problem worth raising.
10. **Discovery `login_endpoint` and `registration_endpoint` contradict standard paths.**
    - §3.2 L170 and L184 forbid redefining endpoint paths via discovery, but the schema
      *requires* `authentication.login_endpoint` (provider-discovery.json).
    - If the two differ from `/api/auth/*`, which wins? A client should use the standard path,
      but the spec should say so.
11. **Message `type` handling is inconsistent.**
    - §2.3 L76 says unknown `message.type` values must render generically, but the
      `BaseMessage.type` enum is closed (`message|memo|article`), so they fail validation.
    - Discovery `capabilities.messageTypes` lists `"reaction"` (L138), but reactions are not
      messages in the schema (`TimelineItem` oneOf exists but is unused).
    - Can a messages page contain Reaction items? Unclear.
12. **Resume depends on a SHOULD.**
    - The event `cursor` is SHOULD (L1471), but `since` replay is MUST (L1473). Without cursors
      the client has no position to resume from.
    - There is no resume for implicit streams (`notification.created`, `read.updated`,
      `dm.conversation`, `member.updated`, call events), and no connection-level sequence.
    - `truncated` is only RECOMMENDED and is absent from `ws/subscribed.json`.
    - Whether a REST page cursor (`prevCursor`/`nextCursor`) is a valid `since` value is implied
      by "same space" but not stated.
    - Fix: make `cursor` MUST and add `truncated` to the schema.
13. **`subscribe.include` defaults are "provider-defined"** (`ws/subscribe.json`).
    - A client can't know whether `reaction.*`, `call.*` or `member.updated` arrive without
      `include`.
    - Fix: define the default as "all channel-scoped events".
14. **Content format is underspecified.**
    - The allowed `mime` set, the Markdown flavour (CommonMark? GFM?), HTML sanitization rules,
      and whether `article` may "embed remote content" (L728, a tracking and privacy risk) are
      all unstated.
    - Can chat (`message`) be markdown?
    - Mentions are free-text only. There is no structured mention or entity list, so renaming or
      parsing is lossy.
    - Fix: specify a baseline and a sanitizer allow-list.
15. **`editUntil` is ambiguous.**
    - If `permissions.editUntil` is absent, may the author edit forever or never? (L1534.)
    - There is no edit history, and `PATCH /api/me/profile`'s response is undefined.
16. **Attachment access control and upload host are unspecified.**
    - `POST /api/media` has no rule for *which* provider a client uploads to: home, or the
      channel host for a remote channel?
    - Are attachment URLs capability URLs, public, or signed? Private-channel media may leak by
      URL (§5.8).
    - For a DM to a remote user, the attachment lives on the sender's provider forever, even
      though the message lives on the recipient's.
    - Is avatar upload the same flow?
17. **Presence fan-out cost.**
    - Remote presence needs a WS to each subject's home provider (§7.5 L1902), even with no
      shared channels. A contact list across 30 providers means 30 sockets, each needing a
      signed handshake and heartbeats.
    - There is no batching or relay.
18. **Remote channels have no unread state** (§7.7 L1932).
    - Read markers are provider-local, so unread badges for any non-home channel must be kept
      purely client-side and won't sync across devices.
19. **Contacts federation is a two-write, client-driven protocol with no reconciliation**
    (§6.7 L1231-1248).
    - If the client crashes between `POST /api/me/contacts` and `POST /api/federation/contacts`,
      the two sides diverge forever. There is no re-sync endpoint or idempotency key.
    - How a recipient learns of an incoming request in real time is undefined: there is no
      `contact.*` WS event, even though `contact-event.json` exists.
20. **No "my groups at home" or "membership changed" events.**
    - Being added via approval of a join request, kicked, or a group being deleted only shows up
      as an `unsubscribed` on currently-subscribed channels.
    - There is no `member.removed`, `group.updated`, `channel.created` or `channel.updated` event,
      so channel lists go stale until refetched.
21. **The channel listing is not paginated** (§5.5 L878-881).
    - It returns `{items}` without `page`, while "every paginated endpoint" rules apply
      elsewhere. Clarify.
    - Similarly, `GET /api/me/follows`, `/api/me/contacts` and `/api/notifications/endpoints`
      return unpaged arrays with different wrapper keys (`follows`, `contacts`, `endpoints`,
      `prefs`, `scopes`). This is not wrong, but it is inconsistent.
22. **Clock dependence.**
    - ±300 s skew is a hard `401` (§4.5 step 3), and the WS `authenticate` timestamp is checked
      the same way.
    - A desktop with a bad clock silently fails everywhere. The spec should recommend that
      providers send a `Date` header and that clients correct their offset from it, and say
      which `errorCode` signals skew. Currently a skew failure is indistinguishable from a bad
      signature.
23. **Device key lifecycle gaps.**
    - Keys never expire, and there is no rotation or replace endpoint.
    - A password reset revokes all keys (§4.1.3), so every device must re-login. There is no way
      to learn of revocation except a `4003` or `401`s.
    - Discovery of a newly registered key on remote providers can take a `503` round
      (L461).
    - There is no "sign out all other devices" shorthand.
    - Account deletion is not defined anywhere.
24. **WS event table drift.** §7.1 L1367-1394 omits `dm.reaction` and `dm.typing`, which appear
    in `ws/types.json` and §7.4. The DM `typing.start` reuses the `channelId` field for a `dmId`
    (L1844), which is awkward for typed clients.
25. **Pagination semantics for `GET /api/me/dms`** ("newest activity first", L1774) conflict
    with the normative `direction` rules (§7.2), where `backward` means newest first by timeline
    position. Does `forward` mean oldest activity? A conversation list reorders as activity
    happens, so cursor stability under reordering is undefined.
26. **The compliance checklist has stale or unclear entries.**
    - "Provide metadata schema registry (optional entries allowed)" (L2344) is a MUST with no
      defined endpoint.
    - "Support all message types or graceful fallback" for clients.
    - `GET /api/tiers` is a MUST, but §11.1 says it should be "referenced in the discovery
      document", and no discovery field exists for it (L2259).
27. **There are no calendar or event primitives.** Not a defect, but worth flagging for
    Postrinch: a client that wants to integrate OFSCP with calendars has only call channels, with
    no scheduling. Event invitations would need a metadata extension (§5.4).

## Seen in Forumall, the reference implementation

Forumall (finished June 2026) predates several spec changes; these are places where the code
shows the spec is thin, not bugs in Forumall.

28. **The server's copy of a client's own message does not echo `clientMessageId`** in
    practice, so the client matches its pending message by the envelope's `correlationId`.
    The spec should require the echo, in `message.created`, for the sender.
29. **No "my memberships, with my role" endpoint**, local or remote. A client finds its own
    role by searching the members list of each group (one call per group).
30. **Media downloads need no authentication.** With gap 16, private-channel attachments are
    readable by anyone holding the URL.
31. **Reactions in a timeline carry no type tag**, so a client tells them from messages by
    their fields (relates to gap 11).
