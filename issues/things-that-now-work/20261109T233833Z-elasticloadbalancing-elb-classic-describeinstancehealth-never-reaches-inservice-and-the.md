Issue-Source: trace-sweep-divergence
Issue-Trace: elb-instance-serving-http-reaches-inservice
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elb-instance-serving-http-reaches-inservice elbclassic-account-limits-and-tag-lifecycle-round-trip
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.Description
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.ReasonCode
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.State
Issue-Key: elasticloadbalancing|AddTags|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 01897137b22b96058352710896276b71185e2aeb

# ELB Classic DescribeInstanceHealth never reaches InService, and the AddTags duplicate-tag-key refusal is not AWS's message

> _Note: this issue was identified against the code base at commit `01897137b22b96058352710896276b71185e2aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceHealth

## Description
2 causes.

Cause 1 of 2 — DescribeInstanceHealth never reports State=InService for a registered backend that is serving the configured health-check target (fixture elb-instance-serving-http-reaches-inservice, step task#10).
AWS: Smithy: DescribeInstanceHealth (input DescribeEndPointStateInput {LoadBalancerName required, Instances}; output DescribeEndPointStateOutput {InstanceStates: list of InstanceState}; errors AccessPointNotFoundException awsQueryError LoadBalancerNotFound/400, InvalidEndPointException awsQueryError InvalidInstance/400). InstanceState$State: 'Valid values: InService | OutOfService | Unknown'. InstanceState$ReasonCode: 'Information about the cause of OutOfService instances... Valid values: ELB | Instance | N/A'. InstanceState$Description enumerates the permitted fixed sentences, among them 'N/A', 'Instance registration is still in progress.' and 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' The reported state is the OUTCOME of the load balancer probing each registered instance on the configured health-check Target: HealthyThreshold consecutive successes yield State='InService' with ReasonCode='N/A' and Description='N/A'; only UnhealthyThreshold consecutive failures yield State='OutOfService'/ReasonCode='Instance'/Description='Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' Those State, ReasonCode and Description strings are fixed AWS text; the only request/state-derived value in the member is InstanceId. The recorded response for this step is exactly InstanceId=<registered instance>, State='InService', ReasonCode='N/A', Description='N/A'.
Emulator: HTTP 200, but the member never leaves OutOfService: State='OutOfService', ReasonCode='Instance', Description='Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' The step's InService poll was still unsatisfied after 19 attempts and the 180s poll cap elapsed. Health is not a probe result at all — it is derived from the registration instant — so InService is unreachable by construction for any backend, however healthy.
Why: a client cannot distinguish a healthy backend from a failed one. Every registration-verification loop, SDK waiter and deploy health gate that waits for InService before sending traffic blocks until its own timeout or concludes the fleet is permanently out of service, and the load balancer can never be in the one state in which AWS routes traffic to an instance.
Fix: the per-load-balancer instance-health state and the health-check evaluation that feeds it in the ELB Classic crate's state, as read by the DescribeInstanceHealth handler — health must be derived from an actual probe of the registered instance against the load balancer's configured Target/Interval/Timeout and thresholds, emitting the InService/'N/A'/'N/A' triple on HealthyThreshold consecutive successes, instead of a registration-timestamp countdown whose only outcomes are the registration-in-progress and failed-health-checks triples.
Traces: elb-instance-serving-http-reaches-inservice#task#10. Code: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/state.rs (LoadBalancer::instance_health) and src/ops/describe_instance_health.rs.

Cause 2 of 2 — AddTags refuses duplicate tag keys with its own sentence instead of AWS's (fixture elbclassic-account-limits-and-tag-lifecycle-round-trip, step task#7).
AWS: Smithy: AddTags (input AddTagsInput {LoadBalancerNames required, Tags required @length min 1}) models DuplicateTagKeysException — errorKind client, httpError 400, awsQueryError code 'DuplicateTagKeys' / httpResponseCode 400, documented 'A tag key was specified more than once.' The recorded refusal is HTTP 400 with Error.Message 'Duplicate Tag Keys found in the request. Tag keys should be unique.' — that message is entirely fixed AWS text: it names neither the offending key nor the load balancer, so it has no request- or state-derived parts.
Emulator: status 400 and the correct error class, but Error.Message is the emulator's own sentence "Tag key '<key>' was specified more than once" — different fixed text, and it interpolates the duplicated key from the request where AWS interpolates nothing.
Why: a client that matches on the refusal text (log assertions, user-facing error surfacing, tests pinned to AWS's wording) sees a message AWS never sends, and a client that trusts the message to name the offending key will read nothing of the sort from real AWS. The message is part of the error contract, so the refusal is only half right.
Fix: the AddTags handler's duplicate-key validation — the message it attaches to DuplicateTagKeysException must be AWS's fixed sentence 'Duplicate Tag Keys found in the request. Tag keys should be unique.' with no interpolated key; the same DuplicateTagKeysException is modeled on CreateLoadBalancer, so the shared tag validation should emit the identical text there.
Traces: elbclassic-account-limits-and-tag-lifecycle-round-trip#task#7. Code: services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/ops/add_tags.rs.

Related (not covering these): open behavioral issue 20261107T072636Z-elb-classic-a-registered-backend-that-passes-its.md reports Cause 1's fault from the behavioral suite — it carries behavioral check ids only and no wire replay key, so this fixture step is not gated by it. Open issue 20261030T195633Z-elasticloadbalancing-elb-classic-describeinstancehealth-serializes-instancestates-as-an-empty.md concerns InstanceStates being serialized as an empty element, the opposite wire shape.

## Evidence
Cause 1 — DescribeInstanceHealth, fixture elb-instance-serving-http-reaches-inservice step task#10 (expected status 200, actual 200).
Expected (AWS), from the recording: DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member = {InstanceId: <registered instance>, State: 'InService', ReasonCode: 'N/A', Description: 'N/A'} once the backend has passed HealthyThreshold consecutive health checks. 'InService' and 'N/A' are fixed AWS text; InstanceId comes from the request/state.
Actual (emulator), verbatim from replay:
  $.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.State: value_mismatch (expected 'InService', got 'OutOfService')
  $.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.ReasonCode: value_mismatch (expected 'N/A', got 'Instance')
  $.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.Description: value_mismatch (expected 'N/A', got 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.')
  $.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.State: poll: not in ['InService'] after 19 attempt(s) (expected 'InService', got 'OutOfService')
  $: poll capped at 180s
The status is 200 in both cases, so the divergence is the VALUE of State/ReasonCode/Description, not the wire shape.

Cause 2 — AddTags, fixture elbclassic-account-limits-and-tag-lifecycle-round-trip step task#7 (expected status 400, actual 400).
Expected (AWS): HTTP 400, awsQueryError code DuplicateTagKeys, ErrorResponse.Error.Message = 'Duplicate Tag Keys found in the request. Tag keys should be unique.' — fixed text in full, with no variable parts.
Actual (emulator), verbatim from replay:
  $.ErrorResponse.Error.Message: value_mismatch (expected 'Duplicate Tag Keys found in the request. Tag keys should be unique.', got "Tag key 'dup' was specified more than once")
The quoted key 'dup' is the request-supplied tag key — the emulator templates a value into a message whose AWS counterpart has no template at all.

Model evidence: InstanceState$State 'Valid values: InService | OutOfService | Unknown'; InstanceState$ReasonCode 'Valid values: ELB | Instance | N/A'; InstanceState$Description enumerates the permitted sentences including 'N/A'. AddTags errors include DuplicateTagKeysException {errorKind client, httpError 400, awsQueryError {code DuplicateTagKeys, httpResponseCode 400}}, doc 'A tag key was specified more than once.'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (trace set simparity-wire-aws-v1).
2. Cause 1: replay fixture elb-instance-serving-http-reaches-inservice up to step task#10. The fixture creates a Classic load balancer with an HTTP listener, configures a health check, registers an EC2 backend that serves the health-check target, then polls DescribeInstanceHealth for State='InService'.
3. Observe the member reports State='OutOfService', ReasonCode='Instance', Description='Instance has failed at least the UnhealthyThreshold number of health checks consecutively.' on every attempt; the poll exhausts 19 attempts and the 180s cap without ever seeing InService. Expected: State='InService', ReasonCode='N/A', Description='N/A'.
4. Cause 2: replay fixture elbclassic-account-limits-and-tag-lifecycle-round-trip up to step task#7, which calls AddTags with the same tag key twice on one load balancer.
5. Observe HTTP 400 with ErrorResponse.Error.Message "Tag key '<key>' was specified more than once". Expected: ErrorResponse.Error.Message exactly 'Duplicate Tag Keys found in the request. Tag keys should be unique.' (error code DuplicateTagKeys, status 400), with no tag key interpolated.
