Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 0ae6f58da9eea98f3529421bd2884e28b9157587

# The "split an ARN into its six fields and require this service's arnNamespace" rule is hand-rolled three times in state.rs

_Filed: 2026-11-12T07:05:12Z_
_Operation: GetResourcePolicy | Class: maintainability_

> _Note: this issue was identified against the code base at commit `0ae6f58da9eea98f3529421bd2884e28b9157587`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three functions in `src/state.rs` each re-implement the same computation and the same branch: `value.splitn(6, ':')` destructured into the identical six-tuple pattern that requires the literal `arn` in field 0 and `ARN_NAMESPACE` in field 2, then each adds its own extra check (character classes, the emulator account, the `<catalog>/Assessment/<id>` resource prefix). The shared half is one rule — "this value is an ARN of this service's namespace, and here are its partition / region / account / resource fields" — copied three times, so the copies can drift: a change to how this service reads an ARN's fields (an extra field, a different namespace comparison) has to be made in three places or the resource-policy surface, the tagging surface and `DescribeAssessment` will disagree about what an ARN of this catalog is. Lifting the common destructure into one helper that returns the four variable fields leaves every refusal, every response and every existing test unchanged, because each caller keeps its own extra checks.

## Evidence

COPY 1 — `src/state.rs:3597-3615` (`is_resource_arn`):
```
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(ARN_NAMESPACE),
        Some(region),
        Some(account),
        Some(resource),
    ) = (fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
    else { return false; };
```
COPY 2 — `src/state.rs:3660-3678` (`parse_resource_arn`):
```
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(_partition),
        Some(ARN_NAMESPACE),
        Some(_region),
        Some(account),
        Some(resource),
    ) = (fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
    else { return None; };
```
COPY 3 — `src/state.rs:3415-3433` (`assessment_id_of`):
```
    let mut fields = identifier.splitn(6, ':');
    let (
        Some("arn"),
        Some(_partition),
        Some(ARN_NAMESPACE),
        Some(_region),
        Some(_account),
        Some(resource),
    ) = (fields.next(), fields.next(), fields.next(), fields.next(), fields.next(), fields.next())
    else { return None; };
```
EXISTING TESTS that cover all three and would have to keep passing byte for byte: `tests/all/resource_policy_validation.rs` and `tests/all/resource_policy_reads.rs` (the `is_resource_arn` 422 and the `parse_resource_arn` 403/404 split), `tests/all/resource_policy_and_tagging.rs` (the same ARNs on the tagging surface) and `tests/all/assessment_identifier_length.rs` plus `tests/all/assessments.rs` (the `assessment_id_of` ARN alternative).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-marketplacecatalog-awsmpseymour/src/state.rs` and read `assessment_id_of` (3408), `is_resource_arn` (3587) and `parse_resource_arn` (3659).
2. Observe the identical `splitn(6, ':')` + six-tuple `(Some("arn"), Some(partition), Some(ARN_NAMESPACE), Some(region), Some(account), Some(resource))` destructure in all three, each followed by its own distinct extra checks.
3. Confirm the drift risk: changing the namespace comparison or the field count in one copy leaves the other two accepting ARNs the first now refuses.
