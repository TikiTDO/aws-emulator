Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only
Issue-Key: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only|_await_notification#23|TimeoutError|raise TimeoutError(f"{what} on {queue_url!r}: the run budget ended first; not graded")
Issue-Commit: 004a13bc6b62a404600c0a09686b1d7dd87c7440

# A deployment that reaches Succeeded publishes no DeploymentSuccess trigger notification to its subscribed SNS topic

> _Note: this issue was identified against the code base at commit `004a13bc6b62a404600c0a09686b1d7dd87c7440`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentGroup

## Description
Related open issue: 20261025T013642Z-codedeploy-sns-sqs-deployment-group-triggerconfigurations-are-stored-but-never-publish.md describes this same root cause and lists this same check id, but the suite did not mark this failure as owned by it (the failure signature moved), and that issue explicitly recorded that its evidence was only a 60 s timeout that 'does not pin the failure point' and asked for a replay that gets past the deployment. This run is that replay: the deployment now reaches 'Succeeded' and the notification still never arrives, which confirms the publish path is missing rather than slow. A fixer may merge this into that issue; it must not close either without re-replaying the check id below.

Cause 1 of 1: a deployment group's triggerConfigurations round-trip through GetDeploymentGroup, but no notification is published to the configured SNS topic when a deployment reaches a matching terminal outcome.

AWS: Smithy (com.amazonaws.codedeploy#CodeDeploy_20141006): CreateDeploymentGroup/UpdateDeploymentGroup accept `triggerConfigurations` (list of TriggerConfig), documented as 'Information about triggers to create when the deployment group is created'; TriggerConfig.triggerTargetArn is 'The Amazon Resource Name (ARN) of the Amazon Simple Notification Service topic through which notifications about deployment or instance events are sent', TriggerConfig.triggerName is 'The name of the notification trigger', and TriggerConfig.triggerEvents is 'The event type or types for which notifications are triggered' with TriggerEventType = DeploymentStart | DeploymentSuccess | DeploymentFailure | DeploymentStop | DeploymentRollback | DeploymentReady | InstanceStart | InstanceSuccess | InstanceFailure | InstanceReady. Recorded behavior (the check's measurement header, us-west-2): when a deployment reaches a terminal outcome, CodeDeploy publishes to each trigger whose triggerEvents contain that outcome and to no other trigger; the message is JSON inside the standard SNS envelope's Message field carrying deploymentId, applicationName, deploymentGroupName, status ('succeeded'/'failed') and region, with eventTriggerName echoing the trigger CONFIGURATION's triggerName (not the triggerEvents value). Setting triggerConfigurations also publishes a one-time non-JSON verification notification to each topic. The individual ids, names and latencies are this run's values; the requirement is that a matching outcome publishes to the subscribed topic and a non-matching outcome does not.

Emulator: the trigger list is accepted and echoed back by GetDeploymentGroup correctly (that assertion passes), and the deployment reaches 'Succeeded' (that assertion passes), but nothing is ever delivered to the success topic's subscribed SQS queue: ReceiveMessage is polled until the run budget is exhausted and no message carrying the deployment's deploymentId ever arrives. No publish happens at any point in the deployment lifecycle, and the one-time verification notification at CreateDeploymentGroup time is absent too.

Why: a client that wires CodeDeploy notifications to SNS (and onward to SQS, Lambda, chat or a pipeline gate) sees deployments complete with no notification at all, so a notification-gated release pipeline silently never advances. Both halves of the contract are unusable: the positive (the matching topic receives a message naming the deployment) and the negative (a topic subscribed to a different event receives nothing) are indistinguishable, so event-filter logic cannot be tested against this emulator — an 'empty queue' result proves only that nothing is wired.

Fix: the deployment lifecycle/transition code of the codedeploy service (services/service-com-amazonaws-codedeploy-codedeploy-20141006 — the deployment state machine in src/state.rs, plus the CreateDeploymentGroup/UpdateDeploymentGroup handlers for the one-time verification notification). On each lifecycle outcome it must match the group's stored triggerConfigurations by triggerEvents and publish the JSON notification body to every matching triggerTargetArn via the SNS service, instead of storing the list and reading it back only in the GetDeploymentGroup projection.

Traces: codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only.

## Evidence
Suite: simparity-behavioral-ga-prod-v1. Per-service: codedeploy__sns__sqs 0/2... (reported 0/1 (0%), 1 failed).

[error] codedeploy__sns__sqs.deployment-trigger-delivers-to-the-subscribed-event-topic-only
  File "check_codedeploy__sns__sqs_deployment_trigger_delivers_to_the_subscribed_event_topic_only.py", line 107, in run
    body = _await_notification(
  File "...", line 253, in _await_notification
    raise TimeoutError(f"{what} on {queue_url!r}: the run budget ended first; not graded")
  TimeoutError: the DeploymentSuccess notification on '<success queue url>': the run budget ended first; not graded

Progress reached before the failure (each an assertion that PASSED, so each is emulator behavior that is correct): GetDeploymentGroup echoed triggerConfigurations == {'sim2real-on-success': ['DeploymentSuccess'], 'sim2real-on-failure': ['DeploymentFailure']}; the deployment created with a valid AppSpecContent revision reached deploymentInfo.status 'Succeeded'.

Expected (AWS): after that deployment reaches 'Succeeded', ReceiveMessage on the queue subscribed to the success topic returns a message whose SNS envelope Message field is JSON with deploymentId == that deployment id, applicationName == the application, deploymentGroupName == the group, eventTriggerName == 'sim2real-on-success' (the configured triggerName) and status 'succeeded', plus the region; the failure topic's queue stays empty (ApproximateNumberOfMessages == '0').

Observed (emulator): no message at all on the success queue for the whole remaining run budget; the queue never receives a notification carrying a deploymentId minted by this run, and no trigger-verification notification arrived when the group was created either. Note the harness classed its own wait as 'not graded' because the run budget expired during the poll — the graded part of the evidence is that the publish is absent after a Succeeded deployment, with the trigger configuration confirmed stored.

## Steps to Reproduce
1. sns CreateTopic T_success and T_failure; sqs CreateQueue Q_success and Q_failure; set each queue's Policy to allow sns.amazonaws.com SendMessage from its topic; Subscribe each queue's ARN to its topic (protocol 'sqs', raw delivery off).
2. CreateApplication(computePlatform='Lambda'); create a Lambda function with versions V1..V3 and an alias A at V1.
3. CreateDeploymentGroup(..., triggerConfigurations=[{triggerName:'on-success', triggerTargetArn:T_success, triggerEvents:['DeploymentSuccess']}, {triggerName:'on-failure', triggerTargetArn:T_failure, triggerEvents:['DeploymentFailure']}]).
4. GetDeploymentGroup: both triggers are echoed (emulator matches AWS here).
5. Observe Q_success/Q_failure right after step 3: AWS delivers a one-time trigger-verification notification to each topic; the emulator delivers none.
6. CreateDeployment with a valid AppSpecContent revision (alias A, CurrentVersion V1, TargetVersion V2) and poll GetDeployment until status == 'Succeeded'.
7. ReceiveMessage(Q_success, WaitTimeSeconds=2) repeatedly. AWS: a JSON notification naming this deploymentId with eventTriggerName 'on-success' and status 'succeeded'. Emulator: nothing ever arrives, and ApproximateNumberOfMessages stays '0'.
8. CreateDeployment with a revision naming a non-existent alias, wait for 'Failed', then ReceiveMessage(Q_failure). AWS: a notification for that deployment on the failure topic only. Emulator: nothing on either topic.
