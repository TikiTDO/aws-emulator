Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3.log-vpc-flow-logs-custom-format-field-order-follows-config ec2__s3.log-vpc-flow-logs-s3-accept-and-rejected-records
Issue-Key: ec2__s3.log-vpc-flow-logs-custom-format-field-order-follows-config|_poll_s3_records#24|Timeout|time.sleep(20)
Issue-Key: ec2__s3.log-vpc-flow-logs-s3-accept-and-rejected-records|_poll_s3_records#29|Timeout|time.sleep(20)
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# VPC flow logs with an S3 destination never deliver log file objects

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
One net-new root cause, reached by two checks.

Cause 1 of 1: a flow log whose LogDestinationType is s3 never writes any log file to the destination bucket, so no record is ever observable.
AWS: recorded behavior only for the delivery itself (Smithy models CreateFlowLogs.LogDestination/LogFormat/MaxAggregationInterval but not the publishing). The recording shows flow log objects landing under the bucket's AWSLogs path within the aggregation interval, each line carrying the fields of the configured LogFormat in exactly the configured order, and both ACCEPT and REJECT records present for traffic that was allowed and denied.
Emulator: both checks end with 'Error: Check timed out after 600.0s' — no object ever appears, so neither the custom field order nor the ACCEPT/REJECT record content can be read.
Why: a client that reads its own flow logs (the normal way to audit or debug VPC traffic) gets an empty bucket forever; a custom LogFormat is accepted at create time and then has no observable effect.
Fix: the flow-log delivery path for the s3 destination — publish aggregated records as objects under the destination prefix, rendering the fields named by LogFormat in order and emitting both ACCEPT and REJECT actions.
Traces: ec2__s3.log-vpc-flow-logs-custom-format-field-order-follows-config, ec2__s3.log-vpc-flow-logs-s3-accept-and-rejected-records.

## Evidence
Check ec2__s3.log-vpc-flow-logs-custom-format-field-order-follows-config [mismatch]: Error: Check timed out after 600.0s
Check ec2__s3.log-vpc-flow-logs-s3-accept-and-rejected-records [mismatch]: Error: Check timed out after 600.0s

Expected: flow log objects under the S3 destination within the aggregation interval, with lines whose fields follow the configured LogFormat order and which include both ACCEPT and REJECT records.
Actual: no object is ever delivered; both checks exhaust their 600 s budget waiting.

## Steps to Reproduce
1. Create a VPC/subnet and a destination bucket whose policy grants the flow-logs delivery principal.
2. CreateFlowLogs with ResourceType=VPC, LogDestinationType=s3, LogDestination=<bucket arn>, a custom LogFormat and MaxAggregationInterval=60.
3. Generate traffic that is both accepted and rejected.
4. Poll ListObjectsV2 on the bucket for the aggregation interval: no object ever appears.
