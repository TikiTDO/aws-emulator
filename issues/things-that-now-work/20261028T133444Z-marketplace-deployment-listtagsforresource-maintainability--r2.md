Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# State each enforced character class once instead of as a constant plus a hand-rolled byte predicate

_Filed: 2026-10-28T13:34:44Z_
_Operation: ListTagsForResource | Class: maintainability_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each of the two character classes this service enforces is written TWICE in `src/state.rs`: once as a string constant that the refusal MESSAGE is built from, and once as a hand-written byte predicate that does the actual ENFORCING. Nothing ties the two copies together, so editing one leaves the other stating a different class, and the service then refuses (or admits) characters its own refusal message does not name.

Copy pair 1 (resource ARNs, reached first by `ListTagsForResource` through `crate::state::validate_resource_arn`): the constant `RESOURCE_ARN_CHARACTER_CLASS` is what the refusal sentence quotes, while `is_resource_arn_byte` is what decides acceptance.

Copy pair 2 (the modelled `TagsMap` key class, reached by `PutDeploymentParameter` through `validate_tags_member(..., TagConstraints::TagsMap)`): the constant `TAG_CHARACTER_CLASS` is what the refusal sentence quotes, while `is_tag_byte` / `matches_tag_character_class` decide acceptance.

The existing tests cannot catch the drift, because they assert that the message CONTAINS the constant — both halves of that assertion move together when the constant is edited, while the predicate stays where it was.

`regex` is already in `Cargo.lock` (Cargo.lock:4935 `name = "regex"`), so each class can be matched from the single constant that states it (anchored to a whole-value match, which is what the two predicates already implement via `bytes().all(..)`), leaving the accepted set, every message and every existing test unchanged. The two classes currently agree character for character, so this is a drift risk and not a behavioural bug — hence LOW.

## Evidence

ARN pair — the class as data, then the class as code, in the same file:

`src/state.rs:131`:
```
pub const RESOURCE_ARN_CHARACTER_CLASS: &str = "[a-zA-Z0-9:*/-]+";
```
`src/state.rs:917-920`:
```
fn is_resource_arn_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'*' | b'/' | b'-')
}
```
and the refusal that quotes the constant while the predicate decides, `src/state.rs:960-966`:
```
    if !(1..=RESOURCE_ARN_MAX_LEN).contains(&candidate.len())
        || !candidate.bytes().all(is_resource_arn_byte)
    {
        return Err(crate::scaffolded::BadRequest::new(format!(
            "Invalid input resource arn: {RESOURCE_ARN_MEMBER} must be \
             1..{RESOURCE_ARN_MAX_LEN} characters of {RESOURCE_ARN_CHARACTER_CLASS}"
        )));
```

Tag-key pair — same shape:

`src/state.rs:173`:
```
pub const TAG_CHARACTER_CLASS: &str = "[a-zA-Z0-9/_+=.:@-]+";
```
`src/state.rs:763-775`:
```
fn is_tag_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(byte, b'/' | b'_' | b'+' | b'=' | b'.' | b':' | b'@' | b'-')
}
...
fn matches_tag_character_class(candidate: &str) -> bool {
    candidate.bytes().all(is_tag_byte)
}
```
and `src/state.rs:848-855`, where the message again quotes the constant while the predicate decides:
```
        if constraints == TagConstraints::TagsMap && !matches_tag_character_class(key) {
            return Err(ValidationException {
                message: format!(
                    "tag key '{key}' is outside the {TAG_CHARACTER_CLASS} characters this member's tag keys may hold"
                ),
```

Why the suite would not see the drift — `tests/all/resource_arn_validation.rs:313-318` asserts the message against the same constant the message is built from:
```
                assert!(
                    reported.contains(RESOURCE_ARN_CHARACTER_CLASS)
                        && reported.contains(&RESOURCE_ARN_MAX_LEN.to_string()),
                    "case {case:?}/{operation}: the message states the member's own syntax, \
                     got {reported:?}"
                );
```

Test coverage that must stay unchanged by the fix: `tests/all/resource_arn_validation.rs` (`a_resource_arn_that_is_not_of_this_vendor_is_refused_on_the_input`, `a_resource_arn_of_this_vendor_is_judged_by_the_store_and_not_refused`), `tests/all/unconstrained_tag_characters.rs` (`a_tag_set_is_judged_by_the_map_its_member_targets`, `the_tagging_limits_still_refuse_what_no_tag_may_carry`, `an_unconstrained_tag_set_is_stored_and_listed_back`), `tests/all/validation_messages.rs`.

The crate already in the lockfile that replaces the hand-rolled match: `Cargo.lock:4935` `name = "regex"`.

## Steps to Reproduce

1. Open `services/service-com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice/src/state.rs` and note that `RESOURCE_ARN_CHARACTER_CLASS` (line 131) and `is_resource_arn_byte` (lines 917-920) state the same character class independently, as do `TAG_CHARACTER_CLASS` (line 173) and `is_tag_byte` (lines 763-767).
2. Simulate the drift: remove `:` from the `RESOURCE_ARN_CHARACTER_CLASS` string (or add a character to it) without touching `is_resource_arn_byte`.
3. Run the crate's tests: `tests/all/resource_arn_validation.rs:313-318` still passes, because it asserts the message CONTAINS the constant and the message is built from that same constant — so the refusal now advertises a class the service does not enforce, with no test failing.
4. Do the same with `TAG_CHARACTER_CLASS` versus `is_tag_byte` and observe the identical silent divergence in the `ValidationException` message built at `src/state.rs:848-855`.
