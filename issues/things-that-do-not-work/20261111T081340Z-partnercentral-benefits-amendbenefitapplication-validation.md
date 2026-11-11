Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 34aa023e13a037c836015cae341f7a7e68727fe7

# The BenefitApplicationDetails text bound is counted in BYTES but refused with this crate's character-counted length clause, so a multi-byte document inside the stated maximum is rejected

_Filed: 2026-11-11T08:13:40Z_
_Operation: AmendBenefitApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `34aa023e13a037c836015cae341f7a7e68727fe7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::details_requirement` is the shared `BenefitApplicationDetails` guard that `AmendBenefitApplication` (on the document its amendments produced), `CreateBenefitApplication` and `UpdateBenefitApplication` (on the document they were handed) all apply. Its text bound is measured in BYTES (`document_weight` adds `text.len()` / `name.len()`), but the refusal it states is built from `ViolationKind::MaxLength(MAX_DETAILS_BYTES)` — the walk's length wording, which this crate documents as counted in CHARACTERS and routes through exactly one helper so that a bound "cannot be worded — or counted — two ways".

The result is a bound stated one way and enforced another: the same handler that refuses an amendment's `NewValue` through the character-counted `length_clause` refuses the document it produced on a byte count, while telling the caller it exceeded a *length* of 65536. A details document of 40,000 two-byte characters is refused with "Member must have length less than or equal to 65536" even though it holds 40,000 characters — one third of the stated maximum for 3-byte characters. The identical request written in ASCII is served, so the number the refusal names is not the bound the caller is held to.

This crate has already fixed this exact class twice (`FileDetail$FileName` counted in bytes, `Amendment$FieldPath` counted in bytes); this is the third copy of the same mismatch and the only one left. Either end of the inconsistency can be made to agree — count the text through the crate's one length check, or stop wording a byte budget as the walk's length requirement — this report only states that the two disagree today.

## Evidence

The crate's single rule for a bound a handler enforces itself, `src/state.rs:3520-3528`:

    /// The one length check every handler of this service makes for itself goes through, so a
    /// bound a handler enforces cannot be worded — or counted — two ways. The requirement is
    /// worded by [`crate::service::constraint_requirement`], the same words the centralized
    /// walk uses for a length, and the length is counted in CHARACTERS, as the walk counts a
    /// `@length` on a string.
    pub fn length_clause(value: &str, min: usize, max: usize) -> Option<String> {
        count_length_clause(value.chars().count(), min, max)
    }

and `src/state.rs:356-357`: "The longest `Amendment$FieldPath` this service reads, in CHARACTERS — the unit [`length_clause`] counts every length this service enforces in."

The guard breaks it — `src/state.rs:2453-2455` (doc) and `src/state.rs:2462-2473` (code):

    /// … The byte bound is a length, so it is worded by the walk's own wording helper
    /// ([`crate::service::constraint_requirement`]) rather than by a copy that can drift.
    …
    let (members, bytes) = document_weight(document);
    …
    (bytes > MAX_DETAILS_BYTES).then(|| {
        crate::service::constraint_requirement(&smithy_server_runtime::ViolationKind::MaxLength(
            MAX_DETAILS_BYTES as i64,
        ))

where the count is bytes — `src/state.rs:2512` and `src/state.rs:2520-2525`:

        Document::String(text) => (0, text.len()),
    …
                        bytes + name.len() + member_bytes,

Both units sit side by side in one handler: `src/ops/amend_benefit_application.rs:544-550` bounds the leaf an amendment writes with the CHARACTER helper (`crate::state::length_clause(&amendment.new_value, 0, crate::state::MAX_DETAILS_VALUE_LEN)`), and `src/ops/amend_benefit_application.rs:889-893` then judges the document it produced with the BYTE guard (`crate::state::details_refusal(crate::state::DETAILS_MEMBER, amended.benefit_application_details.as_ref())`). `src/ops/create_benefit_application.rs:265-272` and `src/ops/update_benefit_application.rs:261-266` call the same guard.

Test coverage does not pin the choice: `tests/all/details_document_guard.rs:57-62` asserts the message "a length bound, so it is worded the way this service words every other length" (`Member must have length less than or equal to {MAX_DETAILS_BYTES}`), and every document it builds is ASCII (`tests/all/details_document_guard.rs:116-125`: `json!({ "m": "v".repeat(bytes.saturating_sub(1)) })`, `"n".repeat(...)`), so bytes and characters coincide in every row and no test states a reason for counting bytes.

## Steps to Reproduce

1. Grant the emulator account the partner entitlement and create an application: `CreateBenefitApplication` with `Catalog`, a `ClientToken`, and the `BenefitIdentifier` of a seeded benefit; note the returned `Id` and `Revision`.
2. Send `AmendBenefitApplication` with 20 amendments, each `FieldPath` = `BenefitApplicationDetails.f<N>` (N = 1..20) and `NewValue` = 1000 copies of a two-byte character (e.g. "é"). It is served: the document now holds 20 members, 20,000 characters, 40,000 bytes.
3. Send a second `AmendBenefitApplication` (new `ClientToken`, current `Revision`) adding 20 more such fields `f21..f40`. The document would hold 40 members and 40,000 characters — inside `MAX_DETAILS_MEMBERS` (256) and far inside the 65536 the refusal names.
4. Observe HTTP 400 `ValidationException` with `Message` = "1 validation error detected: Value at 'benefitApplicationDetails' failed to satisfy constraint: Member must have length less than or equal to 65536", because the 80,000 BYTES exceed `MAX_DETAILS_BYTES` although the document carries 40,000 characters.
5. Repeat the same two amendments with ASCII `NewValue`s of the same character length: both are served, showing the bound the caller is shown (a character length, as every other length this crate enforces) is not the bound enforced.
6. Equivalent single-call form: `CreateBenefitApplication` with `BenefitApplicationDetails` = {"m": "é" x 33000} (33,001 characters, 66,001 bytes) is refused with the same clause naming 65536.
