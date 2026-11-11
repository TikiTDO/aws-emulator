Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs.cfn-ecs-cluster-in-place-update-retained-replacement-then-inactive cloudformation__ecs.cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive
Issue-Key: cloudformation__ecs.cfn-ecs-cluster-in-place-update-retained-replacement-then-inactive|_expect#19|AssertionError|raise AssertionError(
Issue-Key: cloudformation__ecs.cfn-ecs-fargate-service-in-place-update-retained-replacement-then-inactive|_describe#24|AssertionError|assert resp.get("failures", []) == [], f"DescribeServices {arn}: {resp['failures']!r}"
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 net-new causes: AWS::ECS::Cluster in-place update refused, and DescribeServices reports MISSING for a stack-created Fargate service

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two independent root causes in the ECS CloudFormation resource handlers.

Cause 1 of 2: in-place update of AWS::ECS::Cluster is refused with an emulator-authored reason.
AWS: changing mutable cluster properties (settings such as containerInsights, tags, configuration) is an in-place update applied through ECS UpdateCluster/UpdateClusterSettings; the cluster ARN is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE. Only a ClusterName change forces replacement.
Emulator: the resource is UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Cluster]. '; the reason is 'NotImplemented: this emulator does not change the properties of an existing AWS::ECS::Cluster in place; the resource was left as it is.' (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; variable part = resource type). AWS has no NotImplemented error for a supported property change.
Why: cluster settings managed by CloudFormation cannot be changed; every such update rolls back.
Fix: the AWS::ECS::Cluster resource handler's update path in the CloudFormation provisioner.

Cause 2 of 2: DescribeServices does not know a service the stack reported as created.
AWS: after a stack containing an AWS::ECS::Service (Fargate launch type) reaches CREATE_COMPLETE, ECS DescribeServices with cluster=<cluster arn> and services=[<service arn>] returns the service in 'services' and an empty 'failures' list.
Emulator: the call returns the service in 'failures' with reason MISSING: [{'arn': 'arn:aws:ecs:<region>:<account>:service/<cluster>/<service>', 'reason': 'MISSING'}]. The service ARN is the one CloudFormation handed back.
Why: a client cannot read a service's desired/running count, deployment status or task set after creating it with CloudFormation; every ECS deployment-verification and scaling workflow fails on a service AWS would return.
Fix: the AWS::ECS::Service resource handler (it must register the service in ECS state under the cluster) and/or ECS's service store behind DescribeServices.

## Evidence
Cause 1: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Cluster]. '), expected 'UPDATE_COMPLETE'; failed events: [('Cluster', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ECS::Cluster in place; the resource was left as it is.')]
Cause 2: AssertionError: DescribeServices arn:aws:ecs:us-west-2:000000000000:service/sim2real-cfnsvc-cl-.../sim2real-cfnsvc-a-...: [{'arn': 'arn:aws:ecs:us-west-2:000000000000:service/sim2real-cfnsvc-cl-.../sim2real-cfnsvc-a-...', 'reason': 'MISSING'}] — expected failures == [].

## Steps to Reproduce
Cause 1: CreateStack an AWS::ECS::Cluster, then UpdateStack changing only ClusterSettings/Tags.
Cause 2: CreateStack an AWS::ECS::Cluster + TaskDefinition + AWS::ECS::Service (FARGATE); after CREATE_COMPLETE call ecs DescribeServices with the service ARN — it comes back under failures with reason MISSING.
