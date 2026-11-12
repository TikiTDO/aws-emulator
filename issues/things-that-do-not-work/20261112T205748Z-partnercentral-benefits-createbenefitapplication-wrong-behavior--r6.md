Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 75cd4519d4d8ae943d7fac1c316cbee2ee5c3551

# ClientToken fingerprint collapses an integer and an integer-valued float in BenefitApplicationDetails, replaying a different request as a retry

_Filed: 2026-11-12T20:57:48Z_
_Operation: CreateBenefitApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `75cd4519d4d8ae943d7fac1c316cbee2ee5c3551`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::request_fingerprint` is the identity a spent `ClientToken` is judged against. It hashes the canonical text produced by `crate::state::document_json`, which renders every `aws_smithy_types::Number` variant with plain `to_string()`. `Number::PosInt(1)` and `Number::Float(1.0)` therefore render to the identical text `1`, so two requests whose `BenefitApplicationDetails` documents the crate itself regards as DIFFERENT values produce the SAME fingerprint.

The crate uses two different notions of equality for the same document and the coarser one gates idempotency:

* `request_fingerprint` -> `document_json` (stringified) decides "is this the same request?"
* `UpdateBenefitApplication`'s own apply step uses structural `Document` equality (`!=`) to decide "is this the same value?" — and `Document::Number(PosInt(1)) != Document::Number(Float(1.0))`, so the handler would have treated it as a change worth storing and worth moving the revision for.

Consequence on `CreateBenefitApplication`: a create carrying a spent token with `{"Round": 1.0}` where the first carried `{"Round": 1}` is read by `replayed_create` as a true retry, so the handler returns 200 with the FIRST application's `Id`/`Arn`/`Revision` and creates nothing — a success for an application that was never created, with the caller's document silently dropped. The same path on `UpdateBenefitApplication` returns the earlier write's revision and never applies the new document.

Expected (the crate's own stated rule): a request repeating a spent token with any different member is a different request and is refused with the modelled `ConflictException` carrying `IDEMPOTENT_PARAMETER_MISMATCH`. The crate's test already pins exactly this for a neighbouring case — a document differing only in a member's JSON TYPE — so the int/float pair is the one type difference that escapes the rule.

## Evidence

The rule the fingerprint claims to implement — `src/state.rs:2376-2378`:

    /// The fingerprint of one request's parameters: the identity a repeated `ClientToken` is
    /// judged against, so a token already spent replays its first answer only for the SAME
    /// request.

The rendering that breaks it — `src/state.rs:2200-2204`:

        Document::Number(number) => match number {
            Number::PosInt(n) => n.to_string(),
            Number::NegInt(n) => n.to_string(),
            Number::Float(n) => n.to_string(),
        },

(`1u64.to_string()` and `1.0f64.to_string()` are both `"1"`; likewise `-1i64` / `-1.0f64` are both `"-1"`.)

The same crate judging those two documents DIFFERENT, structurally — `src/ops/update_benefit_application.rs:441-446`:

        if let Some(details) = input.benefit_application_details {
            if application.benefit_application_details.as_ref() != Some(&details) {
                application.benefit_application_details = Some(details);
                applied_members.push(MEMBER_DETAILS.to_string());
            }
        }

The document is fed to the fingerprint as a raw value on both write paths — `src/ops/create_benefit_application.rs:348-354`:

            (
                crate::state::DETAILS_MEMBER,
                input
                    .benefit_application_details
                    .as_ref()
                    .map(crate::state::fingerprint_value),
            ),

and `src/ops/update_benefit_application.rs:328-334` (same entry under `MEMBER_DETAILS`).

Where the collision turns into a hollow success — `src/state.rs:1825-1831`:

        if spent.request_fingerprint != request_fingerprint {
            return Err(idempotency_conflict());
        }
        Ok(self
            .catalogs
            .get(&spent.catalog)
            .and_then(|stored| stored.applications.get(&spent.application_id)))

and `src/ops/create_benefit_application.rs:398-404`:

            Ok(Some(existing)) => {
                return Ok(CreateBenefitApplicationOutput {
                    id: Some(existing.id.clone()),
                    arn: Some(existing.arn.clone()),
                    revision: Some(existing.created_revision_text()),
                })
            }

Test coverage that pins the intended rule for every OTHER document difference but misses this one — `tests/all/idempotent_parameter_mismatch.rs:176-179`:

            "a different TYPE in BenefitApplicationDetails",
            &|body: &mut Value| {
                body["BenefitApplicationDetails"] = json!({ "Notes": "spring", "Round": "1" })
            },

and `tests/all/idempotent_parameter_mismatch.rs:60-63`:

    assert_eq!(
        status, 409,
        "case {case:?}: reusing a spent ClientToken for a different request must answer 409, got {status} {out:?}"
    );

## Steps to Reproduce

1. POST `CreateBenefitApplication` with `Catalog: "AWS"`, a valid `BenefitIdentifier`, `ClientToken: "tok-1"` and `BenefitApplicationDetails: {"Round": 1}`; note the returned `Id`.
2. POST `CreateBenefitApplication` again with every member identical except `BenefitApplicationDetails: {"Round": 1.0}`, reusing `ClientToken: "tok-1"`.
3. Expected: 409 `ConflictException` with `IDEMPOTENT_PARAMETER_MISMATCH`, as `tests/all/idempotent_parameter_mismatch.rs` requires for any other differing member (including `{"Round": "1"}`).
4. Actual: 200 with the FIRST call's `Id`/`Arn`/`Revision`; no second application exists and the `{"Round": 1.0}` document was never stored — confirm with `ListBenefitApplications` and `GetBenefitApplication`.
5. Same on the update path: `UpdateBenefitApplication` with `ClientToken: "tok-2"` and `BenefitApplicationDetails: {"Round": 1}`, then again with `{"Round": 1.0}` under `"tok-2"` — the second is replayed, so the revision does not move and the document the handler's own `!=` at `update_benefit_application.rs:442` would have applied is dropped.
