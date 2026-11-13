Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rtbfabric.create-requester-gateway-invalid-vpc-pattern-rejected
Issue-Shape-Id: com.amazonaws.rtbfabric#RTBFabric
Issue-Key: rtbfabric.create-requester-gateway-invalid-vpc-pattern-rejected|run#9|AssertionError|assert error["Message"] == expected_message, f"expected {expected_message!r}, got {error['Message']!r}"
Issue-Commit: c42734c699ac346ade2bea5e1d020e4f24cb85eb

# rtbfabric @pattern ValidationException message diverges: emulator echoes the submitted value and leaves the regex anchors in, where AWS omits the value and strips ^/$

> _Note: this issue was identified against the code base at commit `c42734c699ac346ade2bea5e1d020e4f24cb85eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRequesterGateway

## Description
Cause 1 of 1: the constraint-validation message renderer that produces rtbfabric's `1 validation error detected: ...` envelope for @pattern violations emits a message real AWS never emits. It diverges from AWS in TWO ways in the same template, and both must be corrected for the check to pass.

RELATED, NOT COVERING: the open issue 20261029T191824Z-rtbfabric-5-net-new-causes-associatecertificate-getlinkroutingrule-listtagsforresource.md records the same `Value '<submitted>' at` echo for @range violations (its Cause 4, ListRequesterGateways.maxResults). That issue names only the @range form and only that member; it does NOT name the @pattern form, nor the regex-anchor divergence described here, which is specific to the pattern-constraint message and has no analogue in the @range envelope. This is a net-new cause.

AWS: Smithy (services/service-com-amazonaws-rtbfabric-rtbfabric/smithy-model/com-amazonaws-rtbfabric-rtbfabric.json) models CreateRequesterGateway as POST /requester-gateway; its input member `vpcId` targets shape `VpcId`, is @required, @length min 12 max 21, and carries @pattern `^vpc-[a-f0-9]{8,17}$`. The only modeled client error for a malformed input is `ValidationException` (HTTP 400). Recorded behavior (real AWS) for a vpcId that fails that pattern is HTTP 400, code `ValidationException`, message:

  `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`

Fixed text of that template: `1 validation error detected: Value at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. Variable parts: `<member>` is the offending input member's name, taken from the request binding (here `vpcId`); `<pattern>` is the member's @pattern from the model with the leading `^` and trailing `$` anchors REMOVED (here `vpc-[a-f0-9]{8,17}`, not `^vpc-[a-f0-9]{8,17}$`). The submitted value is NOT part of the message at all — AWS does not echo the caller-supplied string for rtbfabric's constraint rejections.

Emulator: the refusal has the right status (400) and the right code (`ValidationException`), but the message is `1 validation error detected: Value '<submitted>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern-with-anchors>`. It (a) inserts `Value '<submitted>' at` where AWS writes `Value at`, and (b) renders the model's @pattern verbatim including the `^` and `$` anchors, where AWS strips them.

Why: a client or fixture that asserts on the rejection text — which is the only thing distinguishing which constraint was violated, since the code is the same `ValidationException` for every input constraint on this service — mismatches on EVERY @pattern violation of every rtbfabric operation (vpcId, gatewayId, linkId, ruleId, resourceArn, description, domainName all carry @pattern). The anchor leak also exposes a regex that is not the one AWS advertises, so a client that parses the advertised pattern out of the message to build its own client-side check derives a different expression. Separately, the emulator echoes a caller-supplied value back in an error message where AWS deliberately does not.

NOTE on unverified downstream assertions: this check asserts the message before its remaining assertions, so everything after it was never reached and remains unverified — that the refused CreateRequesterGateway left no new gateway id visible through ListRequesterGateways, that the same call with the well-formed vpcId is ACCEPTED and returns status PENDING_CREATION or ACTIVE, that GetRequesterGateway reads back the persisted vpcId, and that the accepted gateway settles ACTIVE. Those must all hold once the message is corrected.

Fix: the shared pre-handler constraint-validation layer that renders the `1 validation error detected` envelope for @pattern violations (the centralized deserialize → validate → handle walk, NOT the CreateRequesterGateway handler — the refusal fires before handle() runs; see services/service-com-amazonaws-rtbfabric-rtbfabric/src/ops/create_requester_gateway.rs lines 164-272, whose body is never entered on this path). Drop the `Value '<submitted>' at` interpolation in favour of `Value at`, and strip a leading `^` and trailing `$` from the pattern before interpolating it.

Traces: rtbfabric.create-requester-gateway-invalid-vpc-pattern-rejected.

## Evidence
Behavioral suite: rtbfabric 2/10 (20%), 8 failed. This check is the only failure not already filed or owned by an open issue.

[mismatch] rtbfabric.create-requester-gateway-invalid-vpc-pattern-rejected
  check_rtbfabric_create_requester_gateway_invalid_vpc_pattern_rejected.py line 42:
    assert error["Message"] == expected_message
  AssertionError: expected "1 validation error detected: Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: vpc-[a-f0-9]{8,17}",
             got "1 validation error detected: Value 'not-a-valid-vpc-id' at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^vpc-[a-f0-9]{8,17}$"

Status and code agree (HTTP 400 / ValidationException — the check's earlier assertion on error["Code"] passed); only the message form diverges, in two places:
  1. `Value 'not-a-valid-vpc-id' at ` vs AWS's `Value at ` — the submitted value is not part of AWS's message. (`not-a-valid-vpc-id` is just the string this check happens to send; the requirement is that NO submitted value is interpolated, for any member.)
  2. `^vpc-[a-f0-9]{8,17}$` vs AWS's `vpc-[a-f0-9]{8,17}` — AWS strips the anchors from the model's @pattern `^vpc-[a-f0-9]{8,17}$`.

Model evidence (smithy_query operation=CreateRequesterGateway): input.vpcId -> {target: VpcId, required: true, length: {min: 12, max: 21}, pattern: "^vpc-[a-f0-9]{8,17}$"}; errors include ValidationException (httpError 400); http {method: POST, uri: /requester-gateway}.

## Steps to Reproduce
Boot the emulator and point a botocore client for service 'rtbfabric' at its endpoint (parameter validation disabled client-side so the malformed value reaches the wire).

1. Create a VPC, a subnet in it and a security group (EC2), and ensure the rtbfabric service-linked role exists.
2. Call CreateRequesterGateway(vpcId='not-a-valid-vpc-id', subnetIds=[<subnet>], securityGroupIds=[<sg>], clientToken=<uuid>) — any string failing ^vpc-[a-f0-9]{8,17}$ will do.
3. Expect ClientError with HTTP 400, Code 'ValidationException' and Message exactly:
     1 validation error detected: Value at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: vpc-[a-f0-9]{8,17}
   The emulator instead returns:
     1 validation error detected: Value 'not-a-valid-vpc-id' at 'vpcId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^vpc-[a-f0-9]{8,17}$
4. Repeat with any other @pattern-constrained rtbfabric member (e.g. GetRequesterGateway(gatewayId='not-a-gateway-id'), or CreateRequesterGateway(description='\u0000bad')) to confirm the renderer is shared and both deviations reproduce there too.
5. Then re-run the whole check: after the message is corrected it must also pass its remaining assertions — ListRequesterGateways shows no new gateway id from the refused call, the same call with the real vpcId succeeds with status PENDING_CREATION/ACTIVE, GetRequesterGateway reads back that vpcId, and the gateway settles ACTIVE.
