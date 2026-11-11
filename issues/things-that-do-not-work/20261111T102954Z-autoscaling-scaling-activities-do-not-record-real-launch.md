Issue-Source: trace-sweep-divergence
Issue-Trace: asglf-activity-arch-mismatch
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asglf-activity-arch-mismatch asglf-activity-instance-profile-absent asglf-disabled-kms-key-launch-cancelled asglf-activity-instance-type-invalid asglf-activity-security-group-other-vpc autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals autoscaling-asghc-traj-scaling-activities-own-group-through-deletion autoscaling-asgdx-traj-standby-park-and-return autoscaling-asgdx-traj-suspended-launch-leaves-desired-unfilled
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member.Cause
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member.Description
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member.Details
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member.StatusCode
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member.StatusMessage
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|2xx->2xx|$.TerminateInstanceInAutoScalingGroupResponse.TerminateInstanceInAutoScalingGroupResult.Activity.Cause
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|2xx->2xx|$.TerminateInstanceInAutoScalingGroupResponse.TerminateInstanceInAutoScalingGroupResult.Activity.Details
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|2xx->2xx|$.TerminateInstanceInAutoScalingGroupResponse.TerminateInstanceInAutoScalingGroupResult.Activity.Progress
Issue-Key: autoscaling|TerminateInstanceInAutoScalingGroup|2xx->2xx|$.TerminateInstanceInAutoScalingGroupResponse.TerminateInstanceInAutoScalingGroupResult
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.NextToken
Issue-Key: autoscaling|DescribeScalingActivities|4xx->2xx|$
Issue-Key: autoscaling|DescribeScalingActivities|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|DescribeScalingActivities|4xx->2xx|status
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities.member[*].StatusCode
Issue-Key: autoscaling|DescribeScalingActivities|2xx->2xx|$.DescribeScalingActivitiesResponse.DescribeScalingActivitiesResult.Activities
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# autoscaling scaling activities do not record real launch outcomes: failed launches report Successful (8 causes)

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
The emulator's scaling-activity log is synthetic: it writes one 'Successful' activity per desired-capacity change regardless of what actually happened to the instance, never writes the activities that standby transitions produce, writes activities for launches that suspended processes should have prevented, and omits the Details/StatusMessage/Progress members AWS carries. 8 net-new causes, all on autoscaling. The autoscaling Smithy model is not present in this checkout (multi-service root, no services/service-com-amazonaws-autoscaling-autoscaling/smithy-model/*.json), so every cause is grounded in recorded behavior only; the fixed text and the variable parts of each message are named per cause.

Cause 1 of 8 — a launch that EC2 rejects for an AMI/instance-type architecture mismatch is logged as a successful launch.
AWS: recorded behavior only. The activity reads StatusCode 'Failed', Description with the fixed text 'Launching a new EC2 instance.  Status Reason: ' + the EC2 failure reason + ' Launching EC2 instance failed.', StatusMessage equal to that same EC2 reason, and Details a JSON object whose keys are the fixed 'Availability Zone ID', 'Subnet ID', 'Availability Zone' and whose values come from the group's placement. Only the reason text, zone, subnet id and instance id are variable.
Emulator: StatusCode 'Successful', Description 'Launching a new EC2 instance: <instance-id>', no StatusMessage, no Details, and a Cause string of its own wording.
Why: a client that polls DescribeScalingActivities to learn why a group never reaches InService is told the launch succeeded, so unlaunchable configurations look healthy and no remediation path is ever taken.
Fix: the scale-out path's activity recording and its EC2 RunInstances error relay (handler + state), plus the Activity serializer for StatusMessage/Details.
Traces: asglf-activity-arch-mismatch#task#10 (cluster also covers asglf-activity-instance-profile-absent#task#10, asglf-disabled-kms-key-launch-cancelled#task#15).

Cause 2 of 8 — a launch with an instance type that does not exist is likewise logged Successful, and an extra terminate activity is invented.
AWS: recorded behavior only. One activity, StatusCode 'Failed', Progress 100, Description 'Launching a new EC2 instance.  Status Reason: ' + "Invalid value '<instance-type>' for InstanceType." + ' Launching EC2 instance failed.', StatusMessage the same reason, Details the placement JSON described in cause 1.
Emulator: two 'Successful' activities (a launch and a terminate of a never-running instance), none carrying StatusMessage or Details.
Why: the failure is invisible and the activity list no longer reflects the group's real history, so retry/backoff logic keyed on StatusCode never fires.
Fix: same as cause 1 — validate the launch against the EC2 instance-type catalog and record the Failed activity instead of a synthetic pair.
Traces: asglf-activity-instance-type-invalid#task#11.

Cause 3 of 8 — TerminateInstanceInAutoScalingGroup's returned Activity carries emulator-authored Cause text, the wrong Progress and no Details.
AWS: recorded behavior only. Cause is the fixed template 'At <timestamp> instance <instance-id> was taken out of service in response to a user request, shrinking the capacity from <old> to <new>.' (only the timestamp, instance id and the two capacities vary), Progress is 0 for the just-started activity, and Details is the placement JSON of cause 1.
Emulator: Cause 'a user request explicitly terminated an instance of the group: changing the desired capacity from 1 to 0.', Progress '50', Details absent.
Why: the Cause string is the documented audit record operators read and match on; a re-worded one breaks both human triage and any log scraping, and Progress 50 reports a half-finished activity AWS reports as just-begun.
Fix: the TerminateInstanceInAutoScalingGroup handler's activity construction and the Activity serializer.
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#17.

Cause 4 of 8 — a second TerminateInstanceInAutoScalingGroup returns an empty result instead of the in-progress Activity.
AWS: recorded behavior only. The response carries TerminateInstanceInAutoScalingGroupResult.Activity with StatusCode 'InProgress', Progress 0, Description 'Terminating EC2 instance: <instance-id>', an empty Cause and the placement Details JSON.
Emulator: TerminateInstanceInAutoScalingGroupResult is serialized empty (got '').
Why: the operation's only output is the activity handle; without it a client has no ActivityId to poll and cannot tell the termination was accepted.
Fix: the TerminateInstanceInAutoScalingGroup handler — always return the activity it recorded.
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#18.

Cause 5 of 8 — DescribeScalingActivities repeats the re-worded terminate Cause and omits Details.
AWS: recorded behavior only. The listed activity carries the same Cause template and Details JSON as cause 3.
Emulator: the emulator-authored Cause text, no Details member; the pagination token it emits ('next-1') is also not of the opaque shape the recording carries (noted for completeness — the token's literal value is per-run and is not the requirement).
Why: the activity history a client reads back does not match what the same account would read from AWS, so audit and diagnosis of a terminate differ.
Fix: the shared Activity state/serializer used by DescribeScalingActivities.
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#19.

Cause 6 of 8 — DescribeScalingActivities for a deleted group answers 200 with an empty list instead of refusing.
AWS: recorded behavior only. After the group is deleted, the call is refused HTTP 400 with Error.Type 'Sender', Error.Code 'ValidationError' and Message 'AutoScalingGroup name not found - AutoScalingGroup <group-name> not found' — fixed text with the requested group name as its only variable part.
Emulator: HTTP 200 with DescribeScalingActivitiesResult.Activities empty, for the whole 60 s poll window (13 attempts).
Why: a client that waits for the not-found refusal to confirm deletion never sees it and either loops to its timeout or concludes the group still exists with no activity.
Fix: the DescribeScalingActivities handler's group-existence check (it must resolve AutoScalingGroupName against state and raise ValidationError when absent).
Traces: autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#21 (cluster also covers autoscaling-asghc-traj-scaling-activities-own-group-through-deletion#task#10).

Cause 7 of 8 — EnterStandby/ExitStandby record no scaling activities.
AWS: recorded behavior only. Parking an instance records an activity with Description 'Moving EC2 instance to Standby: <instance-id>', StatusCode 'Successful', Progress 100 and Cause 'At <timestamp> instance <instance-id> was moved to standby in response to a user request, shrinking the capacity from <old> to <new>.'; returning it records Description 'Moving EC2 instance out of Standby: <instance-id>' with the matching 'was moved out of standby ... increasing the capacity from <old> to <new>.' Cause. Both carry the placement Details JSON.
Emulator: neither activity exists — DescribeScalingActivities still returns only the original launch activity.
Why: standby transitions become unobservable, so a client cannot confirm a park/return completed or correlate it with capacity changes.
Fix: the EnterStandby / ExitStandby handlers must append activities to the group's activity state.
Traces: autoscaling-asgdx-traj-standby-park-and-return#task#16.

Cause 8 of 8 — with the Launch process suspended, the emulator still records a successful launch activity.
AWS: recorded behavior only. With Launch suspended, SetDesiredCapacity records no launch activity at all: DescribeScalingActivities returns an empty Activities list and the desired capacity stays unfilled.
Emulator: an activity 'Launching a new EC2 instance: <instance-id>' with StatusCode 'Successful' is recorded, i.e. the suspended process was not honoured by the activity log.
Why: a client that checks the activity log to prove suspension took effect is told the group launched an instance it must not have launched.
Fix: the scale-out path must consult the group's SuspendedProcesses before recording (or performing) a launch activity.
Traces: autoscaling-asgdx-traj-suspended-launch-leaves-desired-unfilled#task#9 (cluster also covers autoscaling-asghc-traj-scaling-activities-own-group-through-deletion#task#11).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. All eight clusters reported owned_by empty and stub_path empty.

Cause 1 — asglf-activity-arch-mismatch#task#10, DescribeScalingActivities 200 vs 200:
  Activities.member.StatusCode: value_mismatch (expected 'Failed', got 'Successful')
  Activities.member.Description: expected "Launching a new EC2 instance.  Status Reason: The architecture 'arm64' of the specified instance type does not match the architecture 'x86_64' of the specified AMI. ... Launching EC2 instance failed.", got 'Launching a new EC2 instance: i-00000000000000001'
  Activities.member.StatusMessage: missing
  Activities.member.Details: missing (expected the {"Availability Zone ID","Subnet ID","Availability Zone"} JSON)

Cause 2 — asglf-activity-instance-type-invalid#task#11, DescribeScalingActivities 200 vs 200:
  Activities.member: type_mismatch — expected one Failed activity with Description "Launching a new EC2 instance.  Status Reason: Invalid value 't3.bogus' for InstanceType. Launching EC2 instance failed." and StatusMessage "Invalid value 't3.bogus' for InstanceType. Launching EC2 instance failed."; got two 'Successful' activities (a launch plus a terminate).

Cause 3 — autoscaling-asgdx-traj-remove-instance-min-size-and-in-use-refusals#task#17, TerminateInstanceInAutoScalingGroup 200 vs 200:
  Activity.Cause: expected 'At <ts> instance i-... was taken out of service in response to a user request, shrinking the capacity from 1 to 0.', got 'a user request explicitly terminated an instance of the group: changing the desired capacity from 1 to 0.'
  Activity.Progress: expected '0', got '50'
  Activity.Details: missing

Cause 4 — same fixture #task#18: TerminateInstanceInAutoScalingGroupResult: type_mismatch (expected {'Activity': {... 'StatusCode': 'InProgress', 'Progress': '0', 'Description': 'Terminating EC2 instance: i-...'}}, got '')

Cause 5 — same fixture #task#19, DescribeScalingActivities: Activities.member.Cause re-worded as in cause 3; Activities.member.Details missing; NextToken 'next-1' where an opaque token was recorded.

Cause 6 — same fixture #task#21, DescribeScalingActivities expected 400 got 200: expected ErrorResponse.Error {Type Sender, Code ValidationError, Message 'AutoScalingGroup name not found - AutoScalingGroup <name> not found'}; got 200 with Activities '' — poll capped at 60 s after 13 attempts.

Cause 7 — autoscaling-asgdx-traj-standby-park-and-return#task#16, DescribeScalingActivities 200 vs 200: expected the two standby activities ('Moving EC2 instance out of Standby: i-...' and 'Moving EC2 instance to Standby: i-...'), got only the initial 'Launching a new EC2 instance: i-...' activity.

Cause 8 — autoscaling-asgdx-traj-suspended-launch-leaves-desired-unfilled#task#9, DescribeScalingActivities 200 vs 200: Activities: type_mismatch (expected '', got a 'Successful' launch activity).

## Steps to Reproduce
1. Cause 1/2: create a launch template whose instance type mismatches the AMI architecture (or names a nonexistent instance type), create a group with DesiredCapacity 1 on it, then DescribeScalingActivities — the activity reports Successful with no StatusMessage/Details instead of Failed.
2. Cause 3/4/5: with one InService instance, call TerminateInstanceInAutoScalingGroup ShouldDecrementDesiredCapacity=true, then call it again, then DescribeScalingActivities — inspect Activity.Cause/Progress/Details and the empty second result.
3. Cause 6: delete the group, then poll DescribeScalingActivities for it — it answers 200 with an empty list instead of 400 ValidationError.
4. Cause 7: EnterStandby then ExitStandby on the group's instance and call DescribeScalingActivities — neither standby activity is recorded.
5. Cause 8: SuspendProcesses(Launch), SetDesiredCapacity 1, then DescribeScalingActivities — a Successful launch activity is recorded anyway.
