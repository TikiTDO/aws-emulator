Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle
Issue-Key: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 0275b87193e8729c4e6e84626c64cbadccfa7c1a

# ecs__events__iam__s3: ECS state is not region-scoped — ListClusters and ListTaskDefinitions in one region return clusters and task definitions created in another

> _Note: this issue was identified against the code base at commit `0275b87193e8729c4e6e84626c64cbadccfa7c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
One root cause.

=== Cause 1 of 1: ECS clusters and task definitions created in one region are visible to an ECS client in a different region ===
AWS: recorded behaviour only (regional-service model). ECS is a regional service: a cluster and a task definition created in one region exist only in that region's endpoint. An ECS client pointed at a different region must not list them — ListClusters returns no such clusterArn and ListTaskDefinitions returns no such taskDefinitionArn — and every ARN an ECS endpoint returns carries that endpoint's own region. The check's "far region" probe asserts exactly this: the far region holds none of the stack's rules, clusters or task definitions.
Emulator: the far-region ECS endpoint returns the clusters and task definitions created in the deploy region, and the ARNs it returns still carry the creating region (for example a cluster ARN of the form arn:aws:ecs:<deploy-region>:<account>:cluster/<name> and a task-definition ARN arn:aws:ecs:<deploy-region>:<account>:task-definition/<name>:<revision>, both returned by a client configured for a different region). The EventBridge rules of the same stack are correctly absent from the far region, so the defect is specific to ECS's state partitioning, not to the harness's region configuration.
Why: a client that enumerates ECS resources per region sees cross-region duplicates; region-scoping assertions and multi-region deployments break, a cleanup pass in one region deletes or counts another region's resources, and an operator debugging "why is my cluster in the wrong region" is misled by an ARN whose region does not match the endpoint that served it.
Fix: the ecs service state — clusters and task-definition families must be keyed by region, and the ListClusters / ListTaskDefinitions (and DescribeClusters / DescribeTaskDefinition) handlers must filter by the request's region.
Traces: ecs__events__iam__s3.e2e-eventbridge-ecs-task-trigger-debugging-lifecycle (phase 2/4 "solve")

## Evidence
AssertionError: [phase 2/4 solve] us-east-2 must hold none of the stack's rules, clusters or task definitions, got {'far_rules': [], 'far_clusters': ['arn:aws:ecs:us-west-2:000000000000:cluster/sim2real-ebecs-e2e-49598597f6cb-ScheduledTaskCluster-CE382FEE120E'], 'far_task_definitions': ['arn:aws:ecs:us-west-2:000000000000:task-definition/sim2real-ebecs-e2e-49598597f6cb-TaskDefinition-63FF0575040A:1']}

Expected: far_clusters == [] and far_task_definitions == [] for an ECS client in a region other than the one the stack deployed into. (The concrete region pair, account id and generated resource names above are this run's values; the requirement is that no ECS resource created in one region is listed by another region's endpoint, and that a listed ARN's region equals the serving endpoint's region.) Note far_rules is already correctly empty, so EventBridge partitions its state by region while ECS does not.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. In region A (e.g. us-west-2) create an ECS cluster and register a task definition (the check does this via a CloudFormation stack containing a ScheduledTask cluster and task definition).
3. Create an ECS client for region B (e.g. us-east-2) and call ListClusters and ListTaskDefinitions.
4. Observe that region B returns the cluster ARN and the task-definition ARN created in region A, with region A in the ARNs.
