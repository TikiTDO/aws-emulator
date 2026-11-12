Issue-Source: trace-sweep-divergence
Issue-Trace: resiliencehub-app-version-draft-composition
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resiliencehub-app-version-draft-composition resiliencehub-app-version-publish resiliencehub-resiliency-policy-lifecycle
Issue-Shape-Id: com.amazonaws.resiliencehub#AwsResilienceHub
Issue-Key: resiliencehub|CreateAppVersionAppComponent|4xx->2xx|$.message
Issue-Key: resiliencehub|CreateAppVersionAppComponent|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|CreateAppVersionAppComponent|4xx->2xx|status
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->2xx|$.message
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->2xx|status
Issue-Key: resiliencehub|PublishAppVersion|4xx->2xx|$.message
Issue-Key: resiliencehub|PublishAppVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|PublishAppVersion|4xx->2xx|status
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->4xx|$.message
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|CreateAppVersionResource|4xx->4xx|status
Issue-Key: resiliencehub|CreateResiliencyPolicy|4xx->2xx|$.message
Issue-Key: resiliencehub|CreateResiliencyPolicy|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: resiliencehub|CreateResiliencyPolicy|4xx->2xx|status
Issue-Commit: 5b71589754f95f05f88c1a0ee9ccd691bbc5f701

# resiliencehub accepts invalid app-version and policy input with 200 instead of the modeled refusal

> _Note: this issue was identified against the code base at commit `5b71589754f95f05f88c1a0ee9ccd691bbc5f701`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppVersionResource

## Description
Five causes sharing one root fault: resiliencehub performs no input/precondition validation on these write paths, so requests AWS refuses with a modeled 4xx are accepted and mutate state (or, in one case, are refused for the wrong reason because a conflict check runs before validation).

=== Cause 1 of 5: CreateAppVersionAppComponent accepts an unsupported component type ===
AWS: `type` is a required String255 and must name a Resilience Hub Application Component type; an unsupported value is refused with ValidationException (Smithy: CreateAppVersionAppComponent errors include ValidationException, httpError 400) and x-amzn-errortype ValidationException. Recorded response: HTTP 400, message `Invalid AppComponent type: <type>.` — fixed text `Invalid AppComponent type: ` plus the rejected `type` value from the request plus a trailing `.`.
Emulator: answers HTTP 200 and creates the component, with no `message` and no x-amzn-errortype header.
Why: a typo'd or non-component AWS type (e.g. a raw resource type) silently becomes part of the draft; the error AWS raises at create time instead surfaces much later as a failed publish or assessment, and negative tests that assert the 400 pass vacuously.
Fix: type validation in the CreateAppVersionAppComponent handler, before the component is inserted into the draft.
Traces: resiliencehub-app-version-draft-composition#task#6.

=== Cause 2 of 5: CreateAppVersionResource accepts a physical resource that does not exist ===
AWS: the submitted physicalResourceId must resolve to a real resource of the stated resourceType; when it does not, the mapping is refused with ValidationException / HTTP 400. Recorded response: HTTP 400, message `Invalid resource mapping. The given resource does not exist. Resource Type: <resourceType>, Resource Id: <physicalResourceId>` — fixed text `Invalid resource mapping. The given resource does not exist. Resource Type: `, `, Resource Id: `, with the resourceType and physicalResourceId taken from the request.
Emulator: answers HTTP 200 and records the mapping, with no message and no errortype header.
Why: resource existence is the entire point of this call — the application template is supposed to reference real resources. Accepting a ghost id means the client believes a resource is under assessment when nothing is, and the resulting resiliency score covers a resource that does not exist.
Fix: the CreateAppVersionResource handler's validation — resolve the physical resource against emulator state for the given type and refuse with ValidationException when absent.
Traces: resiliencehub-app-version-draft-composition#task#10.

=== Cause 3 of 5: PublishAppVersion publishes a draft with zero resources ===
AWS: publishing is refused unless the draft's app template maps at least one resource — ConflictException, httpError 409 (Smithy: PublishAppVersion errors include ConflictException). Recorded response: HTTP 409 with the fixed, variable-free message `Invalid app template. App template must have at least 1 resource`; the refusal creates no published version.
Emulator: answers HTTP 200 and publishes, with no message and no errortype header — the draft in this fixture holds Application Components but no mapped resources, so the emulator's emptiness precondition is testing the wrong thing (it does not require a RESOURCE).
Why: an empty release is published and starts being addressed by clients and assessments, instead of the client learning its template is incomplete. Note the open issue 'resiliencehub: PublishAppVersion mints numbered versions instead of the "release" alias, and both ConflictException refusal messages diverge from AWS wording' reports the empty-draft message TEXT when the refusal does fire; it does not cover this case, where no refusal fires at all because the precondition only looks at components.
Fix: the PublishAppVersion handler's precondition — require >= 1 mapped resource in the draft (not merely a non-empty draft) and refuse with ConflictException and AWS's message.
Traces: resiliencehub-app-version-draft-composition#task#17.

=== Cause 4 of 5: CreateAppVersionResource refuses an invalid resourceType as a 409 conflict instead of a 400 validation error ===
AWS: `resourceType` must name a supported resource type; an unsupported one is a ValidationException / HTTP 400 with message `Invalid resource type: <resourceType>` (fixed text `Invalid resource type: ` plus the rejected value from the request), and this validation is evaluated BEFORE any duplicate-resource conflict check.
Emulator: answers HTTP 409 with x-amzn-errortype ConflictException and a duplicate-logicalResourceId message, i.e. it skips type validation entirely and reports the first conflicting condition it finds instead.
Why: the client is told to resolve a non-existent name collision while the real fault — an unsupported resource type — is never reported. ValidationException is non-retryable and ConflictException is retryable per its documentation, so an SDK with default retry behavior will retry a request that can never succeed.
Fix: the CreateAppVersionResource handler — add resourceType validation and order it ahead of the duplicate-resource conflict check.
Traces: resiliencehub-app-version-publish#task#5.

=== Cause 5 of 5: CreateResiliencyPolicy accepts an unknown disruption-type key ===
AWS: `policy` is a required DisruptionPolicy map whose KEY is the DisruptionType enum with exactly the values Software, Hardware, AZ, Region; a key outside that set (e.g. the upper-cased `HARDWARE`) is refused with ValidationException / HTTP 400. Recorded response: HTTP 400, message `FailureType <key> does not exist` — fixed text `FailureType ` and ` does not exist`, with the offending key echoed from the request.
Emulator: answers HTTP 200 and creates the policy, with no message and no errortype header, so the unknown key is either stored or silently dropped.
Why: enum keys are case-sensitive; a client that mis-cases a disruption type gets a policy that does not constrain the failure type it thinks it configured, and the resulting assessment verdicts are computed against a policy the client never intended.
Fix: the CreateResiliencyPolicy handler's validation of the DisruptionPolicy map keys against the DisruptionType enum, before the policy is stored.
Traces: resiliencehub-resiliency-policy-lifecycle#task#6 (cluster also covers resiliencehub-resiliency-policy-lifecycle#task#7).

## Evidence
Cause 1 — CreateAppVersionAppComponent, resiliencehub-app-version-draft-composition#task#6:
  status: expected 400, got 200
  $.message: missing (expected 'Invalid AppComponent type: <type>.', recorded with type AWS::SNS::Topic; got None)
  $headers.x-amzn-errortype: missing (expected the modeled error type, got None)
Cause 2 — CreateAppVersionResource, resiliencehub-app-version-draft-composition#task#10:
  status: expected 400, got 200
  $.message: missing (expected 'Invalid resource mapping. The given resource does not exist. Resource Type: <resourceType>, Resource Id: <physicalResourceId>'; got None)
  $headers.x-amzn-errortype: missing
Cause 3 — PublishAppVersion, resiliencehub-app-version-draft-composition#task#17:
  status: expected 409, got 200
  $.message: missing (expected 'Invalid app template. App template must have at least 1 resource'; got None)
  $headers.x-amzn-errortype: missing
Cause 4 — CreateAppVersionResource, resiliencehub-app-version-publish#task#5:
  status: expected 400, got 409
  $.message: expected 'Invalid resource type: <resourceType>' (recorded value AWS::Bogus::Type), got the duplicate-logicalResourceId conflict text
  $headers.x-amzn-errortype: expected the ValidationException type, got 'ConflictException'
Cause 5 — CreateResiliencyPolicy, resiliencehub-resiliency-policy-lifecycle#task#6 (and #task#7):
  status: expected 400, got 200
  $.message: missing (expected 'FailureType <key> does not exist', recorded key HARDWARE; got None)
  $headers.x-amzn-errortype: missing

Smithy citations: CreateAppVersionAppComponent / CreateAppVersionResource / CreateResiliencyPolicy all list ValidationException (httpError 400); PublishAppVersion lists ConflictException (httpError 409); DisruptionPolicy key -> DisruptionType enum [Software, Hardware, AZ, Region]; CreateAppVersionResource.resourceType required String255; CreateAppVersionAppComponent.type required String255; ValidationException/ConflictException each carry `message` (String500). The concrete type names, ids and keys above are the request values filling each message template.

## Steps to Reproduce
1. CreateApp(name=<app>) -> appArn.
2. CreateAppVersionAppComponent(appArn, name=<c>, type="AWS::SNS::Topic") — an AWS resource type, not an AppComponent type. Observe 200 instead of 400 'Invalid AppComponent type: AWS::SNS::Topic.' (Cause 1).
3. CreateAppVersionResource(appArn, resourceName=<r>, logicalResourceId={identifier:<name-that-exists-nowhere>}, physicalResourceId=<same>, resourceType="AWS::S3::Bucket", appComponents=[<c>]). Observe 200 instead of 400 'Invalid resource mapping. The given resource does not exist. ...' (Cause 2).
4. On an app whose draft has components but no mapped resources, PublishAppVersion(appArn). Observe 200 instead of 409 'Invalid app template. App template must have at least 1 resource' (Cause 3).
5. CreateAppVersionResource(..., resourceType="AWS::Bogus::Type", appComponents=[<c>]) reusing an existing logicalResourceId. Observe 409 ConflictException about the duplicate instead of 400 'Invalid resource type: AWS::Bogus::Type' (Cause 4).
6. CreateResiliencyPolicy(policyName=<p>, tier="NonCritical", policy={"HARDWARE": {rtoInSecs:..., rpoInSecs:...}}). Observe 200 instead of 400 'FailureType HARDWARE does not exist' (Cause 5).
Full replay: replay_wire_corpus() and re-check resiliencehub-app-version-draft-composition#task#6, #task#10, #task#17, resiliencehub-app-version-publish#task#5, resiliencehub-resiliency-policy-lifecycle#task#6.
