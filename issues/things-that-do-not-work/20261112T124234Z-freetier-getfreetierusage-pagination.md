Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 50213015a2544358988fbce7e3573d190e622c8f

# A nextToken whose offset is spelled with leading zeros or a leading '+' is served, though the only issuer can never spell it that way

_Filed: 2026-11-12T12:42:34Z_
_Operation: GetFreeTierUsage | Class: pagination_

> _Note: this issue was identified against the code base at commit `50213015a2544358988fbce7e3573d190e622c8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate decides "was this token issued by me" in ONE place, and states the rule in two halves: the token's FORM must be the form `crate::state::encode_page_token` gives a token of this listing, and the offset must be one `cut_page` (the only issuer) could have encoded. The offset half was tightened to `1..stored_rows`; the FORM half is still only "hex, version marker, list kind, and a tail `parse::<usize>` accepts".

`encode_page_token` renders the offset with `format!("{offset}")` on a `usize`, so the only decimal spellings it can ever produce are canonical: no sign, no leading zeros. But `usize`'s `FromStr` accepts any number of leading zeros and an optional leading `+`, so `hex("ftv1:freeTierUsages:01")`, `hex("ftv1:freeTierUsages:0000001")` and `hex("ftv1:freeTierUsages:+1")` all decode to offset 1 and are served HTTP 200, resuming the listing at record 1 — even though no page of this listing ever handed out that token. A corrupted or hand-forged cursor is therefore read as a legitimate one and answered as a success, while the sibling malformations of the very same token (a non-parsable tail, the other listing's kind, non-hex bytes, an out-of-range offset) are all refused with the modelled `ValidationException` (HTTP 400) in the one wording this service refuses every unissued token in.

Both paginated operations decode through the same helper (`ListAccountActivities` passes `PAGE_KIND = "activities"`), so the hole is shared; it is reported once, against the operation whose handler states the claim quoted below.

## Evidence

The only issuer, which can spell an offset only canonically — `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:511-518`:
```
/// Encode the position a page stopped at — the offset of the next unreported
/// row — into an opaque `nextToken`.
...
pub fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_VERSION}:{kind}:{offset}"))
}
```

The decode, whose doc claims it answers `None` for anything that is not that form — `src/state.rs:520-531`:
```
/// The offset a `nextToken` carries, or `None` when the token does not even
/// have the FORM [`encode_page_token`] gives a token of this `kind`.
...
fn decode_page_token(kind: &str, token: &str) -> Option<usize> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let offset = text.strip_prefix(&format!("{PAGE_TOKEN_VERSION}:{kind}:"))?;
    offset.parse::<usize>().ok()
}
```
`parse::<usize>()` is the whole form check on the tail, and it accepts `"01"`, `"0000001"` and `"+1"` as 1 — three spellings `format!("{offset}")` above can never emit.

The rule the two halves are supposed to enforce together — `src/state.rs:538-549`:
```
/// A token is one this service issued only when BOTH halves hold: its form is
/// the form [`encode_page_token`] gives a token of this listing, and the offset
/// it carries is one [`cut_page`] — the only issuer — could have encoded.
```

The claim the handler makes when it refuses — `src/ops/get_free_tier_usage.rs:179-188`:
```
        // A token whose form or whose offset is not one a page of this listing
        // could have issued is a token this operation never issued.
        let offset = match crate::state::page_offset(
            PAGE_KIND,
            input.next_token.as_deref(),
            records.len(),
        ) {
            Some(offset) => offset,
            None => return Err(validation(crate::state::INVALID_NEXT_TOKEN)),
        };
```
and the sibling listing's identical call, `src/ops/list_account_activities.rs:212-219`.

Existing coverage pins the offset half only — `tests/all/unissued_page_token_offset.rs:3-14`:
```
//! A `nextToken` is accepted only when it is one the listing could have issued
//! — in its FORM *and* in the offset it carries.
```
and its probe table, `tests/all/unissued_page_token_offset.rs:126-136`, holds only canonical numerals (`"0"`, `total`, `total+1`, `total+2`, `10*total`, `"999"`, `format!("{}0", usize::MAX)`): no row probes a non-canonical SPELLING of an in-range offset, so the FORM half the module doc promises is unasserted. `tests/all/unissued_page_token_offset.rs:77-91` pins that a one-row page issues exactly `hex("ftv1:<kind>:1")`, which is the canonical spelling the forged ones mimic.

## Steps to Reproduce

1. Boot the emulator with this service mounted (as tests/all/free_tier.rs::harness does).
2. Call GetFreeTierUsage with {"maxResults": 1} and note the issued nextToken is hex("ftv1:freeTierUsages:1") — the canonical spelling, pinned by tests/all/unissued_page_token_offset.rs:77-91.
3. Call GetFreeTierUsage with {"maxResults": 1000, "nextToken": hex("ftv1:freeTierUsages:01")} (also try "0000001" and "+1"): observe HTTP 200 resuming at record 1, though src/state.rs:511-518 can never render the offset as "01", "0000001" or "+1", so no page ever issued that token.
4. Call GetFreeTierUsage with {"nextToken": hex("ftv1:freeTierUsages:1x")}: observe HTTP 400 ValidationException "[Message = The next token is invalid.]" — an equally unissued token, refused only because its tail is not parsable at all.
5. Repeat step 3 against ListAccountActivities with hex("ftv1:activities:01"): same acceptance, because both listings decode through crate::state::decode_page_token (src/ops/list_account_activities.rs:212-219).
6. Compare the outcome of steps 3 and 4 with src/state.rs:520-525 and :538-549, and with the handler comment at src/ops/get_free_tier_usage.rs:179-181, all three of which say a token not in the form encode_page_token produces is a token this operation never issued.
