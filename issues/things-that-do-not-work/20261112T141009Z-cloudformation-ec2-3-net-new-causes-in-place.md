Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found
Issue-Key: cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete|_expect#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete|_expect#21|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found|run#28|AssertionError|assert _outputs(stack) == _version_outputs(first_id, 1, 2), (
Issue-Commit: 1814b3996c94d6e0656759171876e93c26063e2d

# cloudformation__ec2: 3 net-new causes — in-place update of AWS::EC2::RouteTable is refused NotImplemented, a create-only SubnetRouteTableAssociation change is refused instead of replaced, and a launch-template update moves the default version

> _Note: this issue was identified against the code base at commit `1814b3996c94d6e0656759171876e93c26063e2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Three independent root causes in the CloudFormation EC2 resource handlers' update paths. All three checks are named by the open issue 20261111T111216Z-cloudformation-ec2-8-net-new-causes-in-cloudformation-provisioned-ec2.md, but none of these failures is the one that issue carries (it describes the route table being REPLACED by an in-place update, the association's create-then-delete ORDERING, and a launch-template update refused along with VPC/Subnet/IGW): each check now fails at a different point with a different cause, so that issue does not cover them.

Cause 1 of 3: an in-place update of AWS::EC2::RouteTable is refused with an emulator-authored NotImplemented reason.
AWS: AWS::EC2::RouteTable replaces only on VpcId; a change to its Tags only is an in-place update — the logical resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE keeping its rtb- PhysicalResourceId, DescribeRouteTables reports the new tag set (and still exactly the VPC-local route, no associations, no propagating VGWs), and the stack reaches UPDATE_COMPLETE. AWS has no NotImplemented error class for a change to a supported, mutable property. Recorded behavior only for the exact event wording.
Emulator: the resource is UPDATE_FAILED with reason 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::RouteTable in place; the resource was left as it is.' and the stack ends UPDATE_ROLLBACK_COMPLETE with StatusReason 'The following resource(s) failed to update: [RouteTable]. '. The fixed text is 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; the variable parts are the resource type (AWS::EC2::RouteTable) and, in the stack reason, the logical id taken from the template.
Why: a CloudFormation-managed route table cannot be re-tagged at all — a Name-tag edit rolls the whole stack back, so no estate containing a route table can be maintained.
Fix: the AWS::EC2::RouteTable resource handler's update path in the CloudFormation provisioner (apply mutable properties, keep the physical id).
Failing check: cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete.

Cause 2 of 3: a change to a create-only property of AWS::EC2::SubnetRouteTableAssociation is classified as an in-place update and refused, instead of forcing a replacement.
AWS: both properties of AWS::EC2::SubnetRouteTableAssociation (SubnetId, RouteTableId) are create-only, so changing SubnetId is a replacement. Because a subnet can hold only one route-table association, CloudFormation deletes the existing association before creating the replacement (so UpdateReplacePolicy: Retain keeps nothing), the resource gets a new rtbassoc- PhysicalResourceId, and the stack reaches UPDATE_COMPLETE; afterwards only the new subnet resolves to the stack's route table and the old subnet falls back to the VPC's main table. Recorded behavior only for the exact event wording.
Emulator: the resource is UPDATE_FAILED with reason 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::SubnetRouteTableAssociation in place; the resource was left as it is.' and the stack ends UPDATE_ROLLBACK_COMPLETE with StatusReason 'The following resource(s) failed to update: [Association]. ' — i.e. the update engine treated a create-only property change as an in-place property change and then refused it, so no replacement is ever attempted.
Why: re-pointing an association (the normal way to move a subnet between route tables) rolls the stack back, and the delete-before-create window that makes UpdateReplacePolicy: Retain a no-op for this type can never be observed.
Fix: the CloudFormation update engine's update classification for AWS::EC2::SubnetRouteTableAssociation (create-only property -> Replacement) and that handler's delete-before-create replacement path.
Failing check: cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete.

Cause 3 of 3: an in-place update of AWS::EC2::LaunchTemplate makes the newly created version the default version.
AWS: changing LaunchTemplateData on AWS::EC2::LaunchTemplate is an in-place update that creates a new launch-template version; the template's default version is NOT moved. The default version only changes through ModifyLaunchTemplate (Smithy, ec2: ModifyLaunchTemplate input member DefaultVersion, xmlName SetDefaultVersion), and CreateLaunchTemplateVersion has no default-version member at all (Smithy, ec2: CreateLaunchTemplateVersion input = DryRun, ClientToken, LaunchTemplateId, LaunchTemplateName, SourceVersion, VersionDescription, LaunchTemplateData (required), ResolveAlias). So after the first in-place update DescribeLaunchTemplates reports DefaultVersionNumber 1 and LatestVersionNumber 2, DescribeLaunchTemplateVersions reports version 1 with DefaultVersion true and version 2 with DefaultVersion false, and stack Outputs built from Fn::GetAtt <Lt>.DefaultVersionNumber / <Lt>.LatestVersionNumber read '1' and '2'.
Emulator: after the in-place update the stack Outputs are DefaultVersionNumber '2' and LatestVersionNumber '2' — the new version was also made the default (the LaunchTemplateId value in the same output map is a generated id and is not part of the requirement).
Why: anything launching from $Default — RunInstances with Version=$Default, an Auto Scaling group pinned to $Default — silently switches to the brand-new, unvalidated version the moment the stack updates, and a template whose rollback is supposed to leave version 1 serving cannot behave correctly.
Fix: the AWS::EC2::LaunchTemplate resource handler's update path (it must call CreateLaunchTemplateVersion only, never move the default) and EC2's launch-template default-version accounting.
Failing check: cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found.

Traces: the three failing check ids in trace_ids.

## Evidence
Cause 1 — cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete [mismatch]:
  AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [RouteTable]. '), expected 'UPDATE_COMPLETE'; failed events: [('RouteTable', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::RouteTable in place; the resource was left as it is.')]
  Template: one VPC + one AWS::EC2::RouteTable whose Properties are {VpcId: !Ref Vpc, Tags: <base tags> + [{Key: Name, Value: <first>}]}; the update changes only the Name tag value.
  Expected: UPDATE_COMPLETE, the same rtb- PhysicalResourceId, and DescribeRouteTables showing the new Tags with the VPC-local route only.

Cause 2 — cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete [mismatch]:
  AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Association]. '), expected 'UPDATE_COMPLETE'; failed events: [('Association', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::SubnetRouteTableAssociation in place; the resource was left as it is.')]
  Template: VPC + SubnetA + SubnetB + RouteTable + an AWS::EC2::SubnetRouteTableAssociation with UpdateReplacePolicy: Retain and Properties {SubnetId: !Ref SubnetA, RouteTableId: !Ref RouteTable}; the update swaps SubnetId to SubnetB.
  Expected: UPDATE_COMPLETE with the Association's events in the order UPDATE_IN_PROGRESS (old rtbassoc- id) -> DELETE_IN_PROGRESS/DELETE_COMPLETE (old id) -> CREATE of a new rtbassoc- id, a changed PhysicalResourceId, and DescribeRouteTables showing only SubnetB associated.

Cause 3 — cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found [mismatch]:
  AssertionError: outputs after the in-place update {'LaunchTemplateId': 'lt-0000000000000009e', 'DefaultVersionNumber': '2', 'LatestVersionNumber': '2'}
  Expected: {'LaunchTemplateId': <the same lt- id>, 'DefaultVersionNumber': '1', 'LatestVersionNumber': '2'}, and DescribeLaunchTemplates for that id reporting (DefaultVersionNumber, LatestVersionNumber) = (1, 2) with DescribeLaunchTemplateVersions showing version 1 DefaultVersion=true and version 2 DefaultVersion=false.
  Setup: a stack whose AWS::EC2::LaunchTemplate declares LaunchTemplateData (ImageId, InstanceType, UserData); the update changes only InstanceType, which is an in-place update producing version 2.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout.
2. CreateStack: a VPC plus an AWS::EC2::RouteTable with Tags including {Key: Name, Value: first}; wait for CREATE_COMPLETE.
3. UpdateStack with the same template but Name=second; poll DescribeStacks and DescribeStackEvents.
   Observed: RouteTable UPDATE_FAILED with the NotImplemented reason above and the stack UPDATE_ROLLBACK_COMPLETE.
   Expected: UPDATE_COMPLETE, same rtb- physical id, DescribeRouteTables showing the new tags.

Cause 2:
1. CreateStack: VPC + two subnets + a route table + an AWS::EC2::SubnetRouteTableAssociation (UpdateReplacePolicy: Retain) on the first subnet; wait for CREATE_COMPLETE.
2. UpdateStack with SubnetId pointing at the second subnet; poll DescribeStacks and DescribeStackEvents.
   Observed: Association UPDATE_FAILED with the NotImplemented reason above and the stack UPDATE_ROLLBACK_COMPLETE.
   Expected: UPDATE_COMPLETE, the old association deleted before the new one is created, a new rtbassoc- physical id, and only the second subnet associated in DescribeRouteTables.

Cause 3:
1. CreateStack: an AWS::EC2::LaunchTemplate with LaunchTemplateData {ImageId, InstanceType: t3.micro, UserData}, with Outputs for Fn::GetAtt Lt.LaunchTemplateId / Lt.DefaultVersionNumber / Lt.LatestVersionNumber; wait for CREATE_COMPLETE (outputs 1 and 1).
2. UpdateStack changing only InstanceType; wait for UPDATE_COMPLETE, then read DescribeStacks Outputs and `aws ec2 describe-launch-templates --launch-template-ids <id>` and `aws ec2 describe-launch-template-versions --launch-template-id <id>`.
   Observed: DefaultVersionNumber 2, LatestVersionNumber 2 (version 2 is the default).
   Expected: DefaultVersionNumber 1, LatestVersionNumber 2, with version 1 still DefaultVersion=true.
3. Replay the three checks in trace_ids to confirm the fix.
