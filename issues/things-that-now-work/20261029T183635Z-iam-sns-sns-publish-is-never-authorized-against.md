Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish
Issue-Key: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant|_settle#16|Timeout|time.sleep(interval_s)
Issue-Key: iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish|_settle#9|Timeout|time.sleep(interval_s)
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# iam__sns: SNS Publish is never authorized against an identity policy or a topic policy, so both remaining grant-combination checks hang without a verdict (1 cause)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: Publish performs no authorization evaluation, so neither the "topic-policy-only grant" arm nor the "neither grant" arm can be graded.
AWS: Publish (Smithy: PublishInput.TopicArn/TargetArn/Message required; modeled errors include AuthorizationErrorException at HTTP 403 with awsQueryError code AuthorizationError) is authorized by the union of the caller's identity policy and the topic's resource policy. A role with sns:Publish in its identity policy may publish even when the topic policy is silent; a role with neither grant is refused with AuthorizationError (403). The error message is AWS's authorization wording - fixed text plus the variable parts taken from the request and state: the caller's principal ARN, the action sns:Publish and the topic ARN.
Emulator: both checks run to the harness cap and are terminated at 60 s. The emulator neither refuses the ungranted publish nor completes the grant-differential, because SNS performs no IAM/resource-policy evaluation on the data plane at all; the checks cannot establish the denied/allowed boundary they exist to measure.
Why: every authorization expectation a client has on SNS is vacuous here: a role with no grant at all publishes successfully, so least-privilege work validated against this emulator ships policies that do not actually restrict anything - and a client that expects 403 AuthorizationError sees a 200 with a MessageId instead.
Fix: the Publish handler's authorization step - evaluate the caller's identity policy and the topic's resource policy (with explicit-Deny precedence) and refuse with AuthorizationErrorException (403, awsQueryError AuthorizationError) and AWS's message wording when neither admits the call. The open issue 20261025T000319Z-iam-sns-sns-performs-no-iam-authorization-publish.md owns two sibling checks of this same surface but does not carry these two replay keys, so they are filed here.
Traces: iam__sns.xsvc-iam-2-sns-publish-denied-with-neither-grant, iam__sns.xsvc-iam-2-sns-topic-policy-only-authorizes-publish (both: "Check timed out after 60.0s").

## Evidence
Expected: a role with neither an identity-policy nor a topic-policy grant is refused with AuthorizationError at HTTP 403, while a role whose grant comes only from the topic policy succeeds; actual: both checks are terminated by the harness with "Error: Check timed out after 60.0s" because no refusal is ever produced and the allowed/denied boundary never materialises.

## Steps to Reproduce
1. CreateTopic. CreateRole A with an identity policy that does NOT allow sns:Publish, and leave the topic policy without any grant for A.
2. Assume A and call Publish(TopicArn, Message) - expected 403 AuthorizationError, observed a success.
3. CreateRole B with no identity grant but add a topic-policy statement allowing B sns:Publish; assume B and Publish - the grant differential between A and B is not observable.
