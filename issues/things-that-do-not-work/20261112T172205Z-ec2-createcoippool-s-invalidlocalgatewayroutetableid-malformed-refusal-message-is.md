Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.create-coip-pool-invalid-route-table-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.create-coip-pool-invalid-route-table-rejected|run#13|AssertionError|assert message2 == expected2, f"expected message {expected2!r}, got {message2!r}"
Issue-Commit: e985d084470b6245f57d747d28e3fbc5bb80c4f3

# ec2: CreateCoipPool's InvalidLocalGatewayRouteTableID.Malformed refusal message is emulator-authored prose instead of AWS's wording

> _Note: this issue was identified against the code base at commit `e985d084470b6245f57d747d28e3fbc5bb80c4f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCoipPool

## Description
One net-new root cause in the ec2 service heading.

Cause 1 of 1: CreateCoipPool classifies a LocalGatewayRouteTableId that is not shaped like a local-gateway-route-table id correctly (error code InvalidLocalGatewayRouteTableID.Malformed) but serializes an emulator-authored message instead of AWS's message for that error class.

AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2) CreateCoipPool — input CreateCoipPoolRequest.LocalGatewayRouteTableId targets LocalGatewayRoutetableId and is required; the model declares no operation-specific errors, so the refusal is an EC2 query-protocol client error. Recorded behavior: HTTP 400 with Error/Code = InvalidLocalGatewayRouteTableID.Malformed and Error/Message = "The local-gateway-route-table ID '<value>' is malformed". The fixed text of that message is `The local-gateway-route-table ID '` + `' is malformed`; the only variable part is the rejected id, quoted verbatim from the request's LocalGatewayRouteTableId member. The sibling well-formed-but-absent case is already right in the emulator: code InvalidLocalGatewayRouteTableID.NotFound with message "The LocalGateway Route Table ID '<value>' does not exist", with the id again taken from the request — that half of the check passes, so only the malformed wording diverges.

Emulator: the code is right but the message reads `Invalid LocalGateway Route Table id totally-not-an-id` — a different sentence, a different capitalisation of the resource token, and the rejected id is NOT quoted. Replay: `AssertionError: expected message "The local-gateway-route-table ID 'totally-not-an-id' is malformed", got 'Invalid LocalGateway Route Table id totally-not-an-id'`.

Why: a client that surfaces, logs, matches or asserts on the refusal text for a malformed local-gateway-route-table id sees a message AWS never emits. Tooling that parses the quoted id out of the message (or that distinguishes the malformed class from the not-found class by its wording) misreads the refusal, and any human-facing diagnostic diverges from the AWS console/CLI text. The status and the error code are already correct, so the defect is confined to the message template.

Fix: the CreateCoipPool handler's validation/refusal path for LocalGatewayRouteTableId (services/service-com-amazonaws-ec2-amazonec2, create_coip_pool) and the shared local-gateway-route-table id malformed-refusal message template it uses — emit AWS's fixed text with the request's id interpolated and single-quoted, matching the pattern the already-correct NotFound branch of the same operation follows.

Traces: ec2.create-coip-pool-invalid-route-table-rejected (verdict: mismatch).

## Evidence
Failing check: ec2.create-coip-pool-invalid-route-table-rejected [mismatch]

Request: ec2 CreateCoipPool(LocalGatewayRouteTableId="totally-not-an-id")

Expected (AWS): HTTP 400, Error/Code = InvalidLocalGatewayRouteTableID.Malformed, Error/Message = "The local-gateway-route-table ID 'totally-not-an-id' is malformed" — fixed text "The local-gateway-route-table ID '" ... "' is malformed" with the rejected id from the request between the quotes.

Actual (emulator): correct code InvalidLocalGatewayRouteTableID.Malformed, but Error/Message = "Invalid LocalGateway Route Table id totally-not-an-id".

Replay trace:
  File "check_ec2_create_coip_pool_invalid_route_table_rejected.py", line 48, in run
    assert message2 == expected2, f"expected message {expected2!r}, got {message2!r}"
  AssertionError: expected message "The local-gateway-route-table ID 'totally-not-an-id' is malformed", got 'Invalid LocalGateway Route Table id totally-not-an-id'

The same check's earlier assertions pass: DescribeLocalGatewayRouteTables succeeds as the positive control, and CreateCoipPool with the well-formed but nonexistent id lgw-rtb-0123456789abcdef0 already answers InvalidLocalGatewayRouteTableID.NotFound with the exact message "The LocalGateway Route Table ID 'lgw-rtb-0123456789abcdef0' does not exist". No COIP pool is created by either rejected call.

## Steps to Reproduce
1. Call ec2 DescribeLocalGatewayRouteTables (positive control; it succeeds).
2. Record the baseline set of PoolId from ec2 DescribeCoipPools.
3. Call ec2 CreateCoipPool(LocalGatewayRouteTableId="lgw-rtb-0123456789abcdef0") — a well-formed id that cannot exist. Observe InvalidLocalGatewayRouteTableID.NotFound with "The LocalGateway Route Table ID 'lgw-rtb-0123456789abcdef0' does not exist" (already correct).
4. Call ec2 CreateCoipPool(LocalGatewayRouteTableId="totally-not-an-id") — an id not shaped like a local-gateway-route-table id.
5. Read Error/Code and Error/Message from the 400. The code is InvalidLocalGatewayRouteTableID.Malformed as AWS returns, but the message is "Invalid LocalGateway Route Table id totally-not-an-id" instead of "The local-gateway-route-table ID 'totally-not-an-id' is malformed".
6. Re-read ec2 DescribeCoipPools: no new pool (correct).

Replay key: behavioral check ec2.create-coip-pool-invalid-route-table-rejected
