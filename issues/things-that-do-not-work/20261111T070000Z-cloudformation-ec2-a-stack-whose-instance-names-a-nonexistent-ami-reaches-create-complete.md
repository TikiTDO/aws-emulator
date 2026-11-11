Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative
Issue-Key: cloudformation__ec2.deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order|run#8|AssertionError|assert stack["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__ec2__rds.cfn-rds-db-subnet-group-single-az-rejected-negative|run#8|AssertionError|assert failed["status"] == "ROLLBACK_COMPLETE", (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudFormation: a stack whose instance names a nonexistent AMI still reaches CREATE_COMPLETE, and a failure an owning service refused is reported in CloudFormation's own words rather than that service's

> _Note: this cause was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 2 of 5 of `20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md`. Cause 3 of that issue (a CFN-created resource's physical id must be the identifier the owning service issued) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so its two remaining checks are not recorded as working. Their two `Issue-Key` lines and two `Issue-Trace-Ids` moved with it. The third check of the original cause, `cloudformation__ec2.deploy-ec2-1-overlapping-subnet-cidr-rejected-and-rolled-back`, passes now and is not carried here._

## Operation
CreateStack (resource provisioning and the rollback path behind it), RunInstances

## Description
AWS: when a resource creation fails, CloudFormation moves the stack `CREATE_FAILED` -> `ROLLBACK_IN_PROGRESS` -> `ROLLBACK_COMPLETE` and deletes the resources it created, in reverse dependency order; the failed resource's event quotes the OWNING service's own refusal.

Emulator: the rollback machinery itself works (a stack declaring two overlapping subnet CIDRs now rolls back, because Amazon EC2 refuses the second subnet). What is left is the two halves below.

1. `deploy-ec2-1-bad-ami-instance-rolls-back-the-network-in-reverse-order`: a stack declaring an `AWS::EC2::Instance` whose `ImageId` names no image reaches `CREATE_COMPLETE`. The root cause is in Amazon EC2, not in CloudFormation: `services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs` refuses a MALFORMED image id and an unresolvable `resolve:ssm:` alias, but accepts a well-formed `ami-<17 hex>` id the account can see no image for, where AWS answers `InvalidAMIID.NotFound` — "The image id '[<id>]' does not exist". That crate already has the readings the fix needs (`crate::state::require_visible_image`, `account_can_see_image`, `invalid_ami_id_not_found`).

   THE BLOCKER for whoever picks this up: making `RunInstances` apply `require_visible_image` is a few lines, but about forty existing suites under `services/service-com-amazonaws-ec2-amazonec2/tests/all/` launch instances with ids no catalogue image carries (`ami-0123456789abcdef0` and friends — see `instance_lifecycle.rs`, `attach_volume.rs`, `run_instances_network_interfaces.rs`, `instance_detail.rs`, whose own constant is named `UNKNOWN_AMI`). Every one of them has to launch from an image the account can really see (`DescribeImages`, or the published `resolve:ssm:` alias the CloudFormation suite `cloudformation_ec2_instance_rollback.rs` uses) before the refusal can land. That is the work; it is a single rule, applied to one operation, plus the test corpus it invalidates.

2. `cfn-rds-db-subnet-group-single-az-rejected-negative`: the stack DOES roll back now, and the remaining divergence is the WORDING of the failure. The check requires the stack event to attribute the failure to Amazon RDS — `"(Service: Rds" in reasons` — where the emulator reports `Resource handler returned message: "The DB subnet group doesn't meet Availability Zone (AZ) coverage requirement. Current AZ coverage: us-west-2a. Add subnets to cover at least 2 AZs." (...)`: the sentence is right but it carries no `(Service: Rds, Status Code: ..., Request ID: ...)` suffix, which is how the AWS SDK renders a service exception quoted inside a resource-handler message. So what is missing is that an in-process refusal answered by an owning service be quoted the way that service's own SDK raises it, in `crate::state::provisioning::in_process_calls::refusal_of` and the per-type handler-refusal wording beside it.

Why: a client cannot distinguish a good deployment from a broken one, and a negative-path infrastructure test passes against an invalid template.

## Evidence
```
AssertionError: a stack whose instance names a nonexistent AMI ended 'CREATE_COMPLETE' (None), expected 'ROLLBACK_COMPLETE'
```

```
AssertionError: the stack event must attribute the failure to RDS, not to CloudFormation's own validation,
got ' Resource handler returned message: "The DB subnet group doesn'\''t meet Availability Zone (AZ) coverage requirement. ...' 
```

## Steps to Reproduce
1. `CreateStack` with a template declaring an `AWS::EC2::Instance` whose `ImageId` is a well-formed `ami-` id no image carries, and poll `StackStatus`: it reaches `CREATE_COMPLETE` where `ROLLBACK_COMPLETE` is due.
2. `CreateStack` with an `AWS::RDS::DBSubnetGroup` whose subnets are all in one Availability Zone, poll to `ROLLBACK_COMPLETE` and read `DescribeStackEvents`: the failed resource's `ResourceStatusReason` carries no `(Service: Rds` attribution.
