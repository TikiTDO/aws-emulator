Issue-Source: trace-sweep-divergence
Issue-Trace: sns-topic-access-policy-permission-management
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-topic-access-policy-permission-management
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns|AddPermission|2xx->4xx|$.AddPermissionResponse
Issue-Key: sns|AddPermission|2xx->4xx|status
Issue-Commit: a044e80910c4f3e313d61171ceba6e4f6a68da07

# sns: AddPermission refuses a valid grant with 400 InvalidParameter instead of answering 200

> _Note: this issue was identified against the code base at commit `a044e80910c4f3e313d61171ceba6e4f6a68da07`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: a valid AddPermission call against an existing topic is refused as bad input instead of being applied.

AWS: AddPermission adds one Allow statement to a topic's access-control policy. Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice): AddPermission input TopicArn (topicARN, required), Label (label, required), AWSAccountId (DelegatesList, required), ActionName (ActionsList, required); output is Unit, so a successful call answers HTTP 200 with an awsQuery envelope carrying only <AddPermissionResponse><ResponseMetadata><RequestId>...</RequestId></ResponseMetadata></AddPermissionResponse> and no result element. The modelled refusals are InvalidParameterException (awsQueryError InvalidParameter, 400) for input that does not comply with the parameter constraints, NotFoundException (NotFound, 404) for a topic that does not exist, and AuthorizationErrorException (AuthorizationError, 403). The ActionName doc states "Valid values: Any Amazon SNS action name, for example Publish", and the AWSAccountId doc states the values are AWS account IDs. The recording shows the grant in this step satisfying all of that: AWS answers 200 with the empty AddPermissionResponse envelope (the RequestId in the recording is a per-run value, not part of the requirement).

Emulator: the same step is answered HTTP 400 with an ErrorResponse body — no AddPermissionResponse element is produced at all ($.AddPermissionResponse missing). So the emulator turns an accepted grant into an InvalidParameter-class refusal and the statement is never written into the topic's Policy attribute.

Hypothesis: the handler's refusal ladder in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/add_permission.rs rejects input AWS accepts. Every 400 that handler can produce is an InvalidParameterException raised before the write: the foreign-region TopicArn test, the 12-ASCII-digit AWSAccountId test (is_account_id), the ActionName scope test (state::is_action_name against state::ACTION_NAMES, whose membership is derived from the operations the Smithy model BINDS — so an SNS policy action that is not an API operation name, e.g. Receive, is refused "Invalid parameter: Policy statement action out of service scope!" even though SNS admits it in a topic policy), the "policy of <arn> is not a policy document this operation can add a statement to" test (topic.policy_document() returning None after the topic's Policy was replaced by a caller-supplied document earlier in this fixture), the duplicate-Label test (state::STATEMENT_ALREADY_EXISTS_REFUSAL, which must NOT fire for a label previously RemovePermission-ed), and the trailing state::attribute_write_refusal size/shape check on the re-serialised Policy document. Neither the model nor the replay detail names which gate fired, so the fix must re-check each of those gates against this step's request rather than relax one blindly.

Why: a client that manages a topic's access policy through AddPermission (granting another account Publish/Subscribe/Receive on a topic, the standard cross-account fan-out setup) gets a 400 InvalidParameter it has no way to correct. Because the refusal is raised before the store is touched, the topic's Policy attribute keeps its previous statements, so a subsequent GetTopicAttributes shows no grant and every later authorization decision made from that policy denies the delegated account. Code that treats InvalidParameter as a non-retryable caller bug (SDK default) fails the whole setup instead of succeeding as it does against AWS.

Fix: the AddPermission handler and its validation in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/add_permission.rs, together with the policy-action scope set and the topic-policy document/attribute-write helpers it calls in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state.rs. The accepted path must leave handle() returning AddPermissionOutput so the serializer emits the empty 200 AddPermissionResponse envelope.

Traces: sns-topic-access-policy-permission-management#task#5 (expected 200 AddPermissionResponse, got 400 ErrorResponse).

Note: this is NOT covered by the open SNS wire issues that own the other replayed SNS divergences (20261104T115433Z-sns-10-net-new-wire-divergences-arn-reference.md and 20261030T200727Z-sns-5-wire-divergences-the-opt-out-check.md); neither carries this fixture/step, and no open issue names SNS AddPermission (the existing AddPermission issues are all Lambda/SQS).

## Evidence
Fixture sns-topic-access-policy-permission-management, step task#5, operation AddPermission (service sns).
Expected (recorded AWS response): HTTP 200, body {"AddPermissionResponse": {"ResponseMetadata": {"RequestId": "<per-run id>"}}} — fixed text: the AddPermissionResponse/ResponseMetadata/RequestId element names; variable: the request id, which is not part of the requirement.
Actual (emulator): HTTP 400, $.AddPermissionResponse missing (got None); the response body is an ErrorResponse instead, i.e. one of the handler's InvalidParameterException refusals (awsQuery code InvalidParameter). The replay cluster reports only the status and the missing success element, so the emulator's exact Error.Code/Error.Message for this step are not in the replay detail — the handler can only answer 400 via InvalidParameterException, whose message is one of the fixed sentences in src/state.rs ("Invalid parameter: A provided account ID is not valid", "Invalid parameter: Policy statement action out of service scope!", "Invalid parameter: Statement already exists", or "Invalid parameter: Policy Reason: the access policy of <TopicArn> is not a policy document this operation can add a statement to" — fixed text with the TopicArn taken from the request).
Divergence key: service=sns|AddPermission|status=200->400|keys=$.AddPermissionResponse,status
Replay summary: 311 steps, 294 passed, 16 mismatched, score 94.5%; this cluster has owned_by=[] (net-new) and no stub_path.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sns-topic-access-policy-permission-management up to step task#5: the earlier steps create the topic and manage its access policy (they pass), then task#5 issues AddPermission with Action=AddPermission, the topic's ARN as TopicArn, a Label, one AWSAccountId and one ActionName.
3. Observe the response: the emulator answers HTTP 400 with an ErrorResponse (InvalidParameter class) and no AddPermissionResponse element; AWS answered HTTP 200 with the empty AddPermissionResponse envelope.
4. Follow with GetTopicAttributes on the topic: the Policy attribute carries no statement with the requested Label, confirming the grant was never written.
