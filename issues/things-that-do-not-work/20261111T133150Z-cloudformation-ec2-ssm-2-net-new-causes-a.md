Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive
Issue-Key: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive|_deploy_cfn#3|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative|_deploy_cfn#4|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive|run#23|AssertionError|assert del_status in (None, "DELETE_COMPLETE"), f"stack did not delete cleanly: {del_status}"
Issue-Commit: ffc9519ba35ae474158cb1300960bfc0fdc8c7a4

# cloudformation__ec2__ssm: 2 net-new causes — a stack declaring AWS::SSM::PatchBaseline rolls back, and deleting an SSM::Association stack ends DELETE_FAILED

> _Note: this issue was identified against the code base at commit `ffc9519ba35ae474158cb1300960bfc0fdc8c7a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in CloudFormation's handling of SSM resources over an EC2 estate. Both must be closed before submitting.

Cause 1 of 2: a stack whose Resources include AWS::SSM::PatchBaseline (together with AWS::EC2::Instance, AWS::IAM::Role/InstanceProfile and AWS::SSM::Association naming AWS-RunPatchBaseline) never reaches CREATE_COMPLETE.
AWS: recorded behavior only — CreateStack accepts the template and the stack reaches CREATE_COMPLETE, with the patch baseline readable through SSM and the association bound to the tagged instance. AWS::SSM::PatchBaseline is a supported CloudFormation resource type, so the create succeeds and no rollback occurs.
Emulator: the stack reaches ROLLBACK_COMPLETE. Both failing checks deploy a template containing AWS::SSM::PatchBaseline and both roll back, so neither the positive deployment nor the negative tag-filter assertion can ever be reached.
Why: a client cannot provision a patch-baseline estate at all; every template that contains AWS::SSM::PatchBaseline fails, and the failure is reported as a resource failure rather than as an unsupported type, so the client has nothing to act on.
Fix: the CloudFormation resource provider for AWS::SSM::PatchBaseline (its create handler and the SSM state it writes), plus the failure reason the stack event carries so the refusal is attributable.
Traces: cloudformation__ec2__ssm.cfn-deploy-ssm-patch-baseline-associated-ec2-positive, cloudformation__ec2__ssm.cfn-ssm-patch-baseline-wrong-tag-filter-negative.

Cause 2 of 2: DeleteStack on a CREATE_COMPLETE stack holding a VPC, internet gateway, subnet, route table, security group, IAM role/instance profile, EC2 instance and an AWS::SSM::Association ends DELETE_FAILED.
AWS: recorded behavior only — DeleteStack removes the resources in reverse dependency order and the stack reaches DELETE_COMPLETE (after which DescribeStacks by name no longer finds it); the SSM association is deleted with the stack.
Emulator: the stack reaches DELETE_FAILED.
Why: the stack and every resource under it are left stranded; a client's teardown never completes, repeat deployments collide on names, and the SSM association survives the stack that owns it.
Fix: CloudFormation's delete path for these resource types — the dependency ordering and the per-resource delete handlers (notably AWS::SSM::Association and the EC2 network resources it depends on) so the deletion drains instead of failing.
Traces: cloudformation__ec2__ssm.cli-script-cfn-stack-delete-removes-ssm-association-positive.

## Evidence
Cause 1 — expected StackStatus CREATE_COMPLETE. Actual: ROLLBACK_COMPLETE. Replay details: "AssertionError: Stack sim2real-xsvc-stk-36db159eb686 ended in ROLLBACK_COMPLETE" and "AssertionError: Stack sim2real-xsvc-stk-76c8fd9ff13e ended in ROLLBACK_COMPLETE" (the stack names are per-run values). Both templates declare "PatchBaseline": {"Type": "AWS::SSM::PatchBaseline"} plus "Type": "AWS::SSM::Association" with Name "AWS-RunPatchBaseline".

Cause 2 — expected StackStatus DELETE_COMPLETE (or the stack gone). Actual: DELETE_FAILED. Replay detail: "AssertionError: stack did not delete cleanly: DELETE_FAILED". The stack's resource types are AWS::EC2::VPC, InternetGateway, VPCGatewayAttachment, Subnet, RouteTable, Route, SubnetRouteTableAssociation, SecurityGroup, Instance, AWS::IAM::Role, AWS::IAM::InstanceProfile and AWS::SSM::Association.

## Steps to Reproduce
Replay the behavioral suite and re-run the three checks listed in trace_ids.

Cause 1: CreateStack with a template containing an AWS::IAM::Role + AWS::IAM::InstanceProfile, an AWS::EC2::Instance using it, an AWS::SSM::PatchBaseline, and an AWS::SSM::Association naming AWS-RunPatchBaseline targeted by tag. Poll DescribeStacks; observe ROLLBACK_COMPLETE and read DescribeStackEvents for the failing resource and its ResourceStatusReason.

Cause 2: CreateStack with a VPC/IGW/subnet/route-table/security-group/instance-profile/instance template plus one AWS::SSM::Association, wait for CREATE_COMPLETE, then DeleteStack and poll DescribeStacks. Observe DELETE_FAILED instead of DELETE_COMPLETE.
