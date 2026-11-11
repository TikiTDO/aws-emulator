Issue-Source: trace-sweep-divergence
Issue-Trace: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable eventbridgev2-update-event-source-configuration-and-description eventbridgev2-update-event-source-configuration-rejections
Issue-Shape-Id: com.amazonaws.eventbridgev2#AWSEventsV2
Issue-Key: eventbridgev2|DescribeEventSource|4xx->2xx|$
Issue-Key: eventbridgev2|DescribeEventSource|4xx->2xx|$.Message
Issue-Key: eventbridgev2|DescribeEventSource|4xx->2xx|$.__type
Issue-Key: eventbridgev2|DescribeEventSource|4xx->2xx|status
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# EventBridgeV2 deleted event sources stay readable: DescribeEventSource answers 200 forever after DeleteEventSource

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEventSource

## Description
TWO net-new causes on the bootstrapped eventbridgev2 service (sdkId EventBridgeV2, rpcv2Cbor): after DeleteEventSource, the event source is never removed from the emulator's state, so the recorded "poll until the source is gone" step never terminates - every attempt answers 200 with the full source body instead of the modeled 404. Both causes share that mechanism (DeleteEventSource is still a 501 scaffold stub) but are two distinct failing steps in two different fixtures, and BOTH must be closed and re-replayed.

Cause 1 of 2 - DescribeEventSource after a revoke-then-delete never reaches ResourceNotFoundException (fixture eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable, step task#10).
AWS: Smithy: DeleteEventSource (@idempotent) models ResourceNotFoundException (404), InvalidStateException (409) and ConcurrentModificationException (409) and returns an empty output; DescribeEventSource models ResourceNotFoundException (com.amazonaws.eventbridgev2#ResourceNotFoundException, @error client, @httpError 404, single member `Message`, doc "The resource does not exist."). Recorded behavior: once a revoked source has been deleted, describing its ARN answers HTTP 404 with `__type` `com.amazon.nexus.v20250515.exceptions#ResourceNotFoundException` and `Message` "EventSource not found: <event source ARN>" - fixed text "EventSource not found: " plus ONE variable part, the event-source ARN as requested.
Emulator: the poll runs 13 attempts and caps at 60s, every attempt answering HTTP 200 with the complete source (EventSourceArn, Name, EventBusArn, Configuration, State 'ACTIVE', CreationTime, LastModifiedTime) and neither `__type` nor `Message`. Root cause: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/delete_event_source.rs:4 carries the "SCAFFOLDED, NOT IMPLEMENTED" marker and the handler answers 501, so the record is never dropped and src/ops/describe_event_source.rs:188 keeps finding it.
Why: a client's standard "delete, then wait until the resource is gone" loop never completes against the emulator - it hangs until its own timeout and then reports the resource still present. Nothing can be built on removal: teardown is not idempotent, re-creating the same source name collides with state AWS would have cleared, and every error branch keyed on 404/ResourceNotFoundException after a delete is dead code.
Fix: the DeleteEventSource handler (`handle`) and the event-source record in the service state - remove the source (and its indices) so the read path reports the modeled ResourceNotFoundException. The auto-filed scaffold-stub issue for DeleteEventSource covers the 501 ANSWER only; this read divergence is the state consequence and must be re-replayed against this step.
Traces: eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#10. Code: src/ops/delete_event_source.rs (stub handler), src/ops/describe_event_source.rs:186-198.

Cause 2 of 2 - the same read divergence on the fixtures' teardown path (fixture eventbridgev2-update-event-source-configuration-and-description, step teardown#10; the same cluster also covers eventbridgev2-update-event-source-configuration-rejections#teardown#14).
AWS: as Cause 1 - after DeleteEventSource the recorded teardown poll expects the describe to stop answering and report error code ResourceNotFoundException (HTTP 404).
Emulator: 13 attempts, poll capped at 60s, each answering 200 with the whole source body (State 'ACTIVE', Description still the creation-time value).
Why: as Cause 1, and because this is the teardown of two other fixtures, leaked state survives into whatever runs next against the same emulator - a later create of the same name sees a source AWS would have removed.
Fix: same component as Cause 1; this step must be re-replayed separately because it is a different fixture's cleanup path.
Traces: eventbridgev2-update-event-source-configuration-and-description#teardown#10 (cluster also: eventbridgev2-update-event-source-configuration-rejections#teardown#14).

Related open issue that does NOT cover these: 20261107T005809Z-eventbridgev2-event-source-error-paths-return-the-wrong.md reports the wording of the event-source not-found message for an ARN that NEVER existed; it carries neither of the replay keys above and nothing in it exercises a delete, so it cannot close a source that is still present after a successful delete.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; 200 steps, 63 passed, 109 mismatched, 28 blocked). Both clusters are service=eventbridgev2, operation DescribeEventSource, verdict mismatch, expected_status 404 / actual_status 200.

Cause 1 - eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#10:
  status: expected 404, got 200
  $: poll capped at 60s; poll: error code None not in ['ResourceNotFoundException'] after 13 attempt(s)
  expected {'__type': 'com.amazon.nexus.v20250515.exceptions#ResourceNotFoundException', 'Message': 'EventSource not found: arn:aws:events:us-west-2:000000000000:event-sourcev2/aws.service/<name>/<id>'}
  got {'EventSourceArn': ..., 'Name': ..., 'EventBusArn': ..., 'Configuration': {'AwsServiceEventsConfiguration': {'AwsService': 'aws.ec2'}}, 'State': 'ACTIVE', 'CreationTime': ..., 'LastModifiedTime': ...}
  $.__type: missing; $.Message: missing
  (the ARNs, ids and epoch values in the recorded/actual bodies are per-run; the 404 + error code + the message's fixed text and its one ARN-valued part are the requirement)

Cause 2 - eventbridgev2-update-event-source-configuration-and-description#teardown#10 (affected_count 2, also eventbridgev2-update-event-source-configuration-rejections#teardown#14):
  status: expected 404, got 200
  $: poll capped at 60s; poll: error code None not in ['ResourceNotFoundException'] after 13 attempt(s)
  got the full DescribeEventSource body (State 'ACTIVE', Description 'first description', Configuration AwsService 'aws.ec2')

Emulator source: services/service-com-amazonaws-eventbridgev2-awseventsv2/src/ops/delete_event_source.rs:4 "SCAFFOLDED, NOT IMPLEMENTED" (handler answers 501 NotImplemented, nothing is deleted); src/ops/describe_event_source.rs:188 `state.event_source(&input.event_source_arn)` still resolves, and 189-197 is the only path that would produce ResourceNotFoundException.

Model references (services/service-com-amazonaws-eventbridgev2-awseventsv2/smithy-model/com-amazonaws-eventbridgev2-awseventsv2.json): DeleteEventSource {EventSourceArn @required} -> empty output, errors ConcurrentModificationException 409, InvalidStateException 409, ResourceNotFoundException 404, AccessDeniedException 403, InternalException 500, InvalidInputException 400, ThrottlingException 429; DescribeEventSource errors include ResourceNotFoundException (404, member `Message`).

## Steps to Reproduce
1. git_clone the repo, boot the emulator built from it and replay trace set simparity-wire-aws-v1.

Cause 1 (eventbridgev2-revoked-event-source-refuses-update-and-stays-deletable#task#10): CreateEventBus (wait ACTIVE), CreateEventSource on it with Configuration.AwsServiceEventsConfiguration.AwsService = aws.ec2, RevokeResource(Arn=<source arn>), DeleteEventSource(EventSourceArn=<source arn>) - the emulator answers 501 NotImplemented. Now poll DescribeEventSource on that ARN: AWS answers 404 ResourceNotFoundException with Message 'EventSource not found: <source arn>'; the emulator answers 200 with the full source on all 13 attempts until the 60s poll cap.

Cause 2 (eventbridgev2-update-event-source-configuration-and-description#teardown#10): run the same fixture's teardown - DeleteEventSource on the source it created, then poll DescribeEventSource for the not-found error. The emulator keeps answering 200 with the source body (State ACTIVE) and the poll caps out.
