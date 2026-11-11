Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__s3__sqs__ssm.s3pointer-guest-reduces-the-whole-multipart-object-to-a-verified-checksum-and-leaves-nothing-on-failure ec2__s3__sqs__ssm.sqsfail-guest-job-failure-keeps-the-message-writes-no-partial-result-and-redrives-to-the-dlq ec2__s3__sqs__ssm.sqsjob-poisoned-job-leaves-no-partial-result-and-returns-to-the-queue-until-its-input-exists ec2__s3__sqs__ssm.sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq ec2__s3__sqs__ssm.sqsrace-two-workers-one-queue-write-every-job-result-exactly-once
Issue-Key: ec2__s3__sqs__ssm.s3pointer-guest-reduces-the-whole-multipart-object-to-a-verified-checksum-and-leaves-nothing-on-failure|_teardown#7|AssertionError|raise AssertionError((f"{found!r}; " if found else "")
Issue-Key: ec2__s3__sqs__ssm.sqsfail-guest-job-failure-keeps-the-message-writes-no-partial-result-and-redrives-to-the-dlq|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__s3__sqs__ssm.sqsjob-poisoned-job-leaves-no-partial-result-and-returns-to-the-queue-until-its-input-exists|_teardown#7|AssertionError|raise AssertionError((f"{found!r}; " if found else "")
Issue-Key: ec2__s3__sqs__ssm.sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__s3__sqs__ssm.sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__s3__sqs__ssm.sqsrace-two-workers-one-queue-write-every-job-result-exactly-once|_poll#7|Timeout|time.sleep(interval_s)
Issue-Commit: c0ced5d18fa4eb624de2042a998eca41dbf9ea10

# A launched EC2 instance never comes Online in SSM, so no guest workload ever runs

> _Note: this issue was identified against the code base at commit `c0ced5d18fa4eb624de2042a998eca41dbf9ea10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause, reached by six checks.

Cause 1 of 1: an instance launched with an SSM-managed profile never registers as a managed instance — SSM never reports it Online — so the guest program each check installs (an SQS worker that reads a job, computes a result and writes it to S3) never runs, and the instance is also never torn down.
AWS: recorded behavior only for the guest lifecycle (the Smithy models cover RunInstances/DescribeInstanceInformation/SendCommand but not the boot). The recording shows the instance reaching running, the SSM agent registering so DescribeInstanceInformation reports PingStatus Online, the user-data/command workload executing, and each job's result object appearing in S3 (and failures returning the message to the queue or the DLQ). TerminateInstances then moves it to terminated.
Emulator: the wait for the agent to come Online never succeeds — four checks abort with 'Check timed out after 60.0s/600.0s' and two with TimeoutError('the SSM agent on instance <id> coming Online: the run budget ended first; not graded'); the teardown then reports "instance <id> never reached terminated". No result object, no queue redrive and no DLQ message is ever produced.
Why: every workload pattern that runs on an instance (queue worker, poison-message redrive, two-worker exactly-once, multipart-object checksum) is unobservable: a client's instance boots nothing, SSM shows no managed instance to send a command to, and the instance does not reach terminated after TerminateInstances.
Fix: the EC2 instance lifecycle/guest execution path and its SSM registration — a running instance with an SSM profile must register so DescribeInstanceInformation reports it Online, its user-data/commands must execute, and TerminateInstances must drive it to terminated.
Note: the open issue 20261104T055008Z-ec2-s3-sqs-ssm-an-ec2-instance-never.md describes the same family for one other check in this service but does not carry these six checks' replay keys.
Traces: all six checks below.

## Evidence
Six checks under ec2__s3__sqs__ssm, all [mismatch]:
- s3pointer-guest-reduces-the-whole-multipart-object-to-a-verified-checksum-and-leaves-nothing-on-failure: AssertionError: TimeoutError('the SSM agent on instance i-<id> coming Online: the run budget ended first; not graded'); teardown could not remove everything the check created: ["instance: AssertionError('instance i-<id> never reached terminated within 1.0s')"]
- sqsjob-poisoned-job-leaves-no-partial-result-and-returns-to-the-queue-until-its-input-exists: same TimeoutError on the worker instance coming Online
- sqsfail-guest-job-failure-keeps-the-message-writes-no-partial-result-and-redrives-to-the-dlq: Error: Check timed out after 600.0s
- sqsrace-poisoned-job-writes-no-result-object-and-lands-in-dlq: Error: Check timed out after 600.0s
- sqsjob-worker-drains-queue-and-every-result-object-holds-the-independently-computed-answer: Error: Check timed out after 60.0s
- sqsrace-two-workers-one-queue-write-every-job-result-exactly-once: Error: Check timed out after 60.0s

Expected: DescribeInstanceInformation reports PingStatus Online for the launched instance; the guest drains the queue and writes its result objects; TerminateInstances drives the instance to terminated.

## Steps to Reproduce
1. Create a queue, a DLQ, a bucket and an instance profile granting SSM plus the queue/bucket actions.
2. RunInstances with that profile and user data installing the worker; wait for state running.
3. Poll ssm DescribeInstanceInformation for the instance id: it never appears with PingStatus Online.
4. Send a job message; no result object ever appears in the bucket and the message is neither consumed nor redriven.
5. TerminateInstances: the instance never reaches terminated.
