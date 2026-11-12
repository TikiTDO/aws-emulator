Issue-Source: trace-sweep-divergence
Issue-Trace: elasticmapreduce-multi-master-placement-config-mistakes
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticmapreduce-multi-master-placement-config-mistakes elasticmapreduce-multi-master-topology-release-and-type-mistakes elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals emr-studio-create-invalid-vpc-error
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|$.__type
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|$.message
Issue-Key: elasticmapreduce|RunJobFlow|4xx->2xx|status
Issue-Key: elasticmapreduce|CreateStudio|4xx->4xx|$.Message
Issue-Commit: 59f14ace25ed5753b46a36483d50334ce01f6026

# EMR request-validation and refusal paths diverge: placement-group instance role unchecked, CreateStudio VpcId message authored

> _Note: this issue was identified against the code base at commit `59f14ace25ed5753b46a36483d50334ce01f6026`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
The EMR request-validation layer lets an invalid request through and, where it does refuse, writes its own message text instead of the service's. Both causes are in the refusal path of a create operation.

THIS ISSUE BUNDLES 2 CAUSES. Both must be closed.

Cause 1 of 2: RunJobFlow accepts a PlacementGroupConfigs entry whose InstanceRole is not MASTER.
  AWS: 400 with __type = 'ValidationException' and message = 'Placement group supports only the following instance role: [MASTER]'. That message is entirely fixed text - nothing is interpolated from the request - and the bracketed role list is the set of roles the service accepts. Smithy: RunJobFlowInput.PlacementGroupConfigs -> PlacementGroupConfigList of PlacementGroupConfig{InstanceRole (required, enum InstanceRoleType = MASTER|CORE|TASK), PlacementStrategy}, whose InstanceRole documentation states "Starting with Amazon EMR release 5.23.0, the only supported instance role is MASTER". The modelled error list for RunJobFlow carries only InternalServerError, so the refusal envelope itself is recorded behavior only; the recorded response is 400 / ValidationException.
  Emulator: 200 with a JobFlowId - the cluster is created. No __type and no message are emitted at all; the CORE/TASK placement-group request is honoured.
  Why: a client that mis-specifies a placement group gets a running, billable cluster whose placement is not what it asked for, instead of the refusal it is written to handle; any test asserting the guardrail passes vacuously. The cluster then exists in state, so later steps of the same workflow proceed from a topology AWS would never have created. The gap is broad: nine replayed steps across three fixtures (multi-master placement, multi-master topology, and spark perf-diagnosis recreate) hit it.
  Fix: RunJobFlow's input validation (the PlacementGroupConfigs check must reject any InstanceRole other than MASTER before any cluster state is written).
  Traces: elasticmapreduce-multi-master-placement-config-mistakes#task#8 (and the same cluster's other 8 affected steps).

Cause 2 of 2: CreateStudio's invalid-VpcId refusal carries an emulator-authored Message with a baked-in Request ID.
  AWS: 400 with Error Code InvalidRequestException (Smithy: CreateStudio errors = InternalServerException, InvalidRequestException @httpError 400; VpcId is a required XmlStringMaxLen256 member). The recorded Message is volatile and was redacted by the harness, so the exact wording is recorded behavior only: AWS's EMR Studio refusal names the rejected value from the request and ends with the service suffix whose Request ID is the request id of THAT response (the same value as the response's x-amzn-RequestId header).
  Emulator: 400 with Message = "VpcId is not valid. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: 9fa3e20a-b484-4994-97bc-a73291ecdae3; Proxy: null)" - a constant string. The fixed text does not interpolate the VpcId that was rejected, and the Request ID is a hard-coded literal rather than this response's request id, so every refusal of every request reports the same id. In this message the fixed parts are 'is not valid.' and the '(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)' suffix; the variable parts that must come from the request/response are the VpcId value and the Request ID.
  Why: a client or operator correlating an EMR Studio failure by request id is handed a constant that matches no request, and a client that surfaces the message cannot tell which VpcId was rejected when it submits several.
  Fix: CreateStudio's validation/error serializer - interpolate the request's VpcId into the message and take the Request ID from the per-response request id rather than a literal.
  Traces: emr-studio-create-invalid-vpc-error#task#3.

Related but NOT covering: 20261105T163809Z-elasticmapreduce-emr-input-validation-refusals-carry-non-aws-message.md owns the RunJobFlow placementStrategy enum-ordering message and the DescribeReleaseLabel empty-release-label refusal; it carries neither of these two replay keys, and neither of these causes is about enum ordering in a validation sentence.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, score 86.3%), verdict=mismatch for each cluster:

1. RunJobFlow elasticmapreduce-multi-master-placement-config-mistakes#task#8 - status expected 400, got 200; $.__type missing (expected 'ValidationException'); $.message missing (expected 'Placement group supports only the following instance role: [MASTER]'). Cluster covers 9 steps: elasticmapreduce-multi-master-placement-config-mistakes#task#8/#task#9/#task#10/#task#11, elasticmapreduce-multi-master-topology-release-and-type-mistakes#task#12/#task#13/#task#15, elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals#task#9/#task#10.
2. CreateStudio emr-studio-create-invalid-vpc-error#task#3, 400 vs 400 - $.Message value_mismatch: expected the recorded (harness-redacted, '<shaped-not-asserted>') AWS refusal message, got the constant 'VpcId is not valid. (Service: AWSEditors; Status Code: 400; Error Code: InvalidRequestException; Request ID: 9fa3e20a-b484-4994-97bc-a73291ecdae3; Proxy: null)'.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Representative: fixture elasticmapreduce-multi-master-placement-config-mistakes, step task#8 - RunJobFlow with a PlacementGroupConfigs entry whose InstanceRole is not MASTER and expect 400 ValidationException 'Placement group supports only the following instance role: [MASTER]'; the emulator answers 200 with a JobFlowId. The same refusal is exercised by elasticmapreduce-multi-master-topology-release-and-type-mistakes (task#12/#13/#15) and elasticmapreduce-perf-diagnosis-spark-cluster-recreate-refusals (task#9/#10).
4. Fixture emr-studio-create-invalid-vpc-error, step task#3 - CreateStudio with a VpcId that does not resolve and compare the 400 body's Message: it must name the rejected VpcId and carry this response's own request id, not a hard-coded one.
