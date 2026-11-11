Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected
Issue-Key: cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected|run#9|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Commit: a5271544d134d49a9bef1d276b891cd8aa450a13

# CreateTrafficPolicy's invalid-document refusal is emulator-authored: it reports a missing start reference with the wrong message, the wrong suffix format and no InvalidTrafficPolicyDocument error code

> _Note: this issue was identified against the code base at commit `a5271544d134d49a9bef1d276b891cd8aa450a13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrafficPolicy

## Description
One net-new root cause, reached by deploying an AWS::Route53::TrafficPolicy whose document declares no start reference and reading the stack's CREATE_FAILED reason back.

Cause 1 of 1: Route53's refusal of a traffic-policy document with no valid start reference carries the emulator's own sentence and suffix instead of AWS's, and omits the modeled error code.
AWS:      Smithy (route53) CreateTrafficPolicy(Name, Document) declares the modeled error InvalidTrafficPolicyDocument (client, httpError 400), documented as "The format of the traffic policy document that you specified in the Document element is not valid." A document whose StartEndpoint/StartRule names nothing declared is refused with that error, and CloudFormation wraps the service's message verbatim in the resource's CREATE_FAILED reason: `Resource handler returned message: "No start reference was found. (Service: Route 53; Operation: CreateTrafficPolicy; Status Code: 400; Error Code: InvalidTrafficPolicyDocument; Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. Fixed text: the sentence "No start reference was found." and the whole semicolon-separated suffix with the service name rendered "Route 53", the operation name, the status code and the error code; only the request id varies per call.
Emulator: the same template yields `Resource handler returned message: "StartEndpoint <endpoint-name> does not reference a declared endpoint. (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (..., HandlerErrorCode: InvalidRequest)`. Three divergences in one sentence: (a) the message text is the emulator's own prose and echoes the offending StartEndpoint name, which AWS does not do; (b) the suffix uses the comma-separated "Service: Route53" form with no Operation and, crucially, no `Error Code: InvalidTrafficPolicyDocument`; (c) it appends "(SDK Attempt Count: <n>)", which AWS's Route 53 message for this error does not carry. The HTTP status (400) and the CloudFormation HandlerErrorCode (InvalidRequest) do match.
Why:      the error code is the only machine-readable part of this refusal. A client that branches on `InvalidTrafficPolicyDocument` (or parses the `Error Code:` field out of a CloudFormation failure reason to classify a rollback) sees no code at all and falls through to generic handling; a client matching AWS's documented sentence never matches.
Fix:      the Route53 CreateTrafficPolicy handler's document validation and its refusal serialization - raise the modeled InvalidTrafficPolicyDocument with AWS's sentence and AWS's suffix format, and drop the SDK-attempt-count tail - so CloudFormation's AWS::Route53::TrafficPolicy handler wraps the right text.
Traces:   cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected.

## Evidence
Check cloudformation__route53.deploy-route53-trafficpolicy-invalid-document-rejected [mismatch], at run() line 209: `assert got == _REJECTED`.

Actual (emulator), (message, HandlerErrorCode):
  ('StartEndpoint <endpoint-name> does not reference a declared endpoint. (Service: Route53, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)', 'InvalidRequest')
Expected (AWS):
  ('No start reference was found. (Service: Route 53; Operation: CreateTrafficPolicy; Status Code: 400; Error Code: InvalidTrafficPolicyDocument; Request ID: <id>)', 'InvalidRequest')

Fixed text in the AWS form: "No start reference was found." plus "(Service: Route 53; Operation: CreateTrafficPolicy; Status Code: 400; Error Code: InvalidTrafficPolicyDocument; Request ID: ...)". Variable: the request id only (masked as <id> by the harness). The endpoint name the emulator interpolates is a request value AWS does not echo here.

Smithy (route53): CreateTrafficPolicy errors include InvalidTrafficPolicyDocument, errorKind client, httpError 400, documented "The format of the traffic policy document that you specified in the Document element is not valid."

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::Route53::TrafficPolicy whose Document names a StartEndpoint/StartRule that is not among the declared endpoints/rules (and a positive control with a valid document).
2. Wait for the stack to reach ROLLBACK_COMPLETE.
3. Read DescribeStackEvents and take the CREATE_FAILED ResourceStatusReason of the traffic-policy resource.
4. Observe the emulator's sentence, the comma-separated "Service: Route53" suffix with no `Error Code: InvalidTrafficPolicyDocument`, and a trailing "(SDK Attempt Count: <n>)", instead of AWS's "No start reference was found. (Service: Route 53; Operation: CreateTrafficPolicy; Status Code: 400; Error Code: InvalidTrafficPolicyDocument; Request ID: ...)".
