Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.snstrig-subscribe-accepts-a-lambda-arn-for-a-function-that-does-not-exist lambda__sns.snstrig-subscribe-rejected-for-a-partial-arn-endpoint
Issue-Key: lambda__sns.snstrig-subscribe-accepts-a-lambda-arn-for-a-function-that-does-not-exist|run#14|AssertionError|assert refused == want, (
Issue-Key: lambda__sns.snstrig-subscribe-rejected-for-a-partial-arn-endpoint|run#9|AssertionError|assert got == want, f"a partial-ARN endpoint raised {got!r}, expected {want!r}"
Issue-Commit: 55ffe08148b0f5da621dea606ca05d7920bfd4b8

# lambda__sns: Subscribe refuses a non-ARN lambda endpoint with the generic "Invalid parameter: Endpoint" instead of "Invalid parameter: Lambda endpoint ARN"

> _Note: this issue was identified against the code base at commit `55ffe08148b0f5da621dea606ca05d7920bfd4b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
One net-new root cause, grounded in two failing checks listed under '### lambda__sns' with no OWNED / ALREADY FILED marker.

Cause 1 of 1: for Protocol='lambda', Subscribe's InvalidParameter refusal does not name the lambda endpoint form.
AWS: Subscribe validates a lambda endpoint's SHAPE, not the function's existence — a well-formed function ARN naming no function is stored and confirmed (GetSubscriptionAttributes reports Endpoint = that ARN and PendingConfirmation = "false"). An Endpoint that is not a full function ARN is refused with HTTP 400 and Error.Code 'InvalidParameter' (Smithy: Subscribe errors include InvalidParameterException, errorKind client, httpError 400, awsQueryError code 'InvalidParameter' / httpResponseCode 400; the model documents only "Indicates that a request parameter does not comply with the associated constraints", so the wording is recorded behavior only). The recorded Message is "Invalid parameter: Lambda endpoint ARN": entirely FIXED text naming the endpoint FORM the lambda protocol requires — no part of it comes from the request, and it is the same for every rejected value. Both non-ARN forms a caller actually sends are refused this way: a bare function NAME, and the partial-ARN form '<account>:function:<name>' that Lambda itself accepts for FunctionName. The refused call creates nothing (ListSubscriptionsByTopic stays empty) while the full ARN on the same topic subscribes.
Emulator: both forms are refused 400 InvalidParameter but with Message 'Invalid parameter: Endpoint' — the protocol-independent constant INVALID_ENDPOINT_REFUSAL in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs, whose doc comment states the sentence is intentionally "the same whichever protocol was asked for". Replay: AssertionError: ... got ('InvalidParameter', 'Invalid parameter: Endpoint'), expected ('InvalidParameter', 'Invalid parameter: Lambda endpoint ARN').
Why: a caller wiring an SNS trigger typically passes the function name or a partial ARN first — the two forms Lambda's own API accepts. AWS's message tells them a full function ARN is required; the emulator's generic sentence does not, so the mistake is not self-correcting, and tooling that branches on the refusal text (CDK/SDK wrappers, deploy scripts, runbooks) sees a sentence AWS never sends.
Fix: Subscribe's endpoint validation in the sns crate — build the InvalidParameter message from the requested Protocol (the per-protocol endpoint-form sentence) instead of one protocol-independent constant. The sqs-protocol face of this same constant is filed separately under the 'sns' and 'sns__sqs' service headings in this sweep (one issue per service heading, since each takes its own gate verdict). Open issue 20261110T210703Z-lambda-sns-sqs-sns-subscribe-s-authorizationerror-message.md concerns Subscribe's AuthorizationError wording, not this InvalidParameter message, and does not carry these checks.
Traces: lambda__sns.snstrig-subscribe-accepts-a-lambda-arn-for-a-function-that-does-not-exist, lambda__sns.snstrig-subscribe-rejected-for-a-partial-arn-endpoint.

## Evidence
Behavioral suite: 452/602 passed. Both checks are reported under '### lambda__sns' with no OWNED / ALREADY FILED marker.

[mismatch] lambda__sns.snstrig-subscribe-accepts-a-lambda-arn-for-a-function-that-does-not-exist
  Expected: Subscribe(Protocol='lambda', Endpoint=<bare function name>) -> 400 ('InvalidParameter', 'Invalid parameter: Lambda endpoint ARN'); the positive half (a well-formed ARN for a non-existent function) stored with PendingConfirmation 'false'.
  Actual: ('InvalidParameter', 'Invalid parameter: Endpoint').
  Replay detail: File "check_lambda__sns_snstrig_subscribe_accepts_a_lambda_arn_for_a_function_that_does_not_exist.py", line 43, in run — AssertionError: control: a bare function NAME is not a lambda endpoint and must be refused; got ('InvalidParameter', 'Invalid parameter: Endpoint'), expected ('InvalidParameter', 'Invalid parameter: Lambda endpoint ARN').

[mismatch] lambda__sns.snstrig-subscribe-rejected-for-a-partial-arn-endpoint
  Expected: Subscribe(Protocol='lambda', Endpoint='<account>:function:<name>') -> 400 ('InvalidParameter', 'Invalid parameter: Lambda endpoint ARN'), no subscription created, and the full ARN form accepted.
  Actual: ('InvalidParameter', 'Invalid parameter: Endpoint').
  Replay detail: File "check_lambda__sns_snstrig_subscribe_rejected_for_a_partial_arn_endpoint.py", line 33, in run — AssertionError: a partial-ARN endpoint raised ('InvalidParameter', 'Invalid parameter: Endpoint'), expected ('InvalidParameter', 'Invalid parameter: Lambda endpoint ARN').

  Source: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/subscribe.rs — `const INVALID_ENDPOINT_REFUSAL: &str = "Invalid parameter: Endpoint";` used for every protocol.

## Steps to Reproduce
CreateRole a lambda execution role, CreateFunction a python3.12 no-op function and wait for State=Active; CreateTopic a topic. (1) Subscribe(TopicArn=<topic>, Protocol='lambda', Endpoint='<the function's bare NAME>') — must be refused with Code 'InvalidParameter' and Message exactly "Invalid parameter: Lambda endpoint ARN". (2) Subscribe(TopicArn=<topic>, Protocol='lambda', Endpoint='<account-id>:function:<name>') — same refusal, and ListSubscriptionsByTopic must still be empty. (3) Control: Subscribe with the full 'arn:aws:lambda:<region>:<account>:function:<name>' succeeds, and a well-formed ARN naming a function that does NOT exist is also accepted and reads back with PendingConfirmation 'false'. Replay: lambda__sns.snstrig-subscribe-accepts-a-lambda-arn-for-a-function-that-does-not-exist, lambda__sns.snstrig-subscribe-rejected-for-a-partial-arn-endpoint.
