Issue-Source: trace-sweep-divergence
Issue-Trace: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection
Issue-Shape-Id: com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.Description
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.ReasonCode
Issue-Key: elasticloadbalancing|DescribeInstanceHealth|2xx->2xx|$.DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member.State
Issue-Commit: 0e97705e9904239bd55bed92c4be71546a783c93

# ELB Classic DescribeInstanceHealth never reports OutOfService for an instance failing its health check

> _Note: this issue was identified against the code base at commit `0e97705e9904239bd55bed92c4be71546a783c93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceHealth

## Description
Cause 1 of 1: DescribeInstanceHealth pins every registered instance to InService and never derives an unhealthy state from the load balancer's configured health check, so an instance whose target does not answer the health check never transitions.

AWS: Smithy: DescribeInstanceHealth -> DescribeEndPointStateOutput.InstanceStates (list of InstanceState). InstanceState.State documents "Valid values: InService | OutOfService | Unknown"; InstanceState.ReasonCode documents "Information about the cause of OutOfService instances. Specifically, whether the cause is Elastic Load Balancing or the instance. Valid values: ELB | Instance | N/A"; InstanceState.Description documents the fixed set of description sentences, one of which is exactly "Instance has failed at least the UnhealthyThreshold number of health checks consecutively." On AWS, once a registered instance has failed the number of consecutive health-check probes given by the health check's UnhealthyThreshold (the HealthCheck configured by ConfigureHealthCheck), that instance's InstanceState becomes State=OutOfService, ReasonCode=Instance, Description="Instance has failed at least the UnhealthyThreshold number of health checks consecutively." — that whole sentence is fixed text with no variable parts; the InstanceId is the value from the request/state. The recording shows exactly this transition for the instance it registered, with HTTP 200.

Emulator: the same call answers HTTP 200 but with State="InService", ReasonCode="N/A", Description="N/A" for that instance, and it stays that way: the replay polled the operation 13 times over the 180s cap and the Description never entered the expected set. The health state is therefore a constant assigned at registration rather than a function of health-check outcomes; the configured HealthCheck (Target/Interval/Timeout/UnhealthyThreshold/HealthyThreshold) has no effect on what DescribeInstanceHealth reports.

Why: a client that registers an instance and then waits for it to be reported unhealthy — the standard way to validate a health-check configuration, to drive a deployment gate, or to assert that a broken backend is pulled out of rotation — waits forever and then times out, because the emulator never leaves InService. Code branching on State/ReasonCode/Description takes the healthy branch unconditionally, so failure handling and ELB-vs-Instance attribution logic is never exercised, and a test that asserts an instance goes OutOfService cannot pass against the emulator.

Fix: the ELB Classic instance-health evaluation in the load-balancer state model plus the DescribeInstanceHealth handler — the registered-instance records need a health state that is evaluated against the load balancer's configured HealthCheck (consecutive failures vs UnhealthyThreshold, consecutive successes vs HealthyThreshold) and the handler must project that state into InstanceState.State/.ReasonCode/.Description using AWS's documented value and sentence sets, instead of emitting a fixed InService/N/A/N/A triple.

Related but not the same defect: open issue 20261030T195633Z-elasticloadbalancing-elb-classic-describeinstancehealth-serializes-instancestates-as-an-empty.md covers DescribeInstanceHealth serializing InstanceStates as an empty element for a registered instance; here the element IS populated, but its health values never change. That issue does not carry this step's replay key, so this cause is filed separately.

Traces: elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection#task#8.

## Evidence
Fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection, step task#8, operation DescribeInstanceHealth, expected status 200, actual status 200.

Expected (recorded AWS): DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member = {InstanceId: <the registered instance id>, State: "OutOfService", ReasonCode: "Instance", Description: "Instance has failed at least the UnhealthyThreshold number of health checks consecutively."}

Actual (emulator): DescribeInstanceHealthResponse.DescribeInstanceHealthResult.InstanceStates.member = {InstanceId: <same instance id>, State: "InService", ReasonCode: "N/A", Description: "N/A"}

Per-field verdicts from the replay:
- $...InstanceStates.member.State: value_mismatch (expected 'OutOfService', got 'InService')
- $...InstanceStates.member.ReasonCode: value_mismatch (expected 'Instance', got 'N/A')
- $...InstanceStates.member.Description: value_mismatch (expected 'Instance has failed at least the UnhealthyThreshold number of health checks consecutively.', got 'N/A')
- $: poll capped at 180s; Description "not in ['Instance has failed at least the UnhealthyThreshold number of health checks consecutively.']" after 13 attempt(s)

The RequestId difference in the recorded bodies is a per-run value and is NOT part of this finding. In the expected Description, the entire sentence is fixed text (no interpolated values); only InstanceId varies, and it comes from the request/state.

## Steps to Reproduce
1. Replay fixture elbclassic-instance-registration-health-deregistration-and-availability-zone-rejection against the emulator.
2. The fixture creates a Classic load balancer, configures a health check (ConfigureHealthCheck with a target that does not answer and a small UnhealthyThreshold), and registers one EC2 instance with RegisterInstancesWithLoadBalancer.
3. At step task#8 it calls DescribeInstanceHealth for that load balancer and polls for the instance to be reported unhealthy.
4. Observe HTTP 200 with InstanceStates.member State=InService, ReasonCode=N/A, Description=N/A on every attempt; the poll exhausts 13 attempts / the 180s cap without ever seeing State=OutOfService, ReasonCode=Instance and Description="Instance has failed at least the UnhealthyThreshold number of health checks consecutively.".
