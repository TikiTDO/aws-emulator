Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action ec2__iam__logs.xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied
Issue-Key: ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni|_poll_cwl#31|Timeout|time.sleep(20)
Issue-Key: ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action|_poll_cwl#33|Timeout|time.sleep(20)
Issue-Key: ec2__iam__logs.xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied|_poll_cwl#30|Timeout|time.sleep(20)
Issue-Commit: 6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf

# ec2__iam__logs: VPC Flow Logs never deliver any record to their CloudWatch Logs destination, so flow-log scope, traffic-type filtering and the delivery-role negative all hang until the budget ends

> _Note: this issue was identified against the code base at commit `6e1ef7d8ebea1acbe69ee2691b3319db9dddeedf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
One net-new root cause covering three checks. None of the three is marked ALREADY FILED or OWNED.

Cause 1 of 1: a flow log whose LogDestinationType is cloud-watch-logs produces no log events at all.
AWS: ec2 CreateFlowLogs with ResourceType VPC / Subnet / NetworkInterface, TrafficType ALL|ACCEPT|REJECT, LogDestinationType cloud-watch-logs and a DeliverLogsPermissionArn role publishes aggregated flow records into the named log group within the capture window (the documented maximum aggregation interval is 10 minutes, default 600 s; the records are one line per 5-tuple in the configured LogFormat fields). Each record's interface-id selects which ENIs appear, so an ENI-scoped flow log carries only that ENI, a subnet-scoped one only that subnet's ENIs, and a VPC-scoped one all of them; TrafficType filters on the action field; and when the delivery role may not perform logs:PutLogEvents, the flow log's DeliverLogsStatus/DeliverLogsErrorMessage reports the failure while the group stays empty. Recorded behavior for the delivery itself; Smithy models the plumbing (ec2 CreateFlowLogs, DescribeFlowLogs FlowLog.DeliverLogsStatus / DeliverLogsErrorMessage).
Emulator: no record is ever delivered to any of the three log groups, so all three checks are killed with `Error: Check timed out after 600.0s` while polling for the first event. Nothing distinguishes the ENI, subnet and VPC scopes; nothing distinguishes ACCEPT from REJECT; and the denied-delivery-role negative has no positive baseline to contrast with, so it cannot be graded either. Hypothesis: the flow-log subsystem records the FlowLog resource but has no publisher that converts observed traffic into log events.
Why: VPC Flow Logs are the primary network-visibility control. A client sees a flow log that reports as created and active but delivers nothing — so traffic is invisible, scope and TrafficType filters cannot be verified, and the security-relevant negative (a delivery role that may not write) is indistinguishable from the normal case where nothing arrives either.
Fix: the EC2 flow-log delivery path — it must emit flow records for the ENIs its resource scope contains, filtered by TrafficType, into the configured CloudWatch Logs group under the configured LogFormat, and must report delivery failure through DescribeFlowLogs' DeliverLogsStatus/DeliverLogsErrorMessage when the delivery role is not authorized.
Traces: ec2__iam__logs.log-vpc-flow-logs-scope-excludes-non-member-eni, ec2__iam__logs.log-vpc-flow-logs-traffic-type-filter-excludes-other-action, ec2__iam__logs.xsvc-ec2-3-flow-log-delivery-role-without-put-log-events-denied.

## Evidence
Cause 1 — Expected: within the aggregation window, the ENI-scoped group holds records only for that ENI, the subnet-scoped group only the subnet's ENIs, and the VPC-scoped group all of them; the ACCEPT-only flow log omits REJECT records; a flow log whose delivery role lacks logs:PutLogEvents reports a delivery error and its group stays empty. Actual: all three checks end in `Error: Check timed out after 600.0s` — not one flow-log record is ever delivered to any group.

## Steps to Reproduce
1. Create a VPC with two subnets and launch instances so traffic flows between them.
2. Create three log groups and an IAM delivery role with logs:CreateLogStream/logs:PutLogEvents.
3. ec2 CreateFlowLogs three times — ResourceType NetworkInterface (one ENI), Subnet, and VPC — with TrafficType ALL, LogDestinationType cloud-watch-logs, the role arn, and an explicit LogFormat.
4. Generate traffic, then poll logs FilterLogEvents on each group for flow records.
Observed: no events in any group (check killed at its 600 s budget). Repeat with TrafficType ACCEPT only, and with a delivery role that omits logs:PutLogEvents, for the other two checks.
