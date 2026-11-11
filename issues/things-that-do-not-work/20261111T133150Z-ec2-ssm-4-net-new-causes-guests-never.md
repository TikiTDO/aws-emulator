Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides ec2__ssm.mtu-jumbo-frames-carry-an-unfragmented-payload-across-availability-zones-in-one-vpc ec2__ssm.mtu-path-mtu-collapses-to-1500-when-the-same-eni-sends-through-the-internet-gateway ec2__ssm.mtu-tcp-negotiates-a-segment-size-that-reveals-the-path-mtu-without-losing-a-byte ec2__ssm.run-instances-refuses-a-nonexistent-key-pair-and-launches-nothing ec2__ssm.ssh-reachability-derivation-selects-only-the-running-public-open-22-instance
Issue-Key: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful|_poll#5|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal|_poll#5|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides|_poll#5|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Key: ec2__ssm.mtu-jumbo-frames-carry-an-unfragmented-payload-across-availability-zones-in-one-vpc|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.mtu-path-mtu-collapses-to-1500-when-the-same-eni-sends-through-the-internet-gateway|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.mtu-tcp-negotiates-a-segment-size-that-reveals-the-path-mtu-without-losing-a-byte|_poll#5|Timeout|time.sleep(interval_s)
Issue-Key: ec2__ssm.run-instances-refuses-a-nonexistent-key-pair-and-launches-nothing|_client_error#12|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Result: {result!r}")
Issue-Key: ec2__ssm.ssh-reachability-derivation-selects-only-the-running-public-open-22-instance|run#27|AssertionError|assert _instances(ec2, [reachable])[reachable].get("PublicIpAddress") is None, (
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# ec2__ssm: 4 net-new causes — guests never come Online in SSM, MTU/path-MTU checks never complete, RunInstances accepts an absent key pair, and a stopped instance keeps its public IPv4

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Four net-new root causes reached through EC2+SSM. All four must be closed before submitting. The EC2 Smithy model declares no modeled errors for RunInstances or StopInstances (errors: []), so the AWS requirements below are recorded behavior.

Cause 1 of 4: a launched instance's SSM agent never reports PingStatus Online.
AWS: an instance launched from an SSM-enabled AMI with an instance profile granting AmazonSSMManagedInstanceCore and a path to the SSM endpoints — either its own internet gateway or the ssm/ssmmessages/ec2messages interface endpoints — registers with SSM, DescribeInstanceInformation reports PingStatus `Online`, and SendCommand then runs in the guest.
Emulator: the agent never comes Online, through an internet gateway or through interface endpoints.
Why: nothing can be observed from inside a guest, so the stateless-NACL/stateful-security-group distinction, source-group ingress admitting only members, and VPC peering carrying packets only with routes on both sides are all unverifiable; a client cannot validate its network behavior.
Fix: the instance boot path and its SSM registration — the agent's reachability to the SSM endpoints (including over interface VPC endpoints) and the instance-information state DescribeInstanceInformation reads.
Traces: ec2__ssm.nacls-are-stateless-but-security-groups-are-stateful, ec2__ssm.sep-network-truth-sg-source-group-rule-admits-members-only-and-drops-as-timeout-not-refusal, ec2__ssm.sep-network-truth-vpc-peering-carries-packets-only-with-routes-on-both-sides.

Cause 2 of 4: the MTU/path-MTU scenarios never reach their first observation.
AWS: two instances in one VPC exchange a jumbo (9001-byte MTU) unfragmented payload across Availability Zones; the same ENI sending through the internet gateway sees the path MTU collapse to 1500; and TCP negotiates an MSS that reveals the path MTU without losing a byte.
Emulator: each of the three checks ends in a harness timeout — no guest-level measurement is ever produced.
Why: a client cannot observe or rely on VPC MTU behavior; jumbo-frame and path-MTU assumptions are untestable against this emulator.
Fix: the EC2 guest data plane — the interface MTU presented inside the instance and the forwarding path through the internet gateway, together with whatever blocks the guest from becoming reachable in time.
Traces: ec2__ssm.mtu-jumbo-frames-carry-an-unfragmented-payload-across-availability-zones-in-one-vpc, ec2__ssm.mtu-path-mtu-collapses-to-1500-when-the-same-eni-sends-through-the-internet-gateway, ec2__ssm.mtu-tcp-negotiates-a-segment-size-that-reveals-the-path-mtu-without-losing-a-byte.

Cause 3 of 4: RunInstances does not refuse a KeyName that names no key pair.
AWS: a non-DryRun RunInstances naming an absent key pair is refused with code `InvalidKeyPair.NotFound` and the message "The key pair '<name>' does not exist" — fixed text with the requested key name interpolated — and nothing is launched.
Emulator: the call SUCCEEDS with HTTP 200 and an instance is created.
Why: a client that mistypes or deletes its key pair gets a running, billable instance it can never log into, instead of the synchronous refusal AWS guarantees.
Fix: RunInstances' KeyName resolution and validation — look the key pair up in the request's Region and raise InvalidKeyPair.NotFound before any instance is created.
Traces: ec2__ssm.run-instances-refuses-a-nonexistent-key-pair-and-launches-nothing.

Cause 4 of 4: stopping an instance does not release its public IPv4 address.
AWS: a non-Elastic public IPv4 address is released when the instance stops, so DescribeInstances reports no PublicIpAddress for a stopped instance (a new address is assigned on the next start).
Emulator: the stopped instance keeps its address; any retained address is the defect, not the particular value.
Why: a client deriving reachability (for example which hosts are SSH-reachable from the internet) counts a stopped, unreachable instance as publicly addressed, and code that re-reads the address after a start/stop cycle sees a stale value.
Fix: the StopInstances path and the instance's public-address state — release the non-Elastic public IPv4 on stop and omit PublicIpAddress from the describe output.
Traces: ec2__ssm.ssh-reachability-derivation-selects-only-the-running-public-open-22-instance.

## Evidence
Cause 1 — expected DescribeInstanceInformation PingStatus 'Online'. Actual: "TimeoutError: the SSM agent on the server i-00000000000000001 coming Online through the interface endpoints ['vpce-000000000000000a6', 'vpce-000000000000000aa', 'vpce-000000000000000ae']: the run budget ended first; not graded" and "TimeoutError: the SSM agent on the server i-00000000000000001 coming Online: the run budget ended first; not graded" (instance and endpoint ids are per-run values).

Cause 2 — expected a jumbo unfragmented transfer, a 1500-byte path MTU through the internet gateway, and a negotiated MSS matching the path MTU. Actual: all three checks report "Error: Check timed out after 60.0s".

Cause 3 — expected ('InvalidKeyPair.NotFound', "The key pair '<name>' does not exist") and no instance launched. Actual: HTTP 200 — "the call succeeded; real AWS rejects it. Result: {'ReservationId': 'r-00000000000000001', ... 'Instances': [...]}".

Cause 4 — expected PublicIpAddress absent after StopInstances. Actual: "stopping an instance RELEASES its public address — an emulator that keeps it would make the second conjunct pass on a machine that cannot be reached; got '54.0.0.2'".

## Steps to Reproduce
Replay the behavioral suite and re-run the eight checks listed in trace_ids. Individually:
1. Launch an SSM-enabled instance with an AmazonSSMManagedInstanceCore instance profile, once with an internet-gateway route and once in a private subnet with ssm/ssmmessages/ec2messages interface endpoints; poll DescribeInstanceInformation for PingStatus Online.
2. Launch two instances in one VPC in different AZs, set the interface MTU to 9001, and send an unfragmented payload between them and out through the internet gateway; read the MTU and MSS from the guest via SendCommand.
3. RunInstances(ImageId=<valid ami>, InstanceType="t3.micro", KeyName=<a name no key pair uses>, MinCount=1, MaxCount=1, SubnetId=<subnet>) and compare the result against ('InvalidKeyPair.NotFound', "The key pair '<name>' does not exist").
4. Launch an instance that receives a public IPv4, StopInstances, wait for state 'stopped', then DescribeInstances and read PublicIpAddress.
