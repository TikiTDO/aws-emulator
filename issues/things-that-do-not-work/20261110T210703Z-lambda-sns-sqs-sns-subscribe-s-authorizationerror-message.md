Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative
Issue-Key: lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# lambda__sns__sqs: SNS Subscribe's AuthorizationError message blames the resource-based policy where AWS blames the identity-based policy

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Cause 1 of 1: the AuthorizationError message SNS Subscribe returns for an ungranted principal names the wrong policy type. Failing check: lambda__sns__sqs.sns-sqs-cross-account-missing-topic-policy-negative.

AWS: Smithy: Subscribe declares AuthorizationErrorException (error code AuthorizationError, HTTP 403). For an assumed-role principal that holds no identity-based grant for sns:Subscribe and is not named by the topic policy, the message is: "User: <caller ARN> is not authorized to perform: SNS:Subscribe on resource: <topic ARN> because no identity-based policy allows the SNS:Subscribe action". Fixed text (requirement): "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action". Variable parts taken from the request and state: the caller's ARN (arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>), the topic ARN, and the action name (SNS:Subscribe). The error code and HTTP status are correct in the emulator; only the clause naming the policy type diverges.

Emulator: the same message is emitted with "because no resource-based policy allows the SNS:Subscribe action" instead of "because no identity-based policy allows the SNS:Subscribe action" — i.e. the whole prefix matches byte for byte and only the policy-type clause differs.

Why: a client (or an operator) diagnosing the denial is sent to the wrong policy: it will add a statement to the topic's resource policy when AWS is telling it the principal has no identity-based permission at all, and any client asserting on, logging or classifying this message text disagrees with real AWS.

Fix: the SNS Subscribe authorization path's error-message construction — choose the 'no identity-based policy allows the <action> action' clause when the caller holds no identity-based grant, and reserve the resource-based wording for the cases AWS words that way.

Traces: the failing check id above.

## Evidence
Expected (AWS): AuthorizationError with message "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: SNS:Subscribe on resource: <topic arn> because no identity-based policy allows the SNS:Subscribe action".
Actual (emulator): AuthorizationError with "... on resource: <topic arn> because no resource-based policy allows the SNS:Subscribe action" (recorded instance: role sim2real-xsvc-crossprin-<suffix>, session sim2real-xsvc-session, topic arn:aws:sns:us-west-2:<account>:sim2real-xsvc-topic-<suffix> — those values fill the template, only the policy-type clause is the defect).

## Steps to Reproduce
1. CreateQueue and CreateTopic; allow sns.amazonaws.com to send to the queue.
2. Create a bare IAM role (no identity-based policy) in the same account and assume it.
3. SetTopicAttributes(Policy=...) with a policy whose only statement allows the account root, deliberately omitting the role.
4. Call Subscribe(TopicArn, Protocol="sqs", Endpoint=<queue arn>) as the assumed role.
5. Compare the AuthorizationError message's final clause: 'no identity-based policy allows the SNS:Subscribe action' (AWS) vs 'no resource-based policy allows the SNS:Subscribe action' (emulator).
