Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-task-set-external-deployment-workflow
Issue-Step: task#16
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-task-set-external-deployment-workflow ecs-taskset-multiple-primary-switch ecs-taskset-primary-promotion ecs-taskset-basic-lifecycle ecs-taskset-create-describe-update-delete-lifecycle ecs-taskset-crud-lifecycle ecs-taskset-errors ecs-taskset-lifecycle ecs-taskset-tags-and-external-id ecs-taskset-tags-and-include-fields
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeTaskSets|2xx->2xx|$.failures[*]
Issue-Commit: 232d6cb39af83c8f3e5d0960e0729eaeb270e0f7

# ECS DescribeTaskSets failure entries re-synthesize task-set ARNs with cluster and service segments

> _Note: this issue was identified against the code base at commit `232d6cb39af83c8f3e5d0960e0729eaeb270e0f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTaskSets

## Description
DescribeTaskSets reports unresolvable task sets in its `failures` list. The emulator does not echo the identifier the request supplied: it rebuilds the ARN's resource part by prefixing the cluster name and the service name, so every failure entry carries an ARN the caller never asked about. Two divergence clusters (14 affected steps in total) show the same construction defect. This bundle contains 2 causes; both must be closed.

Cause 1 of 2: DescribeTaskSets failure ARN for a deleted/absent task set in an EXTERNAL-deployment service.
AWS: DescribeTaskSets returns `failures: Failures` alongside `taskSets` (Smithy: DescribeTaskSets output members taskSets/failures; Failure members arn/reason/detail). The model does not constrain the ARN's resource layout, so this is recorded behavior only: for a task set named in the request by its short task-set id (`ecs-svc/<19-digit-id>`), AWS answers 200 with a failure whose `arn` is `arn:aws:ecs:<region>:<account-id>:task-set/<the-identifier-the-request-supplied>` and whose `reason` is the fixed string `MISSING`. Fixed parts: the ARN prefix `arn:aws:ecs:`, the literal resource type `task-set/`, and the reason `MISSING`. Variable parts: region and account id from the request context, and the task-set identifier taken verbatim from the request's `taskSets` entry. The request's cluster and service do NOT appear in the resource part.
Emulator: 200 with the same `reason: MISSING`, but `arn` is `arn:aws:ecs:<region>:<account-id>:task-set/<clusterName>/<serviceName>/ecs-svc/<id>` — two extra path segments synthesized from the request's `cluster` and `service` members. The replay reports the entry simultaneously as `missing` (the expected failure) and `extra` (the emulator's), for both failures[0] and failures[1].
Why: a client correlates failure entries back to the identifiers it passed in `taskSets` by string-matching `failures[].arn`. Because the emulator returns an ARN that is not the identifier the caller supplied and not a suffix-stable form of it, the correlation fails: the caller sees a non-empty `failures` list it cannot attribute to any of its requested task sets, so "this task set is gone" is indistinguishable from "an unknown task set failed". SDK waiters and reconcilers that key on the echoed ARN loop forever.
Fix: the DescribeTaskSets handler's failure-list construction / response serializer — echo the requested task-set identifier into the Failure ARN instead of re-deriving it from cluster+service state.
Traces: ecs-task-set-external-deployment-workflow#task#16 (also ecs-taskset-multiple-primary-switch#task#14, ecs-taskset-primary-promotion#task#12).

Cause 2 of 2: the same failure-ARN re-synthesis on the ordinary task-set lifecycle path, after the task set has been deleted.
AWS: identical rule — 200, `failures[0].reason` is the fixed string `MISSING`, `failures[0].arn` is `arn:aws:ecs:<region>:<account-id>:task-set/<requested task-set id>` with no cluster or service segment.
Emulator: `arn:aws:ecs:<region>:<account-id>:task-set/<clusterName>/<serviceName>/ecs-svc/<id>`.
Why: same as cause 1; this cluster covers 7 steps across the basic/CRUD/tags/errors task-set fixtures, i.e. the normal create-describe-delete flow, so it is the path most clients take.
Fix: same component as cause 1 (one edit should close both, but each must be re-replayed).
Traces: ecs-taskset-basic-lifecycle#task#12 (also ecs-taskset-create-describe-update-delete-lifecycle#task#10, ecs-taskset-crud-lifecycle#task#12, ecs-taskset-errors#task#12, ecs-taskset-lifecycle#task#13, ecs-taskset-tags-and-external-id#task#8, ecs-taskset-tags-and-include-fields#task#9).

Related but NOT covering these: open issue 20261029T220901Z-ecs-task-set-responses-omit-empty-tags-report.md concerns task-set tags/status/scale and a cluster-qualified `serviceArn` on the TaskSet object itself; neither it nor 20261029T220901Z-ecs-service-deployment-task-set-and-attribute-error.md carries a replay key for the DescribeTaskSets `failures[].arn` divergence, so these two clusters are unowned.

## Evidence
Cluster A (service=ecs|DescribeTaskSets|status=200->200|keys=$.failures[0],$.failures[1]), fixture ecs-task-set-external-deployment-workflow step task#16, expected 200 / actual 200:
  $.failures[0]: expected {'arn': 'arn:aws:ecs:us-west-2:000000000000:task-set/ecs-svc/0000000000000000001', 'reason': 'MISSING'}, got {'arn': 'arn:aws:ecs:us-west-2:000000000000:task-set/<cluster>/<service>/ecs-svc/0000000000000000001', 'reason': 'MISSING'}
  $.failures[1]: expected {'arn': 'arn:aws:ecs:us-west-2:000000000000:task-set/ecs-svc/0000000000000000002', 'reason': 'MISSING'}, got the same cluster/service-qualified form.
  3 affected steps.

Cluster B (service=ecs|DescribeTaskSets|status=200->200|keys=$.failures[0]), fixture ecs-taskset-basic-lifecycle step task#12, expected 200 / actual 200:
  $.failures[0]: expected {'arn': 'arn:aws:ecs:us-west-2:000000000000:task-set/ecs-svc/0000000000000000001', 'reason': 'MISSING'}, got {'arn': 'arn:aws:ecs:us-west-2:000000000000:task-set/<cluster>/<service>/ecs-svc/0000000000000000001', 'reason': 'MISSING'}
  7 affected steps.

The status, the reason value MISSING and the number of failure entries all agree; only the ARN's resource part diverges. The account id, region, cluster and service names above are per-run fixture values — the requirement is the ARN's SHAPE (resource part = the requested identifier, un-prefixed).

## Steps to Reproduce
1. Replay fixture ecs-task-set-external-deployment-workflow against the emulator and stop at step task#16: it calls DescribeTaskSets with cluster+service and two task-set ids of the form ecs-svc/<19 digits> that no longer exist.
2. Inspect the 200 response's failures[]: each entry's arn is arn:aws:ecs:<region>:<account>:task-set/<cluster>/<service>/ecs-svc/<id>, while the recording has arn:aws:ecs:<region>:<account>:task-set/ecs-svc/<id>.
3. Repeat with fixture ecs-taskset-basic-lifecycle step task#12 (single deleted task set) for the second cause.
4. Both steps pass only when the Failure ARN's resource part is the identifier supplied in the request's taskSets member.
