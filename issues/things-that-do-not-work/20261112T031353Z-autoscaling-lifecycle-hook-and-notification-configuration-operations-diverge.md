Issue-Source: trace-sweep-divergence
Issue-Trace: asg-err-lifecycle-hook-target
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: asg-err-lifecycle-hook-target autoscaling-asgdx-traj-lifecycle-hook-on-live-group-mistakes autoscaling-lifecycle-hook-operations-on-nonexistent-group autoscaling-policy-schedule-hook-notification-tag-lifecycle autoscaling-group-scoped-describes-on-missing-group autoscaling-notification-configuration-defaults-then-deleted-list-empty autoscaling-notification-configuration-on-nonexistent-group autoscaling-pagination-invalid-next-token-errors
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling|PutLifecycleHook|4xx->2xx|$.ErrorResponse
Issue-Key: autoscaling|PutLifecycleHook|4xx->2xx|status
Issue-Key: autoscaling|PutLifecycleHook|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeLifecycleHooks|2xx->4xx|$.DescribeLifecycleHooksResponse
Issue-Key: autoscaling|DescribeLifecycleHooks|2xx->4xx|status
Issue-Key: autoscaling|DeleteLifecycleHook|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeNotificationConfigurations|2xx->2xx|$.DescribeNotificationConfigurationsResponse.DescribeNotificationConfigurationsResult.NotificationConfigurations
Issue-Key: autoscaling|PutNotificationConfiguration|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: autoscaling|DescribeNotificationConfigurations|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 4c92736e4d0a5b6c9b4225b90c132864da8692c0

# autoscaling lifecycle-hook and notification-configuration operations diverge from AWS on refusal shape, message text and deletion state

> _Note: this issue was identified against the code base at commit `4c92736e4d0a5b6c9b4225b90c132864da8692c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
SEVEN net-new root causes in the Auto Scaling lifecycle-hook and notification-configuration surface, all found by wire replay of the autoscaling corpus. They share one fault: these operations do not implement AWS's refusal contract (what is refused at all, with which message) nor keep the state their reads serve in step with their writes. A fixer must close ALL SEVEN before submitting; each cause names its own fixture and step below, and every cause beyond cause 1 is listed in this issue's trace_ids.

Smithy model used throughout: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json.

=== Cause 1 of 7: PutLifecycleHook accepts a NotificationTargetARN whose vendor it cannot notify, instead of refusing it (fixture asg-err-lifecycle-hook-target, step task#8) ===
AWS: Smithy PutLifecycleHookType.NotificationTargetARN doc: "The Amazon Resource Name (ARN) of the notification target that Amazon EC2 Auto Scaling uses to notify you when an instance is in a wait state for the lifecycle hook. You can specify either an Amazon SNS topic or an Amazon SQS queue", @length 0..255. The target's vendor is therefore constrained, and the recorded response shows the refusal: HTTP 400, Error.Type Sender, Error.Code ValidationError, Error.Message "The value in 'NotificationTargetARN' is invalid. The specified vendor 's3' is not valid. Supported vendors: sqs, sns, lambda". Fixed text of that message: "The value in 'NotificationTargetARN' is invalid. The specified vendor '<vendor>' is not valid. Supported vendors: sqs, sns, lambda"; the ONLY variable part is <vendor>, the service field of the ARN submitted in the request (here s3). The member name and the supported-vendor list are fixed text.
Emulator: answers HTTP 200 with an empty PutLifecycleHookAnswer and no ErrorResponse at all — the hook is created against an unusable target.
Why: a client that mistypes or mis-templates the notification target gets no signal at create time; it believes the hook will publish, and the misconfiguration only surfaces (if ever) as silently undelivered lifecycle notifications. Any client or test asserting on the ValidationError is handed a success.
Fix: the input validation of the PutLifecycleHook handler — it must parse NotificationTargetARN's vendor and raise the ValidationError refusal above before the hook is stored.
Traces: asg-err-lifecycle-hook-target#task#8. Handler: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/put_lifecycle_hook.rs.

=== Cause 2 of 7: PutLifecycleHook's invalid-LifecycleTransition message is emulator-authored and echoes the rejected value (fixture autoscaling-asgdx-traj-lifecycle-hook-on-live-group-mistakes, step task#6) ===
AWS: the status (400) and error code match; only the message diverges. AWS's message for this error class is fixed text that names the member and the two valid values and contains NO value from the request: "'LifecycleTransition' must be one of: autoscaling:EC2_INSTANCE_LAUNCHING, autoscaling:EC2_INSTANCE_TERMINATING" (recorded behavior for this error class; the model types LifecycleTransition as a plain string and carries the same two values in LifecycleHook.LifecycleTransition's documentation, but no message text).
Emulator: serves a reworded sentence that interpolates the submitted transition: "The LifecycleTransition value autoscaling:ec2_instance_launching is not valid: it must be one of [autoscaling:EC2_INSTANCE_LAUNCHING, autoscaling:EC2_INSTANCE_TERMINATING]" — the leading clause and the bracketed list are the emulator's own wording, and autoscaling:ec2_instance_launching is the request's value.
Why: clients and CLI users that match, log or assert on the refusal text see a sentence AWS never emits; case-normalisation bugs in caller code are reported with wording that differs from AWS's.
Fix: the PutLifecycleHook validation/refusal path must emit AWS's fixed sentence without echoing the submitted value. Open BEHAVIORAL issue 20261111T132217Z-autoscaling-lifecycle-hooks-are-stored-but-never-executed.md describes this same wording defect as its cause 2, but it is grounded in behavioral check ids only and the replay reports this fixture step as unowned (no open issue carries its replay key), so the wire binding is filed here; closing one does not re-run the other.
Traces: autoscaling-asgdx-traj-lifecycle-hook-on-live-group-mistakes#task#6, plus the same cluster's autoscaling-lifecycle-hook-operations-on-nonexistent-group#task#1 and autoscaling-policy-schedule-hook-notification-tag-lifecycle#task#11.

=== Cause 3 of 7: DescribeLifecycleHooks refuses an unknown group with 400 instead of answering 200 with an empty list (fixture autoscaling-group-scoped-describes-on-missing-group, step task#1) ===
AWS: Smithy DescribeLifecycleHooks models exactly ONE error, ResourceContentionFault (errorKind server, HTTP 500) — there is no not-found refusal in its error set, and its output member LifecycleHooks is an ordinary list. The recording confirms it: for a group name that does not exist the response is HTTP 200 with DescribeLifecycleHooksResult.LifecycleHooks serialized empty.
Emulator: answers HTTP 400 with an ErrorResponse and no DescribeLifecycleHooksResponse element at all.
Why: this is a read that AWS never refuses. A client that polls DescribeLifecycleHooks to decide whether hooks exist (or that lists hooks during teardown, after the group is gone) receives a hard client error where AWS returns an empty list, so its loop aborts instead of concluding "no hooks".
Fix: the DescribeLifecycleHooks handler — resolve an unknown AutoScalingGroupName to an empty hook collection and serialize a 200 answer rather than raising a not-found refusal. Open issue 20261107T081710Z-auto-scaling-describelifecyclehooks-stub.md (unimplemented scaffold stub) and 20261107T152315Z-autoscaling-putlifecyclehook-describelifecyclehooks-do-not-distinguish-an-explicit-empty.md both touch this operation, but the replay reports this cluster as unowned and not stub-blocked, so neither covers this step.
Traces: autoscaling-group-scoped-describes-on-missing-group#task#1, plus the same cluster's autoscaling-lifecycle-hook-operations-on-nonexistent-group#task#2.

=== Cause 4 of 7: DeleteLifecycleHook's not-found refusal reports the wrong condition and interpolates a literal 'null' (fixture autoscaling-lifecycle-hook-operations-on-nonexistent-group, step task#5) ===
AWS: status (400) and error code match; only the message diverges. DeleteLifecycleHook models no not-found error (only ResourceContentionFault, server, HTTP 500), and the recording shows that a delete addressed at a hook that is not registered is refused with the message "No Lifecycle Hook found with name '<LifecycleHookName>' for group '<AutoScalingGroupName>'". Fixed text: "No Lifecycle Hook found with name '" ... "' for group '" ... "'"; the two variable parts are the LifecycleHookName and AutoScalingGroupName taken from the request. AWS reports the missing HOOK, naming both identifiers, even when the group itself is absent.
Emulator: serves "AutoScalingGroup name not found - null" — it reports a different condition (missing group rather than missing hook), and where AWS puts a request value the emulator puts the four-character string null, so the message identifies no resource at all.
Why: the refusal is unactionable. A client cannot tell which hook or group was rejected, and tooling that parses the identifier out of the message gets "null"; anything asserting AWS's hook-scoped wording fails.
Fix: the DeleteLifecycleHook handler's refusal path — raise the hook-scoped message above, filling both identifiers from the request instead of formatting an absent group name.
Traces: autoscaling-lifecycle-hook-operations-on-nonexistent-group#task#5, plus the same cluster's autoscaling-policy-schedule-hook-notification-tag-lifecycle#task#12.

=== Cause 5 of 7: a deleted notification configuration is still served by DescribeNotificationConfigurations (fixture autoscaling-notification-configuration-defaults-then-deleted-list-empty, step task#9) ===
AWS: a notification configuration registered with PutNotificationConfiguration is removed by DeleteNotificationConfiguration, so the next DescribeNotificationConfigurations for that group returns its required NotificationConfigurations member serialized EMPTY. The recording asserts exactly that: an empty NotificationConfigurations after the delete step.
Emulator: still returns one NotificationConfiguration member for the group (AutoScalingGroupName = the group, TopicARN = the topic registered earlier, NotificationType autoscaling:TEST_NOTIFICATION) — the delete does not reach the state this read serves. The earlier describe steps of the same fixture pass, so the configuration is stored correctly and only its removal is lost.
Why: deletion is unobservable. A client that unsubscribes a group from an SNS topic and then verifies the removal sees the subscription still present and either retries forever or concludes the delete failed; teardown/idempotency logic built on "describe returns empty" never converges.
Fix: the autoscaling service state behind notification configurations — DeleteNotificationConfiguration must remove the (group, topic) entry from the collection that DescribeNotificationConfigurations reads (handler: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_notification_configurations.rs).
Traces: autoscaling-notification-configuration-defaults-then-deleted-list-empty#task#9.

=== Cause 6 of 7: PutNotificationConfiguration's unknown-group refusal interpolates 'null' instead of the requested group name (fixture autoscaling-notification-configuration-on-nonexistent-group, step task#1) ===
AWS: status (400) and error code match; only the message diverges. PutNotificationConfiguration takes @required AutoScalingGroupName (XmlStringMaxLen255, @length 1..255), and the recording shows a group that does not exist refused with "AutoScalingGroup name not found - <AutoScalingGroupName>": the prefix "AutoScalingGroup name not found - " is fixed text and the single variable part is the group name copied from the request.
Emulator: serves "AutoScalingGroup name not found - null" — the correct fixed prefix with the literal string null where the requested group name belongs.
Why: the refusal names no resource, so a client configuring notifications for several groups cannot tell which call failed, and log-scraping or message-parsing clients extract "null" as the group name.
Fix: the PutNotificationConfiguration handler's not-found refusal — format the message with the AutoScalingGroupName from the request.
Traces: autoscaling-notification-configuration-on-nonexistent-group#task#1, plus the same cluster's autoscaling-policy-schedule-hook-notification-tag-lifecycle#task#13.

=== Cause 7 of 7: the InvalidNextToken message is worded in the past tense (fixture autoscaling-pagination-invalid-next-token-errors, step task#7) ===
AWS: DescribeNotificationConfigurations models InvalidNextToken (errorKind client, HTTP 400, awsQueryError code InvalidNextToken, shape doc "The NextToken value is not valid."). Status and code match; the message diverges. AWS's text is "The token '<NextToken>' is invalid" — fixed text "The token '" and "' is invalid" around the one variable part, the NextToken submitted in the request.
Emulator: serves "The token '<NextToken>' was invalid" — the same template with the verb changed from "is" to "was".
Why: an exact-match assertion or a localized/parsed error surface built on AWS's wording does not match, and the same reworded tense is likely shared by every paginated autoscaling read that raises InvalidNextToken.
Fix: the InvalidNextToken refusal in the pagination/next-token validation shared by the autoscaling describe handlers — emit AWS's present-tense fixed text.
Traces: autoscaling-pagination-invalid-next-token-errors#task#7.

## Evidence
Cause 1 — PutLifecycleHook, fixture asg-err-lifecycle-hook-target step task#8.
  Expected (AWS, recorded): HTTP 400, ErrorResponse.Error = {Type: Sender, Code: ValidationError, Message: "The value in 'NotificationTargetARN' is invalid. The specified vendor 's3' is not valid. Supported vendors: sqs, sns, lambda"}.
  Actual (emulator): HTTP 200, no ErrorResponse element (replay detail: "status: expected 400, got 200" / "$.ErrorResponse: missing").

Cause 2 — PutLifecycleHook, fixture autoscaling-asgdx-traj-lifecycle-hook-on-live-group-mistakes step task#6 (6 steps in cluster; status 400 on both sides).
  Expected message: "'LifecycleTransition' must be one of: autoscaling:EC2_INSTANCE_LAUNCHING, autoscaling:EC2_INSTANCE_TERMINATING" (all fixed text).
  Actual message: "The LifecycleTransition value autoscaling:ec2_instance_launching is not valid: it must be one of [autoscaling:EC2_INSTANCE_LAUNCHING, autoscaling:EC2_INSTANCE_TERMINATING]".

Cause 3 — DescribeLifecycleHooks, fixture autoscaling-group-scoped-describes-on-missing-group step task#1 (2 steps in cluster).
  Expected (AWS, recorded): HTTP 200, DescribeLifecycleHooksResponse.DescribeLifecycleHooksResult.LifecycleHooks = empty.
  Actual (emulator): HTTP 400, DescribeLifecycleHooksResponse absent (replay detail: "status: expected 200, got 400" / "$.DescribeLifecycleHooksResponse: missing").

Cause 4 — DeleteLifecycleHook, fixture autoscaling-lifecycle-hook-operations-on-nonexistent-group step task#5 (2 steps in cluster; status 400 on both sides).
  Expected message template: "No Lifecycle Hook found with name '<LifecycleHookName>' for group '<AutoScalingGroupName>'" (both values from the request).
  Actual message: "AutoScalingGroup name not found - null".

Cause 5 — DescribeNotificationConfigurations, fixture autoscaling-notification-configuration-defaults-then-deleted-list-empty step task#9 (status 200 on both sides).
  Expected: NotificationConfigurations serialized empty after the configuration was deleted.
  Actual: one member still present — {AutoScalingGroupName: <group>, TopicARN: <topic registered earlier>, NotificationType: autoscaling:TEST_NOTIFICATION} (replay detail: type_mismatch, expected '', got that member).

Cause 6 — PutNotificationConfiguration, fixture autoscaling-notification-configuration-on-nonexistent-group step task#1 (2 steps in cluster; status 400 on both sides).
  Expected message template: "AutoScalingGroup name not found - <AutoScalingGroupName from the request>".
  Actual message: "AutoScalingGroup name not found - null".

Cause 7 — DescribeNotificationConfigurations, fixture autoscaling-pagination-invalid-next-token-errors step task#7 (status 400 on both sides, code InvalidNextToken on both sides).
  Expected message template: "The token '<NextToken from the request>' is invalid".
  Actual message: same template with "was invalid" instead of "is invalid".

## Steps to Reproduce
Setup for the hook causes: ec2:CreateLaunchTemplate (valid ImageId, InstanceType=t3.micro); autoscaling:CreateAutoScalingGroup(G, LaunchTemplate=<lt>, MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>).

1. Cause 1: autoscaling:PutLifecycleHook(LifecycleHookName=H, AutoScalingGroupName=G, LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, NotificationTargetARN="arn:aws:s3:::<some-bucket>") — the emulator answers 200; AWS refuses 400 ValidationError "The value in 'NotificationTargetARN' is invalid. The specified vendor 's3' is not valid. Supported vendors: sqs, sns, lambda".
2. Cause 2: autoscaling:PutLifecycleHook(LifecycleHookName=H, AutoScalingGroupName=G, LifecycleTransition="autoscaling:ec2_instance_launching") and compare the 400 message to AWS's fixed sentence "'LifecycleTransition' must be one of: autoscaling:EC2_INSTANCE_LAUNCHING, autoscaling:EC2_INSTANCE_TERMINATING".
3. Cause 3: autoscaling:DescribeLifecycleHooks(AutoScalingGroupName="<a-group-that-does-not-exist>") — expect 200 with an empty LifecycleHooks list; the emulator answers 400.
4. Cause 4: autoscaling:DeleteLifecycleHook(LifecycleHookName=H, AutoScalingGroupName="<a-group-that-does-not-exist>") and compare the 400 message to "No Lifecycle Hook found with name 'H' for group '<that group>'".
5. Cause 5: autoscaling:PutNotificationConfiguration(AutoScalingGroupName=G, TopicARN=<sns topic arn>, NotificationTypes=[autoscaling:TEST_NOTIFICATION]); autoscaling:DescribeNotificationConfigurations(AutoScalingGroupNames=[G]) shows it; autoscaling:DeleteNotificationConfiguration(AutoScalingGroupName=G, TopicARN=<same topic>); autoscaling:DescribeNotificationConfigurations(AutoScalingGroupNames=[G]) — expect an empty NotificationConfigurations list; the emulator still returns the configuration.
6. Cause 6: autoscaling:PutNotificationConfiguration(AutoScalingGroupName="<a-group-that-does-not-exist>", TopicARN=<sns topic arn>, NotificationTypes=[autoscaling:TEST_NOTIFICATION]) and compare the 400 message to "AutoScalingGroup name not found - <that group name>"; the emulator interpolates null.
7. Cause 7: autoscaling:DescribeNotificationConfigurations(NextToken="not-a-real-token") and compare the 400 InvalidNextToken message to "The token 'not-a-real-token' is invalid"; the emulator says "was invalid".
8. Re-replay the representative step asg-err-lifecycle-hook-target#task#8 and every fixture#step token in this issue's trace_ids.
