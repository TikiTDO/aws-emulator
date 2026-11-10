Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# PutMetadataFlag answers an unmodelled ServiceQuotaExceededException (402) for an emulator store bound, where StartCodegenJob deliberately answers a modelled error for the same condition

_Filed: 2026-10-30T06:39:27Z_
_Operation: PutMetadataFlag | Class: error-response_

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate faces one situation twice — "an emulator-owned bound on a store is reached on an operation whose model declares NO quota error" — and resolves it two contradictory ways, so the same class of refusal reaches a client as two different statuses, one of which the operation's model never declares.

`PutMetadataFlag`'s modelled errors are `InvalidParameterException` (400) and `UnauthorizedException` (401) only. The handler widens `ServiceQuotaExceededException` (402) in through `extra_errors` and answers it when the metadata store is at `METADATA_ENVIRONMENT_QUOTA`, justifying it with "`CreateComponent`, which lists the shape, uses it for the same emulator bound" — but `CreateComponent` *models* that shape, and `PutMetadataFlag` does not, so an SDK client of `PutMetadataFlag` has no modelled exception class for the answer (and for this service the refusal carries no error-type discriminator at all, see `src/service/untyped_errors.rs`, so the status is all a client has).

`StartCodegenJob` is in exactly the same position — an emulator bound (`CODEGEN_JOB_QUOTA`) on an operation whose model declares no quota error — and the crate explicitly refuses to widen the 402 there, choosing a *modelled* refusal instead and writing down why. Both reasons cannot hold at once: either widening an unmodelled quota error is acceptable (then StartCodegenJob's reasoning is wrong) or it is not (then PutMetadataFlag's 402 is wrong). As written, the crate's own rule for this case is self-contradictory and the two bounds answer 402 and 429 respectively.

## Evidence

The model's error list for this operation (smithy_query query="operation", operation="PutMetadataFlag"):

    "errors": [{"shape": "InvalidParameterException", "errorKind": "client", "httpError": 400},
               {"shape": "UnauthorizedException", "errorKind": "client", "httpError": 401}]

Side 1 — services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/put_metadata_flag.rs:116-125 widens the unmodelled 402 in:

    // * `ServiceQuotaExceededException` (HTTP 402) — `appId` and `environmentName`
    //   are request-supplied `@httpLabel` values, so the metadata store this
    //   operation writes is bounded like the component store
    //   (`crate::state::METADATA_ENVIRONMENT_QUOTA`), and past that bound the write is
    //   refused rather than growing the state without end. `CreateComponent`, which
    //   lists the shape, uses it for the same emulator bound.
    #[smithy_macros::operation_error(
        "com.amazonaws.amplifyuibuilder#PutMetadataFlag",
        extra_errors = ["InternalServerException", "ServiceQuotaExceededException"]
    )]

and answers it at put_metadata_flag.rs:222-233:

        if !store.can_hold_metadata_for(&key) {
            return Err(PutMetadataFlagErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException { message: Some(format!(
                    "this emulator already holds metadata for the maximum of {} app \
                     environments", crate::state::METADATA_ENVIRONMENT_QUOTA)) },
            ).into());
        }

Side 2 — the sibling handler in the SAME crate, facing the same condition, states the opposite rule: services/service-com-amazonaws-amplifyuibuilder-amplifyuibuilder/src/ops/start_codegen_job.rs:187-204:

        // The store is bounded: past its quota a start is refused rather than adding
        // another record. This operation models no quota error, so the refusal is the
        // modelled `ThrottlingException` (HTTP 429) — the one client refusal whose
        // semantics, the service declining to take more work, fit a store that cannot
        // take another job; a `ValidationException` would blame the request, which is
        // well formed.
        if store.total_codegen_jobs() >= crate::state::CODEGEN_JOB_QUOTA {
            return Err(StartCodegenJobErrorModeled::ThrottlingException( ... ))

(`StartCodegenJob` does model `ThrottlingException`; `PutMetadataFlag` models no 402 and no 429.)

Test coverage today: tests/all/metadata_flags.rs pins only the success write ("Rule 1", :158) and the unknown-feature 400 ("Rule 2", :230) — no test exercises either quota refusal, so no test pins the 402 on this operation.

## Steps to Reproduce

1. Send PUT /app/{appId}/environment/{environmentName}/metadata/features/autoGenerateForms with body {"newValue":"true"} for 1000 DISTINCT (appId, environmentName) pairs, so `metadata_flags` reaches `METADATA_ENVIRONMENT_QUOTA` (state.rs:122).
2. Send the same PUT for a 1001st, fresh (appId, environmentName).
3. Observed: HTTP 402 with a `ServiceQuotaExceededException` payload — a shape the model never declares for PutMetadataFlag (its errors are InvalidParameterException 400 and UnauthorizedException 401), so no SDK exception class matches it.
4. Compare the sibling bound: POST /app/d1234abcd/environment/staging/codegen-jobs past CODEGEN_JOB_QUOTA answers HTTP 429 ThrottlingException, because start_codegen_job.rs:187-192 states that an operation modelling no quota error must refuse with a MODELLED error instead — the rule put_metadata_flag.rs:116-121 breaks.
