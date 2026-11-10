Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes.update-pipe-desired-state-halts-then-resumes-delivery
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes.update-pipe-desired-state-halts-then-resumes-delivery|_receive_all#19|Timeout|resp = sqs.receive_message(
Issue-Commit: 85a6615925d8a8ca7c6929fc0b4b9616c3090859

# pipes: UpdatePipe(DesiredState) never drives delivery — a pipe set RUNNING via UpdatePipe neither polls its SQS source nor delivers to its target, so the STOPPED-withholds/RUNNING-resumes toggle never converges

> _Note: this issue was identified against the code base at commit `85a6615925d8a8ca7c6929fc0b4b9616c3090859`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePipe

## Description
Cause 1 of 1: UpdatePipe is the IaC path for toggling a pipe on and off (CloudFormation and other tools write DesiredState rather than calling StartPipe/StopPipe), and in the emulator that path produces no data-plane behavior at all.

AWS: UpdatePipe takes DesiredState of RequestedPipeState (RUNNING | STOPPED) and the pipe's runtime follows it: with DesiredState=RUNNING the pipe's poller receives from the SQS source, renders TargetParameters.InputTemplate and sends the result to the target, deleting the consumed source message; with DesiredState=STOPPED the poller stops, a message published afterwards is NOT delivered to the target and stays countable on the source (ApproximateNumberOfMessages reflects it); setting DesiredState back to RUNNING then delivers that withheld message. DescribePipe reports both DesiredState and CurrentState and CurrentState converges to the requested state. Smithy (services/service-com-amazonaws-pipes-pipes): UpdatePipe input DesiredState -> RequestedPipeState, RoleArn required, PUT /v1/pipes/{Name} -> 200; output carries DesiredState and CurrentState; DescribePipe exposes the same pair. The delivery/withholding semantics themselves are not expressed in the model — recorded behavior only: the recording shows the pre-stop control message arriving on the target queue, the post-stop message absent from the target and still counted on the source, and that same message arriving on the target after the resume.

Emulator: the check never gets past its positive control. CreatePipe(DesiredState=RUNNING) succeeds and DescribePipe reports CurrentState=RUNNING, but a message sent to the source queue never appears on the target queue, so the check hangs polling the target and the harness kills it ("Check timed out after 60.0s"). Because the RUNNING leg never delivers, the STOPPED leg (withholding, source retention) and the RUNNING-again leg (resumed delivery of the withheld message) are unobservable: DesiredState is stored and echoed as pure metadata with no poller attached to it.

Why: a client that toggles a pipe via UpdatePipe gets a pipe that reports RUNNING and moves no data. Nothing ever reaches the target, so consumers downstream of the pipe see no traffic and a test or deployment that gates on first delivery after a resume hangs instead of failing fast. Conversely, no code path can confirm that STOPPED actually withholds, so a stop is indistinguishable from a silently-broken pipe.

Fix: the pipes data-plane runtime and its binding to the pipe's state — the per-pipe poller/dispatcher that must be started and stopped by the DesiredState transition that UpdatePipe records (not only by StartPipe/StopPipe), plus the source-receive / InputTemplate-render / target-send path it drives. Related open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md describes the absent runtime for StartPipe-driven pipes but does not carry this check's replay key; this finding is specifically the UpdatePipe-driven DesiredState toggle, including the STOPPED-withholds-then-RUNNING-resumes sequence.

Traces: behavioral check pipes.update-pipe-desired-state-halts-then-resumes-delivery (mismatch, timed out after 60.0s).

## Evidence
Expected (real AWS, per the recording): after CreatePipe(..., DesiredState="RUNNING") and CurrentState=RUNNING, sqs:SendMessage to the source queue is followed by a message on the TARGET queue whose Body is the rendered InputTemplate "<$.body>" (i.e. the source body). Then UpdatePipe(Name, RoleArn, DesiredState="STOPPED") -> DescribePipe DesiredState=STOPPED and CurrentState settles STOPPED and holds; a message sent while stopped is absent from the target for the whole observation window and the source reports ApproximateNumberOfMessages=1. Then UpdatePipe(..., DesiredState="RUNNING") -> CurrentState=RUNNING and that withheld message is delivered to the target with the same body.

Actual (emulator): CreatePipe and DescribePipe answer 200 with CurrentState=RUNNING, but no message ever arrives on the target queue for the pre-stop control send. The check blocks in its receive loop on the target queue and the harness terminates it: "Check timed out after 60.0s". No error code or status diverges — the divergence is that the data plane produces nothing, so the DesiredState toggle (STOPPED withholding, RUNNING resuming) is never exercised. DesiredState/CurrentState are stored and echoed only.

## Steps to Reproduce
1. sqs:CreateQueue twice (source and target); read QueueArn for each via GetQueueAttributes.
2. iam:CreateRole trusting pipes.amazonaws.com and PutRolePolicy granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source ARN and sqs:SendMessage on the target ARN.
3. pipes:CreatePipe(Name, RoleArn, Source=<source ARN>, SourceParameters={"SqsQueueParameters":{"BatchSize":1}}, Target=<target ARN>, TargetParameters={"InputTemplate":"<$.body>"}, DesiredState="RUNNING"); poll DescribePipe until CurrentState=RUNNING.
4. sqs:SendMessage to the source queue, then poll ReceiveMessage on the TARGET queue. Observe: nothing ever arrives (AWS delivers the body) — the check hangs here and times out.
5. pipes:UpdatePipe(Name, RoleArn, DesiredState="STOPPED"); poll DescribePipe for CurrentState=STOPPED and hold. Send another message; observe it is neither delivered (correct only incidentally) nor, in AWS, lost — the source should report ApproximateNumberOfMessages=1.
6. pipes:UpdatePipe(Name, RoleArn, DesiredState="RUNNING"); poll the target queue for the withheld message. Observe: never delivered.
