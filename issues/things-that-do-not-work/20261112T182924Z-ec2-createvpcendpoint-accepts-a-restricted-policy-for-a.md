Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-vpce-traj-iot-data-endpoint-create-refusals
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVpcEndpoint|4xx->2xx|$.Response
Issue-Key: ec2|CreateVpcEndpoint|4xx->2xx|status
Issue-Related: 20261109T225653Z-ec2-vpc-endpoint-plane-missing-endpoint-policy-refusal.md
Issue-Commit: 9f30cc5f50e7736550fc9d928f8a80b554132b0d

# ec2 CreateVpcEndpoint accepts a restricted policy for a service that only supports full access

> _Note: this issue was identified against the code base at commit `9f30cc5f50e7736550fc9d928f8a80b554132b0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Cause 1 of 1: fixture ec2-vpce-traj-iot-data-endpoint-create-refusals, step task#4.
AWS: CreateVpcEndpoint accepts PolicyDocument (Smithy: CreateVpcEndpointRequest.PolicyDocument, String; VpcEndpointType, ServiceName) but refuses a non-full-access policy for an interface endpoint whose service only supports the full-access policy. Recorded behavior for this step: HTTP 400 with an ec2Query error document whose Errors.Error.Code is 'InvalidParameter' and whose Message is "Service <serviceName> only supports the full-access endpoint policy." - the fixed text is "Service " ... " only supports the full-access endpoint policy." and the variable part is the endpoint service name taken from the request's ServiceName (here com.amazonaws.us-west-2.iot.data); no endpoint is created.
Emulator: answers HTTP 200 and creates the endpoint, returning a CreateVpcEndpointResponse with a vpcEndpoint element (including generated dnsEntrySet entries) and no error at all.
Why: the client believes a restricted endpoint policy was accepted and is in force. On AWS the call fails and the endpoint does not exist; against the emulator the client proceeds with an endpoint it thinks is policy-restricted, so a negative test for this guardrail passes and a security assumption (that the restriction was honoured) is silently wrong. The fixture's later teardown also diverges because an endpoint that should never have existed does.
Fix: CreateVpcEndpoint's input validation in the VPC-endpoint operation - the policy-document check against the endpoint service's supported policy, raising the ec2Query error InvalidParameter with the message above before any state is written.

## Evidence
ec2-vpce-traj-iot-data-endpoint-create-refusals#task#4, CreateVpcEndpoint: expected status 400, got 200. Expected body $.Response = {'Errors': {'Error': {'Code': 'InvalidParameter', 'Message': 'Service com.amazonaws.us-west-2.iot.data only supports the full-access endpoint policy.'}}, 'RequestID': <per-run>} - got None. Instead the emulator returned $.CreateVpcEndpointResponse with vpcEndpoint.dnsEntrySet.item[0].dnsName and item[1].dnsName populated (generated vpce- names) where the recording has no such response at all. divergence_key: service=ec2|CreateVpcEndpoint|status=400->200|keys=$.Response,status.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay fixture ec2-vpce-traj-iot-data-endpoint-create-refusals through task#4: CreateVpcEndpoint with VpcEndpointType=Interface, ServiceName=com.amazonaws.us-west-2.iot.data and a PolicyDocument that is not the full-access policy.
3. The emulator answers HTTP 200 and creates the endpoint; AWS answers HTTP 400 with Code 'InvalidParameter' and Message "Service com.amazonaws.us-west-2.iot.data only supports the full-access endpoint policy."
