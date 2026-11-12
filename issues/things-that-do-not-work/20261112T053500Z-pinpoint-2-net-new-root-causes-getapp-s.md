Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pinpoint.delete-app-cascades-channel-configuration pinpoint.push-channel-state-transition-changes-send-messages-outcome
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint.delete-app-cascades-channel-configuration|run#11|AssertionError|assert message == "Application not found", f"unexpected GetApp message {message!r}"
Issue-Key: pinpoint.push-channel-state-transition-changes-send-messages-outcome|run#11|AssertionError|assert before["StatusMessage"] == (
Issue-Commit: fa890fd22907b6043ac6347c6d1f000950c19503

# pinpoint: 2 net-new root causes — GetApp's NotFoundException Message interpolates the ApplicationId, and SendMessages' per-destination StatusMessage for an unconfigured channel is emulator prose instead of the service's 'BAIDU channel not found. Please enable ...' text

> _Note: this issue was identified against the code base at commit `fa890fd22907b6043ac6347c6d1f000950c19503`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Two net-new root causes in the Pinpoint emulator, both error/status TEXT divergences on operations that are otherwise implemented and answer the right status. Numbered below; each carries its own failing check ids. A fixer must close BOTH causes before submitting.

Note on overlap: open issue 20261111T071029Z-pinpoint-notfoundexception-body-diverges-requestid-member-omitted-and describes the same GetApp message interpolation from the WIRE corpus (fixture pinpoint-app-lifecycle#task#4) and additionally the missing RequestID member. That issue was not reported as owning this behavioral check, so Cause 1 is filed here with its behavioral grounding; fixing one should fix the other, and Cause 1 adds nothing about RequestID.

=== Cause 1 of 2: GetApp's NotFoundException Message appends the requested ApplicationId to the service's fixed text ===
AWS: Smithy: GetApp -> GET /v1/apps/{ApplicationId}, modeled error NotFoundException with @httpError 404; NotFoundException is a structure of Message (__string, "The message that's returned from the API.") and RequestID. For an application id that does not exist (here: one that was just deleted), the service answers 404 NotFoundException with Message exactly "Application not found" — that whole string is FIXED text, with NO variable part: the service does not interpolate the requested ApplicationId or any other request value into it. (Recorded/behavioral basis for the literal text; the model only types the member.)
Emulator: status 404 and error code NotFoundException are correct, but the Message is "Application not found: <ApplicationId>" — the fixed text plus ": " plus the ApplicationId taken from the request path. The hex id seen in the replay output is the check's generated application id, a per-run value, and is not part of the requirement; the defect is the interpolation itself.
Why: a client that classifies or asserts on the error message text — error-classification code, SDK retry/diagnostic middleware, or a test pinned to the service's wording — sees a message that varies with the requested id, so an equality check that passes against AWS fails against the emulator, and the emulator echoes a request value AWS never puts in the body.
Fix: the GetApp handler's not-found branch (the NotFoundException it constructs), which should emit the class's fixed text rather than a formatted one. Note the sibling SendMessages handler already constructs this same exception with the correct fixed "Application not found"; GetApp diverges from it.
Traces: pinpoint.delete-app-cascades-channel-configuration (failed at the GetApp message assertion after DeleteApp: expected 'Application not found', got 'Application not found: 97435f8441d3f98b864befa3423112ab'). Because the check stops there, its following assertion — GetBaiduChannel on the deleted app must be rejected with NotFoundException and the GENERIC Message "Resource not found" (fixed text, no value interpolated), distinct from the app-level message — was never reached and is UNVERIFIED; it must hold once Cause 1 is fixed, so re-replay this check to confirm both.

=== Cause 2 of 2: SendMessages' per-destination StatusMessage for a channel the application has not configured is emulator prose ===
AWS: Smithy: SendMessages -> POST /v1/apps/{ApplicationId}/messages, success 200 with MessageResponse; MessageResult.StatusMessage is "The status message for delivering the message" and StatusCode is "the downstream service status code" — the model does not state the text, so this is recorded behavior only. When a direct send names an address whose ChannelType is a configurable push channel that the application has no channel configured/enabled for, the service answers 200 overall with that destination's MessageResult carrying StatusCode 404, DeliveryStatus PERMANENT_FAILURE and StatusMessage "BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>." The fixed parts are "<CHANNEL> channel not found. Please enable the ", " channel for this application " and the terminating "."; the variable parts are the channel type (taken from the destination's ChannelType in the request, BAIDU here) appearing twice and the ApplicationId (taken from the request path).
Emulator: StatusCode 404 and DeliveryStatus PERMANENT_FAILURE are correct, but StatusMessage is "The BAIDU channel is not enabled for this application" — emulator-authored prose that drops the "channel not found" classification, drops the imperative "Please enable the <CHANNEL> channel" clause, omits the ApplicationId entirely and omits the trailing period.
Why: the per-destination StatusMessage is the only place a direct send reports WHY one destination failed, so clients surface it to operators and some match on it to distinguish "channel not configured" from the provider-level rejection that follows once the channel IS enabled. Against the emulator that text never matches, and it does not name the application the operator has to go enable the channel on.
Fix: the SendMessages handler's per-destination verdict for an unconfigured/disabled configurable channel (the Target::verdict branch that formats "The {channel} channel is not enabled for this application"), which needs the service's template including the ApplicationId.
Traces: pinpoint.push-channel-state-transition-changes-send-messages-outcome (failed at the pre-enable StatusMessage assertion). Because the check stops there, its post-enable half is UNVERIFIED and must also hold once Cause 2 is fixed: after UpdateBaiduChannel enables BAIDU, the same send must answer StatusCode 400 (not 404) with DeliveryStatus PERMANENT_FAILURE and a StatusMessage that is a JSON document whose errorMessage is "Invalid notification" and whose channelType is the destination's channel ("BAIDU") — i.e. enabling the channel must change the failure MODE, not just the wording. Re-replay this check to confirm the whole transition.

## Evidence
Suite: simparity-behavioral-aws-v1, service heading '### pinpoint' (1/6 passed). Both failures are verdict 'mismatch' — the emulator answered, with the wrong text.

Cause 1 — check pinpoint.delete-app-cascades-channel-configuration:
  AssertionError: unexpected GetApp message 'Application not found: 97435f8441d3f98b864befa3423112ab'
  Expected (AWS): HTTP 404, Error.Code 'NotFoundException', Error.Message 'Application not found' (entirely fixed text).
  Actual (emulator): HTTP 404, Error.Code 'NotFoundException', Error.Message 'Application not found: <ApplicationId from the request path>'.
  Code basis: services/service-com-amazonaws-pinpoint-pinpoint/src/ops/get_app.rs:161-168 builds NotFoundException with format!("Application not found: {}", input.application_id); compare services/service-com-amazonaws-pinpoint-pinpoint/src/ops/send_messages.rs:214-220, which uses the correct fixed "Application not found".
  Model basis: com.amazonaws.pinpoint#NotFoundException {Message: __string, RequestID: __string}, errorKind client, httpError 404; bound to GetApp (GET /v1/apps/{ApplicationId}).

Cause 2 — check pinpoint.push-channel-state-transition-changes-send-messages-outcome:
  AssertionError: unexpected pre-enable message 'The BAIDU channel is not enabled for this application'
  Expected (AWS), MessageResponse.Result[<address>] for a BAIDU address on an app with no BAIDU channel: StatusCode 404, DeliveryStatus 'PERMANENT_FAILURE', StatusMessage 'BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>.'
  Actual (emulator): StatusCode 404 (match), DeliveryStatus 'PERMANENT_FAILURE' (match), StatusMessage 'The BAIDU channel is not enabled for this application' (mismatch — no 'channel not found' classification, no 'Please enable' clause, no ApplicationId, no trailing period).
  Code basis: services/service-com-amazonaws-pinpoint-pinpoint/src/ops/send_messages.rs:542-548 (Target::verdict) formats "The {channel} channel is not enabled for this application".
  Model basis: com.amazonaws.pinpoint#MessageResult {DeliveryStatus (required), StatusCode (required), StatusMessage, MessageId, UpdatedToken}; SendMessages POST /v1/apps/{ApplicationId}/messages, success 200.

## Steps to Reproduce
Cause 1:
1. Boot the emulator from this checkout and run the behavioral suite (simparity-behavioral-aws-v1).
2. CreateApp with any name; note the returned ApplicationResponse.Id.
3. Call DeleteApp on that id (or simply use an id that was never created).
4. Call GetApp with that ApplicationId. The call is correctly rejected 404 NotFoundException, but Error.Message is 'Application not found: <that id>'; AWS answers Error.Message exactly 'Application not found'.
5. After fixing, also call GetBaiduChannel on the deleted app: AWS rejects it with NotFoundException and the generic Message 'Resource not found'.
   Re-replay check pinpoint.delete-app-cascades-channel-configuration.

Cause 2:
1. CreateApp with any name; note the ApplicationResponse.Id.
2. Without configuring any channel, call SendMessages on that app with MessageRequest.Addresses = {<some push token>: {ChannelType: 'BAIDU'}} and MessageConfiguration.BaiduMessage.Body set.
3. Read MessageResponse.Result[<token>]. StatusCode 404 and DeliveryStatus PERMANENT_FAILURE are right, but StatusMessage is 'The BAIDU channel is not enabled for this application'; AWS returns 'BAIDU channel not found. Please enable the BAIDU channel for this application <ApplicationId>.'
4. Then UpdateBaiduChannel(Enabled=true, ApiKey, SecretKey) and repeat the same send: AWS now answers StatusCode 400 with a JSON StatusMessage whose errorMessage is 'Invalid notification' and channelType is 'BAIDU'.
   Re-replay check pinpoint.push-channel-state-transition-changes-send-messages-outcome.
