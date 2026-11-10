Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant
Issue-Key: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant|run#17|AssertionError|assert trio == ("OK", "OK", want_refusal), (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# iam__sns: SNS Publish's AuthorizationError blames the resource-based policy where AWS blames the identity-based policy

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: when an IAM principal holds neither an identity-based grant nor a resource-based grant for sns:Publish, SNS refuses the Publish with the right code and status but the wrong final clause of the message. Failing check: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant.

AWS: Smithy (sns 2010-03-31) Publish declares AuthorizationErrorException, awsQueryError code `AuthorizationError`, HTTP 403. For a principal with no identity-based permission the message is: "User: <caller ARN> is not authorized to perform: SNS:Publish on resource: <topic ARN> because no identity-based policy allows the SNS:Publish action". Fixed text (the requirement): "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action". Variable parts, taken from the request and from state: the caller's ARN (here an IAM user ARN arn:aws:iam::<account>:user/<user-name>), the topic ARN, and the action name (SNS:Publish). The emulator's status and error code are already correct — only the policy-type clause diverges.

Emulator: the same message is returned with "because no resource-based policy allows the SNS:Publish action". Everything before that clause matches byte for byte; only 'resource-based' appears where AWS says 'identity-based'.

Why: the refusal points the caller at the wrong policy. An operator or an automated remediation reads 'no resource-based policy allows …' and edits the topic policy, when AWS is saying the principal has no identity-based permission at all; any client that asserts on, logs, classifies or auto-remediates from this message text disagrees with real AWS. The check's two positive controls (a principal granted by the topic policy, and one granted by an identity policy) both succeed, so only the denial wording is wrong.

Fix: the SNS Publish authorization path's error-message construction — select the 'no identity-based policy allows the <action> action' clause when the caller holds no identity-based grant, and reserve the resource-based wording for the cases AWS words that way.

Note: open issue 20261110T210703Z-lambda-sns-sqs-sns-subscribe-s-authorizationerror-message.md reports the same wrong clause for the Subscribe operation under a different service heading; it does not cover Publish and does not carry this check's replay key, but one fix in the shared message builder may close both.

Traces: the failing check id above.

## Evidence
Expected (AWS): ('AuthorizationError', 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: SNS:Publish on resource: arn:aws:sns:<region>:<account>:<topic> because no identity-based policy allows the SNS:Publish action').
Actual (emulator): ('AuthorizationError', 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: SNS:Publish on resource: arn:aws:sns:<region>:<account>:<topic> because no resource-based policy allows the SNS:Publish action'). The user name, topic name, account and region in the recorded run fill the template; the 'resource-based' vs 'identity-based' clause is the defect. The check's other two samples (topic-policy-granted user, identity-policy-granted user) both returned OK.

## Steps to Reproduce
1. CreateTopic.
2. CreateUser with NO policies and CreateAccessKey for it; wait until the key signs calls.
3. Leave the topic's default policy in place (no statement naming that user) and attach no identity policy to the user.
4. Call Publish(TopicArn, Message) signed as that user and compare the AuthorizationError message's final clause: 'because no identity-based policy allows the SNS:Publish action' (AWS) vs 'because no resource-based policy allows the SNS:Publish action' (emulator).
