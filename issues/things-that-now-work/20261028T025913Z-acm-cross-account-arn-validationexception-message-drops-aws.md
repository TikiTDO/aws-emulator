Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.create-acme-domain-validation-for-unowned-endpoint-denied
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.create-acme-domain-validation-for-unowned-endpoint-denied|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: fc7b8ecfc4b84c9c5332d5a237ecda88106519ae

# ACM cross-account ARN ValidationException message drops AWS's "Resource identifier for <resource-type> has a validation error:" envelope (1 cause)

> _Note: this issue was identified against the code base at commit `fc7b8ecfc4b84c9c5332d5a237ecda88106519ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAcmeDomainValidation

## Description
One net-new root cause in ACM's cross-account-ARN refusal wording. The error CLASS and HTTP status are now correct (a previous issue, 20261024T113112Z-acm-acme-surface-diverges-directory-url-path-prevalidationtype.md, fixed that half and is resolved); what still diverges is the message text, which is a separate defect in the shared refusal renderer rather than a reopening of that fix — AWS also refuses this call, so nothing that AWS serves is left refused.

Cause 1 of 1: the ValidationException message for an ARN input member naming another account is rendered with an emulator-authored prefix instead of ACM's member-scoped validation envelope.

AWS: Smithy CreateAcmeDomainValidation (services/service-com-amazonaws-acm-certificatemanager/smithy-model/com-amazonaws-acm-certificatemanager.json) declares AcmeEndpointArn as required, @length 1..200, @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$ — the account segment is part of the input the service validates — and models ValidationException (HTTP 400, awsQueryError code ValidationError, doc "The supplied input failed to satisfy constraints of an Amazon Web Services service."). The model does not state the message text, so: recorded behavior only. AWS answers HTTP 400 ValidationException with the message

    Resource identifier for acme-endpoint has a validation error: Same account ARN expected but got cross-account ARN

Anatomy of that message: the fixed text is "Resource identifier for " … " has a validation error: Same account ARN expected but got cross-account ARN". The only variable part is the resource-type token naming WHICH identifier member failed — here "acme-endpoint", the resource-type segment of the AcmeEndpointArn the request carried (Hypothesis: for another ARN-typed member ACM substitutes that member's own resource type; the recording only exercises acme-endpoint). No account id, ARN, domain name or request id appears in the message.

Emulator: HTTP 400 ValidationException (correct class) with the message "Invalid ARN in request: Same account ARN expected but got cross-account ARN". The second clause matches AWS exactly; the leading envelope is replaced by the emulator's own "Invalid ARN in request: " prefix, and the member/resource-type token AWS names is absent altogether. The renderer is documented in-crate as "WHOLLY FIXED text for this class … it carries nothing from the request", which is precisely the premise that is wrong: AWS's form is member-scoped.

Why: a client that classifies an ACM refusal by its message — which is all ACM gives here, since every cross-account and every malformed-identifier refusal shares one error code (ValidationException) — cannot tell WHICH identifier member of a multi-ARN request was rejected, because the emulator omits the resource-type token that carries that information. Any assertion, log scraper, retry classifier or user-facing error mapper keyed on AWS's "Resource identifier for <type> has a validation error: …" envelope (the shape ACM uses across its identifier-validation refusals) fails to match, and a test that passes on AWS fails here even though the status and code are right.

Fix: the shared cross-account-ARN refusal renderer in the ACM service crate, and its call sites, so the sentence is emitted in AWS's member-scoped envelope with the offending identifier's resource type substituted — not in each handler.

Traces: acm.create-acme-domain-validation-for-unowned-endpoint-denied.

## Evidence
Failing check acm.create-acme-domain-validation-for-unowned-endpoint-denied, verdict [mismatch]:

  File "check_acm_create_acme_domain_validation_for_unowned_endpoint_denied.py", line 57, in run
    assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
  AssertionError: expected message 'Resource identifier for acme-endpoint has a validation error: Same account ARN expected but got cross-account ARN', got 'Invalid ARN in request: Same account ARN expected but got cross-account ARN'

The check's earlier assertion `assert code == "ValidationException"` PASSED, so the divergence is confined to the message text; HTTP status 400 and the error code are correct. The check's remaining assertions (the caller's own endpoint still lists exactly its one pre-existing validation, and a validation against the owned endpoint still succeeds) were not reached.

Source of the diverging text — services/service-com-amazonaws-acm-certificatemanager/src/service.rs:193-195:

    pub(crate) fn cross_account_arn_refusal() -> String {
        "Invalid ARN in request: Same account ARN expected but got cross-account ARN".to_string()
    }

and its call site, services/service-com-amazonaws-acm-certificatemanager/src/ops/create_acme_domain_validation.rs:210-214:

    if !crate::state::arn_is_in_caller_account(&input.acme_endpoint_arn) {
        return Err(validation_exception(crate::service::cross_account_arn_refusal()));
    }

The doc comment above the helper (service.rs:183-192) asserts the sentence is "WHOLLY FIXED text for this class — it carries nothing from the request"; AWS's recorded form does carry one request-derived token (the identifier member's resource type, "acme-endpoint").

Smithy citations: CreateAcmeDomainValidation.AcmeEndpointArn -> AcmeEndpointArn, required, @length min 1 max 200, @pattern ^arn:aws[a-z-]*:acm:[a-z0-9-]+:[0-9]{12}:acme-endpoint/[a-zA-Z0-9-]+$; errors AccessDeniedException(403) / ConflictException(400) / InternalServerException(500) / ResourceNotFoundException(400) / ServiceQuotaExceededException(400) / ThrottlingException(400) / ValidationException(400, awsQueryError ValidationError).

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. acm.create_acme_endpoint(AuthorizationBehavior='PRE_APPROVED', CertificateAuthority={'PublicCertificateAuthority': {}}, Contact='NOT_REQUIRED', Tags=[...]) and poll describe_acme_endpoint until Status is ACTIVE.
3. Take the returned AcmeEndpointArn and replace its account segment (field 5) with an account id that is not the caller's (e.g. 111122223333), leaving every other segment intact so the modeled @pattern still matches.
4. Call acm.create_acme_domain_validation(AcmeEndpointArn=<that cross-account ARN>, DomainName=<a fresh lower-case domain>, PrevalidationOptions={'DnsPrevalidation': {}}).
5. Observe HTTP 400 ValidationException (correct) whose Error.Message is 'Invalid ARN in request: Same account ARN expected but got cross-account ARN'. AWS answers the same status and code with 'Resource identifier for acme-endpoint has a validation error: Same account ARN expected but got cross-account ARN'.
6. Re-replay acm.create-acme-domain-validation-for-unowned-endpoint-denied to confirm the fix; the check also asserts the refusal leaks nothing into the caller's own endpoint and that a validation on the owned endpoint still succeeds.
