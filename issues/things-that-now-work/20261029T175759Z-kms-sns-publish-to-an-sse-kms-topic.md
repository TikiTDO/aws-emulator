Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__sns.publish-fails-while-kms-master-key-is-disabled
Issue-Key: kms__sns.publish-fails-while-kms-master-key-is-disabled|run#18|Timeout|time.sleep(20)
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__sns: Publish to an SSE-KMS topic keeps succeeding after the topic's CMK is disabled — SNS never depends on the key's live state (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
ONE cause. The check polls Publish for the refusal AWS eventually produces and never sees it, so it is killed at the suite's 60 s cap. NOTE on calibration: the check measured that real AWS keeps accepting Publish for roughly 300 s after DisableKey (SNS caches the data key) and therefore budgets 420 s for the refusal to appear, which the 60 s harness cap cannot cover on its own; the divergence filed here is the emulator-side one the open issue below already describes for the same surface, and a fixer should confirm it with a direct DisableKey/Publish probe rather than only the timed replay.

Related OPEN issues that do NOT carry this check's replay key (named for context only): 20261027T030148Z-sns-6-net-new-behavioral-root-causes-no.md, whose causes include 'Publish ignores the topic KMS key state', and 20261025T025317Z-sns-authorizes-nothing-on-the-data-plane-topic.md.

Cause 1 of 1 — Publish to a topic whose KmsMasterKeyId names a disabled customer CMK is accepted instead of refused KMSDisabled.
AWS (recorded behavior; the SNS model declares no key-state semantics): a topic created with Attributes KmsMasterKeyId = a customer CMK (here supplied as an alias) encrypts every message under that key. Once the key is disabled, Publish fails with Error.Code 'KMSDisabled' — not a generic InvalidParameter or AuthorizationError — and re-enabling the key restores Publish. The control runs first, before anything is broken: Publish on the CMK-encrypted topic returns a MessageId while the key is Enabled.
Emulator: Publish keeps returning a MessageId with the key Disabled; KmsMasterKeyId is an opaque attribute once accepted and SNS consults KMS on no Publish, so the refusal the check polls for never appears and the check is killed at the 60 s cap.
Why: disabling the CMK is the control that stops an encrypted topic from accepting traffic. An emulator that keeps accepting publishes makes that control unverifiable, hides the KMSDisabled code clients must handle (and retry/alarm on), and lets a client believe messages are enveloped under a key that could not have encrypted them.
Fix: the SNS Publish handler's encryption path — resolve the topic's KmsMasterKeyId (alias included) and call KMS for the data key on publish, mapping a disabled/unusable key to KMSDisabled.
Traces: kms__sns.publish-fails-while-kms-master-key-is-disabled (60 s timeout).

## Evidence
"Error: Check timed out after 60.0s". The check's shape: CreateKey; CreateAlias; CreateTopic(Attributes={'KmsMasterKeyId': <alias>}); Publish as a positive control, asserting a MessageId is returned (this is the leg that proves the topic is usable before anything is broken); DisableKey; then a bounded poll of Publish whose helper names a success PUBLISHED ('A Publish that SUCCEEDS while the key is disabled is the over-permissive failure this check exists to catch') and which only completes when the code becomes 'KMSDisabled'. The harness killed the check inside that poll, i.e. every Publish after DisableKey was accepted.

## Steps to Reproduce
CreateKey; CreateAlias for it; CreateTopic(Attributes={'KmsMasterKeyId': '<alias>'}); Publish (succeeds — control); DisableKey; DescribeKey to confirm KeyState=Disabled; Publish again repeatedly — observe a MessageId returned instead of Error.Code 'KMSDisabled'; EnableKey and Publish to confirm recovery.
