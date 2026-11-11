Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# GetAccountActivity's activityId re-check hand-writes its own clause and echoes the submitted id, breaking the two refusal rules this crate states once

_Filed: 2026-11-11T06:44:33Z_
_Operation: GetAccountActivity | Class: error-response_

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states two rules about the `message` a handler-raised `ValidationException` of this service carries:

* `crate::service::validation`'s rule 8: a MODELLED constraint a handler re-checks for itself is stated through the very clause writer the boundary walk's own refusal goes through, "so no second rendering of a value set, of a member path or of a requirement exists in this crate to drift from the one above";
* `crate::state::stated_validation`'s doc: the sentence it wraps "states the condition and nothing of the request".

Every other handler-side re-check of a modelled constraint in this crate obeys rule 8: `language_code_refusal` on BOTH activity readers and `enum_value_refusal` on `UpgradeAccountPlan` go through `crate::service::validation::outside_enum`, and `enum_element_refusal` on `ListAccountActivities` goes through `outside_element_enum`; the service-enforced page-size ceiling goes through `above_ceiling`. `GetAccountActivity`'s re-check of the modelled `ActivityId` constraints (`@length(32,32)` + `@pattern(^[a-zA-Z0-9]+$)`) is the one that does not: it hand-writes a second rendering of the member path AND of the requirement ("activityId: … must be exactly 32 characters matching ^[a-zA-Z0-9]+$"), and it interpolates the SUBMITTED id into the sentence before handing it to `stated_validation` — the envelope whose own doc forbids exactly that.

The inconsistency is visible inside this one file: the sibling not-found refusal immediately below is documented as echoing "nothing of the request — not the id that was asked for", while the refusal above it echoes the id. The clause-writer machinery the fix needs already exists in this crate (`crate::service::validation`'s `one_clause` / `Violation` + `ViolationKind` path that `outside_enum` and `above_ceiling` use), so this is a path skipping a helper the crate already has, not a new feature.

No test pins the hand-written wording — `tests/all/resource_not_found_status.rs:228-235` asserts only the error CODE for a malformed id — so routing it through the one clause writer leaves every existing assertion unchanged. Behavioural impact is nil today because the framework's constraint walk refuses a malformed `activityId` in front of `handle` (the handler comment says so), which is why this is LOW; the defect is the second, drift-prone rendering of a modelled requirement and the request echo inside an envelope documented to carry none.

## Evidence

The rule the handler breaks (rule 8), `services/service-com-amazonaws-freetier-awsfreetierservice/src/service/validation.rs:39-44`:
```
//! 8. a MODELLED constraint a handler re-checks for itself, so its refusal stays this
//!    operation's 400 whatever the boundary does with the value, is stated through the
//!    very clause writer the walk's own refusal goes through
//!    ([`outside_enum`], [`outside_element_enum`]) — so no second rendering of a value
//!    set, of a member path or of a requirement exists in this crate to drift from the
//!    one above.
```

The envelope rule it also breaks, `services/service-com-amazonaws-freetier-awsfreetierservice/src/state.rs:702-707`:
```
/// `sentence` states the condition and nothing of the request: a reason string
/// that varied with the input would make a client's branch on it depend on
/// something the rule does not distinguish.
pub fn stated_validation(sentence: &str) -> String {
    format!("[Message = {sentence}]")
}
```

The handler code that does the opposite, `services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_account_activity.rs:176-186`:
```
        // An id that cannot be an `ActivityId` is a malformed request, not a
        // missing activity: the framework refuses it before `handle` runs, and
        // checking here keeps that a `ValidationException` rather than a
        // not-found whatever the boundary does.
        let activity_id = input.activity_id.as_str();
        if !crate::state::is_activity_id(activity_id) {
            return Err(validation(format!(
                "activityId: {activity_id} must be exactly 32 characters matching \
                 ^[a-zA-Z0-9]+$"
            )));
        }
```
and the envelope it is handed to, `src/ops/get_account_activity.rs:232-240`:
```
/// Refuse the request with this operation's own modelled `ValidationException`
/// (HTTP 400), stating `message` in the envelope this service's validation
/// front door states a refusal in ([`crate::state::stated_validation`]).
fn validation(message: impl AsRef<str>) -> GetAccountActivityError {
    GetAccountActivityErrorModeled::ValidationException(ValidationException {
        message: crate::state::stated_validation(message.as_ref()),
    })
```

The sibling refusal in the SAME file, documented to do the opposite, `src/ops/get_account_activity.rs:256-263`:
```
/// What this service states for a well-formed `ActivityId` that names no
/// activity of the caller's account: a FIXED sentence with no variable parts.
///
/// The message echoes nothing of the request — not the id that was asked for,
/// not the account it was asked of — because the condition it reports is the
/// same for every id the catalog does not hold…
const ACTIVITY_NOT_FOUND: &str = "Activity not found";
```

The sibling re-checks in this crate that DO go through the one clause writer — `src/state.rs:890-892`:
```
pub fn language_code_refusal(language_code: Option<&str>) -> Option<String> {
    enum_value_refusal(LANGUAGE_CODE_MEMBER, language_code?, LANGUAGE_CODE_VALUES)
}
```
and `src/state.rs:831-838`:
```
pub fn enum_value_refusal(member: &str, value: &str, declared: &[&str]) -> Option<String> {
    if declared.contains(&value) {
        return None;
    }
    Some(crate::service::validation::outside_enum(
        member, value, declared,
    ))
}
```

The duplicated constraint itself, `src/state.rs:894-898`:
```
/// Whether `activity_id` satisfies the modelled `ActivityId` constraints:
/// exactly 32 characters, each of `[a-zA-Z0-9]`.
pub fn is_activity_id(activity_id: &str) -> bool {
    activity_id.len() == 32 && activity_id.chars().all(|c| c.is_ascii_alphanumeric())
}
```

Test coverage: `tests/all/resource_not_found_status.rs:228-235` asserts only `error_code(&payload) == "ValidationException"` for the four malformed-id rows and asserts nothing about the message, and `tests/all/activity_plane_refusals.rs:312-383` pins the clause-writer wording for `languageCode` on BOTH readers (`"1 validation error detected: Value '{code}' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [...]"`) — so the one member of this operation whose refusal is hand-written is also the one with no wording test.

## Steps to Reproduce

1. Read `src/service/validation.rs:39-44` (rule 8: a handler's re-check of a modelled constraint is stated through the walk's own clause writer, so no second rendering of a member path or a requirement exists in this crate) and `src/state.rs:702-707` (the `stated_validation` envelope's sentence "states the condition and nothing of the request").
2. Read `src/ops/get_account_activity.rs:176-186`: the re-check of the modelled `ActivityId` `@length`/`@pattern` builds its own sentence, naming the member path and restating the requirement, and interpolates the submitted `activity_id` into it.
3. Read `src/ops/get_account_activity.rs:232-240`: that sentence is handed to `crate::state::stated_validation`, the envelope documented to carry nothing of the request.
4. Compare with every other handler-side re-check in the crate — `src/state.rs:890-892` and `:831-838` (`language_code_refusal` -> `enum_value_refusal` -> `crate::service::validation::outside_enum`), `src/state.rs:851-869` (`enum_element_refusal` -> `outside_element_enum`), `src/state.rs:600-603` (`max_results_refusal` -> `above_ceiling`): all of them route through `crate::service::validation`, which `activityId` alone bypasses.
5. Compare with the sibling refusal in the same handler, `src/ops/get_account_activity.rs:256-263`, which is documented as echoing "nothing of the request — not the id that was asked for": one file states two opposite rules for its two refusals.
6. Confirm nothing pins the hand-written wording: `tests/all/resource_not_found_status.rs:228-235` checks only the error code for malformed ids, while `tests/all/activity_plane_refusals.rs:312-383` pins the clause-writer wording for the operation's other re-checked member.
