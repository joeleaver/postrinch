# Gmail sign-in, app verification and CASA

Researched 2026-11-02. Google changes these rules; check the sources before acting on a number.

## How to sign in

- **OAuth 2.0, installed-app flow:** authorization code with PKCE, opened in the system browser, redirected to `http://127.0.0.1:<random port>`. Google blocks sign-in inside embedded web views, and the out-of-band (copy and paste a code) flow is retired.
- A desktop app's client secret is not secret, and Google knows it; PKCE is what protects the flow.
- `access_type=offline` for a refresh token; `prompt=consent` the first time (a refresh token comes only with consent); `openid email` to learn which account signed in.
- **Many accounts:** one OAuth client ID; run the flow once per account; keep each refresh token in the OS keychain, keyed by address. `prompt=select_account` when adding one, `login_hint=<address>` when one must sign in again.
- **Passwords:** "less secure apps" sign-in is gone. App passwords still work for some personal accounts with 2-step verification, but Google discourages them and many Workspace domains block them. A manual fallback for IMAP at most.

## Which scopes are restricted

From Google's Gmail scope page:

| Scope | Class |
|---|---|
| `https://mail.google.com/` (IMAP/SMTP with OAuth, and the whole API) | Restricted |
| `gmail.readonly`, `gmail.modify`, `gmail.compose`, `gmail.metadata`, `gmail.insert` | Restricted |
| `gmail.send` | Sensitive |
| `gmail.labels` | Non-sensitive |

A mail client cannot avoid a restricted scope: even `gmail.metadata` is one. Mail clients are a permitted use: "Built-in and web email clients that allow users to compose, send, read, and process email via a user interface" (Gmail API policy).

## What a public release takes

1. **Verification by Google:** branding (2-3 business days), a privacy policy that meets the Limited Use rules, a demo video, a justification for each scope. Restricted-scope review "can potentially take several weeks".
2. **A CASA security assessment** for restricted scopes, renewed "at least every 12 months".
   - Google charges nothing and mentions a free Tier 2 route; an outside assessor charges directly. Google's figures: $500-$4,500, 2-3 weeks for Tier 2, 4-6 for Tier 3. One lab lists Tier 2 at $540-$1,800 a year and Tier 3 at $4,500.
   - Google picks the tier from the data's sensitivity, the user count and its own risk signals, and can raise it later.
3. **Limited Use**, for as long as the app has access: the data serves only user-facing features; no ads, no selling or transferring it, no person reading it without explicit consent, no training models on it beyond that user's own personal features.

## The open question: local-only clients

Does an app that keeps all mail and tokens on the user's machine, with no server of its own, need the assessment?

- **For no:** the restricted-scope verification page requires it "if you store or transmit restricted scope data on servers", and for any app that "has the ability to access data from or through a third-party server".
- **For yes:** the Help Center's security assessment article says apps requesting restricted scopes "must undergo an annual security assessment", with no qualifier.
- **Nobody has Google's answer in public:** at least four Google developer-forum threads from August-October 2026 ask exactly this, and none has a reply from Google.

**Plan:** ask Google's verification team before building anything that depends on the answer, and budget for the assessment anyway.

## Before verification, and alternatives

- **Personal use** needs no verification: you alone, or a few people you know. The "unverified app" screen shows, and a 100-new-user cap applies "over the entire lifetime of the project", never reset.
- **"In production", unverified.** In testing mode (External user type), refresh tokens expire after 7 days. Publishing the project to production without submitting it for verification keeps the warning screen and the cap but not the expiry. This is the setting while Postrinch is built.
- **Internal** (Workspace) apps skip the screen and the cap, for that organisation's users only.
- **Bring your own client ID:** a setting where a user pastes an OAuth client from their own Google Cloud project, which puts each of them under personal use. Cheap to build; for technical users.
- **Mail brokers** (Nylas and others) lend their verified app, but mail then goes through their servers, which is the opposite of local-first.

## Token rules that shape the design

- At most **100 refresh tokens per Google account per client ID**; the oldest is invalidated silently. Do not mint tokens carelessly (repeated "add account" of the same address).
- A refresh token stops working when it is unused for six months, when the user changes their password (for Gmail scopes), when the user revokes access, or under a Workspace admin policy (`admin_policy_enforced`, `invalid_grant`). Each is "this account must sign in again".

## Sources

- [Google: Restricted scope verification](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification)
- [Google Cloud Help: OAuth verification FAQ](https://support.google.com/cloud/answer/13463817?hl=en)
- [Gmail API: Choose scopes](https://developers.google.com/workspace/gmail/api/auth/scopes)
- [Gmail API: Policy](https://developers.google.com/workspace/gmail/api/policy)
- [Google OAuth 2.0: refresh token expiration](https://developers.google.com/identity/protocols/oauth2#expiration)
- [Forum: CASA for on-device Gmail processing (unanswered)](https://discuss.google.dev/t/gmail-restricted-scope-is-the-casa-assessment-required-when-mail-is-processed-on-device-and-only-a-derived-subscription-list-reaches-our-server/398372)
- [Forum: client-only app with no server (unanswered)](https://discuss.google.dev/t/does-a-client-only-android-app-with-no-server-need-the-casa-security-assessment-for-drive-readonly/393638)
- [Forum: Is CASA required for all restricted scopes?](https://discuss.google.dev/t/is-casa-required-for-all-access-restricted-scopes/340650)
- [Nylas: Gmail restricted scopes, verification and CASA](https://developer.nylas.com/docs/provider-guides/google/google-verification-security-assessment-guide/)
- [Nylas: Google OAuth verification costs and timelines](https://www.nylas.com/blog/google-oauth-app-verification/)
- [DeepStrike: Google CASA 2026](https://deepstrike.io/blog/google-casa-security-assessment-2025)
- [Latenode community: CASA Tier 2 for indie developers](https://community.latenode.com/t/is-casa-tier-2-assessment-necessary-for-all-gmail-api-apps-options-for-indie-developers/22861)
