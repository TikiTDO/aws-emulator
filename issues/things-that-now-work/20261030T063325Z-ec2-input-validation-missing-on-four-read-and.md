Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-flowlog-idformat-iaminstanceprofile-describes
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-flowlog-idformat-iaminstanceprofile-describes ec2-gateway-vpc-endpoint-lifecycle ec2-host-reservation-describes ec2-image-describes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeIamInstanceProfileAssociations|4xx->2xx|$.Response
Issue-Key: ec2|DescribeIamInstanceProfileAssociations|4xx->2xx|status
Issue-Key: ec2|CreateVpcEndpoint|4xx->2xx|$.Response
Issue-Key: ec2|CreateVpcEndpoint|4xx->2xx|status
Issue-Key: ec2|DescribeHostReservationOfferings|4xx->2xx|$.Response
Issue-Key: ec2|DescribeHostReservationOfferings|4xx->2xx|status
Issue-Key: ec2|DescribeImageReferences|4xx->2xx|$.Response
Issue-Key: ec2|DescribeImageReferences|4xx->2xx|status
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 input validation missing on four read and create paths: invalid ids, filters and service names are answered 200

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIamInstanceProfileAssociations

## Description
Four net-new wire divergences share one root cause: these handlers never validate the identifier or name they are given, so a request AWS rejects with a 400 Query error is served as an empty/successful 200. This issue bundles 4 causes; all 4 must be closed.

Cause 1 of 4: DescribeIamInstanceProfileAssociations with an instance-id filter value that is not a real instance id.
AWS: recorded behavior only (the EC2 model declares no error shapes: DescribeIamInstanceProfileAssociations.errors == []; the Filters member is modeled as FilterList). The recording shows HTTP 400, Code 'InvalidParameterValue', Message "A filter with an invalid instance-id of '<value>' was given". Fixed text "A filter with an invalid instance-id of '" ... "' was given"; the value is the filter value from the request (recorded run: i-0123456789abcdef0).
Emulator: HTTP 200 with no Response error element at all.
Why: a client that mistypes a filter value gets an empty result set instead of a rejection, so it concludes the association does not exist rather than that its request was wrong.
Fix: filter-value validation in the DescribeIamInstanceProfileAssociations handler.
Traces: ec2-flowlog-idformat-iaminstanceprofile-describes#task#3.

Cause 2 of 4: CreateVpcEndpoint naming a VPC endpoint service that does not exist.
AWS: recorded behavior only - HTTP 400, Code 'InvalidServiceName', Message "The Vpc Endpoint Service '<serviceName>' does not exist". Fixed text "The Vpc Endpoint Service '" ... "' does not exist"; the value is the ServiceName from the request (recorded run: com.amazonaws.us-west-2.not-a-real-service).
Emulator: HTTP 200 - the endpoint is created against a service name that is not in the service catalog.
Why: a typo in ServiceName silently yields a dead endpoint instead of a refusal, and the negative contract for this operation is unreachable.
Fix: ServiceName resolution against the endpoint-service catalog in the CreateVpcEndpoint handler, before state is mutated.
Traces: ec2-gateway-vpc-endpoint-lifecycle#task#10.

Cause 3 of 4: DescribeHostReservationOfferings with a malformed offering id.
AWS: recorded behavior only - HTTP 400, Code 'InvalidHostReservationOfferingId.Malformed', Message "The specified host reservation Offering ID '<id>' is not valid." Fixed text "The specified host reservation Offering ID '" ... "' is not valid."; the value is the OfferingId from the request (recorded run: hro-00000000000000000).
Emulator: HTTP 200 with no error element.
Why: callers cannot distinguish 'bad id' from 'no offerings', so id-format bugs in client code go unnoticed.
Fix: OfferingId format validation in the DescribeHostReservationOfferings handler.
Traces: ec2-host-reservation-describes#task#3.

Cause 4 of 4: DescribeImageReferences with a malformed image id.
AWS: recorded behavior only - HTTP 400, Code 'InvalidAMIID.Malformed', Message "One or more ImageIds isn't valid. Verify that each ImageId is in the correct format and try again." This message is entirely fixed text - it names no request value.
Emulator: HTTP 200 with no error element.
Why: the modeled negative path for AMI id format is unreachable, so clients relying on InvalidAMIID.Malformed to detect bad input never see it.
Fix: ImageIds format validation in the DescribeImageReferences handler.
Traces: ec2-image-describes#task#3.

## Evidence
Cause 1: expected 400 with $.Response.Errors.Error = {Code: InvalidParameterValue, Message: "A filter with an invalid instance-id of 'i-0123456789abcdef0' was given"}; actual 200, $.Response missing.
Cause 2: expected 400 / InvalidServiceName / "The Vpc Endpoint Service 'com.amazonaws.us-west-2.not-a-real-service' does not exist"; actual 200, $.Response missing.
Cause 3: expected 400 / InvalidHostReservationOfferingId.Malformed / "The specified host reservation Offering ID 'hro-00000000000000000' is not valid."; actual 200, $.Response missing.
Cause 4: expected 400 / InvalidAMIID.Malformed / "One or more ImageIds isn't valid. Verify that each ImageId is in the correct format and try again."; actual 200, $.Response missing.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-flowlog-idformat-iaminstanceprofile-describes#task#3, ec2-gateway-vpc-endpoint-lifecycle#task#10, ec2-host-reservation-describes#task#3 and ec2-image-describes#task#3. Each step sends a request carrying an invalid id/name and must receive the 400 Query error above rather than a 200.
