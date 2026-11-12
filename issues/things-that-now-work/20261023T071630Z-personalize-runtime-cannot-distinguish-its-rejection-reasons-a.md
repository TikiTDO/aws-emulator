Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-runtime.get-recommendations-wrong-resource-type-arn-not-found personalize-runtime.get-action-recommendations-wrong-resource-type-arn-not-found personalize-runtime.get-recommendations-missing-campaign-and-recommender-rejected
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime.get-recommendations-wrong-resource-type-arn-not-found|run#12|AssertionError|assert err["Message"] == f"The given campaign ARN is invalid: {schema_arn}", (
Issue-Key: personalize-runtime.get-action-recommendations-wrong-resource-type-arn-not-found|run#12|AssertionError|assert err["Message"] == f"The given campaign ARN is invalid: {schema_arn}", (
Issue-Key: personalize-runtime.get-recommendations-missing-campaign-and-recommender-rejected|run#6|AssertionError|assert msg == "campaignArn or recommenderArn required.", f"expected the required-field message, got {msg!r}"
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# personalize-runtime cannot distinguish its rejection reasons: a wrong-resource-type ARN and a request naming no target both report the wrong InvalidInputException message

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
InvalidInputException (400) is the single code personalize-runtime uses for "no target named", "target named but does not exist" and "target ARN is not a campaign ARN", so the message is the only observable that separates them. The emulator collapses two of those three reasons onto the wrong message. Two causes, both in the same ARN-resolution stage.

Cause 1 of 2 — a well-formed ARN of the wrong RESOURCE TYPE is reported as not-found instead of invalid.
AWS: passing a genuine, ACTIVE Personalize *schema* ARN as `campaignArn` is refused with InvalidInputException (HTTP 400) and the message "The given campaign ARN is invalid: <arn>" — the rule is request-derived: the ARN's resource-type segment is not `campaign/...`, so it is not a campaign reference at all and existence is never consulted. That is distinct from a well-formed, nonexistent CAMPAIGN ARN, which gets the same code with the message "<arn> does not exist or is not active yet." (the check's own positive control confirms that second message). Smithy: `campaignArn` targets `Arn` (length 0..256, pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$) — the pattern admits any personalize resource type, so the type check is the service's, not the model's; errors InvalidInputException (400) / ResourceNotFoundException (404).
Emulator: a schema ARN passed as `campaignArn` is looked up straight in the campaign registry and misses, producing "arn:aws:personalize:us-west-2:000000000000:schema/<name> does not exist or is not active yet." — the resource-type segment is never examined, on both GetRecommendations and GetActionRecommendations.
Why: a client that mixes up two ARNs it holds (a very common error, e.g. passing a schema or dataset-group ARN) is told the campaign is "not active yet", which is a retryable condition; it polls and retries instead of surfacing a malformed-reference bug. The emulator also implies a campaign of that ARN could later appear.
Fix: an ARN resource-type check in the runtime's resolution stage — `resolve()` in `src/ops/get_recommendations.rs` and the equivalent in `get_action_recommendations.rs` / `get_personalized_ranking.rs`, or the shared `campaign()` lookup on the registry — rejecting a `campaignArn` whose resource segment is not `campaign` with the invalid-ARN message before the registry lookup.
Traces: personalize-runtime.get-recommendations-wrong-resource-type-arn-not-found, personalize-runtime.get-action-recommendations-wrong-resource-type-arn-not-found.

Cause 2 of 2 — the "named neither target" refusal uses a different message text.
AWS: GetRecommendations with neither `campaignArn` nor `recommenderArn` (neither is @required in the model, since either resource type may back the operation) is refused with InvalidInputException (HTTP 400) and the message "campaignArn or recommenderArn required." Recorded behavior only for the exact text; the rule — this condition yields its own message, distinct from the ARN-existence message returned once a target IS named — is what the recording establishes, and the message is the only observable that separates the two, since the code is identical.
Emulator: same code and status, but the message is "Either campaignArn or recommenderArn is required."
Why: a client (or an error-classifying wrapper) that discriminates "you named no target" from "your target does not exist" — the only way to tell them apart, the code being shared — matches neither branch and misclassifies the rejection.
Fix: the no-target arm of `resolve()` in `src/ops/get_recommendations.rs` (the `(None, None)` match arm, around line 374).
Traces: personalize-runtime.get-recommendations-missing-campaign-and-recommender-rejected.

## Evidence
Cause 1, GetRecommendations: expected InvalidInputException with Message "The given campaign ARN is invalid: arn:aws:personalize:us-west-2:000000000000:schema/sim2real-personalize-schema-e74f4224f8d6"; actual Message "arn:aws:personalize:us-west-2:000000000000:schema/sim2real-personalize-schema-e74f4224f8d6 does not exist or is not active yet." (code InvalidInputException, matching). Same divergence on GetActionRecommendations with its own schema ARN. The ARNs are real: the check creates the schema and asserts DescribeSchema echoes it back before the runtime call.
Cause 2: expected Message "campaignArn or recommenderArn required."; actual "Either campaignArn or recommenderArn is required." (code InvalidInputException, matching).
Source: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs` lines 365-390 (`resolve()`: the `(None, None)` arm's literal, and the `(Some(arn), None)` arm going straight to `state.campaign(arn)`); `src/state.rs` lines 16-25 document the single not-found message used for every unresolvable ARN.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: personalize.CreateSchema, take the returned `schemaArn`, then call personalize-runtime.GetRecommendations (and GetActionRecommendations) with campaignArn=<that schemaArn> and any userId. Observed: 400 InvalidInputException "<schemaArn> does not exist or is not active yet." Expected: 400 InvalidInputException "The given campaign ARN is invalid: <schemaArn>". Then repeat with a fabricated own-account `campaign/<name>` ARN — AWS returns the "does not exist or is not active yet." message there, so the two cases must not share a message.
3. Cause 2: call GetRecommendations with only userId (no campaignArn, no recommenderArn). Observed: 400 InvalidInputException "Either campaignArn or recommenderArn is required." Expected: 400 InvalidInputException "campaignArn or recommenderArn required."
