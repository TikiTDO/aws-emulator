Issue-Source: trace-sweep-divergence
Issue-Trace: devicefarm-network-profile-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devicefarm-network-profile-lifecycle
Issue-Shape-Id: com.amazonaws.devicefarm#DeviceFarm_20150623
Issue-Key: devicefarm|GetNetworkProfile|2xx->2xx|$.networkProfile.name
Issue-Key: devicefarm|GetNetworkProfile|4xx->2xx|$.__type
Issue-Key: devicefarm|GetNetworkProfile|4xx->2xx|$.message
Issue-Key: devicefarm|GetNetworkProfile|4xx->2xx|status
Issue-Commit: 2de9945b1cbfa0dea7c3fabd80306f1dc4263384

# GetNetworkProfile serves stale network-profile state after UpdateNetworkProfile and DeleteNetworkProfile

> _Note: this issue was identified against the code base at commit `2de9945b1cbfa0dea7c3fabd80306f1dc4263384`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetNetworkProfile

## Description
GetNetworkProfile in the devicefarm emulator answers from a network-profile store that never reflects the mutations made earlier in the same resource lifecycle: a renamed profile is read back under its old name, and a profile that no longer exists is read back as if it were still there. Two causes, both observed in the devicefarm-network-profile-lifecycle fixture.

Cause 1 of 2: GetNetworkProfile returns the pre-update networkProfile.name (fixture devicefarm-network-profile-lifecycle, step task#5).
AWS: Smithy com.amazonaws.devicefarm#UpdateNetworkProfile takes arn (AmazonResourceName, required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$) plus the mutable members name (Name, @length 0..256), description, type, uplink/downlinkBandwidthBits, uplink/downlinkDelayMs, uplink/downlinkJitterMs and uplink/downlinkLossPercent (PercentInteger, @range 0..100, default 0), and returns the UPDATED NetworkProfile. GetNetworkProfile (output networkProfile: NetworkProfile) then reports that same updated resource: on AWS a successful UpdateNetworkProfile is durable, so a later GetNetworkProfile on the same ARN returns the name the update set. The recording shows exactly this: the profile is renamed, and the following GetNetworkProfile reports the new name (recorded value in this run: 'sim2real-probe-df-np-v2-86a66305'; the '-v2-' name is the value the update supplied, the suffix is a per-run generated token and is not a requirement).
Emulator: GetNetworkProfile returns 200 with $.networkProfile.name still set to the name CreateNetworkProfile persisted (recorded run: got 'sim2real-probe-df-np-523fae56', expected 'sim2real-probe-df-np-v2-86a66305'); every other member matches. The update never lands because UpdateNetworkProfile is an unimplemented scaffold that answers 501 (services/service-com-amazonaws-devicefarm-devicefarm-20150623/src/ops/update_network_profile.rs:179) and therefore mutates no state, while GetNetworkProfile keeps serving the create-time record (ops/get_network_profile.rs:160-164, store.network_profiles).
Why: a client that renames or re-tunes a network profile and then reads it back — the normal confirm-after-write pattern, and what CDK/CloudFormation-style drift checks do — sees the old name/bandwidth/latency values and concludes its update was lost or was applied to the wrong resource. Any test that asserts on the post-update profile fails against the emulator, and a test harness that reconfigures one profile between runs silently exercises the old network conditions.
Fix: UpdateNetworkProfile's handler must apply every supplied member onto the stored network-profile record in the service state store (the NetworkProfile record behind state.network_profiles) and return the updated resource, so GetNetworkProfile's read of that store reports it. This is the state/handler layer, not serialization: the serialized shape already matches.
Traces: devicefarm-network-profile-lifecycle#task#5 (GetNetworkProfile, 200 vs 200, $.networkProfile.name value_mismatch). Related file/line: src/ops/update_network_profile.rs:179 (scaffold 501), src/ops/get_network_profile.rs:160-164 (store read).

Cause 2 of 2: GetNetworkProfile returns 200 for a network profile that no longer exists instead of the modeled NotFoundException (fixture devicefarm-network-profile-lifecycle, step task#7).
AWS: Smithy com.amazonaws.devicefarm#GetNetworkProfile models NotFoundException (errorKind client, HTTP 400, doc 'The specified entity was not found.'), and DeleteNetworkProfile removes the profile named by arn (empty output). After a successful DeleteNetworkProfile, GetNetworkProfile on that ARN fails with HTTP 400 and awsJson error __type 'NotFoundException'. The recorded response body carries message 'No networkprofile found for arn <arn>' — the fixed text is 'No networkprofile found for arn ' (note: 'networkprofile' as one word, no 'was'), and the trailing ARN is the arn member of the request; the specific ARN in the recording (arn:aws:devicefarm:us-west-2:000000000000:networkprofile:0000000000000001-emulated/0000000000000001-emulated) is this run's account/ids filling that template, not part of the requirement.
Emulator: the step answers HTTP 200 with a networkProfile body; $.__type and $.message are absent (expected __type 'NotFoundException' and the message above). The profile is still in the store because DeleteNetworkProfile is an unimplemented scaffold answering 501 (src/ops/delete_network_profile.rs:152) and removes nothing, so GetNetworkProfile's lookup still hits (src/ops/get_network_profile.rs:161). Secondly, when GetNetworkProfile does miss, it words the error 'No network profile found for arn {arn}' (src/ops/get_network_profile.rs:167) — three words — which does not match AWS's fixed text 'No networkprofile found for arn '; so fixing deletion alone would turn this step into a message-wording mismatch of the same class as open issue 20261029T180559Z-devicefarm-notfoundexception-message-wording-diverges-from-aws-for.md (that issue covers GetProject only and does not own this step). Both must be corrected for this step to pass.
Why: a deleted resource that still reads back as live is a wrong-success: a client's cleanup verification passes while the profile persists, teardown loops never terminate (they poll for NotFoundException), and code branching on NotFoundException to decide 'recreate vs reuse' takes the wrong branch and reuses a profile the caller believes it destroyed. A client matching on the error message to distinguish a missing profile from a bad ARN also mis-classifies the failure when the wording differs.
Fix: DeleteNetworkProfile's handler must remove the record from state.network_profiles (and refuse an unknown ARN with NotFoundException itself), so GetNetworkProfile's existing not-found branch fires; and that branch's message template in src/ops/get_network_profile.rs must use AWS's fixed text 'No networkprofile found for arn ' followed by the request's arn. Handler/state plus the error-message construction in the same handler.
Traces: devicefarm-network-profile-lifecycle#task#7 (GetNetworkProfile, expected 400 NotFoundException, got 200). Related file/line: src/ops/delete_network_profile.rs:152 (scaffold 501), src/ops/get_network_profile.rs:165-170 (not-found branch and message).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up=true, 44 steps, 13 passed, 15 mismatched, 16 blocked, score 29.5%).

Cause 1 — fixture devicefarm-network-profile-lifecycle, step task#5, operation GetNetworkProfile, verdict mismatch, expected_status 200 / actual_status 200:
  detail: "$.networkProfile.name: value_mismatch (expected 'sim2real-probe-df-np-v2-86a66305', got 'sim2real-probe-df-np-523fae56')"
  divergence_key: service=devicefarm|GetNetworkProfile|status=200->200|keys=$.networkProfile.name
  Expected: the name set by the preceding UpdateNetworkProfile. Actual: the create-time name.

Cause 2 — fixture devicefarm-network-profile-lifecycle, step task#7, operation GetNetworkProfile, verdict mismatch, expected_status 400 / actual_status 200:
  detail: "status: expected 400, got 200\n$.__type: missing (expected 'NotFoundException', got None)\n$.message: missing (expected 'No networkprofile found for arn arn:aws:devicefarm:us-west-2:000000000000:networkprofile:0000000000000001-emulated/0000000000000001-emulated', got None)"
  divergence_key: service=devicefarm|GetNetworkProfile|status=400->400? (reported status=400->200)|keys=$.__type,$.message,status
  Expected: HTTP 400, __type NotFoundException, message fixed text 'No networkprofile found for arn ' + request arn. Actual: HTTP 200 with a live networkProfile body.

Model evidence (services/service-com-amazonaws-devicefarm-devicefarm-20150623/smithy-model/com-amazonaws-devicefarm-devicefarm-20150623.json):
  GetNetworkProfile: input arn (required, @length 32..1011, pattern ^arn:aws:devicefarm:.+$); output networkProfile: NetworkProfile; errors ArgumentException, LimitExceededException, NotFoundException, ServiceAccountException (all client, HTTP 400).
  UpdateNetworkProfile: input arn (required) + name (@length 0..256), description, type, uplink/downlinkBandwidthBits, uplink/downlinkDelayMs, uplink/downlinkJitterMs, uplink/downlinkLossPercent (@range 0..100); output networkProfile: NetworkProfile.
  DeleteNetworkProfile: input arn (required); empty output; same four modeled errors.
  NotFoundException doc: 'The specified entity was not found.'

Emulator source evidence: src/ops/update_network_profile.rs:179 and src/ops/delete_network_profile.rs:152 both return Scaffolded::not_implemented(...); src/ops/get_network_profile.rs:160-171 reads store.network_profiles and words its miss as 'No network profile found for arn {arn}'.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1); fixture devicefarm-network-profile-lifecycle drives CreateNetworkProfile -> GetNetworkProfile -> UpdateNetworkProfile -> GetNetworkProfile (task#5) -> DeleteNetworkProfile -> GetNetworkProfile (task#7).
3. Cause 1: at step task#5, call GetNetworkProfile with the ARN of a profile whose name was just changed by UpdateNetworkProfile. Observe 200 with $.networkProfile.name still equal to the CreateNetworkProfile name instead of the name the update supplied.
4. Cause 2: at step task#7, call DeleteNetworkProfile on that ARN, then GetNetworkProfile on the same ARN. Observe HTTP 200 with a networkProfile body instead of HTTP 400 with __type 'NotFoundException' and message 'No networkprofile found for arn <that arn>'. Note that even on a miss the emulator words the message 'No network profile found for arn <arn>' (three words), which still diverges from AWS's fixed text.
5. Both steps pass only when UpdateNetworkProfile persists its members, DeleteNetworkProfile removes the record, and GetNetworkProfile's NotFoundException message uses AWS's wording.
