Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2.cfn-ec2-vpc-create-update-in-place-then-delete cloudformation__ec2.cfn-ec2-subnet-create-update-in-place-replace-retained-then-delete cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found cloudformation__ec2.cfn-ec2-internet-gateway-tag-update-in-place-attached-then-delete cloudformation__ec2.cfn-ec2-vpc-gateway-attachment-swap-gateway-in-place-then-delete cloudformation__ec2.cfn-ec2-route-ref-is-the-route-table-id-and-destination cloudformation__ec2.cfn-ec2-instance-tag-update-in-place-retained-replacement-keeps-both-running-then-terminated cloudformation__ec2.cfn-ec2-route-retarget-in-place-replace-retained-then-delete cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete cloudformation__ec2.cfn-ec2-security-group-ingress-update-in-place-replace-retained-then-delete cloudformation__ec2.cfn-ec2-security-group-create-update-in-place-replace-retained-then-delete cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete
Issue-Key: cloudformation__ec2.cfn-ec2-vpc-create-update-in-place-then-delete|_expect#24|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-subnet-create-update-in-place-replace-retained-then-delete|_expect#20|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found|_expect#25|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-internet-gateway-tag-update-in-place-attached-then-delete|_expect#19|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-vpc-gateway-attachment-swap-gateway-in-place-then-delete|run#10|AssertionError|assert attachment == f"IGW|{vpc_id}", f"attachment physical id {attachment!r}"
Issue-Key: cloudformation__ec2.cfn-ec2-route-ref-is-the-route-table-id-and-destination|run#10|AssertionError|assert outputs["RouteRef"] == want, (
Issue-Key: cloudformation__ec2.cfn-ec2-instance-tag-update-in-place-retained-replacement-keeps-both-running-then-terminated|_expect#15|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ec2.cfn-ec2-route-retarget-in-place-replace-retained-then-delete|run#14|AssertionError|assert _outputs(stack) == {"RouteCidr": _FIRST_DEST}, f"GetAtt outputs {_outputs(stack)!r}"
Issue-Key: cloudformation__ec2.cfn-ec2-route-table-create-update-in-place-then-delete|run#21|AssertionError|assert _physical_ids(cfn, ref["stack"])["RouteTable"] == rt_id, (
Issue-Key: cloudformation__ec2.cfn-ec2-security-group-ingress-update-in-place-replace-retained-then-delete|_wait_for#10|AssertionError|raise AssertionError(f"{what}: {got!r}, expected {want!r}")
Issue-Key: cloudformation__ec2.cfn-ec2-security-group-create-update-in-place-replace-retained-then-delete|_wait_for#15|AssertionError|raise AssertionError(f"{what}: {got!r}, expected {want!r}")
Issue-Key: cloudformation__ec2.cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete|run#26|AssertionError|assert rows == want_rows, (
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 8 net-new causes in CloudFormation-provisioned EC2: in-place updates refused, Route Ref wrong, NetworkInterfaces unsupported, null GetAtt outputs, spurious replacement, ingress rules never created, and replacement ordering

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Eight independent root causes in the CloudFormation EC2 resource handlers.

Cause 1 of 8: in-place updates of EC2 resources are refused with an emulator-authored reason.
AWS: recorded behavior only. Changing a mutable property of AWS::EC2::VPC (EnableDnsHostnames/tags), AWS::EC2::Subnet (MapPublicIpOnLaunch/tags), AWS::EC2::LaunchTemplate (LaunchTemplateData -> a new version), AWS::EC2::InternetGateway/AWS::EC2::VPCGatewayAttachment is an in-place update: UPDATE_IN_PROGRESS -> UPDATE_COMPLETE on the logical resource, physical id unchanged, stack UPDATE_COMPLETE.
Emulator: the resource is UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [<LogicalId>]. '; the resource reason is 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::<Type> in place; the resource was left as it is.' (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.', variable part = resource type). AWS has no NotImplemented error for a supported property change.
Why: no CloudFormation-managed VPC estate can be edited at all — tag changes, DNS-hostname flips and launch-template revisions all roll the stack back.
Fix: the AWS::EC2::* resource handlers' update paths in the CloudFormation provisioner.
Failing checks: cfn-ec2-vpc-create-update-in-place-then-delete, cfn-ec2-subnet-create-update-in-place-replace-retained-then-delete, cfn-ec2-launch-template-in-place-new-version-retained-replacement-then-not-found, cfn-ec2-internet-gateway-tag-update-in-place-attached-then-delete, cfn-ec2-vpc-gateway-attachment-swap-gateway-in-place-then-delete.

Cause 2 of 8: Ref on AWS::EC2::Route is not the composite route id.
AWS: the physical resource id of AWS::EC2::Route is '<RouteTableId>|<DestinationCidrBlock>' (e.g. rtb-0123456789abcdef0|0.0.0.0/0), so Fn::Ref resolves to that string.
Emulator: Ref returns a generated stack resource name, 'sim2real-ec2rtref-9fb3feb5ad0e-Route-1214EF740A68', where 'rtb-...|0.0.0.0/0' was required.
Why: templates and operators use the Ref to locate the route in the route table; a generated name identifies nothing in EC2.
Fix: the AWS::EC2::Route resource handler's physical-resource-id construction.
Failing check: cfn-ec2-route-ref-is-the-route-table-id-and-destination.

Cause 3 of 8: AWS::EC2::Instance rejects the NetworkInterfaces property.
AWS: NetworkInterfaces on AWS::EC2::Instance is supported — RunInstances takes NetworkInterfaces[] (DeviceIndex, SubnetId, Groups, AssociatePublicIpAddress) and the instance reaches running.
Emulator: the resource fails at create. Stack 'ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to create: [Instance]. ' and resource reason 'Resource handler returned message: "this emulator does not apply the NetworkInterfaces property of AWS::EC2::Instance" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. The fixed text is 'this emulator does not apply the NetworkInterfaces property of AWS::EC2::Instance'; the HandlerErrorCode InvalidRequest and the request token are the variable framing. AWS never refuses a modelled, supported property.
Why: the standard way to put an instance on a specific subnet with specific security groups and a public IP is unavailable, so VPC-scoped instance templates cannot deploy.
Fix: the AWS::EC2::Instance resource handler's property mapping onto RunInstances.
Failing check: cfn-ec2-instance-tag-update-in-place-retained-replacement-keeps-both-running-then-terminated.

Cause 4 of 8: a stack output built from Fn::GetAtt on an EC2 route resolves to null.
AWS: a stack Output's value is always resolved and returned; an output that cannot be resolved fails the stack rather than being returned empty.
Emulator: DescribeStacks returns the output with a null value — 'GetAtt outputs {RouteCidr: None}' where the route's CidrBlock was expected.
Why: any consumer of that output (cross-stack export, nested-stack wiring, CLI/CDK reads) gets nothing.
Fix: the AWS::EC2::Route handler's GetAtt attribute resolution and the stack-output serializer.
Failing check: cfn-ec2-route-retarget-in-place-replace-retained-then-delete.

Cause 5 of 8: an in-place update of AWS::EC2::RouteTable replaces the route table.
AWS: changing only mutable properties (tags) of a route table is an in-place update; the RouteTableId (physical id) is unchanged.
Emulator: after the update the logical resource's physical id is a different rtb- id ('an in-place update keeps the route table' fails), so routes, subnet associations and anything referencing the old id are orphaned.
Fix: the AWS::EC2::RouteTable handler's update classification.
Failing check: cfn-ec2-route-table-create-update-in-place-then-delete.

Cause 6 of 8: AWS::EC2::SecurityGroupIngress creates no ingress rule.
AWS: an AWS::EC2::SecurityGroupIngress resource calls AuthorizeSecurityGroupIngress, so DescribeSecurityGroupRules for the group returns a rule with IsEgress false and the declared IpProtocol, FromPort, ToPort, CidrIpv4 and Description.
Emulator: DescribeSecurityGroupRules returns [] for the group — the declared rule (tcp 8443-8443 from 192.0.2.0/24, 'sim2real ingress one') does not exist even though the stack reported the resource created.
Why: traffic a template authorises is not actually authorised; security posture declared in CloudFormation is not the posture the emulator enforces.
Fix: the AWS::EC2::SecurityGroupIngress handler (it must authorise the rule in EC2 state).
Failing check: cfn-ec2-security-group-ingress-update-in-place-replace-retained-then-delete.

Cause 7 of 8: a security group read back after a CloudFormation update does not match the template-declared state.
AWS: after the stack settles, DescribeSecurityGroups for the group returns exactly the declared description, ingress/egress permissions and the stack's tag set (sim2real plus aws:cloudformation:stack-name / stack-id / logical-id).
Emulator: the read-back never converges on the expected value within the check's wait; recorded behavior only for the exact diverging field.
Why: a client cannot trust that the group CloudFormation reports as complete carries the rules and tags it declared.
Fix: the AWS::EC2::SecurityGroup handler's create/update property application and EC2's security-group state.
Failing check: cfn-ec2-security-group-create-update-in-place-replace-retained-then-delete.

Cause 8 of 8: replacing an AWS::EC2::SubnetRouteTableAssociation creates the new association before deleting the old one.
AWS: for a resource type whose replacement must be delete-before-create (a subnet can hold only one route-table association), CloudFormation deletes the existing association first and then creates the replacement.
Emulator: the events are CREATE_IN_PROGRESS / CREATE_COMPLETE on the new rtbassoc- id followed by DELETE_IN_PROGRESS / DELETE_COMPLETE on the old one — create-then-delete.
Why: on real AWS that ordering is impossible (the second association would be refused); a client relying on the documented ordering, or on the old association remaining valid until the new one exists, sees the wrong sequence and the wrong window of validity.
Fix: the CloudFormation update engine's replacement ordering for delete-before-create resource types.
Failing check: cfn-ec2-subnet-route-table-association-replace-deletes-first-despite-retain-then-delete.

## Evidence
Cause 1: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Vpc]. '), expected 'UPDATE_COMPLETE'; failed events: [('Vpc', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::EC2::VPC in place; the resource was left as it is.')] — same for [Subnet], [Lt] (AWS::EC2::LaunchTemplate) and [Attachment] (AWS::EC2::VPCGatewayAttachment).
Cause 2: AssertionError: Ref of the AWS::EC2::Route returned 'sim2real-ec2rtref-9fb3feb5ad0e-Route-1214EF740A68', expected 'rtb-00000000000000098|0.0.0.0/0'.
Cause 3: AssertionError: stack ended 'ROLLBACK_COMPLETE' ('The following resource(s) failed to create: [Instance]. '); failed events: [('Instance', 'CREATE_FAILED', 'Resource handler returned message: "this emulator does not apply the NetworkInterfaces property of AWS::EC2::Instance" (RequestToken: ..., HandlerErrorCode: InvalidRequest)')].
Cause 4: AssertionError: GetAtt outputs {'RouteCidr': None}.
Cause 5: AssertionError: an in-place update keeps the route table.
Cause 6: AssertionError: ingress rules of sg-...: [], expected [{'GroupId': 'sg-...', 'IsEgress': False, 'IpProtocol': 'tcp', 'FromPort': 8443, 'ToPort': 8443, 'CidrIpv4': '192.0.2.0/24', 'Description': 'sim2real ingress one'}].
Cause 7: AssertionError: security group sg-...: <read-back> , expected <template-declared permissions/tags>.
Cause 8: AssertionError: Association must be deleted before its replacement is created, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'rtbassoc-...9e'), ('CREATE_COMPLETE', None, 'rtbassoc-...9e'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'rtbassoc-...9c')].

## Steps to Reproduce
Cause 1: CreateStack a VPC/Subnet/LaunchTemplate/InternetGateway+VPCGatewayAttachment template, then UpdateStack changing only a mutable property; read DescribeStackEvents.
Cause 2: CreateStack a VPC + InternetGateway + RouteTable + Route, with an Output of Ref on the Route; read DescribeStacks.
Cause 3: CreateStack an AWS::EC2::Instance that declares NetworkInterfaces[0] with DeviceIndex/SubnetId/Groups.
Cause 4: CreateStack a Route with an Output of Fn::GetAtt Route.CidrBlock; read DescribeStacks.
Cause 5: CreateStack a RouteTable, UpdateStack changing only its tags, then compare PhysicalResourceId.
Cause 6: CreateStack an AWS::EC2::SecurityGroupIngress on a group; call DescribeSecurityGroupRules.
Cause 7: CreateStack an AWS::EC2::SecurityGroup with ingress/egress and tags, UpdateStack in place, then DescribeSecurityGroups.
Cause 8: CreateStack an AWS::EC2::SubnetRouteTableAssociation, UpdateStack pointing it at a different route table, then read DescribeStackEvents ordering.
