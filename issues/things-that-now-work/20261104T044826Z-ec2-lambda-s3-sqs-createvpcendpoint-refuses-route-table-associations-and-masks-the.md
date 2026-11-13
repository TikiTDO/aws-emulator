Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control ec2__lambda__s3__sqs.gateway-endpoint-route-lands-only-in-the-named-route-table ec2__lambda__s3__sqs.gateway-endpoint-describe-omits-eni-and-dns-entries ec2__lambda__s3__sqs.isolated-subnet-has-no-path-to-a-cross-region-analytics-bucket ec2__lambda__s3__sqs.gateway-endpoint-for-interface-only-service-rejected-with-control ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle
Issue-Key: ec2__lambda__s3__sqs|CreateVpcEndpoint|NotImplemented
Issue-Key: ec2__lambda__s3__sqs.gateway-endpoint-for-interface-only-service-rejected-with-control|run#6|AssertionError|assert got == expected, (
Issue-Key: ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: ec2__lambda__s3__sqs.e2e-reconciliation-no-output-lifecycle|_poll#5|Timeout|time.sleep(interval_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# CreateVpcEndpoint refuses route-table associations and masks the Gateway-vs-Interface refusal; SQS event source mappings never deliver

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
Three net-new root causes.

Cause 1 of 3: CreateVpcEndpoint refuses any Gateway endpoint that names route tables.
AWS: recorded behavior only — the recordings call ec2:CreateVpcEndpoint with VpcEndpointType=Gateway and RouteTableIds and get a 200 whose vpcEndpoint carries the endpoint id, State and the associations, after which the route lands only in the named route table and DescribeVpcEndpoints omits NetworkInterfaceIds/DnsEntries for a Gateway endpoint.
Emulator: the call is refused with error code NotImplemented and message "operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator" — emulator-authored text with no AWS counterpart; AWS has no NotImplemented error for this operation.
Why: no gateway-endpoint topology can be built, so route scoping, duplicate-association rejection, the Gateway endpoint's describe shape and any "is this subnet isolated from S3?" probe are all unreachable.
Fix: the EC2 CreateVpcEndpoint handler's route-table-association branch and the VPC endpoint / route table state it writes.
Traces: ec2__lambda__s3__sqs.duplicate-gateway-endpoint-on-route-table-rejected-with-control, .gateway-endpoint-route-lands-only-in-the-named-route-table, .gateway-endpoint-describe-omits-eni-and-dns-entries, .isolated-subnet-has-no-path-to-a-cross-region-analytics-bucket.

Cause 2 of 3: the same NotImplemented refusal masks the real refusal for a Gateway endpoint on an Interface-only service.
AWS: recorded behavior only — the recording asks for a Gateway endpoint on com.amazonaws.<region>.sqs and grades the error pair; it names the expected pair exactly: code 'InvalidParameter', message 'Endpoint type (Gateway) does not match available service types ([Interface]).' The fixed text is that whole sentence; the variable parts are the requested endpoint type and the service's available types.
Emulator: it answers ('NotImplemented', 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator').
Why: a client that asks for the wrong endpoint type for a service gets an error that neither names the mistake nor matches AWS's code, so the misconfiguration is undiagnosable.
Fix: the EC2 CreateVpcEndpoint handler's endpoint-type/service-type validation, which must run and refuse with InvalidParameter before the unimplemented association path is reached.
Traces: ec2__lambda__s3__sqs.gateway-endpoint-for-interface-only-service-rejected-with-control.

Cause 3 of 3: an SQS event source mapping never delivers, so the reconciliation scenarios never settle.
AWS: recorded behavior only — the recordings send messages to the queue and then observe the handler writing one far-end S3 object per invocation request id; on AWS this lands inside the suite's 60s budget.
Emulator: both checks are cut off with "Error: Check timed out after 60.0s" — the mapping reports itself Enabled but no batch is ever handed to the function, so no object is ever written.
Why: an SQS-driven worker silently processes nothing while its mapping reports healthy, and the per-request-id output a client uses to reconcile work never appears.
Fix: the Lambda event-source-mapping poller for SQS (receive/batch/invoke/delete) and the mapping state it reports.
Traces: ec2__lambda__s3__sqs.event-source-mapping-writes-one-far-end-object-per-request-id, .e2e-reconciliation-no-output-lifecycle.

## Evidence
Expected (AWS): CreateVpcEndpoint with RouteTableIds -> 200 with associations; a Gateway endpoint on an Interface-only service -> ('InvalidParameter', 'Endpoint type (Gateway) does not match available service types ([Interface]).'); the SQS mapping delivers each message to the handler.
Actual (emulator): ('NotImplemented', 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator') for both endpoint cases; 'Error: Check timed out after 60.0s' for the two mapping checks.

## Steps to Reproduce
1. CreateVpc/CreateSubnet/CreateRouteTable, then CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[...]). Observe NotImplemented.
2. CreateVpcEndpoint(VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.sqs'). Observe NotImplemented instead of InvalidParameter.
3. CreateQueue, CreateEventSourceMapping to a Lambda, SendMessage, and poll for the handler's S3 output. Observe nothing is delivered within 60s.
