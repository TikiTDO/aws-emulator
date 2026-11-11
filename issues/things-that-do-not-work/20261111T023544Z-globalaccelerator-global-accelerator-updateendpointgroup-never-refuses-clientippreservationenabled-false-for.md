Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: globalaccelerator.client-ip-preservation-false-rejected-for-instance-endpoint
Issue-Shape-Id: com.amazonaws.globalaccelerator#GlobalAccelerator_V20180706
Issue-Key: globalaccelerator.client-ip-preservation-false-rejected-for-instance-endpoint|run#11|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# global-accelerator: UpdateEndpointGroup never refuses ClientIPPreservationEnabled=false for an EC2 instance endpoint — the scaffold stub answers 501 NotImplemented instead of 400 InvalidArgumentException

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateEndpointGroup

## Description
One net-new root cause, grounded in a failing behavioral check of this suite.

Related but NOT covering: open issue 20261029T185228Z-global-accelerator-4-net-new-root-causes-updateendpointgroup.md. Its Cause 4 names the same UpdateEndpointGroup scaffold stub, but it keys and reproduces only the HealthCheckIntervalSeconds refusal ('Invalid HealthCheck Interval Seconds: 20 expected values are: 10 or 30'); the endpoint-registration validation described here (the ClientIPPreservationEnabled rule for EC2-instance endpoints, its message, its no-mutation guarantee and the accepted positive control) is a separate behavior that issue does not key, and the check below was not marked owned by it.

Cause 1 of 1: registering an EC2 instance endpoint with ClientIPPreservationEnabled=false is not refused as an invalid argument, because UpdateEndpointGroup is an unimplemented scaffold stub that answers 501 for every request.
AWS: Smithy (services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/smithy-model/com-amazonaws-globalaccelerator-globalaccelerator-v20180706.json) models UpdateEndpointGroup with required EndpointGroupArn, EndpointConfigurations (list, @length 0..10) and errors AccessDeniedException(403), EndpointGroupNotFoundException(404), InternalServiceErrorException(500), InvalidArgumentException(400), LimitExceededException(403). EndpointConfiguration.ClientIPPreservationEnabled is a boolean whose documentation states client IP address preservation is supported for Application Load Balancer, Amazon EC2 instance and Network-Load-Balancer-with-security-group endpoints, and that the default is true for ALB endpoints; the model does not state the per-endpoint-type rule, so the refusal itself is recorded behavior only. The recording shows UpdateEndpointGroup with a single EndpointConfiguration whose EndpointId is an EC2 instance id and ClientIPPreservationEnabled=false answered HTTP 400, error code InvalidArgumentException, Message exactly 'Client IP Preservation cannot be set to false for EC2 instances' — the whole string is fixed text, with no instance id, ARN or Region interpolated. The refusal is also non-mutating: DescribeEndpointGroup afterwards still reports EndpointDescriptions == []. The identical call with ClientIPPreservationEnabled=true is ACCEPTED: the instance becomes the group's only endpoint and DescribeEndpointGroup reports that endpoint with ClientIPPreservationEnabled true; a subsequent UpdateEndpointGroup with EndpointConfigurations=[] deregisters it.
Emulator: every UpdateEndpointGroup request answers HTTP 501 with error code 'NotImplemented' — the handler body is still the scaffold's Err(crate::scaffolded::Scaffolded::not_implemented("UpdateEndpointGroup")). The check's whole setup (CreateAccelerator with Enabled=false, CreateListener TCP 80/80, CreateEndpointGroup in the caller's Region) succeeded, so the 501 is the update path alone. Neither the InvalidArgumentException refusal, nor its message, nor the no-mutation guarantee, nor the accepted true case, nor endpoint deregistration is observable.
Why: a client cannot register or deregister endpoints in an endpoint group at all. Code that branches on InvalidArgumentException to report 'client IP preservation cannot be disabled for this endpoint type' instead sees a 501 it has no handling for and treats a permanently invalid request as a transient server fault (501 is not retried by the SDK's standard retry policy either, so the call simply dies with an unmodelled error), and provisioning flows that register an instance endpoint and then verify ClientIPPreservationEnabled never get past the registration call.
Fix: implement UpdateEndpointGroup's handler against the existing store and remove the SCAFFOLDED marker — specifically its endpoint-configuration validation, which must reject ClientIPPreservationEnabled=false for an EC2-instance EndpointId with InvalidArgumentException and the fixed message above, leave the stored endpoint group's EndpointDescriptions untouched on refusal, and on acceptance replace the group's endpoint set with the supplied configurations (an empty list deregistering all of them) so DescribeEndpointGroup round-trips EndpointId, Weight and ClientIPPreservationEnabled.
Traces: globalaccelerator.client-ip-preservation-false-rejected-for-instance-endpoint.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator (bootstrap_signal false; globalaccelerator 2/8 passed; the other failing globalaccelerator and cloudformation__globalaccelerator checks were marked ALREADY FILED / OWNED and skipped).

[mismatch] globalaccelerator.client-ip-preservation-false-rejected-for-instance-endpoint — AssertionError at check line 218: "registering EC2 instance i-00000000000000001 with ClientIPPreservationEnabled=False returned 'NotImplemented', expected 'InvalidArgumentException'" (the instance id is a per-run value, not part of the requirement).

Expected vs actual:
- expected: HTTP 400, Error.Code 'InvalidArgumentException', Error.Message 'Client IP Preservation cannot be set to false for EC2 instances' (entirely fixed text); then DescribeEndpointGroup(...).EndpointGroup.EndpointDescriptions == []; then the same UpdateEndpointGroup with ClientIPPreservationEnabled=true succeeding and DescribeEndpointGroup reporting exactly that EndpointId with ClientIPPreservationEnabled true; then UpdateEndpointGroup with EndpointConfigurations=[] deregistering it.
- actual: HTTP 501, Error.Code 'NotImplemented' for the first UpdateEndpointGroup call, so none of the later assertions were reached.

Source confirms the stub (commit as cloned; may have drifted): services/service-com-amazonaws-globalaccelerator-globalaccelerator-v20180706/src/ops/update_endpoint_group.rs — handle() at line 176 binds its inputs with `let _ = (&input, ctx, &self.engine);` (line 183) and returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateEndpointGroup"))` at line 193, under the file's '//! SCAFFOLDED, NOT IMPLEMENTED' marker.

Model grounding: UpdateEndpointGroup errors {AccessDeniedException 403, EndpointGroupNotFoundException 404, InternalServiceErrorException 500, InvalidArgumentException 400, LimitExceededException 403}; input EndpointConfigurations @length 0..10; EndpointConfiguration.ClientIPPreservationEnabled boolean documented as supported for ALB, EC2 instance and NLB-with-security-group endpoints. 'NotImplemented' is not a modeled error of this operation.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (globalaccelerator is bootstrapped and serving).
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-run globalaccelerator.client-ip-preservation-false-rejected-for-instance-endpoint.
3. Create a registrable EC2 instance endpoint (a running t3.micro in a VPC with an internet gateway and a public IP), then CreateAccelerator(IpAddressType=IPV4, Enabled=False) -> CreateListener(Protocol=TCP, PortRanges=[80-80]) -> CreateEndpointGroup(EndpointGroupRegion=<caller region>, TrafficDialPercentage=100.0, HealthCheckPort=80, HealthCheckProtocol=TCP, HealthCheckIntervalSeconds=30, ThresholdCount=3).
4. Call UpdateEndpointGroup(EndpointGroupArn=<group>, EndpointConfigurations=[{EndpointId: <instance-id>, Weight: 128, ClientIPPreservationEnabled: false}]). Expect HTTP 400 InvalidArgumentException with Message 'Client IP Preservation cannot be set to false for EC2 instances'; observe HTTP 501 'NotImplemented'.
5. Call DescribeEndpointGroup(EndpointGroupArn=<group>) and confirm EndpointDescriptions is still [] (the refusal must not register the endpoint).
6. Positive control: repeat step 4 with ClientIPPreservationEnabled=true. Expect 200 and DescribeEndpointGroup reporting one EndpointDescription with that EndpointId and ClientIPPreservationEnabled true; observe HTTP 501 'NotImplemented' instead.
7. Call UpdateEndpointGroup(EndpointGroupArn=<group>, EndpointConfigurations=[]) and expect the endpoint to be deregistered.
