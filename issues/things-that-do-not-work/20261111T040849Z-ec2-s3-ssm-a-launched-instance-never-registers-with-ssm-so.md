Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected
Issue-Key: ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected|_poll#6|TimeoutError|raise TimeoutError(f"{what or 'condition'}: the run budget ended first; not graded")
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# A launched instance never registers with SSM, so a VPC-endpoint policy deny can never be exercised from the guest

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
One net-new root cause.

Cause 1 of 1: the client instance the check launches never becomes an SSM managed instance, so the in-guest S3 write that the VPC endpoint policy is supposed to deny is never attempted.
AWS: recorded behavior only for the guest lifecycle (Smithy models DescribeInstanceInformation/SendCommand, not the boot). The recording shows the instance reaching running, the SSM agent registering so DescribeInstanceInformation reports PingStatus Online, and a command run in the guest whose S3 PutObject is refused by the gateway endpoint's policy even though the instance's IAM policy allows it.
Emulator: TimeoutError: 'the SSM agent on the client i-<id> coming Online: the run budget ended first; not graded' — the wait never succeeds and nothing further executes.
Why: a client cannot run anything inside an instance, so network-level controls (VPC endpoint policies) cannot be verified at all and a deny that AWS enforces is simply unobservable.
Fix: the EC2 instance lifecycle/guest execution path and its SSM registration — a running instance with an SSM instance profile must register as a managed instance and execute commands.
Traces: ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected.

## Evidence
Check ec2__s3__ssm.sep-network-truth-vpc-endpoint-policy-deny-blocks-an-iam-allowed-s3-write-rejected [error]:
TimeoutError: the SSM agent on the client i-<id> coming Online: the run budget ended first; not graded

Expected: DescribeInstanceInformation reports the instance Online, and the in-guest PutObject is refused by the VPC endpoint policy while IAM allows it.
Actual: the agent never comes Online; the guest never runs.

## Steps to Reproduce
1. Create a VPC, an S3 gateway endpoint with a policy denying the write, a bucket and an instance profile whose IAM policy allows the write and SSM.
2. RunInstances with that profile and wait for running.
3. Poll ssm DescribeInstanceInformation: the instance never reports PingStatus Online, so SendCommand has no target.
