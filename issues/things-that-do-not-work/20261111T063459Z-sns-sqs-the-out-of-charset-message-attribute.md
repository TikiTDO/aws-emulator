Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected
Issue-Key: sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected|run#14|AssertionError|assert not wrong, f"attribute names outside the charset were not refused as AWS does: {wrong!r}"
Issue-Commit: c43f13612f17253679972bb58b777ab0e011820f

# sns__sqs: the out-of-charset message-attribute-name refusal message is re-worded (Publish's ParameterValueInvalid text is not AWS's)

> _Note: this issue was identified against the code base at commit `c43f13612f17253679972bb58b777ab0e011820f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One root cause.

Cause 1 of 1: Publish's refusal of a message-attribute name containing a character outside the allowed set carries a message AWS does not emit.
AWS: Publish refuses an attribute name holding any character outside alphanumerics, hyphen, underscore and period with ParameterValueInvalid / HTTP 400 (Smithy: Publish -> InvalidParameterValueException, awsQueryError code "ParameterValueInvalid", httpError 400) and delivers nothing. The message is "Invalid non-alphanumeric character '<ref>' was found in the message attribute name. Can only include alphanumeric characters, hyphens, underscores, or dots." — everything except <ref> is fixed text, and <ref> is the offending character written as an unpadded uppercase-hex character reference taken from the request ('#x20' for a space, '#x9' for a tab, '#x23', '#x2F', '#x3A', '#x7E'). Allowed punctuation and non-ASCII letters (e.g. "A-b_c.d9", "café") are accepted and delivered under the published names.
Emulator: the error code, status and the hex reference are right, but the sentence is restructured: "Message attribute name contains invalid non-alphanumeric character '<ref>'. You can use only alphanumeric characters, hyphens (-), underscores (_), or periods (.)."
Why: every out-of-charset refusal shares one error code, so the message is the only thing a client can key on to learn which rule it broke; this one is not a string AWS ever serves, and it also renames the allowed set ("periods (.)" vs "dots").
Fix: the message-attribute-name charset validation message in the sns Publish validation path (same validator as the name length/period rules).
Traces: sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected.

## Evidence
AssertionError: attribute names outside the charset were not refused as AWS does: for 'a b', 'a\tb', 'a#b', 'a/b', 'a:b', 'a~b' the emulator answered ('ParameterValueInvalid', "Message attribute name contains invalid non-alphanumeric character '#x20'. You can use only alphanumeric characters, hyphens (-), underscores (_), or periods (.).") and its siblings for '#x9', '#x23', '#x2F', '#x3A', '#x7E'.
Expected, per character: ('ParameterValueInvalid', "Invalid non-alphanumeric character '#x20' was found in the message attribute name. Can only include alphanumeric characters, hyphens, underscores, or dots.").

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateTopic, create an SQS queue with a policy allowing sns.amazonaws.com, Subscribe the queue.
3. Publish with a single String message attribute whose NAME is 'a b', then 'a\tb', 'a#b', 'a/b', 'a:b', 'a~b'; read Error.Code and Error.Message of each refusal.
4. Replay check sns__sqs.message-attribute-name-outside-the-allowed-charset-rejected.
