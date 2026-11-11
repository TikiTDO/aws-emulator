Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-and-instance-metrics-count-guest-bytes-unrouted-gateway-counts-none
Issue-Key: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-and-instance-metrics-count-guest-bytes-unrouted-gateway-counts-none|_poll#2|Timeout|time.sleep(interval_s)
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# The NAT-gateway / instance network-metric scenario never converges: the run is killed at its 600 s budget with no answer (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Cause 1 of 1: the NAT-gateway traffic-accounting scenario never reaches the state it waits on, and the run is terminated at its budget instead of any call answering or refusing.

AWS: recorded behavior only. The recording builds a VPC with two public subnets, a private subnet routed through a NAT gateway and a second, unrouted gateway, launches SSM-managed instances with the AmazonSSMManagedInstanceCore profile, waits for the agent to report online, drives a known 32 MiB transfer over one TCP connection from the private guest through the NAT gateway with ssm SendCommand, then reads AWS/NATGateway (BytesOutToDestination/ActiveConnectionCount) and AWS/EC2 (NetworkOut/NetworkPacketsOut) for the instance; every stage — instance registration, command invocation reaching a terminal status, and the metric read — answers inside the recording's own 600 s budget, and the unrouted gateway's series stays at none.

Emulator: the run produces no per-call divergence to quote: the harness kills it with "Check timed out after 600.0s". One of the awaited transitions (the SSM agent reporting online for the instance, the SendCommand invocation reaching a terminal status, or the AWS/NATGateway and AWS/EC2 series appearing) never happens, so the scenario cannot complete.

Why: a client waiting on these transitions blocks forever rather than receiving an answer or a modelled error. A caller cannot distinguish "not yet" from "never": there is no terminal status and no error code to branch on, so any automation that polls SSM command status or NAT-gateway metrics hangs instead of failing fast.

Fix: the awaited state transitions in this path must terminate — SSM instance registration/command invocation must reach a terminal InvocationStatus, and the EC2/NAT-gateway metric publication must produce (or definitively not produce) its series — rather than leaving a poller without any answer. The failure is a non-convergence of this scenario's state machine, not a mis-serialized response: no status, code or message was returned to compare.

Traces: cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-and-instance-metrics-count-guest-bytes-unrouted-gateway-counts-none.

## Evidence
Check cloudwatch__ec2__ssm.obs-ec2-1-nat-gateway-and-instance-metrics-count-guest-bytes-unrouted-gateway-counts-none [mismatch]:

Error: Check timed out after 600.0s

Expected: the scenario completes — the SSM-managed instance comes online, the SendCommand invocation reaches a terminal status, and the AWS/NATGateway BytesOutToDestination / AWS/EC2 NetworkOut series account for the 32 MiB transfer while the unrouted gateway reports none.
Actual: no stage completes inside the 600 s budget; the run is killed with no HTTP status, error code or message produced by any single call.

## Steps to Reproduce
1. Create a VPC (10.73.0.0/16) with two public subnets, a private subnet, an internet gateway, a NAT gateway routed from the private subnet and a second NAT gateway left unrouted.
2. Launch instances with an instance profile carrying AmazonSSMManagedInstanceCore and wait for ssm DescribeInstanceInformation to report them online.
3. ssm SendCommand to push 32 MiB over one TCP connection from the private guest through the NAT gateway and poll the invocation for a terminal status.
4. Read AWS/NATGateway (BytesOutToDestination, ActiveConnectionCount) for both gateways and AWS/EC2 (NetworkOut, NetworkPacketsOut) for the instance.
5. The sequence never completes against the emulator; the run is terminated at the 600 s budget.
