Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected cloudformation__sns.sns-topic-display-name-too-long-rejected
Issue-Key: cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected|_assert_rejected#3|AssertionError|assert stack_failed == [_STACK_REASON], (
Issue-Key: cloudformation__sns.sns-topic-display-name-too-long-rejected|_resource_failure#9|AssertionError|raise AssertionError(f"could not parse handler reason: {reason!r}")
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# cloudformation__sns: 2 net-new causes — a refused AWS::SNS::Topic property value emits no stack-level CREATE_FAILED reason, and the resource-handler failure reason omits the "(Service: …, Status Code: …, Request ID: …)" segment

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct net-new root causes in how a CloudFormation stack reports an AWS::SNS::Topic refusal. Each is numbered with its own failing check ids; BOTH must be closed before this issue is done.

Related, but not covering these: 20261109T223435Z-cloudformation-sns-6-net-new-causes-topicinlinepolicy-ref.md Cause 3 was "the stack reaches CREATE_COMPLETE instead of rolling back" for these same two checks. Both stacks now DO reach ROLLBACK_COMPLETE, so that cause is behind us; both checks now fail one step later, on the events/reasons the rollback must carry. 20261109T185037Z-cloudformation-sns-sqs-6-net-new-causes-custom-resource-responseurl-points.md mentions a stack-level CREATE_FAILED/PROPERTY_VALIDATION gap under service cloudformation__sns__sqs; it does not carry either check below.

Cause 1 of 2: a property-value validation failure of AWS::SNS::Topic rolls the stack back without emitting any stack-level CREATE_FAILED event or reason.
AWS: recorded behavior only (CloudFormation's resource-schema property validation is not described by a Smithy model). When a template sets AWS::SNS::Topic MaximumMessageSize outside its schema range — below the minimum of 1024 or above the maximum of 1048576 — the stack is refused BEFORE provisioning: it settles at ROLLBACK_COMPLETE, and the stack's own resource (the event whose LogicalResourceId is the stack name) gets exactly one CREATE_FAILED event whose ResourceStatusReason is the fixed text "Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation." — only the error count is a value derived from the request, the rest is fixed. DescribeEvents additionally carries one VALIDATION_ERROR OperationEvent for the offending property: LogicalResourceId "Topic", ResourceType "AWS::SNS::Topic", ValidationName "PROPERTY_VALIDATION", ValidationStatus "FAILED", ValidationFailureMode "FAIL", ValidationPath "/Resources/Topic/Properties/MaximumMessageSize" and ValidationStatusReason "#/MaximumMessageSize: <submitted value> is not greater or equal to 1024" (below the minimum) or "#/MaximumMessageSize: <submitted value> is not less or equal to 1048576" (above the maximum) — the property path and the submitted value are values from the request, the wording and the two bounds are the rule. ListStackResources on such a stack is empty and the topic name never resolves in SNS (GetTopicAttributes answers NotFound "Topic does not exist").
Emulator: the stack reaches ROLLBACK_COMPLETE, but the list of CREATE_FAILED ResourceStatusReasons for the stack itself is EMPTY — no stack-level CREATE_FAILED event is recorded at all, so the "Validation failed with N error(s)…" reason is never served and the VALIDATION_ERROR OperationEvent behind it cannot be reached either.
Why: a client that reads DescribeStackEvents/DescribeEvents to learn WHY its deploy failed sees a rollback with no reason and no property path, so it cannot tell a schema-invalid template from any other failure, and automation that keys on the validation reason never fires.
Fix: the CloudFormation provisioner's property-validation path — it must record the stack-level CREATE_FAILED event with that reason and the matching VALIDATION_ERROR operation event when a resource property fails schema validation.
Traces: cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected.

Cause 2 of 2: the resource-handler failure reason drops the service/status/request-id segment and re-words SNS's message.
AWS: a CloudFormation resource whose handler got a service refusal reports ResourceStatusReason in the shape 'Resource handler returned message: "<the service's own message> (Service: <ServiceName>, Status Code: <HTTP status>, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: <code>)' — optionally with a trailing " (SDK Attempt Count: <n>)" inside the quotes. The wrapper text, the parenthesised key names and their order are fixed; the message, service name, status, request id, request token and handler error code are values. For an AWS::SNS::Topic whose DisplayName exceeds SNS's 100-character limit, the inner message is "Invalid parameter: Attributes Reason: DisplayName", the Service is "Sns", the Status Code is 400 and the HandlerErrorCode is "InvalidRequest". Smithy: SNS models InvalidParameterException with awsQueryError code "InvalidParameter" and httpError 400, which is the refusal being wrapped.
Emulator: the reason is 'Resource handler returned message: "Invalid parameter: DisplayName" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)' — the whole "(Service: Sns, Status Code: 400, Request ID: <id>)" segment is missing from inside the quotes, and the quoted SNS message drops the "Attributes Reason:" part.
Why: every client and tool that parses a handler failure reason to recover the failing service, the HTTP status or the request id (the CLI/CDK all surface exactly that) gets a string AWS never emits, and the quoted SNS message is not the one SNS serves.
Fix: the handler-failure-reason formatter in the CloudFormation provisioner (the service/status/request-id wrapping), and the AWS::SNS::Topic handler's propagation of SNS's verbatim InvalidParameter message.
Traces: cloudformation__sns.sns-topic-display-name-too-long-rejected.

## Evidence
Cause 1: AssertionError: the stack's CREATE_FAILED reasons were [], expected ['Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.'] — for MaximumMessageSize=100 (below the 1024 minimum); the same leg repeats for 2000000 (above the 1048576 maximum), whose expected VALIDATION_ERROR reason is '#/MaximumMessageSize: 2000000 is not less or equal to 1048576'.

Cause 2: AssertionError: could not parse handler reason: 'Resource handler returned message: "Invalid parameter: DisplayName" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Expected the parsed tuple ('Invalid parameter: Attributes Reason: DisplayName', 'Sns', '400', 'InvalidRequest') out of a reason of the form 'Resource handler returned message: "<message> (Service: <svc>, Status Code: <status>, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: <code>)'.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Cause 1: CreateStack with a single AWS::SNS::Topic whose Properties set MaximumMessageSize=100 (and again 2000000); wait for the stack to settle, then read DescribeStackEvents for CREATE_FAILED events whose LogicalResourceId is the stack name, and DescribeEvents for VALIDATION_ERROR OperationEvents. Replay check cloudformation__sns.deploy-sns-topic-maximum-message-size-boundary-rejected.
3. Cause 2: CreateStack with a single AWS::SNS::Topic whose DisplayName is 101 characters; wait for ROLLBACK_COMPLETE and read the CREATE_FAILED event for logical id "Topic". Replay check cloudformation__sns.sns-topic-display-name-too-long-rejected.
