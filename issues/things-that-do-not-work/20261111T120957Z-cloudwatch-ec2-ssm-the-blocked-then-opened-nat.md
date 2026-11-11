Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-carries-no-transfer-while-the-sink-ingress-is-blocked-then-counts-it-once-opened
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-carries-no-transfer-while-the-sink-ingress-is-blocked-then-counts-it-once-opened|_poll#2|Timeout|time.sleep(interval_s)
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# cloudwatch__ec2__ssm: the blocked-then-opened NAT-gateway transfer scenario never converges — the run is killed at its 600 s budget with no answer

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Cause 1 of 1: the NAT-gateway traffic-accounting scenario whose sink ingress is first blocked and then opened never reaches any of the states it waits on, and the run is terminated at its budget instead of some call answering or refusing.

AWS: recorded behavior only (metric emission and agent registration are not expressible in the Smithy model). The recording builds a VPC with public and private subnets, a NAT gateway routed from the private subnet, launches SSM-managed instances, waits for the SSM agent to report online, drives a known 32 MiB transfer over one TCP connection from the private guest with ssm SendCommand while the sink's ingress is blocked by a security group, then opens the ingress and repeats. While the ingress is blocked AWS/NATGateway BytesOutToDestination for the gateway stays below the block threshold (only the agent's own traffic and dropped SYNs); once the ingress is opened the same series accounts for the 32 MiB exactly once. Every stage — instance registration, each SendCommand invocation reaching a terminal InvocationStatus, and each metric read — answers inside the recording's own 600 s budget.

Emulator: the run produces no per-call divergence to quote: the harness kills it with 'Check timed out after 600.0s'. One of the awaited transitions (the SSM agent reporting online, a SendCommand invocation reaching a terminal status, or the AWS/NATGateway series appearing and then moving) never happens.

Why: a client waiting on these transitions blocks indefinitely rather than receiving an answer or a modeled error. It cannot distinguish 'not yet' from 'never' — there is no terminal status and no error code to branch on — so any automation that polls SSM command status or NAT-gateway metrics hangs instead of failing fast, and the security-group block/open contrast is unobservable.

Fix: the awaited state transitions on this path must terminate — SSM instance registration and command invocation must reach a terminal InvocationStatus, and the AWS/NATGateway metric publication must produce (or definitively not produce) its series — rather than leaving a poller with no answer. This is a non-convergence of the scenario's state machine, not a mis-serialized response: no status, code or message was returned to compare.

Traces: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-carries-no-transfer-while-the-sink-ingress-is-blocked-then-counts-it-once-opened.

Related open issue that does NOT carry this check's replay key and therefore does not cover it; a fixer should merge with it: 20261111T064629Z-cloudwatch-ec2-ssm-the-nat-gateway-instance-network-metric-scenario-never.md (same family, carries only the unrouted-gateway check).

## Evidence
Check cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-carries-no-transfer-while-the-sink-ingress-is-blocked-then-counts-it-once-opened [mismatch]:

Error: Check timed out after 600.0s

Expected: the SSM-managed instances come online, each SendCommand invocation reaches a terminal status, AWS/NATGateway BytesOutToDestination stays below the block threshold while the sink's ingress is blocked, and accounts for the 32 MiB exactly once after the ingress is opened.
Actual: no stage completes inside the 600 s budget; the run is killed with no HTTP status, error code or message produced by any single call.

## Steps to Reproduce
1. Create a VPC (10.74.0.0/16) with a public subnet, a private subnet, an internet gateway and a NAT gateway routed from the private subnet.
2. Launch a private t3.micro guest and a sink instance with an instance profile carrying AmazonSSMManagedInstanceCore; wait for ssm DescribeInstanceInformation to report them online.
3. With the sink's security group denying ingress on the sink port, ssm SendCommand to push 32 MiB over one TCP connection from the private guest and poll the invocation for a terminal status.
4. Read AWS/NATGateway BytesOutToDestination / ActiveConnectionCount for the gateway and assert it stays below the block threshold.
5. Authorize the sink ingress, repeat the 32 MiB transfer, and assert the series accounts for it once.
6. Against the emulator the sequence never completes; the run is terminated at the 600 s budget.
