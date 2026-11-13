Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-app-client-error-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-app-client-error-paths cognito-idp-client-secret-error-paths cognito-idp-user-pool-client-secret-errors cognito-idp-user-pool-client-secret-management
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|DeleteUserPoolClientSecret|4xx->4xx|$.message
Issue-Key: cognito-idp|DeleteUserPoolClientSecret|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DeleteUserPoolClientSecret|4xx->4xx|status
Issue-Commit: 2019710a3c4752758402c7d5e42a18d73361dfac

# cognito-idp DeleteUserPoolClientSecret not-found refusal: wrong HTTP status, invented message text and missing x-amzn-errortype header

> _Note: this issue was identified against the code base at commit `2019710a3c4752758402c7d5e42a18d73361dfac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteUserPoolClientSecret

## Description
This issue has 1 cause (1 net-new wire divergence cluster, 6 affected steps across 4 fixtures). The single cause is the DeleteUserPoolClientSecret unknown-ClientSecretId refusal, and it diverges in three response fields at once — HTTP status, the Message string, and the x-amzn-errortype header. All three belong to the one refusal path and all three must be corrected before this cause is closed.

Cause 1 of 1: DeleteUserPoolClientSecret aimed at a ClientSecretId that does not exist for the target app client is refused with the right error class but the wrong status, the wrong message and no error-type header.

AWS: DeleteUserPoolClientSecret (Smithy: input UserPoolId @required pattern ^[\w-]+_[0-9a-zA-Z]+$, ClientId @required @sensitive, ClientSecretId @required @length 1..128; errors InvalidParameterException (client, @httpError 400), LimitExceededException (client, 400), ResourceNotFoundException (client, @httpError 404), TooManyRequestsException (429), InternalServerException (500)) refuses a ClientSecretId that was never issued for the client — or one already deleted — with ResourceNotFoundException. On the wire the recorded service does NOT apply the modeled @httpError(404): the refusal arrives as HTTP 400 with __type ResourceNotFoundException in the awsJson1_1 body. The model documents ResourceNotFoundException's message only as "The message returned when the Amazon Cognito service returns a resource not found exception", so the wording here is recorded behavior only: the recorded Message is the fixed sentence 'User pool client secret does not exist.' — the entire string is fixed text, with NO variable parts: it interpolates neither the user pool id, the client id, nor the supplied ClientSecretId. The recorded response also carries the awsJson error-type header x-amzn-errortype naming the exception ('ResourceNotFoundException:').

Emulator: picks the right error class (ResourceNotFoundException) but (a) serves HTTP 404 instead of 400 — it honours the shape's @httpError(404) trait where the recorded service pins client errors to 400; (b) renders the message as 'Client secret <ClientSecretId> does not exist.' — different fixed text, plus the request's ClientSecretId interpolated into it and a trailing period on a sentence AWS words differently (in the recorded step the interpolated value is the request's ClientSecretId, a per-run request value and not part of the requirement); and (c) omits the x-amzn-errortype response header entirely.

Why: a 404 from an AWS JSON-RPC endpoint conventionally means 'no such route/operation', so SDK error classifiers, API gateways and proxies in front of the emulator treat an ordinary modelled client refusal as a transport/routing failure — retry, alarm and circuit-breaker logic misfires, and secret-rotation code branching on ResourceNotFoundException never sees it. The missing x-amzn-errortype removes the header-based discriminator older SDKs and tooling use to resolve the error shape at all, so the refusal can degrade to a generic unmodeled error. And because the Message embeds the per-request secret id, no equality or hash matcher written against AWS's fixed sentence ever matches, and the request's secret identifier is echoed back in error text that AWS keeps free of it.

Fix: two components in the cognito-idp service crate. (1) The DeleteUserPoolClientSecret handler's not-found branch (its validation/lookup step) must raise ResourceNotFoundException with the fixed, non-interpolated sentence and no added period beyond AWS's own. (2) The crate's awsJson1_1 error serializer / error-to-status mapping must pin client errors to HTTP 400 rather than reading @httpError off the error shape, and must emit the x-amzn-errortype header naming the exception on every error response.

Related open issues that do NOT carry this cluster's replay key (the replay reports this cluster unowned): 20261029T222840Z-cognito-idp-deleteuserpoolclientsecret-refusals-carry-emulator-invented-messages.md covers DeleteUserPoolClientSecret refusal rendering from the behavioral corpus only and states a different expected sentence ('Client secret does not exist', no period) for its own grounding, so it does not pin this wire step's expected text; 20261029T184956Z-cognito-idp-every-error-response-omits-the-x.md owns the x-amzn-errortype omission for other operations' steps; 20261029T184956Z-cognito-idp-modeled-httperror-codes-are-served-where.md owns the @httpError-vs-400 mapping for other operations' steps. None of them is exercised by this fixture step.

Traces: cognito-idp-app-client-error-paths#task#5 (representative); same cluster also at cognito-idp-client-secret-error-paths#task#6, cognito-idp-client-secret-error-paths#task#9, cognito-idp-user-pool-client-secret-errors#task#7, cognito-idp-user-pool-client-secret-errors#task#9, cognito-idp-user-pool-client-secret-management#task#10.

## Evidence
Divergence cluster service=cognito-idp|DeleteUserPoolClientSecret|status=400->404|keys=$.message,$headers.x-amzn-errortype,status (verdict: mismatch, affected_count 6).

Expected (recorded AWS), fixture cognito-idp-app-client-error-paths step task#5:
  HTTP 400
  body $.__type = ResourceNotFoundException
  body $.message = 'User pool client secret does not exist.'  (entire string is fixed text; no request or state values interpolated)
  header x-amzn-errortype = 'ResourceNotFoundException:'

Actual (emulator):
  HTTP 404
  body $.message = 'Client secret fabricated-garbage-secret-id does not exist.'
  header x-amzn-errortype = absent (None)

Field-by-field: status: expected 400, got 403/404 -> got 404. $.message: value_mismatch. $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:', got None).

In the emulator's message, 'fabricated-garbage-secret-id' is the ClientSecretId the request supplied — a per-run request value, not part of any requirement. The requirement is AWS's fixed sentence with nothing interpolated and no secret id echoed. The error class (ResourceNotFoundException) is correct; what diverges is the status, the message text and the presence of the error-type header.

Smithy (DeleteUserPoolClientSecret): ResourceNotFoundException is errorKind client with @httpError(404); the recorded service does not apply that trait on the wire and answers 400, so the status requirement is recorded behavior only.

## Steps to Reproduce
1. CreateUserPool, then CreateUserPoolClient with GenerateSecret=true in the emulator.
2. Call DeleteUserPoolClientSecret with that UserPoolId and ClientId but a ClientSecretId that was never issued for the client (any string within @length 1..128), or re-use a ClientSecretId that was already deleted.
3. Observe the refusal: the emulator answers HTTP 404 with body message 'Client secret <the ClientSecretId you sent> does not exist.' and no x-amzn-errortype response header. AWS answers HTTP 400, body message exactly 'User pool client secret does not exist.' (no interpolation), and header x-amzn-errortype naming ResourceNotFoundException.
4. Replay to confirm: fixture cognito-idp-app-client-error-paths step task#5 (and the rest of the cluster: cognito-idp-client-secret-error-paths#task#6, cognito-idp-client-secret-error-paths#task#9, cognito-idp-user-pool-client-secret-errors#task#7, cognito-idp-user-pool-client-secret-errors#task#9, cognito-idp-user-pool-client-secret-management#task#10).
5. Re-read ListUserPoolClientSecrets after the refusal: the secret set is unchanged, confirming the defect is confined to how the refusal is rendered.
