Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.destination-lifecycle logs.put-destination-invalid-target-arn-rejected logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|PutDestination|InvalidParameterException
Issue-Key: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint|_poll_until#9|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: f886c6313691e264df098b8c73e16bb38687852c

# logs: PutDestination and PutSubscriptionFilter refuse an ACTIVE Kinesis target with InvalidParameterException "Could not deliver test message to specified Kinesis stream" (3 causes)

> _Note: this issue was identified against the code base at commit `f886c6313691e264df098b8c73e16bb38687852c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestination

## Description
REOPENS 20261028T000431Z-logs-cloudwatch-logs-destinations-putsubscriptionfilter-to-a-destination-with.md: its fix left the call refused.

That issue recorded PutDestination and PutSubscriptionFilter answering 200 for a Kinesis destination and then delivering nothing; the fix that closed it added a test-message pre-check on the Kinesis target, and that pre-check now refuses the SAME calls for a stream that is demonstrably ACTIVE and a role that is correctly trusted and permissioned. Three failing checks that previously got past these calls are now stopped at them.

This bundle has 3 causes, numbered below with their own failing check ids; ALL THREE must be closed before this issue is submitted. None of the four failing checks is marked ALREADY FILED or OWNED in the replay report. Related open issues that do NOT carry these replay keys: 20261109T143515Z-cloudformation-iam-kinesis-logs-an-aws-logs-destination-the-stack-reports-as.md (a CFN-created AWS::Logs::Destination is not listed by DescribeDestinations), 20261029T182816Z-logs-startquery-and-eight-other-logs-operations-accept.md (which owns logs.put-destination-nonexistent-target-rejected), and 20261109T143515Z-iam-kinesis-logs-sts-putsubscriptionfilter-accepts-a-destination-in-another-region.md.

=== Cause 1 of 3: PutDestination refuses an ACTIVE Kinesis targetArn ===
Failing checks: logs.destination-lifecycle, logs.put-destination-invalid-target-arn-rejected
AWS: Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328) PutDestination requires destinationName (@length 1..512, pattern ^[^:*]*$), targetArn (@length min 1) and roleArn (@length min 1); it returns a Destination and models InvalidParameterException (400), OperationAbortedException (400) and ServiceUnavailableException (500). For a Kinesis data stream whose StreamStatus is ACTIVE and a roleArn whose trust policy allows sts:AssumeRole for Service logs.<region>.amazonaws.com and whose inline policy allows kinesis:PutRecord and kinesis:PutRecords on that stream ARN, AWS answers 200 with destination{destinationName, targetArn, roleArn, arn, creationTime}. The message "Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state." IS AWS's InvalidParameterException text, but only when that test delivery genuinely fails — the stream is absent or not ACTIVE (CREATING/DELETING/UPDATING), or the role is not assumable by CloudWatch Logs, or it does not permit the put.
Emulator: 400 InvalidParameterException with exactly that message, for a stream the check polled to ACTIVE via DescribeStreamSummary and a role created exactly as above — and it keeps refusing across the checks' IAM-propagation retry windows (10 attempts / ~30 s in logs.destination-lifecycle).
Why: the whole CloudWatch Logs destination control plane becomes unreachable — DescribeDestinations, PutDestinationPolicy and DeleteDestination can never be exercised because no destination can be created, and cross-account subscription is impossible. Worse, the refusal misdiagnoses: it tells the caller to check the stream's state when the stream IS ACTIVE, so a client is sent to fix a resource that is not broken. In logs.put-destination-invalid-target-arn-rejected the NEGATIVE half passes (targetArn='not-an-arn' is correctly refused InvalidParameterException 'Error parsing ARN from given role or target' and creates nothing) and it is the positive control immediately afterwards that is refused — so the emulator now refuses valid and invalid targets alike and the distinction is unobservable.
Fix: the test-message pre-check in PutDestination's validation path, and the Kinesis-target resolution it shares — resolve the targetArn against current Kinesis state, treat an ACTIVE stream reachable by an assumable, PutRecord-permitted role as deliverable, and reserve this message for a target that is genuinely undeliverable.
Traces: logs.destination-lifecycle, logs.put-destination-invalid-target-arn-rejected.

=== Cause 2 of 3: PutSubscriptionFilter refuses a direct, ACTIVE Kinesis destinationArn on the first call ===
Failing check: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events
AWS: Smithy PutSubscriptionFilter requires logGroupName, filterName, filterPattern and destinationArn, takes an optional roleArn, returns Unit (an empty 200), and models InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException (all 400) and ServiceUnavailableException (500). With destinationArn naming an ACTIVE Kinesis stream (here created with ShardCount=1 and polled ACTIVE) and roleArn a role trusted by logs.<region>.amazonaws.com with kinesis:PutRecord/PutRecords on that stream ARN and NO SourceArn condition on the trust policy, AWS accepts the call with 200 — this is exactly the wiring AWS documents for subscription delivery to Kinesis.
Emulator: 400 InvalidParameterException "Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state." on every attempt for the check's full 180 s retry window.
Why: no subscription filter can be created against a Kinesis destination at all, so the entire subscription-delivery surface downstream of it is unreachable — including the AWS/Logs ForwardedLogEvents metric under its LogGroupName/DestinationType/FilterName dimensions, which must count only the events the filter pattern MATCHED rather than the number written.
Fix: the same test-message pre-check, shared by PutSubscriptionFilter's destination validation.
Traces: logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events.

=== Cause 3 of 3: after its Kinesis target is deleted and recreated under the same name, reissuing the identical PutSubscriptionFilter is refused forever ===
Failing check: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint
AWS: recorded behavior only for the re-validation step (PutSubscriptionFilter returns Unit, so the model says nothing about destination health). A Kinesis stream recreated under the same name in the same account and region presents the IDENTICAL ARN, so a stored destinationArn resolves again untouched. CloudWatch Logs marks a subscription's destination unhealthy on a non-retryable delivery failure and re-validates it on the NEXT PutSubscriptionFilter call: reissuing the identical configuration (same logGroupName, filterName, filterPattern, destinationArn, roleArn) against the recreated ACTIVE stream is accepted with 200, and delivery resumes without the outage-window event being re-delivered.
Emulator: the FIRST PutSubscriptionFilter in this check is accepted and delivery works, so the pre-check can pass; but after kinesis DeleteStream(EnforceConsumerDeletion=True), confirmation that the stream is gone, and CreateStream under the same name polled back to ACTIVE with the identical ARN, the reissued identical call is refused 400 InvalidParameterException with the same "Could not deliver test message..." message for the full 180 s window. The emulator never re-resolves a destination that has come back; it appears to carry a sticky unhealthy/absent verdict for that ARN.
Why: a destination outage is permanently unrecoverable against the emulator. The documented repair action — reissue the filter — is refused, so the restore half of the DeliveryErrors contract (delivery resumes, and the event written during the outage is NOT re-delivered) can never be reached, and a client that follows AWS's remediation is told its freshly recreated ACTIVE stream is not ACTIVE.
Fix: destination health/stream resolution must be re-read from current Kinesis state on every PutSubscriptionFilter rather than cached per destination ARN, so a recreated target revalidates.
Traces: logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint.

## Evidence
Cause 1 — logs.destination-lifecycle (verdict: error):
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the PutDestination operation: Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state.
  Setup immediately before the call, all succeeded: kinesis CreateStream(StreamModeDetails={'StreamMode':'ON_DEMAND'}) polled via DescribeStreamSummary to StreamStatus 'ACTIVE' (120 s budget), StreamARN read from the same response; iam CreateRole with trust {Effect: Allow, Principal: {Service: 'logs.<region>.amazonaws.com'}, Action: 'sts:AssumeRole'} and iam PutRolePolicy allowing ['kinesis:PutRecord','kinesis:PutRecords'] on that stream ARN.
  Call under test: logs.put_destination(destinationName=<name>, targetArn=<stream ARN>, roleArn=<role ARN>, tags={...}), wrapped in a retry of 10 attempts x 3 s specifically for InvalidParameterException (IAM propagation).
  Expected: 200 with destination.destinationName == the submitted name and destination.targetArn == the submitted stream ARN; the destination then listed by DescribeDestinations, PutDestinationPolicy round-tripping the accessPolicy, and DeleteDestination removing it.
  Actual: InvalidParameterException on every attempt.

Cause 1 — logs.put-destination-invalid-target-arn-rejected (verdict: error):
  Same exception on PutDestination. The NEGATIVE assertions earlier in the check passed: put_destination(targetArn='not-an-arn', roleArn=<arn>) was refused with code InvalidParameterException and message 'Error parsing ARN from given role or target', and DescribeDestinations(DestinationNamePrefix=<name>) returned []. The failure is the positive control that follows (ON_DEMAND stream polled ACTIVE + a logs-trusted role with kinesis:PutRecord/PutRecords).

Cause 2 — logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events (verdict: mismatch):
  AssertionError: PutSubscriptionFilter accepts role arn:aws:iam::<account>:role/<role> for <log group> once the IAM trust policy has propagated not met within 180.0s. Last result: 'An error occurred (InvalidParameterException) when calling the PutSubscriptionFilter operation: Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state.'
  Setup: logs CreateLogGroup + CreateLogStream; kinesis CreateStream(ShardCount=1) polled to ACTIVE then tagged; iam CreateRole trusted by logs.<region>.amazonaws.com with NO SourceArn condition (the check documents that a SourceArn condition makes AWS's own test delivery fail) + PutRolePolicy allowing kinesis:PutRecord/PutRecords on the stream ARN.
  Call under test: logs.put_subscription_filter(logGroupName, filterName, filterPattern='SIM2REALMATCH', destinationArn=<stream ARN>, roleArn=<role ARN>), retried every 2 s for 180 s.
  Expected: 200 (empty body). Actual: InvalidParameterException for the whole window.

Cause 3 — logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint (verdict: mismatch):
  AssertionError: PutSubscriptionFilter re-accepts role arn:aws:iam::<account>:role/<role> for <log group> not met within 180.0s. Last result: 'An error occurred (InvalidParameterException) when calling the PutSubscriptionFilter operation: Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state.'
  Sequence: CreateStream(ShardCount=1) polled ACTIVE -> logs-trusted role -> PutSubscriptionFilter ACCEPTED (200) -> delivery verified at the Kinesis consumer -> DeleteStream(EnforceConsumerDeletion=True), polled until the stream is absent -> PutLogEvents during the outage -> CreateStream under the SAME name, and the check asserts (and this assertion passed) that the recreated stream presents the identical ARN -> reissue the IDENTICAL PutSubscriptionFilter.
  Expected: the reissued call is accepted with 200, after which delivery resumes and the outage-window event is never re-delivered.
  Actual: refused with the same message for 180 s, although the recreated stream is ACTIVE and its ARN is byte-identical to the one the first, accepted call used.

Smithy citations: PutDestination input {destinationName: DestinationName @length 1..512 @pattern ^[^:*]*$ required, targetArn: TargetArn @length min 1 required, roleArn: RoleArn @length min 1 required, tags: Tags}, output {destination: Destination}, errors [InvalidParameterException 400, OperationAbortedException 400, ServiceUnavailableException 500]. PutSubscriptionFilter input {logGroupName, filterName, filterPattern @length 0..1024, destinationArn @length min 1, roleArn, distribution, applyOnTransformedLogs}, output Unit, errors [InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException (400), ServiceUnavailableException (500)].
Stream names, role names, log-group names, account id and ARNs in the messages above are per-run values, not part of the requirement.

## Steps to Reproduce
Cause 1:
1. kinesis CreateStream(StreamName=S, StreamModeDetails={'StreamMode':'ON_DEMAND'}); poll DescribeStreamSummary until StreamStatus == 'ACTIVE'; read StreamARN.
2. iam CreateRole(R) with trust {Principal:{Service:'logs.<region>.amazonaws.com'}, Action:'sts:AssumeRole'}; iam PutRolePolicy(R) allowing kinesis:PutRecord + kinesis:PutRecords on the stream ARN.
3. logs PutDestination(destinationName=D, targetArn=<stream ARN>, roleArn=<R ARN>), retrying InvalidParameterException for ~30 s.
   Observed: 400 InvalidParameterException 'Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state.' on every attempt.
   Expected: 200 with destination{destinationName: D, targetArn: <stream ARN>, roleArn, arn, creationTime}.
4. Then logs DescribeDestinations(DestinationNamePrefix=D) -> must list D; PutDestinationPolicy(D, accessPolicy allowing {"AWS": <account id>} logs:PutSubscriptionFilter on the destination ARN) -> the policy must round-trip on DescribeDestinations; DeleteDestination(D) -> D must disappear.
5. Negative control that must keep working: PutDestination(targetArn='not-an-arn') -> 400 InvalidParameterException 'Error parsing ARN from given role or target', and DescribeDestinations(prefix) empty.

Cause 2:
1. logs CreateLogGroup(G) + CreateLogStream(G, LS).
2. kinesis CreateStream(StreamName=S, ShardCount=1); poll DescribeStream until ACTIVE.
3. iam CreateRole(R) trusted by logs.<region>.amazonaws.com with NO SourceArn condition; PutRolePolicy(R) allowing kinesis:PutRecord/PutRecords on the stream ARN.
4. logs PutSubscriptionFilter(logGroupName=G, filterName=F, filterPattern='SIM2REALMATCH', destinationArn=<stream ARN>, roleArn=<R ARN>), retrying for 180 s.
   Observed: 400 InvalidParameterException 'Could not deliver test message...' throughout. Expected: 200.

Cause 3:
1..4 as in cause 2, but note the first PutSubscriptionFilter is ACCEPTED by the emulator; confirm delivery at the Kinesis consumer.
5. kinesis DeleteStream(StreamName=S, EnforceConsumerDeletion=True); poll until DescribeStream reports the stream absent.
6. logs PutLogEvents(G, LS, [one matching event]) during the outage.
7. kinesis CreateStream(StreamName=S, ShardCount=1); poll ACTIVE; confirm the StreamARN equals the original.
8. Reissue the IDENTICAL logs PutSubscriptionFilter(G, F, 'SIM2REALMATCH', <same stream ARN>, <same role ARN>), retrying for 180 s.
   Observed: 400 InvalidParameterException 'Could not deliver test message...' throughout.
   Expected: 200; then delivery resumes for new events and the outage-window event is never re-delivered.
