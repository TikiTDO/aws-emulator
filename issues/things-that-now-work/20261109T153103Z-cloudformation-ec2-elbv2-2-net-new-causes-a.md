Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle
Issue-Key: elastic-load-balancing-v2|DescribeListeners|ListenerNotFound
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle|run#7|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle|run#7|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudformation__ec2__elbv2: 2 net-new causes — a listener created by a CREATE_COMPLETE stack is unknown to ELBv2, and the stack's Outputs entries carry no OutputValue

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2). Related open filings that do NOT carry these replay keys: 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md and 20261102T085002Z-cloudwatch-ec2-elbv2-opensearch-a-deployed-stack-s.md describe the same family under other headings and carry none of the three ids below; the suite marks none of them OWNED.

Cause 1 of 2: a listener created by a stack that reached CREATE_COMPLETE is unknown to ELBv2 — DescribeListeners on the ARN the stack exports answers ListenerNotFound.
AWS: Smithy (elasticloadbalancingv2) DescribeListeners declares ListenerNotFoundException (awsQueryError code ListenerNotFound, HTTP 400) for an ARN that does not exist. CloudFormation reports CREATE_COMPLETE for AWS::ElasticLoadBalancingV2::Listener only once the listener exists, so on AWS the exported ARN describes successfully and its Port/Protocol/DefaultActions are readable.
Emulator: the stack reaches CREATE_COMPLETE and exports a listener ARN, but DescribeListeners answers HTTP 400 ListenerNotFound with message "One or more listeners not found" — the resource the stack claims to have created does not exist in ELBv2 state.
Why: CREATE_COMPLETE is the only signal a client has that provisioning succeeded. A stack that reports success with absent resources turns every downstream call into a not-found error attributed to the consumer, and a rollback/retry loop can never converge.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::Listener must create the listener in the ELBv2 service's state, and must fail the resource (rolling the stack back) when it cannot.
Traces: cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle.

Cause 2 of 2: DescribeStacks returns Outputs entries that have no OutputValue member at all.
AWS: Smithy (cloudformation) DescribeStacks -> Stacks[].Outputs[] of Output {OutputKey, OutputValue, Description, ExportName}. For a stack in CREATE_COMPLETE, AWS populates OutputValue with the resolved value of each declared output (here the load balancer ARN/DNS name and the target group ARN); an Output without OutputValue is not a shape AWS returns for a completed stack.
Emulator: the Output entries are present but carry no OutputValue key, so reading out["OutputValue"] raises KeyError: 'OutputValue'.
Why: stack outputs are the documented hand-off between a stack and everything downstream. An output with a key and no value cannot be distinguished from a value that is legitimately empty, and every consumer that indexes OutputValue — SDK, CLI and template cross-stack references alike — fails on a stack the API reports as complete.
Fix: the CloudFormation DescribeStacks serializer and the stack-output resolution path must resolve each declared output and emit OutputValue alongside OutputKey.
Traces: cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle; cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudformation__ec2__elbv2 0/3 (0%), 3 failed; none OWNED.

Cause 1: [error] cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle
  botocore.errorfactory.ListenerNotFoundException: An error occurred (ListenerNotFound) when calling the DescribeListeners operation: One or more listeners not found

Cause 2: [error] cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle
  KeyError: 'OutputValue'
[error] cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle
  KeyError: 'OutputValue'

Expected: a CREATE_COMPLETE stack's listener ARN describes successfully, and each Outputs[] entry carries both OutputKey and OutputValue. Actual: ListenerNotFound for the exported ARN, and Outputs entries with no OutputValue member.

Smithy consulted: elasticloadbalancingv2 DescribeListeners (ListenerNotFoundException, code ListenerNotFound, HTTP 400); cloudformation DescribeStacks output Stacks[].Outputs[] of Output {OutputKey, OutputValue, Description, ExportName}.

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the three ids in trace_ids reproduce.
2. Create a stack with AWS::ElasticLoadBalancingV2::LoadBalancer, ::TargetGroup and ::Listener, plus Outputs exporting each resource's Ref/attribute; wait for CREATE_COMPLETE.
3. Cause 1: elbv2.describe_listeners(ListenerArns=[<listener output>]). Expect the listener; the emulator answers 400 ListenerNotFound.
4. Cause 2: cloudformation.describe_stacks(StackName=<id>) and read Stacks[0].Outputs[*]['OutputValue']. Expect a resolved value per output; the emulator's entries have no OutputValue key.
5. Re-replay the ids in trace_ids to confirm the fix.
