Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.get-acme-external-account-binding-credentials-cross-account-denied
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.get-acme-external-account-binding-credentials-cross-account-denied|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 03bbe43a8a5ebde1fc83c18c0454ae56c6488a2e

# ACM cross-account ARN ValidationException uses one envelope service-wide: GetAcmeExternalAccountBindingCredentials answers "Resource identifier for acme-external-account-binding has a validation error: …" where AWS answers "Invalid ARN in request: …" (1 cause)

> _Note: this issue was identified against the code base at commit `03bbe43a8a5ebde1fc83c18c0454ae56c6488a2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAcmeExternalAccountBindingCredentials

## Description
One net-new root cause: ACM renders EVERY cross-account-ARN ValidationException through a single member-scoped envelope, but AWS's envelope for this verdict is not uniform across ACM's ACME operations. GetAcmeExternalAccountBindingCredentials gets the wrong sentence.

Cause 1 of 1: the shared cross-account-ARN refusal renderer emits the "Resource identifier for <resource-noun> has a validation error: " envelope unconditionally, so GetAcmeExternalAccountBindingCredentials words its cross-account refusal with that envelope (and the resource noun "acme-external-account-binding" taken from the ARN) where AWS words it "Invalid ARN in request: ". The error class (ValidationException) and HTTP status (400) are correct; only the message's leading envelope diverges.
Failing check ids for this cause: acm.get-acme-external-account-binding-credentials-cross-account-denied.

AWS: Smithy (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) declares GetAcmeExternalAccountBindingCredentials.AcmeExternalAccountBindingArn -> AcmeExternalAccountBindingArn, required, @length min 1 max 200, @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$ — the 12-digit account segment is part of the input the service validates — and models ValidationException (client error, httpError 400, awsQueryError code ValidationError, doc "The supplied input failed to satisfy constraints of an Amazon Web Services service.") alongside AccessDeniedException(403), ResourceNotFoundException(400), ThrottlingException(400), InternalServerException(500). The model does not state message text, so for the text: recorded behavior only. For this operation AWS answers HTTP 400, Error.Code ValidationException, Error.Message

    Invalid ARN in request: Same account ARN expected but got cross-account ARN

Anatomy of that message: it is WHOLLY fixed text for this refusal class — "Invalid ARN in request: Same account ARN expected but got cross-account ARN". No part of it is taken from the request or from state: no account id, no ARN, no resource type, no binding/endpoint id, nothing per-run (request id, clock time). Note AWS validates the ARN's account BEFORE consulting the store, so the sentence is the same whether or not a record with that id exists, and it never discloses existence. Hypothesis: ACM's envelope for this verdict is chosen per operation/per ARN member — CreateAcmeDomainValidation's recorded cross-account refusal uses the member-scoped "Resource identifier for acme-endpoint has a validation error: Same account ARN expected but got cross-account ARN" form (see the resolved 20261028T025913Z-acm-cross-account-arn-validationexception-message-drops-aws.md), while this operation's recorded refusal uses the fixed "Invalid ARN in request: " form; the recordings only exercise these two operations, so the requirement stated here is per-operation wording, not one envelope for the whole service.

Emulator: HTTP 400 with Error.Code ValidationException (correct class and status) and Error.Message "Resource identifier for acme-external-account-binding has a validation error: Same account ARN expected but got cross-account ARN" — the trailing verdict clause "Same account ARN expected but got cross-account ARN" matches AWS exactly, but the leading envelope is the member-scoped one and it additionally injects a request-derived resource noun that AWS's sentence for this operation does not contain. The positive controls around the refusal pass: the real binding's KeyId/MacKey are served before and are unchanged after the rejected cross-account attempt, so only the wording diverges.

Why: ACM collapses every cross-account and malformed-identifier refusal onto the one error code ValidationException, so the message is the only discriminator a client has. A caller, error mapper, log scraper or test keyed on AWS's exact sentence for this operation does not match, so code that passes against AWS fails here even though the status and code are right; worse, the emulator's sentence advertises the rejected identifier's resource type, which AWS's sentence for this operation does not reveal, so a client cannot rely on the emulator to show what AWS discloses in a credential-bearing read's refusal.

Fix: the shared cross-account-ARN refusal renderer in the ACM service crate and the way its call sites select an envelope — the renderer currently hard-codes ONE envelope for all callers, so it cannot serve two recorded wordings; it needs the envelope to be a property of the refusing operation/member (the ACME-EAB credential read's fixed "Invalid ARN in request: " sentence vs the identifier-scoped envelope CreateAcmeDomainValidation records), not of the service. Not a change to the handler's control flow: the account check already fires before the store is read and already raises ValidationException, which is correct.

Traces: acm.get-acme-external-account-binding-credentials-cross-account-denied.

Related already-open/resolved issues, neither of which carries this check: the resolved 20261028T025913Z-acm-cross-account-arn-validationexception-message-drops-aws.md introduced the member-scoped envelope for CreateAcmeDomainValidation (that call is still correctly refused, so this is not a reopening of it — its own check's wording is the one the renderer now satisfies); the resolved 20261027T102721Z-acm-acme-external-account-binding-credential-surface-diverges.md fixed this operation's cross-account error CLASS (it used to answer ResourceNotFoundException) and the class is now correct; the open 20261105T044227Z-acm-constraint-violation-validationexception-messages-omit-the-rejected.md concerns the separate modeled-constraint clause template ("N validation errors detected: …") and not this envelope.

## Evidence
Failing check acm.get-acme-external-account-binding-credentials-cross-account-denied, verdict [mismatch]:

  File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
  File "check_acm_get_acme_external_account_binding_credentials_cross_account_denied.py", line 49, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message 'Invalid ARN in request: Same account ARN expected but got cross-account ARN', got 'Resource identifier for acme-external-account-binding has a validation error: Same account ARN expected but got cross-account ARN'

The preceding assertion `assert code == "ValidationException"` PASSED, so HTTP 400 and the error code are correct and the divergence is confined to the message's leading envelope. The assertions before the refusal also passed: GetAcmeExternalAccountBindingCredentials on the real binding returned a non-empty KeyId and MacKey. The assertions after the refusal (KeyId/MacKey unchanged after the rejected cross-account attempt) were not reached.

Message anatomy, per AWS's recorded sentence for this operation: fixed text = the entire string "Invalid ARN in request: Same account ARN expected but got cross-account ARN"; variable parts = none (no account id, ARN, resource type, id, timestamp or request id). The emulator's string keeps the final clause verbatim but replaces the leading fixed text with "Resource identifier for " + <resource noun derived from the request's ARN> + " has a validation error: ".

Source of the diverging text — services/service-com-amazonaws-acm-certificatemanager/src/service.rs:229-235:

    pub(crate) fn cross_account_arn_refusal(arn: &str) -> String {
        format!(
            "Resource identifier for {} has a validation error: Same account ARN expected but got \
             cross-account ARN",
            crate::state::acm_arn_resource_noun(arn)
        )
    }

Its doc comment (service.rs:213-228) states the sentence is "Rendered here once, so every operation that holds an ARN member to the caller's account answers the same sentence about its own member" — that single-envelope premise is what diverges, since the recordings show two different envelopes for two different ACME operations.

Call site — services/service-com-amazonaws-acm-certificatemanager/src/ops/get_acme_external_account_binding_credentials.rs:176-187:

    if !crate::state::arn_is_in_caller_account(&input.acme_external_account_binding_arn) {
        return Err(... ValidationException { message: Some(crate::service::cross_account_arn_refusal(&input.acme_external_account_binding_arn)) } ...);
    }

The handler comment at lines 169-175 asserts the verdict is reported "in the one sentence this service words that verdict with — fixed text carrying nothing from the request — as every sibling ACME operation reports it", while the renderer it calls now injects a request-derived resource noun and the other envelope.

Smithy citations: GetAcmeExternalAccountBindingCredentials.AcmeExternalAccountBindingArn -> AcmeExternalAccountBindingArn, required, @length 1..200, @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+/acme-external-account-binding/[a-zA-Z0-9-]+$; output KeyId (String), MacKey (sensitive); errors AccessDeniedException(403) / InternalServerException(500) / ResourceNotFoundException(400) / ThrottlingException(400) / ValidationException(400, awsQueryError ValidationError).

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Create an ACME endpoint and wait for ACTIVE: acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', CertificateAuthority={'PublicCertificateAuthority': {}}, Tags=[...]), then poll acm.describe_acme_endpoint(AcmeEndpointArn=<arn>) until Status == 'ACTIVE'.
3. Create an IAM role trusted by acm-acme.amazonaws.com, and a binder principal holding iam:PassRole on it plus acm:CreateAcmeExternalAccountBinding; with that principal call acm.create_acme_external_account_binding(AcmeEndpointArn=<endpoint arn>, RoleArn=<role arn>) and keep the returned AcmeExternalAccountBindingArn.
4. Positive control: acm.get_acme_external_account_binding_credentials(AcmeExternalAccountBindingArn=<that arn>) returns a non-empty KeyId and MacKey.
5. Replace field 5 (the 12-digit account segment) of that binding ARN with an account id that is not the caller's (e.g. 111122223333), leaving every other segment intact so the modeled @pattern still matches, and call acm.get_acme_external_account_binding_credentials(AcmeExternalAccountBindingArn=<that cross-account arn>).
6. Observe HTTP 400 / Error.Code ValidationException (both correct) with Error.Message 'Resource identifier for acme-external-account-binding has a validation error: Same account ARN expected but got cross-account ARN'. AWS answers the same status and code with the fixed sentence 'Invalid ARN in request: Same account ARN expected but got cross-account ARN'.
7. Re-replay acm.get-acme-external-account-binding-credentials-cross-account-denied to confirm the fix; the check continues past the message assertion to verify the real binding's KeyId and MacKey are unchanged after the rejected cross-account attempt. Also re-replay acm.create-acme-domain-validation-for-unowned-endpoint-denied to confirm CreateAcmeDomainValidation still answers its own recorded envelope ('Resource identifier for acme-endpoint has a validation error: …').
