Issue-Source: trace-sweep-divergence
Issue-Trace: account-region-transition-conflict-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: account-region-transition-conflict-errors
Issue-Shape-Id: com.amazonaws.account#Account
Issue-Key: account|EnableRegion|4xx->4xx|$.fieldList[*].message
Issue-Key: account|DisableRegion|4xx->4xx|$.fieldList[*].message
Issue-Related: 20261029T232208Z-account-validationexception-refusal-payloads-diverge-from-aws-wrong.md
Issue-Commit: 7ed1ec67b82a7e181ea8080af214a2818436a6ca

# account enabled-by-default Region opt refusal diverges: wrong message, wrong reason, missing fieldList (2 causes)

> _Note: this issue was identified against the code base at commit `7ed1ec67b82a7e181ea8080af214a2818436a6ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableRegion

## Description
Two net-new wire divergences in the AWS Account Management service (sdkId `account`), both in the BODY of the 400 `ValidationException` that an opt-in/opt-out call earns when its `RegionName` names a Region the partition turns on by DEFAULT. The emulator picks the correct HTTP status (400) and the correct error shape (`com.amazonaws.account#ValidationException`, @error client, httpError 400); three members of the payload the model declares for that shape diverge — `message` (required, @sensitive), `reason` (`ValidationExceptionReason`: `invalidRegionOptTarget | fieldValidationFailed`) and `fieldList` (`ValidationExceptionFieldList`, documented "The field where the invalid entry was detected"). The two causes are the SAME refusal constructor reached from two different operation handlers, and each is a separately replayed step, so BOTH must be re-replayed and closed before this issue is submitted.

=== Cause 1 of 2: EnableRegion on an enabled-by-default Region (fixture account-region-transition-conflict-errors, step task#1, EnableRegion) ===
AWS: Smithy (services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json): `EnableRegion` POST /enableRegion, input member `RegionName` @required @length(min 1, max 50); modelled errors `ValidationException` (client, 400), `ConflictException` (409), `AccessDeniedException` (403), `TooManyRequestsException` (429), `InternalServerException` (500). `ValidationExceptionReason` has exactly two values, `invalidRegionOptTarget` and `fieldValidationFailed`; `ValidationExceptionField` is {`name` required, `message` required}. A Region that is enabled by default in the partition (us-east-1 in the commercial partition) is not an opt target, and the recorded service refuses it as a FIELD VALIDATION failure on that member: 400 `ValidationException` with `reason` = `fieldValidationFailed`, a `message` of the fixed form `<RegionName> is not a valid region for opt-in or opt-out.` — fixed text ` is not a valid region for opt-in or opt-out.` including the trailing period, variable part the Region code taken verbatim from the request's `RegionName` — and a `fieldList` of ONE entry whose `name` is the offending member (`RegionName`) and whose `message` is that same sentence. Recorded behavior only for the exact wording (the model contributes the error shape, the reason enum and `fieldList`'s meaning); the recorded response for `RegionName=us-east-1` is message `us-east-1 is not a valid region for opt-in or opt-out.`, reason `fieldValidationFailed`, fieldList `[{"name": "RegionName", "message": "us-east-1 is not a valid region for opt-in or opt-out."}]` (that Region code is the request's value filling the template, not part of it).
Emulator: the same request is refused 400 `ValidationException` whose `message` is `Region <RegionName> is enabled by default and cannot be enabled or disabled` (recorded instance: `Region us-east-1 is enabled by default and cannot be enabled or disabled`), whose `reason` is `invalidRegionOptTarget`, and whose `fieldList` is ABSENT — three diverging members in one body.
Why: `reason` is the enum a client branches on, and the emulator reports the value AWS reserves for a different condition than the one it signals here, so a caller that distinguishes "the Region value itself failed field validation" from "this Region is not a legal opt target" classifies the refusal the wrong way round. `fieldList` is how a caller learns WHICH input member was rejected and it is missing entirely, so per-field error surfacing (forms, CLIs, SDK-level field mapping) has nothing to bind to. And the sentence a CLI or log line surfaces is one AWS never emits, so message-matching tests and user-facing text both diverge. Code written against AWS passes there and fails here.
Fix: the refusal constructor in this crate's state module — `region_not_opt_target` in services/service-com-amazonaws-account-account/src/state.rs, the payload selected for `RegionKind::EnabledByDefault` by `crate::state::region_opt_target` (state.rs:586-592, the single statement of the rule both opt operations decide through). It must emit AWS's sentence with the request's `RegionName` interpolated, set `reason` to `fieldValidationFailed`, and populate `fieldList` with one entry naming `RegionName` and carrying the same sentence — exactly the shape the neighbouring field-validation constructor already builds (state.rs:1630-1638). No serializer or state change is needed: the handler refuses before the store is touched.
Traces: account-region-transition-conflict-errors#task#1.

=== Cause 2 of 2: DisableRegion carries the identical wrong payload for the same Region class (fixture account-region-transition-conflict-errors, step task#2, DisableRegion) ===
AWS: Smithy: `DisableRegion` POST /disableRegion, `RegionName` @required @length(min 1, max 50); same modelled error set including `ValidationException` (400) and `ConflictException` (409). The opt-OUT path refuses an enabled-by-default Region exactly as the opt-in path does: 400 `ValidationException`, `reason` = `fieldValidationFailed`, message `<RegionName> is not a valid region for opt-in or opt-out.`, `fieldList` = one entry with `name` `RegionName` and that sentence. Recorded behavior only for the wording; the recorded body for `RegionName=us-east-1` is identical to cause 1's.
Emulator: 400 `ValidationException` with message `Region <RegionName> is enabled by default and cannot be enabled or disabled`, `reason` = `invalidRegionOptTarget`, `fieldList` absent — the same three diverging members.
Why: same client impact as cause 1, on the opt-OUT surface. A caller that handles both directions of the Region opt lifecycle sees the wrong reason enum and no field list on either, so neither direction can be classified or surfaced the way AWS documents.
Fix: same component as cause 1 (`region_not_opt_target`, reached from src/ops/disable_region.rs via `crate::state::region_opt_target`). Listed as its own cause because it is a distinct replayed step: the fix must be confirmed to reach the DisableRegion handler too, not only EnableRegion.
Traces: account-region-transition-conflict-errors#task#2.

Relation to already-open issues: 20261029T232208Z-account-validationexception-refusal-payloads-diverge-from-aws-wrong.md describes the same two region causes in prose, but its replay pointer binds ONLY `account-contact-information-validation-errors#task#1` (its `Issue-Trace`/`Issue-Step`), and its `Issue-Trace-Ids` lists bare fixture ids with no step halves, which is why this replay still reports both region steps as unowned (`owned_by` empty) while the PutContactInformation step is owned. These two causes therefore need a pointer that actually re-replays them; that is what this issue carries. 20261029T151445Z-account-enableregion-error-response.md reports only the unset `fieldList` and says nothing about the message or the `reason` value, and 20261029T185854Z-account-validationexception-messages-diverge-from-aws-in-three.md reports the region sentence from the BEHAVIORAL corpus with no wire replay key and nothing about the `reason` value or the DisableRegion path. Closing any of those leaves both steps below failing.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator (/__admin/health 200, data listener accepting): 7 fixtures, 34 steps, 31 passed, 3 mismatched, 0 errored, 0 blocked, score 91.2%. bootstrap_signal false. All 3 mismatches are service `account`. One of them (PutContactInformation, account-contact-information-validation-errors#task#1) is reported owned_by an open issue and is NOT filed here; the two below are reported with stub_path empty and owned_by EMPTY, i.e. no open issue carries their replay key.

Cause 1 — fixture `account-region-transition-conflict-errors`, step `task#1`, EnableRegion
  divergence_key: service=account|EnableRegion|status=400->400|keys=$.fieldList[0].message,$.message,$.reason
  verdict: mismatch; expected_status 400, actual_status 400 (status and error shape correct, body wrong)
  $.message:            expected 'us-east-1 is not a valid region for opt-in or opt-out.'
                        actual   'Region us-east-1 is enabled by default and cannot be enabled or disabled'
  $.reason:             expected 'fieldValidationFailed'
                        actual   'invalidRegionOptTarget'
  $.fieldList[0].message: expected 'us-east-1 is not a valid region for opt-in or opt-out.'
                        actual   'Region us-east-1 is enabled by default and cannot be enabled or disabled'
  (the expected fieldList entry is {name: 'RegionName', message: <that sentence>}; 'us-east-1' is the request's RegionName filling the template)

Cause 2 — fixture `account-region-transition-conflict-errors`, step `task#2`, DisableRegion
  divergence_key: service=account|DisableRegion|status=400->400|keys=$.fieldList[0].message,$.message,$.reason
  Identical three-member divergence to cause 1 (same expected body, same actual body), produced by the same helper reached from the DisableRegion handler.

Emulator source of the diverging body:
  services/service-com-amazonaws-account-account/src/state.rs:586-592 — `region_opt_target`, the one decision both opt operations make, selects `region_not_opt_target(region_name)` for `Some(RegionKind::EnabledByDefault)`.
  services/service-com-amazonaws-account-account/src/state.rs — `region_not_opt_target` builds the "Region <name> is enabled by default and cannot be enabled or disabled" message with reason `invalidRegionOptTarget` and `field_list: None` (the file has since drifted from the line numbers an earlier pass cited; locate it by name).
  services/service-com-amazonaws-account-account/src/state.rs:1630-1638 — the neighbouring field-validation constructor, which already emits `reason: Some(FIELD_VALIDATION_FAILED)` plus a one-entry `field_list` with `name`=member and the same sentence: the shape this refusal must adopt.

Model facts used (smithy_query on services/service-com-amazonaws-account-account/smithy-model/com-amazonaws-account-account.json):
  EnableRegion: POST /enableRegion, success 200, input {AccountId (pattern ^\d{12}$), RegionName required @length(1,50)}, errors AccessDenied(403), Conflict(409), InternalServer(500, retryable), TooManyRequests(429, throttling), Validation(400).
  DisableRegion: POST /disableRegion, same input/error set.
  ValidationException: client error, httpError 400; members `message` (required, @sensitive), `reason` (ValidationExceptionReason), `fieldList` (list of ValidationExceptionField) — "The field where the invalid entry was detected."
  ValidationExceptionReason values: ['invalidRegionOptTarget', 'fieldValidationFailed'].
  ValidationExceptionField: {name required, message required @sensitive}.

## Steps to Reproduce
Boot the emulator with the account service mounted and replay fixture `account-region-transition-conflict-errors` (steps task#1 and task#2), or issue the equivalent requests against the data listener with an `account` client for the commercial partition.

Cause 1 — step task#1 (EnableRegion):
1. Confirm the precondition: /getRegionOptStatus with RegionName='us-east-1' reports an ENABLED_BY_DEFAULT Region (any Region the catalog turns on by default reproduces this).
2. POST /enableRegion with {"RegionName": "us-east-1"}.
3. Observe 400 ValidationException with message `Region us-east-1 is enabled by default and cannot be enabled or disabled`, reason `invalidRegionOptTarget` and NO fieldList. AWS answers 400 ValidationException with message `us-east-1 is not a valid region for opt-in or opt-out.`, reason `fieldValidationFailed`, and fieldList [{"name": "RegionName", "message": "us-east-1 is not a valid region for opt-in or opt-out."}].

Cause 2 — step task#2 (DisableRegion):
4. POST /disableRegion with {"RegionName": "us-east-1"} — the identical wrong body, because both handlers decide through crate::state::region_opt_target.

Controls that must keep passing after the fix:
5. /enableRegion with a genuine opt-in Region (e.g. af-south-1) still succeeds (200, empty body).
6. /enableRegion with a Region the catalog does not hold (e.g. zz-nowhere-1) still answers 400 ValidationException with reason `fieldValidationFailed` and a fieldList naming RegionName.

Replay to confirm the fix: account-region-transition-conflict-errors#task#1 and account-region-transition-conflict-errors#task#2.
