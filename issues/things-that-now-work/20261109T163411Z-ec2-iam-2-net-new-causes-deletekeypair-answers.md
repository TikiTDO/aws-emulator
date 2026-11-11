Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle ec2__iam.request-spot-fleet-overrides-instance-type-and-requirements-conflict-rejected
Issue-Key: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle|phase#8|_PhaseError|raise _PhaseError(
Issue-Key: ec2__iam.request-spot-fleet-overrides-instance-type-and-requirements-conflict-rejected|run#21|AssertionError|assert [i["InstanceType"] for i in launched] == [_INSTANCE_TYPE], (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# ec2__iam: 2 net-new causes — DeleteKeyPair answers NotImplemented, and a requested spot fleet never launches its instances

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteKeyPair

## Description
Two distinct root causes in the EC2 surface.

Cause 1 of 2: DeleteKeyPair is an unimplemented scaffold stub.
AWS: DeleteKeyPair removes a key pair and answers 200 with Return true (and is idempotent for an already-absent name). Smithy: ec2#DeleteKeyPair takes KeyName or KeyPairId and returns DeleteKeyPairResult{Return, KeyPairId}; its errors are EC2's common query errors. `NotImplemented` is not one of them.
Emulator: DeleteKeyPair answers the non-AWS error code `NotImplemented` with message `operation DeleteKeyPair is scaffolded but not implemented in this emulator`, which fails even the cleanup phase of an otherwise complete instance lifecycle.
Why: a client cannot delete key material it created, so every provision/teardown cycle leaks key pairs, and the error code it receives is outside the operation's modelled error set so its SDK error handling cannot classify it.
Fix: implement the DeleteKeyPair handler against the EC2 key-pair state (the ec2 delete_key_pair operation handler).
Traces: ec2__iam.ec2ebs-e2e-deploy-ec2-instance-with-ebs-lifecycle.

Cause 2 of 2: an accepted RequestSpotFleet never produces instances.
AWS: a spot fleet request whose LaunchSpecifications carry an explicit InstanceType is fulfilled: DescribeSpotFleetInstances (and the fleet's ActiveInstances) report one instance of exactly that type once the fleet reaches fulfilled capacity. Smithy: ec2#DescribeSpotFleetInstances returns ActiveInstances[]{InstanceId, InstanceType, SpotInstanceRequestId} — recorded behavior only for fulfilment, which the recording shows producing exactly one t3-class instance.
Emulator: the fleet is accepted but its instance list stays empty, so the fulfilled-fleet assertion sees no instance of the requested type.
Why: a client that provisions capacity through Spot Fleet gets a request that reports as created but never yields compute; nothing downstream (overrides precedence, instance-requirements conflicts, capacity accounting) can be observed, and a client waiting for capacity waits forever.
Fix: the spot-fleet fulfilment path in the EC2 state machine — RequestSpotFleet must launch instances per LaunchSpecifications/Overrides and DescribeSpotFleetInstances must report them.
Traces: ec2__iam.request-spot-fleet-overrides-instance-type-and-requirements-conflict-rejected.

## Evidence
Cause 1 — Expected: DeleteKeyPair returns 200. Actual: An error occurred (NotImplemented) when calling the DeleteKeyPair operation: operation DeleteKeyPair is scaffolded but not implemented in this emulator (surfaced as [phase 5/6 cleanup] RuntimeError).
Cause 2 — Expected: one t3.micro instance in the fulfilled fleet. Actual: AssertionError: expected one t3.micro instance in the fulfilled fleet, got []

## Steps to Reproduce
Cause 1: CreateKeyPair, then DeleteKeyPair with that KeyName; observe NotImplemented.
Cause 2: RequestSpotFleet with LaunchSpecifications/Overrides naming InstanceType t3.micro and a valid IamFleetRole; poll DescribeSpotFleetInstances; observe ActiveInstances stays empty.
