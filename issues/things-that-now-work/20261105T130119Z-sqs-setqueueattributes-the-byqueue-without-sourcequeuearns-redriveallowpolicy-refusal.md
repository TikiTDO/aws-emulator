Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sqs.redrive-allow-policy-document-validation-rejected
Issue-Shape-Id: com.amazonaws.sqs#AmazonSQS
Issue-Key: sqs.redrive-allow-policy-document-validation-rejected|run#13|AssertionError|assert msg == (f"Value {text} for parameter RedriveAllowPolicy is invalid. Reason: "
Issue-Commit: e924affd30ac1a5870106526f18d5e4efb90f778

# SQS SetQueueAttributes: the byQueue-without-sourceQueueArns RedriveAllowPolicy refusal carries an emulator-authored Reason sentence

> _Note: this issue was identified against the code base at commit `e924affd30ac1a5870106526f18d5e4efb90f778`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes

## Description
One net-new root cause in the SQS RedriveAllowPolicy validation surface. The validation itself now fires (the 10-ARN arity limit is enforced and the stored policy is left untouched — that part of the earlier, now-resolved issue 20261029T193114Z-sqs-5-net-new-causes-four-refusal-messages.md, Cause 4, behaves correctly). What remains is the wording of one of its refusal messages. This is NOT a REOPENS: the call is refused, with the right status and the right error code; only the Reason sentence diverges.

Cause 1 of 1: SetQueueAttributes refuses a RedriveAllowPolicy of redrivePermission "byQueue" that carries no sourceQueueArns with the correct code but the wrong Reason sentence.

AWS: HTTP 400 with Error.Code `InvalidParameterValue` and the message
  `Value <policy-document> for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. When you specify the byQueue permission type, you must also specify one or more sourceQueueArns values.`
Fixed text of that template: `Value ` + ` for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. When you specify the byQueue permission type, you must also specify one or more sourceQueueArns values.` The only variable part is `<policy-document>` — the RedriveAllowPolicy attribute value exactly as sent in the request (the JSON text, byte-for-byte, not a re-serialization). Smithy: sqs#SetQueueAttributes declares InvalidAttributeName (HTTP 400) and InvalidAttributeValue (HTTP 400) and does not model InvalidParameterValue, so the status/code pairing for this refusal class is recorded behavior only — the recording shows HTTP 400 / `InvalidParameterValue`, which the emulator already matches. The sibling arity refusal on the same code path (byQueue with 11 source ARNs -> `... Reason: Amazon SQS can't create the redrive allow policy. The maximum number of source queue ARNs you can specify is 10.`) is already correct in the emulator and is the positive evidence that only this one Reason string is wrong.

Emulator: same status and same code, but the Reason tail is an emulator-authored sentence: `... Reason: Amazon SQS can't create the redrive allow policy. The sourceQueueArns parameter is required when the redrivePermission parameter is set to byQueue.` The shared prefix (`Value <doc> for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy.`) is right; only the explanatory sentence after it is substituted.

Why: the refusal message is part of the contract. A client, IaC provider or operator tooling that classifies this refusal by message text — distinguishing "no sourceQueueArns for byQueue" from the 10-ARN-limit and the allowAll-with-ARNs cases, all of which share the code `InvalidParameterValue` and the same Reason prefix — reads a sentence AWS never emits and cannot tell the three apart. Any assertion or log-matching rule written against AWS fails here even though the call was correctly rejected.

Fix: the sqs SetQueueAttributes RedriveAllowPolicy validation's refusal-message template for the byQueue-requires-sourceQueueArns branch — one message string, in the same validator that already produces the correct 10-ARN-limit sentence. Note the check aborts at this assertion, so the two later branches of the same validator (allowAll carrying sourceQueueArns -> `... Reason: Amazon SQS can't create the redrive allow policy. To specify one or more source queue ARNs, you must set redrivePermission to byQueue.` with code `InvalidParameterValue`, and an unknown redrivePermission -> code `InvalidAttributeValue` with `Invalid value for the parameter RedriveAllowPolicy. Reason: Amazon SQS can't create the redrive allow policy, as it’s in an unsupported format.`) were never exercised and are unverified rather than known-good; they should be checked against the same wording while the template is corrected.

Traces: sqs.redrive-allow-policy-document-validation-rejected

## Evidence
Check sqs.redrive-allow-policy-document-validation-rejected, assertion at check_sqs_redrive_allow_policy_document_validation_rejected.py line 73:

AssertionError: byQueue with 0 ARNs gave message 'Value {"redrivePermission": "byQueue"} for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can\'t create the redrive allow policy. The sourceQueueArns parameter is required when the redrivePermission parameter is set to byQueue.'

Expected (AWS):   Value {"redrivePermission": "byQueue"} for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. When you specify the byQueue permission type, you must also specify one or more sourceQueueArns values.
Actual (emulator): Value {"redrivePermission": "byQueue"} for parameter RedriveAllowPolicy is invalid. Reason: Amazon SQS can't create the redrive allow policy. The sourceQueueArns parameter is required when the redrivePermission parameter is set to byQueue.

Error.Code matched (`InvalidParameterValue`), HTTP status matched (400), and the preceding case in the same check — byQueue with 11 source ARNs, whose Reason is 'The maximum number of source queue ARNs you can specify is 10.' — passed, as did the positive control storing a 10-ARN byQueue policy. The divergence is confined to the Reason sentence of the missing-sourceQueueArns branch. The `{"redrivePermission": "byQueue"}` document above is the request's own attribute value echoed back; it fills the template's variable slot and is not itself part of the requirement.

## Steps to Reproduce
1. Create a queue (it will act as the DLQ): CreateQueue(QueueName=<name>-dlq).
2. Store a valid policy as a positive control: SetQueueAttributes(QueueUrl=<dlq>, Attributes={"RedriveAllowPolicy": '{"redrivePermission": "byQueue", "sourceQueueArns": [<10 queue ARNs>]}'}) and confirm GetQueueAttributes(["RedriveAllowPolicy"]) reads it back unchanged.
3. Call SetQueueAttributes(QueueUrl=<dlq>, Attributes={"RedriveAllowPolicy": '{"redrivePermission": "byQueue"}'}) — i.e. byQueue with no sourceQueueArns.
4. Inspect the ClientError: Code is `InvalidParameterValue` (correct) and HTTP status is 400 (correct), but Message ends with 'The sourceQueueArns parameter is required when the redrivePermission parameter is set to byQueue.' instead of AWS's 'When you specify the byQueue permission type, you must also specify one or more sourceQueueArns values.'
5. For contrast, repeat step 3 with 11 source ARNs: that refusal's Reason ('The maximum number of source queue ARNs you can specify is 10.') already matches AWS, confirming only the one branch's template is wrong.
