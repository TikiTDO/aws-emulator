Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.update-provisioned-limit-exceeds-quota-rejected cognito-idp.update-provisioned-limit-excessive-value-rejected cognito-idp.update-get-provisioned-limit
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.update-provisioned-limit-exceeds-quota-rejected|_error#15|AssertionError|raise AssertionError(f"expected UpdateProvisionedLimit to be rejected, but it succeeded: {result!r}")
Issue-Key: cognito-idp.update-provisioned-limit-excessive-value-rejected|_error#13|AssertionError|raise AssertionError(
Issue-Key: cognito-idp.update-get-provisioned-limit|run#14|AssertionError|assert down_message == expected_down_message, (
Issue-Commit: 101c48ba65dcbcc03ccf2390a824ac8e29c4e93e

# cognito-idp: 2 net-new causes in UpdateProvisionedLimit — a RequestedLimitValue far above the ceiling is answered 200 with the unchanged value instead of ServiceQuotaExceededException, and the below-free-limit refusal message is emulator-authored

> _Note: this issue was identified against the code base at commit `101c48ba65dcbcc03ccf2390a824ac8e29c4e93e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateProvisionedLimit

## Description
Two net-new root causes in cognito-idp UpdateProvisionedLimit, both reached through the API_CATEGORY/UserCreation limit (FreeLimitValue 50 RPS). Neither is carried by the open issues that already own the other provisioned-limit checks: 20261029T182116Z-cognito-idp-provisioned-limit-api-diverges-no-default.md (its Cause 4 describes an over-quota refusal that DOES happen but with the wrong wording — here the request is not refused at all) and 20261103T200951Z-cognito-idp-2-net-new-causes-getprovisionedlimit-refuses.md (unknown-category refusal and the replica lifecycle). A resolved issue covers neither.

Cause 1 of 2 — a RequestedLimitValue far above the account's ceiling is accepted.
AWS: UpdateProvisionedLimit's value "must be between the default limit and your account-level maximum limit in Service Quotas" (service model doc). A value above that ceiling is refused with the modeled ServiceQuotaExceededException (Smithy: UpdateProvisionedLimit errors InvalidParameterException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 400, NotAuthorizedException 403, TooManyRequestsException 429; ServiceQuotaExceededException doc: "The request exceeded your account's service quota. To increase your limit, use or submit a Service Quotas increase request."), HTTP 400, message 'Requested value <RequestedLimitValue> exceeds Service Quota ceiling of <ceiling>' — fixed text, with the requested value taken from the request and the ceiling from the account's Service Quotas state. The account's provisioned value must be left unchanged.
Emulator: UpdateProvisionedLimit(LimitClass=API_CATEGORY, Attributes={Category: UserCreation}, RequestedLimitValue=2000000000) answers HTTP 200 with Limit {ProvisionedLimitValue: 50, FreeLimitValue: 50} — no error at all, and the requested value is neither stored nor refused. The over-ceiling gate IS reached for a small over-ceiling value: in the same suite, RequestedLimitValue = current+1 (51) is correctly refused with ServiceQuotaExceededException and the exact AWS sentence, so only the large value escapes the range check. Hypothesis: the request's RequestedLimitValue does not arrive at the range comparison for a large integer (state.rs's set_api_category_limit does carry an AboveQuotaCeiling branch, and UserCreation's default ceiling equals its free rate of 50, so 2000000000 > ceiling should fire), and the response renders the pre-existing stored value.
Why: a client that relies on AWS refusing an unprovisionable rate sees a 200 and believes 2,000,000,000 RPS was provisioned, while the account still enforces 50; quota-handling and retry/alarm paths that branch on ServiceQuotaExceededException are never exercised, and a caller cannot distinguish "accepted" from "silently ignored" because the echoed Limit is the old value.
Fix: the requested-rate range check behind the UpdateProvisionedLimit handler (the AboveQuotaCeiling branch of the account's provisioned-rate state, reached from the handler's set_api_category_limit call) must see the request's RequestedLimitValue for every modeled integer value and refuse above the ceiling.
Traces: cognito-idp.update-provisioned-limit-exceeds-quota-rejected, cognito-idp.update-provisioned-limit-excessive-value-rejected.

Cause 2 of 2 — the below-free-limit refusal message is emulator-authored prose.
AWS: a RequestedLimitValue below the limit's FreeLimitValue (the rate included at no additional cost, LimitType.FreeLimitValue) is refused with InvalidParameterException (HTTP 400) and the message 'Requested value <RequestedLimitValue> is below the free limit of <FreeLimitValue>' — fixed text, with the requested value from the request and the free limit from the limit the account holds. The error class the emulator uses is already correct; only the sentence diverges.
Emulator: the same request is refused with InvalidParameterException but the message 'The requested limit value 49 is below the default limit of 50 requests per second for this limit.' — different fixed text ('The requested limit value ... is below the default limit of ... requests per second for this limit.' instead of 'Requested value ... is below the free limit of ...'), plus a trailing period AWS does not write.
Why: a client or test that matches AWS's sentence (or parses the bound out of it) does not recognise the refusal, and operators reading emulator output see wording that never appears in AWS.
Fix: the BelowFreeRate refusal message in the provisioned-limit state that UpdateProvisionedLimit renders (the InvalidParameterException message built for a below-free rate).
Traces: cognito-idp.update-get-provisioned-limit.

## Evidence
Verbatim from the replay.

Cause 1 — cognito-idp.update-provisioned-limit-exceeds-quota-rejected:
  AssertionError: expected UpdateProvisionedLimit to be rejected, but it succeeded: {'Limit': {'LimitDefinition': {'LimitClass': 'API_CATEGORY', 'Attributes': {'Category': 'UserCreation'}}, 'ProvisionedLimitValue': 50, 'FreeLimitValue': 50}, 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}
  (the check asserted code == 'ServiceQuotaExceededException' and message.startswith('Requested value 2000000000 exceeds Service Quota ceiling of '))

Cause 1 — cognito-idp.update-provisioned-limit-excessive-value-rejected:
  AssertionError: expected this call to be rejected, but it SUCCEEDED and returned {'Limit': {'LimitDefinition': {'LimitClass': 'API_CATEGORY', 'Attributes': {'Category': 'UserCreation'}}, 'ProvisionedLimitValue': 50, 'FreeLimitValue': 50}, 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}

Expected vs actual (Cause 1): expected HTTP 400 ServiceQuotaExceededException, message 'Requested value 2000000000 exceeds Service Quota ceiling of <ceiling>'; actual HTTP 200 with the unchanged Limit (ProvisionedLimitValue == FreeLimitValue == the pre-existing value). The same suite shows the ceiling+1 case refused correctly, so the gate exists but is not reached for the large value.

Cause 2 — cognito-idp.update-get-provisioned-limit:
  AssertionError: expected the exact below-floor message 'Requested value 49 is below the free limit of 50', got 'The requested limit value 49 is below the default limit of 50 requests per second for this limit.'
Expected vs actual (Cause 2): same code (InvalidParameterException) and status (400); the fixed text differs — 'Requested value {RequestedLimitValue} is below the free limit of {FreeLimitValue}' (AWS) vs 'The requested limit value {RequestedLimitValue} is below the default limit of {FreeLimitValue} requests per second for this limit.' (emulator). 49/50 here are the request's value and the limit's FreeLimitValue, not part of the template.

## Steps to Reproduce
1. cognito-idp GetProvisionedLimit with LimitDefinition={'LimitClass': 'API_CATEGORY', 'Attributes': {'Category': 'UserCreation'}} and note ProvisionedLimitValue (P) and FreeLimitValue (F).
2. Cause 1: UpdateProvisionedLimit with the same LimitDefinition and RequestedLimitValue=2000000000. Expect HTTP 400 ServiceQuotaExceededException with message 'Requested value 2000000000 exceeds Service Quota ceiling of <ceiling>'. Observed: HTTP 200 with Limit.ProvisionedLimitValue == P (unchanged). Then GetProvisionedLimit again — the value is still P, so the write was neither applied nor refused. For contrast, UpdateProvisionedLimit with RequestedLimitValue=P+1 is refused correctly with ServiceQuotaExceededException and AWS's sentence.
3. Cause 2: UpdateProvisionedLimit with the same LimitDefinition and RequestedLimitValue=F-1. Expect HTTP 400 InvalidParameterException with message 'Requested value <F-1> is below the free limit of <F>'. Observed the same code with 'The requested limit value <F-1> is below the default limit of <F> requests per second for this limit.'
