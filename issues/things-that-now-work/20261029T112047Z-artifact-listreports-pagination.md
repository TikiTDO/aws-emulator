Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 05bbbfdc708f909c61f7a8cb3ec61be4744981df

# The pagination cursor's "seal" takes no server-side secret, so a cursor a client assembles is honoured as a resume point this service never cut

_Filed: 2026-10-29T11:20:47Z_
_Operation: ListReports | Class: pagination_

> _Note: this issue was identified against the code base at commit `05bbbfdc708f909c61f7a8cb3ec61be4744981df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate (`ListReports`, `ListReportVersions`, `ListCustomerAgreements`, `ListComplianceInquiries`, `ListComplianceInquiryQueries`) reads its `nextToken` through the one cursor codec in `crate::state`. The codec's own contract, stated in the module comment above it, is that a value a client assembled must decode to NO cursor and be refused with the modelled `ValidationException`, "rather than being honoured as a resume point this service never cut".

The code does not implement that rule. `cursor_digest` is a bare SHA-256 over a PUBLIC constant (`CURSOR_DIGEST_DOMAIN`) concatenated with the payload — there is no per-process secret, no key, nothing a caller does not have. `decode_cursor` accepts any token whose trailing field equals `cursor_digest(payload)`, so any caller can assemble `hex("artifact|reports|-|<position>|" + sha256(domain + payload))` for ANY position and have it honoured: `ListReports` then resumes the catalog at that position and silently skips every entry before it, instead of answering the 400 `ValidationException` the codec promises. The same forgery defeats the SCOPING the codec claims for a sub-collection cursor — the scope field is `cursor_digest(report_id)`, equally computable — so a cursor "cut for one report" can be assembled for any other report or inquiry, which is exactly what the comment says a token cut for one resource must not do.

Two further spellings the minting side never writes are accepted for the same reason: `decode_position` finishes with `resume_at.parse()`, and Rust's `usize::from_str` accepts a leading `+` and leading zeros, while `encode_cursor` only ever writes `usize::to_string()`. So `…|reports|-|+0005` is honoured as position 5 although no call of this service ever emitted that payload.

The crate already knows the difference between a derivable value and an unguessable receipt: `mint_inquiry_export_token` rejects a counter-minted token precisely because "a counter-minted token is derivable by a caller that was handed no URL", and uses a v4 UUID instead. The cursor codec claims the same protection in prose while computing it from values every caller holds.

## Evidence

The rule the codec states for itself — services/service-com-amazonaws-artifact-artifact/src/state.rs:2133-2144:
```
//   * WHICH resource a scoped page belongs to is carried as the DIGEST of that resource's
//     id, so the cursor still binds to exactly one resource — a token cut for one report
//     is refused on another — without putting the id on the wire;
//   * the whole payload is SEALED with a digest over it, so a value a client assembled
//     decodes to no cursor at all and is refused with the modelled `ValidationException`,
//     rather than being honoured as a resume point this service never cut.
```

The digest that is supposed to seal it — src/state.rs:2193-2204 (no secret, only a public domain constant):
```
/// The digest a cursor's payload is sealed with.
fn cursor_digest(payload: &str) -> String {
    use ::sha2::Digest;
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(CURSOR_DIGEST_DOMAIN.as_bytes());
    hasher.update(payload.as_bytes());
    ::hex::encode(hasher.finalize())
}

/// The domain string every cursor digest is taken over, so a digest from this codec can
/// never collide with one this service computes for anything else.
const CURSOR_DIGEST_DOMAIN: &str = "com.amazonaws.artifact#pagination-cursor\n";
```

The check that therefore cannot tell a cursor this service cut from one a client assembled — src/state.rs:2209-2227:
```
fn decode_cursor(kind: &str, scope: Option<&str>, token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let cursor = String::from_utf8(decoded).ok()?;
    let (payload, digest) = cursor.rsplit_once(CURSOR_SEPARATOR)?;
    if digest != cursor_digest(payload) {
        return None;
    }
```
and the position parse that also admits spellings `encode_cursor` never writes — src/state.rs:2231-2233 vs src/state.rs:2237-2238:
```
fn decode_position(kind: &str, scope: Option<&str>, token: &str) -> Option<usize> {
    decode_cursor(kind, scope, token)?.parse().ok()
}
...
pub fn reports_page_token(position: usize) -> String {
    encode_cursor(REPORTS_CURSOR, None, &position.to_string())
```

The handler that spends it — src/ops/list_reports.rs:167-175:
```
        // A `nextToken` is opaque: the only thing it has to be is one of THIS operation's
        // own cursors. Anything that does not decode to one was never issued here, and is
        // refused with the service's one fixed invalid-token text, ...
        let resume_at = crate::state::resume_position(
            input.next_token.as_deref(),
            crate::state::parse_reports_page_token,
        )
        .map_err(ListReportsErrorModeled::ValidationException)?;
```

The crate's own contrasting treatment of a derivable value — src/state.rs:1732-1739:
```
/// A receipt rather than a key into a store, so unlike [`mint_counter_id`] it is NOT drawn
/// from a counter: a counter-minted token is derivable by a caller that was handed no URL,
/// and a URL carrying only derivable values authorises nobody. It is a version-4 UUID's 32
/// hex digits, ...
fn mint_inquiry_export_token() -> String {
    ::uuid::Uuid::new_v4().simple().to_string()
}
```

Test coverage of the gap: none. tests/all/pagination_token_opacity.rs:16-20 restates the same rule ("the payload is sealed with a digest over it, so a value a client assembled decodes to no cursor at all rather than to a resume point the service never cut"), but every row it feeds (lines 301-318) is a NON-cursor or a MUTILATED cursor — `"not-a-token"`, `"0"`, a bare record id, a bare ARN, `hex("artifact")`, its own cursor with one character changed, its own cursor truncated, and another listing's cursor. No row assembles a well-formed cursor with a correctly computed digest, so the current behaviour is unpinned and refusing a forged cursor changes no existing assertion.

## Steps to Reproduce

1. Start the emulator and call `GET /v1/report/list?maxResults=1`; note the catalog has several reports and that the answer carries a `nextToken`.
2. Call `GET /v1/report/list` with no token and record the full ordered list of report ARNs (N entries).
3. Build a cursor WITHOUT ever calling the service: payload = `artifact|reports|-|2`; digest = hex(SHA256("com.amazonaws.artifact#pagination-cursor\n" + payload)); token = hex(payload + "|" + digest).
4. Call `GET /v1/report/list?nextToken=<token>`: it answers 200 and resumes at position 2, silently skipping the first two catalog entries — although src/state.rs:2136-2138 says a value a client assembled must decode to no cursor and be refused with `ValidationException`.
5. Repeat with payload `artifact|reports|-|+0002` (a spelling `encode_cursor` never writes, since it emits `usize::to_string()`): it is honoured identically, because `decode_position` finishes with `str::parse::<usize>()`.
6. Repeat the construction with payload `artifact|reportVersions|<hex sha256 of the domain + "report-7g5j2DTVeNXWx7St">|1` and spend it on `GET /v1/report/listVersions?reportId=report-7g5j2DTVeNXWx7St&nextToken=…`: the scope "digest of the resource id" the comment relies on is equally computable, so a cursor nobody was issued binds to whichever report the caller chooses.
