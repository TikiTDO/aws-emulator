Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 92fe44509b790ef44b44d785952419008ad12946

# Pagination cursor identity keys on the request's spelling of a filter, not the selection it means

_Filed: 2026-11-13T07:45:35Z_
_Operation: ListLinuxSubscriptionInstances | Class: pagination_

> _Note: this issue was identified against the code base at commit `92fe44509b790ef44b44d785952419008ad12946`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two `Filters`-driven lists bind every `NextToken` to a "list identity" derived from `crate::state::filter_signature(&filters)`. The crate normalises a filter in ONE place but not the other:

* `filter_selects` treats an ABSENT `Filter$Operator` as `Equal` (`unwrap_or(OPERATOR_EQUAL)`), and compares the filter's `Values` as a SET (`values.contains(held)`), so `{Name:"Status",Values:["running"]}` and `{Name:"Status",Values:["running"],Operator:"Equal"}` select exactly the same list, and so do `Values:["a","b"]` and `Values:["b","a"]`.
* `filter_signature` renders the operator with `unwrap_or_default()` — the EMPTY string for an absent one, never `"Equal"` — and renders the values in request order. The two spellings of one selection therefore hash to two different list identities.

Consequence: a client that pages a list and then spells the default operator explicitly (or reorders the `Values` of a `uniqueItems` set) on the continuation request is refused with `ValidationException` "NextToken is not a valid pagination token" (`decode_page_token`, state.rs:1076-1082), even though the token names an element of exactly the list the continuation selects. The walk cannot be resumed and the caller has to restart from the beginning. `ListRegisteredSubscriptionProviders` has the same defect through `source_filter_signature`, which renders `SubscriptionProviderSources` in request order although the handler applies it as a set (`sources.contains(&record.source)`).

The handlers' own comment states the intended rule as being about the SELECTION, not about the request's spelling of it.

## Evidence

Both sides of the normalisation, in this crate:

`src/state.rs:1155-1158` — the crate's own equivalence rule:
```
/// The comparison an absent `Filter$Operator` means: `Filters` selects the results that
/// "match one or more sets of key-value pairs that you specify", so a filter with no
/// operator selects on equality (`Operator`: Contains | Equal | NotEqual).
pub const OPERATOR_EQUAL: &str = "Equal";
```

`src/state.rs:1216-1223` — the selection path APPLIES that normalisation, and compares values as a set:
```
    let equals = member_values.iter().any(|held| values.contains(held));
    match filter.operator.as_deref().unwrap_or(OPERATOR_EQUAL) {
```

`src/state.rs:1129-1141` — the cursor path SKIPS it:
```
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        signature.push_str(&length_prefixed(
            [
                filter.name.clone().unwrap_or_default(),
                filter.operator.clone().unwrap_or_default(),
            ]
            .into_iter()
            .chain(filter.values.clone().unwrap_or_default()),
```

`src/ops/list_linux_subscription_instances.rs:191-198` — the rule the handler says it is implementing:
```
        let (page, next_token) = crate::state::paginate(
            &selected,
            // The cursor belongs to THIS list: the operation plus the filter selection, so
            // a cursor minted for another selection is not accepted here.
            &format!(
                "linux-subscription-instances/{}",
                crate::state::filter_signature(&filters)
            ),
```
(`src/ops/list_linux_subscriptions.rs:194-201` is the same call; `src/ops/list_registered_subscription_providers.rs:174-181` uses `source_filter_signature`, defined at `src/state.rs:1118-1120` as `length_prefixed(sources.iter().cloned())`.)

Refusal the mismatch produces, `src/state.rs:1075-1082`:
```
fn decode_page_token(kind: &str, token: &str) -> Result<Vec<String>, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
...
    if token_identity != list_identity(kind) {
        return Err(invalid());
```

Test coverage: `tests/all/linux_subscriptions.rs:784-802` pins only the intended case — "A cursor is a position in ONE list: the one the filter selection produced. The token minted by a filtered first page is not honoured for a different selection" — minting with `{Name:"Region",Operator:"Contains"}` and replaying under `{Name:"Status",Operator:"Equal"}`, a genuinely different selection. No test spells the same selection two ways, so the absent-vs-`Equal` and the values-order cases are uncovered.

## Steps to Reproduce

1. Enable discovery and let a pass record more instances than one page holds.
2. POST /subscription/ListLinuxSubscriptionInstances with body {"MaxResults":1,"Filters":[{"Name":"Status","Values":["running"]}]} (no Operator) and keep the returned NextToken.
3. POST the same operation with {"MaxResults":1,"NextToken":"<token>","Filters":[{"Name":"Status","Values":["running"],"Operator":"Equal"}]} — the same selection, with the default operator spelled out.
4. Observe HTTP 400 ValidationException "NextToken is not a valid pagination token" instead of the next page; the same happens if only the order of a multi-value `Values` list differs between the two requests.
