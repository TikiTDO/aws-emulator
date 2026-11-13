Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dms.subnet-group-requires-two-azs
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms.subnet-group-requires-two-azs|run#17|AssertionError|assert message == (
Issue-Commit: 7846fec8a86647e77da10e6415de80579a2805a4

# DMS CreateReplicationSubnetGroup AZ-coverage refusal carries emulator-authored wording instead of DMS's ReplicationSubnetGroupDoesNotCoverEnoughAZs message

> _Note: this issue was identified against the code base at commit `7846fec8a86647e77da10e6415de80579a2805a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationSubnetGroup

## Description
One net-new root cause in database-migration-service.

Cause 1 of 1: the message on the ReplicationSubnetGroupDoesNotCoverEnoughAZs refusal is not the text DMS serves for that error class. Failing check: database-migration-service.subnet-group-requires-two-azs.

AWS: CreateReplicationSubnetGroup refuses a subnet list that does not span the minimum number of Availability Zones. Smithy (services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json): CreateReplicationSubnetGroup takes required ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription and SubnetIds (SubnetIdentifierList, a plain string list with no modeled minimum) and models the error ReplicationSubnetGroupDoesNotCoverEnoughAZs (errorKind client, HTTP 400, "The replication subnet group does not cover enough Availability Zones (AZs). Edit the replication subnet group and add more AZs."); the operation's own documentation states "The VPC needs to have at least one subnet in at least two availability zones in the Amazon Web Services Region, otherwise the service will throw a ReplicationSubnetGroupDoesNotCoverEnoughAZs exception." The model does not carry the message template, so the wording is recorded behavior only; the recording shows DMS answers HTTP 400, error code ReplicationSubnetGroupDoesNotCoverEnoughAZs, with this message:

  "Replication Subnet Group doesn't meet availability zone coverage requirement. Please create a subnet group containing subnets to cover at least <min> availability zones using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group. Current AZ coverage: <covered>. Valid availability zones are: [<zone>, <zone>, ...]"

Its FIXED text is: "Replication Subnet Group doesn't meet availability zone coverage requirement. Please create a subnet group containing subnets to cover at least ", " availability zones using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group. Current AZ coverage: ", ". Valid availability zones are: [", "]". Its VARIABLE parts are: <min>, the AZ minimum the operation documents; <covered>, the count of DISTINCT Availability Zones the subnets named by THIS request sit in; and <zone>, ... , the available, opt-in-not-required zones of the request's Region, comma-and-space separated inside square brackets. The concrete zone names and counts in any one recording fill that template and are not part of it.

Emulator: the status (400) and the error code (ReplicationSubnetGroupDoesNotCoverEnoughAZs) are right, but the message's second sentence is emulator-authored. It serves "Replication Subnet Group doesn't meet availability zone coverage requirement. Please add subnets to cover at least <min> availability zones. Current AZ coverage: <covered>. Valid availability zones are: [<zone>, ...]" — i.e. the fixed clause "Please create a subnet group containing subnets to cover at least" is replaced with "Please add subnets to cover at least", and the fixed clause " using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group" is dropped entirely. The first and last sentences, the minimum, the coverage count and the zone list are all correct; only that middle fixed text diverges.

Why: a client that matches, logs or surfaces the service's remediation text gets a message that omits the remediation AWS gives (create or modify the subnet group via the console or the create-replication-subnet-group / modify-replication-subnet-group CLI commands). Operators and any test, runbook or support tooling that asserts on the refusal text see a message the real service never emits, so a correctly-classified refusal still fails verification against AWS.

Fix: the message construction in this operation's validation/refusal path — the helper that builds the ReplicationSubnetGroupDoesNotCoverEnoughAZs refusal, which is the single place both the empty-SubnetIds branch and the distinct-zone-count branch route through. Only the fixed text of the second sentence needs to become DMS's; the interpolated minimum, coverage count and Region zone list already match.

Traces: database-migration-service.subnet-group-requires-two-azs (mismatch; assertion at check line 99 compares the refusal message). Handler/line citations: services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_replication_subnet_group.rs lines 345-363 (`not_enough_zones`, message format string at lines 353-359), reached from lines 210-212 (empty list) and lines 270-272 (insufficient distinct zones). The check's later assertions — the ResourceNotFoundFault wording for deleting the refused group, and the positive two-zone control — were never reached, so this issue claims nothing about them.

## Evidence
Check database-migration-service.subnet-group-requires-two-azs, verdict: mismatch.

The check creates a VPC and ONE subnet in a single AZ, then calls CreateReplicationSubnetGroup with that single subnet. The error CODE assertion passes (ReplicationSubnetGroupDoesNotCoverEnoughAZs, HTTP 400); the MESSAGE assertion fails.

Expected (AWS, as the recording words it, with this run's values filling the template):
  "Replication Subnet Group doesn't meet availability zone coverage requirement. Please create a subnet group containing subnets to cover at least 2 availability zones using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group. Current AZ coverage: 1. Valid availability zones are: [us-west-2a, us-west-2b, us-west-2c]"

Actual (emulator):
  "Replication Subnet Group doesn't meet availability zone coverage requirement. Please add subnets to cover at least 2 availability zones. Current AZ coverage: 1. Valid availability zones are: [us-west-2a, us-west-2b, us-west-2c]"

Diff, in fixed text only:
  - "Please create a subnet group containing subnets to cover at least" -> "Please add subnets to cover at least"
  - " using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group" -> dropped

Stack excerpt:
  File "check_database_migration_service_subnet_group_requires_two_azs.py", line 99, in run
    assert message == (
  AssertionError: Replication Subnet Group doesn't meet availability zone coverage requirement. Please add subnets to cover at least 2 availability zones. Current AZ coverage: 1. Valid availability zones are: [us-west-2a, us-west-2b, us-west-2c]

Source of the emulator wording (services/.../src/ops/create_replication_subnet_group.rs:353-359):
  "Replication Subnet Group doesn't meet availability zone coverage requirement. Please add subnets to cover at least {} availability zones. Current AZ coverage: {covered}. Valid availability zones are: [{}]"

The zone names, the minimum 2 and the coverage 1 shown above are this run's values; the requirement is the template, not these literals.

## Steps to Reproduce
1. Create a VPC and exactly ONE subnet in it, in a single Availability Zone (ec2 CreateVpc, ec2 CreateSubnet with an explicit AvailabilityZone).
2. Call dms CreateReplicationSubnetGroup with ReplicationSubnetGroupIdentifier=<any valid name>, ReplicationSubnetGroupDescription=<any text>, SubnetIds=[<that one subnet id>].
3. Observe the refusal: HTTP 400 with code ReplicationSubnetGroupDoesNotCoverEnoughAZs (correct).
4. Read Error.Message. The emulator's second sentence is "Please add subnets to cover at least 2 availability zones." AWS's is "Please create a subnet group containing subnets to cover at least 2 availability zones using the DMS console or CLI commands create-replication-subnet-group or modify-replication-subnet-group."
5. The same divergent message is served for an EMPTY SubnetIds list, which routes through the same refusal helper.
