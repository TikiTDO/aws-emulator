Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected
Issue-Key: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected|run#17|AssertionError|assert match is not None, f"CREATE_FAILED reason is not a resource-handler message: {reason!r}"
Issue-Commit: 3c33b9e3725ab606ee2a23453a6abf264aabc19e

# cloudformation__events: a resource provider's CREATE_FAILED reason omits the (Service, Status Code, Request ID) wrapping and reports HandlerErrorCode InvalidRequest instead of GeneralServiceException

> _Note: this issue was identified against the code base at commit `3c33b9e3725ab606ee2a23453a6abf264aabc19e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Two net-new causes in CloudFormation's resource-handler error wrapping, both grounded in the single failing check cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected. The emulator now correctly fails the stack (CREATE_FAILED) for an AWS::Events::Rule whose ScheduleExpression is malformed - that part of open issue 20261109T171600Z-cloudformation-6-net-new-causes-aws-events-endpoint.md (its Cause 4, keyed on the older assertion 'expected stack status CREATE_FAILED, got CREATE_COMPLETE') no longer reproduces - but the ResourceStatusReason it publishes for the failed resource is not the reason AWS publishes. That open issue does not carry this failure's replay key, and it is named here only for context.

Cause 1 of 2: the quoted provider message drops the service-call attribution.
AWS: when a resource provider's underlying service call is refused, the resource's CREATE_FAILED StackEvent carries ResourceStatusReason of the fixed form `Resource handler returned message: "<service message> (Service: <ServiceName>, Status Code: <httpStatus>, Request ID: <requestId>)" (RequestToken: <clientRequestToken>, HandlerErrorCode: <code>)`, optionally with ` (SDK Attempt Count: <n>)` after the Request ID group. Fixed text: `Resource handler returned message: "`, ` (Service: `, `, Status Code: `, `, Request ID: `, `)" (RequestToken: `, `, HandlerErrorCode: `, `)`. Variable parts, all taken from the failing call and the stack operation: the service's own message (here EventBridge's PutRule refusal `Parameter ScheduleExpression is not valid.` - Smithy: PutRule.ScheduleExpression is a ScheduleExpression @length 0..256 and the refusal is a client 400), the service name (`EventBridge`), the HTTP status of that refusal (`400`), the service request id, and the stack's request token.
Emulator: the reason is `Resource handler returned message: "Parameter ScheduleExpression is not valid." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` - the quoted message ends at the service sentence, with the entire ` (Service: ..., Status Code: ..., Request ID: ...)` group absent.
Why: every client and operator that reads a CREATE_FAILED reason loses which service refused the call and with what HTTP status, and tooling that parses the documented wrapping (the shape CloudFormation guarantees for provider failures) does not match the emulator's text at all, so a stack failure cannot be attributed to a downstream service.
Fix: the CloudFormation resource-provider error-wrapping path that renders ResourceStatusReason for a provider whose service call failed - it must carry the invoked service's sdkId, the refusal's HTTP status and the service request id into the quoted message, for every provider, not only AWS::Events::Rule.
Traces: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected.

Cause 2 of 2: HandlerErrorCode is classified as InvalidRequest where AWS reports GeneralServiceException.
AWS: a provider failure caused by the downstream service refusing the call is reported with `HandlerErrorCode: GeneralServiceException` (InvalidRequest is reserved for a request CloudFormation itself rejects before calling the service). The value is fixed per failure class, not per request.
Emulator: the same event reports `HandlerErrorCode: InvalidRequest`.
Why: HandlerErrorCode is the machine-readable classification of a failed resource; a client that retries or escalates on GeneralServiceException (a downstream fault) versus InvalidRequest (a bad template) takes the wrong branch, and a template that is in fact well-formed is blamed.
Fix: the HandlerErrorCode classification in the same resource-provider error-wrapping path - a refusal surfaced by the downstream service must map to GeneralServiceException.
Traces: cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected.

## Evidence
Check cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected, assertion `assert match is not None, f"CREATE_FAILED reason is not a resource-handler message: {reason!r}"`:

AssertionError: CREATE_FAILED reason is not a resource-handler message: 'Resource handler returned message: "Parameter ScheduleExpression is not valid." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'

Expected (the check's regex over AWS's wrapping, then the tuple it asserts):
  ^Resource handler returned message: "(?P<message>.*) \(Service: (?P<service>[A-Za-z0-9]+), Status Code: (?P<status>\d+), Request ID: [^)]*\)(?: \(SDK Attempt Count: \d+\))?" \(RequestToken: [^,]*, HandlerErrorCode: (?P<handler>[A-Za-z]+)\)$
  (message, service, status, handler) == ("Parameter ScheduleExpression is not valid.", "EventBridge", "400", "GeneralServiceException")

Got: no `(Service: EventBridge, Status Code: 400, Request ID: ...)` group at all, and handler `InvalidRequest`. The positive control in the same check (the identical template with `rate(5 minutes)`) reaches CREATE_COMPLETE, and the stack correctly reaches CREATE_FAILED for `rate(5 whatevers)`, so only the reason text diverges.

## Steps to Reproduce
1. Replay cloudformation__events.deploy-cfn-events-rule-invalid-schedule-expression-rejected.
2. CreateStack (DisableRollback=true) with a template whose only resource is an AWS::Events::Rule with Properties {Name: <n>, ScheduleExpression: "rate(5 whatevers)", State: "ENABLED"}.
3. Poll DescribeStacks until the stack reaches CREATE_FAILED.
4. DescribeStackEvents and read the ResourceStatusReason of the single CREATE_FAILED event for logical id `Rule`.
5. Compare against AWS's wrapping: the quoted message must end with ` (Service: EventBridge, Status Code: 400, Request ID: <id>)` and the trailing group must read `HandlerErrorCode: GeneralServiceException`.
