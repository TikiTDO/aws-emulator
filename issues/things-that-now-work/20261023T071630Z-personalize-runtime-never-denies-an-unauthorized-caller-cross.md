Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: personalize-runtime.get-recommendations-cross-account-campaign-arn-not-found personalize-runtime.get-recommendations-missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime.get-recommendations-cross-account-campaign-arn-not-found|run#11|AssertionError|assert err_cross["Code"] == "AccessDeniedException", f"expected AccessDeniedException, got {err_cross!r}"
Issue-Key: personalize-runtime.get-recommendations-missing-iam-permission-denied|run#16|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# personalize-runtime never denies an unauthorized caller: cross-account and permission-less GetRecommendations get InvalidInputException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRecommendations

## Description
The Personalize Runtime data plane has no authorization stage at all: every request reaches campaign/recommender resolution regardless of whose account the referenced ARN names and regardless of what the signing principal is allowed to do. Two distinct causes, both reproduced on GetRecommendations.

Cause 1 of 2 — a campaignArn in a DIFFERENT account is resolved instead of denied.
AWS: a campaignArn whose account segment is not the caller's own account is refused at the authorization layer, before any resource lookup, with HTTP 403 AccessDeniedException (message example, built from the caller: "User: <caller principal ARN> is not authorized to access this resource"). The identical request against the caller's OWN account for the same, nonexistent campaign name reaches business logic and gets HTTP 400 InvalidInputException ("<arn> does not exist or is not active yet."). The two codes are never interchanged, so the error code cannot be used to probe whether a same-named campaign exists in someone else's account. Smithy: GetRecommendations models only InvalidInputException (400) and ResourceNotFoundException (404) — AccessDeniedException is the service-framework authz refusal, not a modeled operation error; recorded behavior only for the status/code pairing.
Emulator: the cross-account ARN takes exactly the same path as an own-account ARN and comes back HTTP 400 InvalidInputException with "arn:aws:personalize:us-west-2:123456789012:campaign/<name> does not exist or is not active yet." — no account check anywhere in the resolution path.
Why: the emulator is overly permissive in the one place it must not be. A client that branches on AccessDeniedException (surface an IAM/resource-policy problem) versus InvalidInputException (treat the campaign as not yet ACTIVE and retry/poll) takes the wrong branch and retries forever; and a caller can distinguish nothing-exists from not-authorized, i.e. the emulator teaches a cross-account probe that real AWS refuses.
Fix: an authorization pre-check in the runtime's ARN-resolution stage (the `resolve()` helper in `src/ops/get_recommendations.rs` and its siblings for GetPersonalizedRanking / GetActionRecommendations, or the shared lookup on `ComAmazonawsPersonalizeruntimeAmazonpersonalizeruntimeState`): compare the ARN's account segment with the request's SigV4 credential-scope account and answer 403 AccessDeniedException BEFORE the registry lookup, so the existence of the resource is never consulted.
Traces: personalize-runtime.get-recommendations-cross-account-campaign-arn-not-found.

Cause 2 of 2 — the signing principal's permissions are never evaluated.
AWS: credentials from a role whose only policy statement allows `personalize:DescribeCampaign` cannot call GetRecommendations: the call is refused with HTTP 403 AccessDeniedException before any resource lookup. The identical call with unrestricted ambient credentials reaches business logic and returns HTTP 400 InvalidInputException for the nonexistent campaign — that contrast is the proof the refusal is an authz-layer one. Recorded behavior only (the model declares no authorization error for the operation).
Emulator: STS AssumeRole hands back session credentials, the restricted client signs with them, and GetRecommendations answers HTTP 400 InvalidInputException — identical to the unrestricted call. No IAM policy is consulted for the data-plane action.
Why: a client or test that asserts least-privilege (that a role WITHOUT personalize:GetRecommendations is blocked) passes against the emulator while the same deployment would be denied by AWS; any negative-permission test built on the emulator is vacuous.
Fix: an identity-authorization stage ahead of the handler (shared middleware consulting the IAM service's role policies for the `personalize:GetRecommendations` / `personalize:GetPersonalizedRanking` / `personalize:GetActionRecommendations` action), returning 403 AccessDeniedException; the runtime handlers themselves must stay unchanged after it refuses.
Traces: personalize-runtime.get-recommendations-missing-iam-permission-denied.

## Evidence
Cause 1: expected AccessDeniedException (HTTP 403); actual {'Code': 'InvalidInputException', 'Message': 'arn:aws:personalize:us-west-2:123456789012:campaign/sim2real-personalize-camp-2c5b0698c58e does not exist or is not active yet.'} (HTTP 400). Check assertion that failed: `assert err_cross["Code"] == "AccessDeniedException"`.
Cause 2: expected AccessDeniedException, got InvalidInputException for a session whose role policy grants only personalize:DescribeCampaign. Check assertion that failed: `assert code == "AccessDeniedException"`.
In both cases the positive control (own-account ARN / ambient credentials) legitimately yields InvalidInputException, so the emulator gives the SAME answer for an authorized and an unauthorized caller.
Source: `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/ops/get_recommendations.rs` lines 183-199 and `resolve()` at lines 365-390 — resolution goes straight from the input ARNs to the registry lookup with no account or principal check; `src/state.rs` module doc states a missing registry entry is the single source of truth for the refusal.

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: call GetRecommendations with campaignArn=arn:aws:personalize:<region>:123456789012:campaign/<any-name> (an account that is not the caller's) and any userId. Observed: HTTP 400 InvalidInputException "<arn> does not exist or is not active yet." Expected: HTTP 403 AccessDeniedException. Repeat with the caller's own account id in the ARN: both answers are identical today, whereas AWS returns 403 for the first and 400 for the second.
3. Cause 2: iam.CreateRole + iam.PutRolePolicy allowing only `personalize:DescribeCampaign`, sts.AssumeRole for that role, then call GetRecommendations with those session credentials and a nonexistent own-account campaignArn. Observed: HTTP 400 InvalidInputException. Expected: HTTP 403 AccessDeniedException, while the same call with the ambient credentials still returns 400 InvalidInputException.
