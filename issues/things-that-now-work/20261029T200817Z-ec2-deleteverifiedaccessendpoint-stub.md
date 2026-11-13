Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-verified-access-delete-lifecycle
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteVerifiedAccessEndpoint
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DeleteVerifiedAccessEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVerifiedAccessEndpoint

## Description
`DeleteVerifiedAccessEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteVerifiedAccessEndpoint` call, so each one unblocks only when `DeleteVerifiedAccessEndpoint` works: `ec2-verified-access-delete-lifecycle#task#9`, `ec2-verified-access-delete-lifecycle#task#14`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_endpoint.rs`. Replaying the corpus, the emulator refused the `DeleteVerifiedAccessEndpoint` call: status: expected 200, got 501
$.DeleteVerifiedAccessEndpointResponse: missing (expected {'requestId': '349b6e7d-8b5e-4da3-88c6-b9261150c805', 'verifiedAccessEndpoint': {'attachmentType': 'vpc', 'cidrOptions': {'cidr': '10.70.1.0/24', 'portRangeSet': {'item': {'fromPort': '443', 'toPort': '443'}}, 'protocol': 'tcp', 'subnetIdSet': {'item': 'subnet-00000000000000029'}}, 'creationTime': '2026-10-29T19:59:32.371494Z', 'description': '', 'endpointDomain': '*.vae-00000000000000030.vai-0000000000000002d.prod.verified-access.us-west-2.amazonaws.com', 'endpointType': 'cidr', 'lastUpdatedTime': '2026-10-29T19:59:32.371494Z', 'securityGroupIdSet': {'item': 'sg-0000000000000002b'}, 'sseSpecification': {'customerManagedKeyEnabled': 'false'}, 'status': {'code': 'deleting'}, 'verifiedAccessEndpointId': 'vae-00000000000000030', 'verifiedAccessGroupId': 'vagr-0000000000000002f', 'verifiedAccessInstanceId': 'vai-0000000000000002d'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteVerifiedAccessEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
