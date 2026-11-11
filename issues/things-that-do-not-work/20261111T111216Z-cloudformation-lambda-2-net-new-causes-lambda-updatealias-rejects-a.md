Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfn-lambda-alias-in-place-routing-retained-replacement-invokes-versions-then-not-found cloudformation__lambda.cfn-lambda-version-runtime-policy-change-refused-retained-replacement-invokes-both-then-not-found
Issue-Key: cloudformation__lambda.cfn-lambda-alias-in-place-routing-retained-replacement-invokes-versions-then-not-found|_expect#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda.cfn-lambda-version-runtime-policy-change-refused-retained-replacement-invokes-both-then-not-found|run#23|AssertionError|assert _REFUSED.fullmatch(failed[0][2] or ""), (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 net-new causes: Lambda UpdateAlias rejects a RoutingConfig with a deserialization error, and AWS::Lambda::Version in-place updates are refused with emulator-authored text

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAlias

## Description
Two independent root causes behind the Lambda CloudFormation checks.

Cause 1 of 2: an alias update that carries RoutingConfig is refused with a JSON deserialization error.
AWS: Lambda UpdateAlias accepts RoutingConfig.AdditionalVersionWeights, a MAP of version string -> weight double (AWS::Lambda::Alias RoutingConfig.AdditionalVersionWeights is a list of (FunctionVersion, FunctionWeight) pairs that CloudFormation converts to that map). The call returns 200 with the alias's new RoutingConfig and the stack update completes.
Emulator: the update fails the resource. Stack 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Alias]. ' and resource reason 'Resource handler returned message: "expected object start, got array start (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The quoted text 'expected object start, got array start' is a serializer-internal parse error, not an AWS error message: AWS answers a malformed RoutingConfig with InvalidParameterValueException and a message naming the parameter, and a well-formed one with 200.
Why: weighted alias routing — the standard Lambda canary/blue-green mechanism — cannot be configured through CloudFormation or the SDK at all, and the caller is handed an HTTP 400 with an internal parser message it cannot act on.
Fix: the Lambda UpdateAlias/CreateAlias request deserializer for RoutingConfig.AdditionalVersionWeights (it must accept the modelled map) and/or the AWS::Lambda::Alias handler that builds that request.
Failing check: cfn-lambda-alias-in-place-routing-retained-replacement-invokes-versions-then-not-found.

Cause 2 of 2: the UPDATE_FAILED reason for a change to an existing AWS::Lambda::Version is emulator-authored.
AWS: an AWS::Lambda::Version is immutable; a change to it is answered by publishing a new version (a replacement), and where the update is refused the reason is Lambda's own wording wrapped in CloudFormation's 'Resource handler returned message: "…" (RequestToken: <token>, HandlerErrorCode: <code>)' envelope.
Emulator: the reason is 'NotImplemented: this emulator does not change the properties of an existing AWS::Lambda::Version in place; the resource was left as it is.' — fixed emulator text naming the emulator, with the resource type as its only variable part. AWS emits no such message.
Why: a client keying off the reason (CI gates, fix-forward automation, operators) sees a message AWS never produces, and the replacement behaviour (a new version published, both versions invocable) never happens.
Fix: the AWS::Lambda::Version resource handler's update classification in the CloudFormation provisioner.
Failing check: cfn-lambda-version-runtime-policy-change-refused-retained-replacement-invokes-both-then-not-found.

## Evidence
Cause 1: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Alias]. '), expected 'UPDATE_COMPLETE'; failed events: [('Alias', 'UPDATE_FAILED', 'Resource handler returned message: "expected object start, got array start (Service: Lambda, Status Code: 400, Request ID: 6f28eeea-d2f0-34e2-e1c2-5453e0045ce2) (SDK Attempt Count: 1)" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')]
Cause 2: AssertionError: the version's UPDATE_FAILED reason must be the existing-version refusal: 'NotImplemented: this emulator does not change the properties of an existing AWS::Lambda::Version in place; the resource was left as it is.'

## Steps to Reproduce
Cause 1: CreateStack a function with two published versions and an AWS::Lambda::Alias; UpdateStack adding RoutingConfig.AdditionalVersionWeights (or call lambda UpdateAlias with RoutingConfig directly) — the call is refused with 'expected object start, got array start'.
Cause 2: CreateStack an AWS::Lambda::Version, then UpdateStack changing the function's runtime/policy so the version resource is updated; read the UPDATE_FAILED reason.
