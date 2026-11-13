Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering
Issue-Key: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering|run#16|TimeoutError|raise TimeoutError("the admitted records reaching the target: the run budget ended first; not graded")
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# iam__kinesis__pipes__sqs: a RUNNING pipe with a Kinesis source admits no record to its SQS target, so data-field filtering and InputTemplate rendering never happen

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
One net-new root cause (1 of 1).

Cause 1 of 1: a pipe whose Source is a Kinesis stream and whose Target is an SQS queue reports RUNNING but never consumes the stream, so the records its FilterCriteria admits never reach the target and the rendered InputTemplate is never observable.

AWS: recorded behavior only for the runtime — Smithy (services/service-com-amazonaws-pipes-pipes) models CreatePipe/StartPipe with Source, SourceParameters.KinesisStreamParameters, FilterCriteria.Filters[].Pattern, Target and TargetParameters.InputTemplate, and the model states no runtime semantics. The recording shows that once DescribePipe reports CurrentState RUNNING, the pipe polls the Kinesis shard, base64-decodes each record's data, admits only the records matching the filter pattern on the decoded data field, renders the InputTemplate against each admitted record, and sends the rendered body to the SQS target; non-matching records are dropped and never appear on the target.

Emulator: nothing is ever delivered. The check waits for the admitted records on the target queue and ends with TimeoutError: "the admitted records reaching the target: the run budget ended first; not graded" — the target queue stays empty for the whole budget, so neither the filter decision nor the template rendering can be judged.

Why: a Kinesis-to-SQS pipe is inert. A customer gets a pipe that reports RUNNING and silently consumes nothing: no delivery, no filtering, no transformation, and no error to diagnose.

Fix: the pipes crate's source poller for Kinesis streams (services/service-com-amazonaws-pipes-pipes) — the shard-iterator read loop, the filter stage over the decoded data field, the InputTemplate rendering stage and the SQS target send.

Related but not covering: open issue 20261109T163411Z-iam-kinesis-pipes-sqs-an-eventbridge-pipe-with-a-kinesis-source-delivers.md describes the same shape but does not carry this check's replay key, and 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md owns a different key.

Traces: iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering.

## Evidence
Expected: after the pipe reaches RUNNING, PutRecords of a matching and a non-matching record leads to exactly the matching record's rendered InputTemplate body arriving on the SQS target, and the non-matching record never arriving.
Actual: the target queue stays empty for the whole wait; TimeoutError "the admitted records reaching the target: the run budget ended first; not graded" (check_iam__kinesis__pipes__sqs_...py line 66).

## Steps to Reproduce
1. CreateStream (Kinesis) and CreateQueue (SQS); create a role trusted by pipes.amazonaws.com granting the stream read actions and sqs:SendMessage on the queue.
2. CreatePipe with Source=<stream arn>, SourceParameters.KinesisStreamParameters(StartingPosition), FilterCriteria whose pattern matches a field inside the records' decoded data, Target=<queue arn> and TargetParameters.InputTemplate referencing that field; StartPipe and wait for CurrentState RUNNING.
3. PutRecord(s) with one matching and one non-matching payload; poll the target queue.
4. Nothing arrives. Replay iam__kinesis__pipes__sqs.kinesis-source-filter-on-decoded-data-field-and-input-template-rendering.
