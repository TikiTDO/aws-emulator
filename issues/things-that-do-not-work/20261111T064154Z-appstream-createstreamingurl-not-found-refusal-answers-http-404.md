Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-streaming-and-builder-not-found-rejections
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-streaming-and-builder-not-found-rejections
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|CreateStreamingURL|4xx->4xx|status
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# appstream CreateStreamingURL not-found refusal answers HTTP 404 instead of AWS's 400

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStreamingURL

## Description
One net-new root cause. The replay report did not mark this cluster OWNED or stub-blocked. Overlap note: the open issue 20261102T092115Z-appstream-stack-not-found-refusals-diverge-wrong-http.md describes the same status-class fault on DescribeStacks/UpdateStack/DeleteStack, and 20261109T145337Z-appstream-2-net-new-causes-createstreamingurl-is-a.md describes CreateStreamingURL when it was still a 501 scaffold; neither covers this step, so the CreateStreamingURL refusal path is restated here with its own replay key. A fixer should expect the fix to be the same status mapping those issues touch, applied to this handler.

Cause 1 of 1: CreateStreamingURL's not-found refusal is served with HTTP status 404 where AWS serves HTTP 400.
AWS: Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateStreamingURL{StackName required (len 1..), FleetName required (len 1..), UserId required (StreamingUrlUserId len 2..32, pattern ^[\w+=,.@-]*$), ApplicationId, Validity, SessionContext} -> {StreamingURL, Expires}; modeled errors InvalidParameterCombinationException (400), OperationNotPermittedException (400), ResourceNotAvailableException (400) and ResourceNotFoundException, whose shape carries @httpError(404). Recorded behavior: AWS AppStream is an AWS JSON 1.1 service that serves this refusal over HTTP 400 — the recorded response for a CreateStreamingURL naming a stack/fleet that does not exist is status 400 with the error code in the body (__type) and the AppStream not-found sentence as its message, i.e. the model's 404 httpError is not what the service puts on the wire. The error code and message are the requirement alongside the status; the names and account id inside the message are variable parts taken from the request and the emulator's own account, the surrounding sentence is fixed text.
Emulator: HTTP 404. The body is already correct — the replay report lists the diverging keys as `status` only (expected 400, got 404), so the emulator's __type and message matched the recording byte-for-byte and the status line alone is wrong.
Why: HTTP status is load-bearing for every AWS SDK and for proxies in front of the emulator. botocore classifies a 404 differently from a 400 (and some clients treat a 404 on a POST RPC endpoint as a routing/endpoint error rather than a modeled API error), so a client that catches ResourceNotFoundException from CreateStreamingURL can see the call raise a generic HTTP/endpoint failure instead, and retry policies keyed on status class behave differently than against AWS.
Fix: the CreateStreamingURL handler's refusal path in the appstream service crate (and the shared error-to-HTTP-status mapping it uses) must emit status 400 for this not-found refusal, as AppStream does on the wire, leaving the already-correct error code and message untouched.
Traces: appstream-streaming-and-builder-not-found-rejections#task#3 (CreateStreamingURL, expected 400, got 404, keys=status).

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator booted from this checkout (emulator_up true): 150 steps, 72 passed, 78 mismatched, score 48%.

fixture appstream-streaming-and-builder-not-found-rejections, step task#3, operation CreateStreamingURL, verdict mismatch, affected_count 1:
  expected_status 400, actual_status 404
  detail: "status: expected 400, got 404" — no body keys are listed, so the error code and message in the emulator's response already match the recorded AWS refusal; only the HTTP status diverges.

Smithy evidence (same model file): CreateStreamingURL errors {InvalidParameterCombinationException 400, OperationNotPermittedException 400, ResourceNotAvailableException 400, ResourceNotFoundException httpError 404}; shape ResourceNotFoundException{Message} errorKind client, doc "The specified resource was not found." The recorded AWS response for this step is status 400, so the wire status for this refusal class is 400 and not the modeled 404.

The stack, fleet and user names and the account id in the recorded message are per-run values, not requirements; the requirement is the HTTP 400 status together with the already-matching error code and message.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay fixture appstream-streaming-and-builder-not-found-rejections from trace set simparity-wire-aws-v1.
3. At step task#3, call CreateStreamingURL(StackName=<a stack that does not exist>, FleetName=<a fleet that does not exist>, UserId=<a valid user id>): observe the refusal arriving with HTTP status 404 while carrying the correct error code and message, where AWS answers HTTP 400 with that same body.
4. Re-replay appstream-streaming-and-builder-not-found-rejections#task#3 to confirm the status is 400.
