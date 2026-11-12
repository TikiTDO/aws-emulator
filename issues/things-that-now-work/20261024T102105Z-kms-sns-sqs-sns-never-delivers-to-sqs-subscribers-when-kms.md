Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns__sqs.sse-kms-topic-delivers-plaintext-to-sqs-subscribers kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns
Issue-Key: kms__sns__sqs.sse-kms-topic-delivers-plaintext-to-sqs-subscribers|_await_bodies#13|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns|_await_bodies#17|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# SNS never delivers to SQS subscribers when KMS encryption is involved: both SSE-KMS topic/queue delivery checks hang to the 60 s cap with no message ever arriving (1 cause)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1 — a message published to an SNS topic with KmsMasterKeyId set is never delivered to its confirmed SQS subscribers, so both delivery checks run to the harness cap without reaching a verdict.
AWS: a topic encrypted at rest with a customer managed key decrypts on the way out and delivers the published body and message attributes in plaintext to its subscribers — the RawMessageDelivery=true queue receives the exact published bytes and the default subscriber receives an envelope whose Message/MessageAttributes are the published ones, within seconds of Publish. For an SSE-KMS QUEUE, delivery is withheld while the queue's key policy does not grant the SNS service principal kms:GenerateDataKey*/kms:Decrypt and begins once it does. Recorded behavior only for the latency: the recordings observe delivery promptly after Publish; the checks allow 120 s.
Emulator: neither check ever observes a delivery. Both end as 'Check timed out after 60.0s' while polling ReceiveMessage for the published body — the subscription is confirmed and Publish returns a MessageId, but nothing is ever enqueued. No verdict could be reached for the plaintext-passthrough claim or for the withhold-then-deliver claim, so once delivery works, the second check additionally requires that the delivery be WITHHELD until the queue CMK's key policy grants the SNS service principal, and resumed afterwards — a fixer must satisfy both.
Why: any fan-out design that uses SNS->SQS with encryption at rest cannot be exercised end to end; a consumer under test never receives a message, and the KMS-policy failure mode that silently drops production notifications is invisible.
Fix: the SNS topic-to-SQS delivery path (the subscription delivery worker), its KMS envelope handling for an encrypted topic, and the key-policy gate for the SNS service principal on an SSE-KMS target queue.

## Evidence
kms__sns__sqs.sse-kms-topic-delivers-plaintext-to-sqs-subscribers: 'Error: Check timed out after 60.0s'. The check creates a CMK, a topic with Attributes {'KmsMasterKeyId': <key arn>} (GetTopicAttributes echoes it correctly), two queues subscribed (one with RawMessageDelivery=true), publishes a body with a MessageAttribute, then awaits the body on the raw queue (its own budget is 120 s); nothing arrives.
kms__sns__sqs.delivery-to-a-kms-encrypted-queue-withheld-until-the-key-policy-grants-sns: 'Error: Check timed out after 60.0s' — same shape, polling for the withheld-then-granted delivery.
Expected: ReceiveMessage on the raw-delivery queue returns the exact published body (and the envelope subscriber the published Message/MessageAttributes) within seconds of Publish.

## Steps to Reproduce
1. kms:CreateKey. 2. sns:CreateTopic with Attributes={'KmsMasterKeyId': <key arn>}. 3. sqs:CreateQueue twice; Subscribe both to the topic (one with RawMessageDelivery=true) and confirm. 4. sns:Publish a body with a MessageAttribute. 5. Poll sqs:ReceiveMessage on both queues — no message ever arrives. 6. Repeat with the KMS encryption on the QUEUE (KmsMasterKeyId queue attribute) and no SNS grant in the key policy: delivery must be withheld, then begin after the key policy grants sns.amazonaws.com.
