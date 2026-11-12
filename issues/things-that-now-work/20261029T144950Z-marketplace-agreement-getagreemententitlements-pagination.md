Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: a8bb77f04de11c29da4c1926ec69265ce5cd3881

# page_offset honours nextToken spellings page_token never mints (uppercase hex, a leading '+'), so a token this service never issued is served as a cursor

_Filed: 2026-10-29T14:49:50Z_
_Operation: GetAgreementEntitlements | Class: pagination_

> _Note: this issue was identified against the code base at commit `a8bb77f04de11c29da4c1926ec69265ce5cd3881`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paginated operation of this crate decodes its `nextToken` through the single `crate::state::page_offset` helper, which the crate documents as returning `None` "when it is not a token this service minted for that scope". The minting side (`page_token`) renders the 8-character offset half with `{offset:0width$x}` — lowercase hex only — and its doc asserts "Every character is a hex digit".

The decoding side does not hold the token to that spelling. After the length check and the byte-exact comparison of the 16-character digest half, the offset half is parsed with `usize::from_str_radix(offset, 16)`, which additionally accepts

* uppercase and mixed-case hex digits (`A`-`F`), e.g. `0000000A` for offset 10, and
* a leading `+` sign, e.g. `+0000001` for offset 1 (`+` is inside `NextToken`'s modelled `^[a-zA-Z0-9+/=_-]+$`, so the boundary validator passes it through).

Neither spelling is a token this service ever minted, yet both are accepted and resume the page. So the one condition the crate's own suite says must be refused — "a token this service never minted" answered with HTTP 400 `ValidationException` / `INVALID_NEXT_TOKEN` — is silently honoured instead, and one logical page has several accepted token spellings while the `nextToken` the response hands back is only ever the lowercase one. Any client or proxy layer that normalises case on an opaque token (or that re-renders it) is given a cursor the service claims not to have issued, and the refusal a caller is told to branch on never arrives.

This reaches every paginated operation of the service — `GetAgreementEntitlements`, `GetAgreementTerms`, `ListAgreementCharges`, `ListAgreementCancellationRequests`, `ListAgreementPaymentRequests`, `ListBillingAdjustmentRequests`, `ListAgreementInvoiceLineItems` and `SearchAgreements` all call `page_offset` — and is filed against `GetAgreementEntitlements` as the first one on the checklist whose handler calls it (src/ops/get_agreement_entitlements.rs:190).

## Evidence

The minting side, which writes the offset as lowercase hex only — services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:1316-1326:
```
/// The `nextToken` that resumes `scope` at `offset`.
///
/// Every character is a hex digit, so the token satisfies `NextToken`'s
/// `^[a-zA-Z0-9+/=_-]+$` and its 8192-character bound.
pub fn page_token(scope: &str, offset: usize) -> String {
    format!(
        "{digest}{offset:0width$x}",
        digest = scope_digest(scope),
        width = PAGE_TOKEN_OFFSET_LEN
    )
}
```

The decoding side, which honours spellings the line above never writes — state.rs:1328-1339:
```
/// The offset `token` resumes `scope` at, or `None` when it is not a token this
/// service minted for that scope.
pub fn page_offset(scope: &str, token: &str) -> Option<usize> {
    if !token.is_ascii() || token.len() != PAGE_TOKEN_LEN {
        return None;
    }
    let (digest, offset) = token.split_at(PAGE_TOKEN_DIGEST_LEN);
    if digest != scope_digest(scope) {
        return None;
    }
    usize::from_str_radix(offset, 16).ok()
}
```
(the digest half IS compared byte-exactly, so only the offset half is loose; `usize::from_str_radix` accepts `A`-`F` and an optional leading `+`.)

The handler that turns `None` into the refusal, and so never sees these tokens — src/ops/get_agreement_entitlements.rs:188-201:
```
        let offset = match input.next_token.as_deref() {
            None => 0,
            Some(token) => match crate::state::page_offset(&scope, token) {
                Some(offset) => offset,
                None => {
                    return Err(
                        GetAgreementEntitlementsErrorModeled::ValidationException(
                            crate::state::validation_exception("INVALID_NEXT_TOKEN", "nextToken"),
                        )
                        .into(),
                    );
```

The crate's own stated rule for a token it did not mint — tests/all/page_token_scopes.rs:5-10:
```
//! The rule under test is the one every listing of this crate builds its page
//! tokens from: the token a listing mints belongs to the scope of the query that
//! minted it ... so it resumes exactly that query and no other, and a call whose
//! parameters differ in any one of those parts refuses it with HTTP 400
//! `ValidationException` / `INVALID_NEXT_TOKEN`.
```

Test coverage: the refusal is pinned only for structurally different tokens, never for a re-spelling of a real one — tests/all/agreement_lifecycle.rs:1348-1376 drives `("a token this service never minted", "notatokenatall")` and a token minted for another agreement; tests/all/agreement_entitlements.rs:437-528, tests/all/page_token_scopes.rs:84-112 and tests/all/listing_resolution_order.rs:97 do the same. A grep of tests/all for `to_uppercase`/`uppercase` finds only tests/all/payment_request_balance.rs:133 (a currency-code assertion), so no test exercises an uppercased or `+`-prefixed offset half.

## Steps to Reproduce

1. Create and accept an agreement so it grants more than one entitlement (as tests/all/agreement_entitlements.rs does), and note the agreementId.
2. Call GetAgreementEntitlements with {"agreementId": <id>, "maxResults": 1} and read the 24-character `nextToken` from the response, e.g. `d2f1...a90000001`.
3. Uppercase only the last 8 characters of that token (the offset half) wherever they contain a-f — e.g. `...0000000A` from `...0000000a` — leaving the first 16 characters untouched, and resend GetAgreementEntitlements with that token.
4. Observe HTTP 200 with the page at that offset, although `page_token` never mints that spelling and state.rs:1328-1329 says `page_offset` answers `None` for anything the service did not mint; the documented HTTP 400 ValidationException / INVALID_NEXT_TOKEN never arrives.
5. Replace the offset half with `+0000001` (keeping the 16-character digest half) and resend: `NextToken`'s pattern `^[a-zA-Z0-9+/=_-]+$` admits `+`, so the request reaches the handler and is again answered HTTP 200 at offset 1.
