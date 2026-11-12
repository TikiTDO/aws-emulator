Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.list-identities-v1-token-continues-v2-list
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|ListEmailIdentities|BadRequestException
Issue-Commit: 3c42ba1b0bd2ccb43c9358bd117a182df6732fd7

# ses: a v1 ListIdentities NextToken is refused by SESv2 ListEmailIdentities (400 BadRequestException "Invalid NextToken <...>.") instead of continuing the same identity listing — the two surfaces mint privately-tagged, mutually unreadable cursors

> _Note: this issue was identified against the code base at commit `3c42ba1b0bd2ccb43c9358bd117a182df6732fd7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEmailIdentities

## Description
One net-new root cause, grounded in the single failing behavioral check the replay report marked neither ALREADY FILED nor OWNED. No open issue covers it: 20261108T111632Z-ses-a-v1-listconfigurationsets-nexttoken-is-refused-by.md owns the same hand-off for the CONFIGURATION-SET listing (ListConfigurationSets), a different collection and different handlers; 20261028T010932Z-ses-2-net-new-behavioral-root-causes-createconfigurationseteventdestination.md owns only the WORDING of SESv2 ListEmailIdentities' forged-cursor refusal (and that wording now matches, so only the cross-surface interchange is left); 20261109T160343Z-sesv2-listsuppresseddestinations-refuses-an-invalid-nexttoken-with-the.md and 20261102T110414Z-sesv2-3-net-new-refusal-message-divergences-listdedicatedippools.md own other sesv2 listings' sentences.

=== Cause 1 of 1: a NextToken issued by SES (v1) ListIdentities is rejected by SESv2 ListEmailIdentities instead of resuming the account's identity listing at that position ===

AWS: SES classic and SESv2 are two API surfaces over ONE account-level email-identity directory, and the identity listing's pagination cursor is shared between them: a NextToken handed back by v1 `ListIdentities` is accepted by v2 `ListEmailIdentities` and continues the SAME walk, so the continued page does not re-serve the identity page one already reported, and the walk from that cursor reaches the remaining identities. Smithy (ses, com.amazonaws.ses#ListIdentities): input `IdentityType` + `MaxItems` + `NextToken` ("The token to use for pagination."), output `Identities` (required) + `NextToken` ("The token used for pagination."), paginated inputToken/outputToken `NextToken`, pageSize `MaxItems`; the operation models NO errors at all. Smithy (sesv2, com.amazonaws.sesv2#ListEmailIdentities): POST /v2/email/list-identities, input `Filter` + `NextToken` + `PageSize`, output `EmailIdentities` + `NextToken`, paginated inputToken/outputToken `NextToken`, errors `BadRequestException` (httpError 400) and `TooManyRequestsException` (429). Neither model scopes "a token returned from a previous call" to one of the two surfaces and neither declares a per-surface cursor namespace, so the interchange itself is recorded behavior only: the recording shows the v2 call carrying the freshly minted v1 token answering HTTP 200 with a page that omits page one's identity, and continuing to exhaustion over the rest of the directory.

Emulator: the v2 call is REFUSED. `sesv2.ListEmailIdentities(NextToken=<the token ses.ListIdentities(MaxItems=1) just returned>)` answers HTTP 400, error code `BadRequestException`, message `Invalid NextToken <...>.` — fixed text `Invalid NextToken <` and `>.`, with the one variable slot being the rejected token copied from the request (in the replay that slot held the hex cursor the v1 listing had just minted, which decodes to an integrity tag followed by the identity name). The two surfaces encode cursors in private, mutually unreadable formats: the ses crate's `ops/list_identities.rs` mints `crate::state::encode_page_token(crate::state::LISTING_IDENTITIES, name)` (hex of a listing-tag ‖ key) and decodes only under that listing tag, while the sesv2 crate's `ops/list_email_identities.rs` mints/decodes with its own `crate::state::encode_page_token`/`decode_page_token` under the sesv2 identity-listing tag — so the v1 token fails the v2 decode, `decode_page_token` returns None, and `invalid_next_token_message(token)` fires through `bad_request`. The resume SEMANTICS also disagree and would still be wrong after a shared format: the v1 cursor names the FIRST identity of the next page and v1 resumes INCLUSIVELY at it (`Bound::Included(resume_from)`), while the v2 cursor names the LAST identity of the page just served and v2 resumes STRICTLY AFTER it (`partition_point(key <= last)`), so a cursor handed across surfaces would be off by one even once it decoded. Each surface's own round trip is fine (the v1 MaxItems=1 walk and the v2 PageSize=1 walk both page exactly), so only the CROSS-surface continuation diverges.

Why: a client paging the account's email identities while migrating from SES to SESv2 — or any SDK/tool that holds a cursor and issues the next page through whichever client it happens to have — gets a hard 400 on a cursor the service itself just issued, where AWS serves the next page. Because the refusal is `BadRequestException` "Invalid NextToken <...>.", the caller cannot distinguish a genuinely corrupt or stale cursor from a perfectly valid one merely presented to the other surface, so the only recovery is restarting the walk and any mid-walk hand-off is impossible. It also makes the cursor an API-surface-private value where AWS's is a position in the account's identity directory, breaking migration and dual-client parity tests outright.

Fix: the identity page-cursor codec shared by the two listing handlers — `crate::state::encode_page_token`/`decode_page_token` with `LISTING_IDENTITIES` in services/service-com-amazonaws-ses-simpleemailservice/src/ops/list_identities.rs, and the sesv2 counterparts used by services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_email_identities.rs. The identity listing's cursor must live in ONE place both surfaces read and write (alongside the shared identity directory) with ONE agreed resume semantics, so a cursor either surface issues names a position in the account's listing rather than a position private to the API that minted it. An unissued or malformed cursor must still be refused on each surface with that surface's own modelled error and wording (sesv2: 400 BadRequestException "Invalid NextToken <token>.").

Traces: ses.list-identities-v1-token-continues-v2-list

## Evidence
Check ses.list-identities-v1-token-continues-v2-list (verdict: error), under the suite report's `### ses` heading:
  botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the ListEmailIdentities operation: Invalid NextToken <ac742462be5ba54773696d327265616c2d62656861762d746f6b312d3439316263353031616264332e6578616d706c652e636f6d>.
  raised from the cross-API continuation leg `v2_resp = v2.list_email_identities(NextToken=token)`, where `token` is the NextToken returned by `v1.list_identities(MaxItems=1)` immediately before. (The hex string is that run's cursor — it is the message's variable slot, not part of the requirement; it decodes to an integrity tag followed by the per-run identity name the v1 page ended at.)

  Expected (AWS): HTTP 200 from SESv2 ListEmailIdentities, an `EmailIdentities` page that CONTINUES the walk — the identity v1 returned on page 1 absent from it and from the remainder of the v2 walk — and the three identities the check created all reachable from (v1 page 1 ∪ v2 continuation).
  Actual (emulator): HTTP 400, Code=BadRequestException, Message="Invalid NextToken <" + <the token sent> + ">." — the surrounding text is fixed emulator text (`invalid_next_token_message`, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_email_identities.rs:248-250, raised via `bad_request`), the only variable part being the echoed token.

  Legs that PASSED before it, so the defect is narrowly the cross-surface hand-off and not pagination in general:
   - three domain identities created through sesv2.create_email_identity;
   - v1.list_identities(MaxItems=1) returned exactly one identity AND a non-empty NextToken — i.e. the token under test is one the emulator itself minted and considers valid on its own surface.

  Cursor formats and resume rules that make the hand-off structurally impossible:
   - ses crate (ops/list_identities.rs:203-237): `decode_page_token(LISTING_IDENTITIES, token)` must match the tag recomputed for that listing; the minted token names the identity the NEXT page starts at and the walk resumes with `Bound::Included(resume_from)`.
   - sesv2 crate (ops/list_email_identities.rs:186-230): `crate::state::decode_page_token(token)` decodes only this crate's own identity-listing cursor; the minted token names the LAST identity of the page served and the walk resumes strictly after it (`partition_point(|(key, _)| key <= last)`).
  So a v1 token never satisfies the v2 decode, `.ok_or_else(|| bad_request(invalid_next_token_message(token)))` fires, and even a shared encoding would resume one identity off.

## Steps to Reproduce
1. Create three domain identities so the directory has more than one page at page size 1: sesv2.create_email_identity(EmailIdentity=<fresh-domain-i>.example.com) for i in 0..3.
2. Take a fresh v1 cursor: first = ses.list_identities(MaxItems=1); assert len(first["Identities"]) == 1; page1_name = first["Identities"][0]; token = first["NextToken"] (non-empty). (This passes today.)
3. Hand that cursor to the other surface: sesv2.list_email_identities(NextToken=token).
   Expected: HTTP 200, an EmailIdentities page continuing the account listing, with page1_name NOT among its IdentityName values.
   Actual: botocore BadRequestException — HTTP 400, Code=BadRequestException, Message="Invalid NextToken <" + token + ">."
4. Continue the walk from the same cursor with sesv2.list_email_identities(NextToken=...) until no NextToken is returned: page1_name must never reappear, and all three identities created in step 1 must appear in (page1_name ∪ the continuation).
5. The symmetric direction is broken the same way and should be checked with the same fix: take a NextToken from sesv2.list_email_identities(PageSize=1) and pass it as ses.list_identities(NextToken=...) — the classic handler decodes only under its own LISTING_IDENTITIES hex tag and refuses it (InvalidParameterValue "Invalid NextToken <...>.").
6. Regression guard: an unissued/forged cursor must STILL be refused on each surface in that surface's own words (sesv2: 400 BadRequestException "Invalid NextToken <token>.").
