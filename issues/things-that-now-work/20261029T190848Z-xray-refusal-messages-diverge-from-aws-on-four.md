Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.create-sampling-rule-attributes-exceed-max-rejected xray.get-insight-summaries-window xray.delete-default-sampling-rule-fails xray.update-sampling-rule-negative-priority-rejected xray.put-encryption-config-kms-missing-keyid-fails xray.encryption-config-kms-enforced-on-put-trace-segments
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|CreateSamplingRule|ValidationException
Issue-Key: xray.get-insight-summaries-window|run#15|AssertionError|assert message2 == (
Issue-Key: xray.delete-default-sampling-rule-fails|run#11|AssertionError|assert (error.get("Code"), error.get("Message")) == (
Issue-Key: xray.update-sampling-rule-negative-priority-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: xray.put-encryption-config-kms-missing-keyid-fails|run#5|AssertionError|assert message == EXPECTED_MESSAGE, f"MESSAGE={message!r}"
Issue-Key: xray.encryption-config-kms-enforced-on-put-trace-segments|run#13|AssertionError|assert cfg["KeyId"] == expected_key_arn, (
Issue-Commit: c4171394ecd9e8acdff90bddf637ffaae1174b81

# xray: refusal messages diverge from AWS on four surfaces (ValidationException drops the offending Value, Default-rule delete, UpdateSamplingRule priority, PutEncryptionConfig missing KeyId) and the encryption config reports the KMS alias ARN instead of the resolved key ARN (5 causes)

> _Note: this issue was identified against the code base at commit `c4171394ecd9e8acdff90bddf637ffaae1174b81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEncryptionConfig

## Description
Five net-new root causes. Four are refusal-message defects on calls the emulator already rejects with the right status and code; the fifth is a wrong value served on a successful read.

Cause 1 of 5: the ValidationException message omits the offending value, on every constraint it renders.
AWS: for a modelled length violation AWS answers 400 ValidationException with the message template `1 validation error detected: Value '<rendered value>' at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>`. Fixed text: `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must have length less than or equal to `. Variable parts, all taken from the request and the model: the rendered value (a map rendered `{k=v, k=v}` in the service's own key order, a list rendered `[A, B]`), the lowerCamel member path, and the maximum from the Smithy @length trait — Smithy: CreateSamplingRule.SamplingRule.Attributes targets com.amazonaws.xray#AttributeMap @length max 5; GetInsightSummaries.States targets InsightStateList @length max 1.
Emulator: emits the template with the whole `Value '<rendered value>'` clause collapsed to a bare `Value` — `1 validation error detected: Value at 'samplingRule.attributes' failed to satisfy constraint: Member must have length less than or equal to 5` and `1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 1`. The status (400), the code and the member path and bound are right; the rejected value is never rendered.
Why: the rejected value is the only part of the message that tells a caller WHICH input was too long when a request carries several constrained collections; a client (or a human reading a log) loses that entirely, and any matcher on AWS's text fails.
Fix: the shared validation-error message formatter used by the xray crate's input validation — render the offending value into the template for list and map length violations.
Traces: xray.create-sampling-rule-attributes-exceed-max-rejected, xray.get-insight-summaries-window.

Cause 2 of 5: DeleteSamplingRule on the reserved Default rule is refused with the wrong message.
AWS: Smithy DeleteSamplingRule (input RuleName/RuleARN, error InvalidRequestException 400). Deleting the service-managed `Default` rule answers 400 InvalidRequestException with message `Cannot delete default sampling rule` — entirely fixed text, no value from the request.
Emulator: `the service-managed \`Default\` sampling rule cannot be deleted`.
Why: a client that distinguishes "protected resource" from other InvalidRequestException causes by message never matches, so a deliberate attempt to delete Default is indistinguishable from an unrelated bad request.
Fix: the DeleteSamplingRule handler's reserved-rule guard in the xray crate.
Traces: xray.delete-default-sampling-rule-fails.

Cause 3 of 5: UpdateSamplingRule with an invalid Priority is refused with the wrong message.
AWS: Smithy UpdateSamplingRule takes SamplingRuleUpdate (Priority is a NullableInteger with NO modelled range) and declares InvalidRequestException (400). AWS refuses an out-of-range Priority with the fixed message `Invalid update request for sampling rule` — it does NOT echo the member or the value.
Emulator: `Priority must be between 1 and 9999, not -5`, which both diverges from AWS's fixed text and interpolates the member, the bounds and the rejected value.
Why: same as above — the refusal text is part of the contract; a client keyed on AWS's wording sees an unknown message, and the emulator advertises a bound as service behavior that AWS does not state.
Fix: the UpdateSamplingRule handler's validation in the xray crate.
Traces: xray.update-sampling-rule-negative-priority-rejected.

Cause 4 of 5: PutEncryptionConfig(Type=KMS) with no KeyId is refused with the wrong message.
AWS: Smithy PutEncryptionConfig input Type (required enum) and KeyId (optional, @length 1..3000) — the conditional requirement is server-side, and AWS answers 400 InvalidRequestException with the fixed message `Key ID is required for type KMS`. The rejected change must not take effect (GetEncryptionConfig unchanged), which the emulator does honour.
Emulator: `KeyId is required when Type is KMS: it selects the KMS key to encrypt with`.
Why: a client branching on the refusal text to prompt for a key never matches AWS's wording.
Fix: the PutEncryptionConfig handler's conditional-required validation in the xray crate. The open issue 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md covers PutEncryptionConfig ACCEPTING a nonexistent key and enum/length misclassification, and 20261025T052746Z-xray-2-net-new-behavioral-root-causes-putencryptionconfig.md covers the enum ValidationException value ordering; neither covers this missing-KeyId message.
Traces: xray.put-encryption-config-kms-missing-keyid-fails.

Cause 5 of 5: the encryption config reports the KMS alias ARN it was given instead of the resolved key ARN.
AWS: Smithy EncryptionConfig.KeyId is documented as "The ID of the KMS key used for encryption, if applicable". After PutEncryptionConfig(Type=KMS, KeyId=<alias>), the config settles UPDATING -> ACTIVE and GetEncryptionConfig reports the key's real ARN — `arn:aws:kms:<region>:<account-id>:key/<key-uuid>`, the Arn that kms:DescribeKey returns for that alias — not the alias the caller passed. Recorded behavior for the AWS managed `alias/aws/xray` key.
Emulator: GetEncryptionConfig reports an alias-form ARN, `arn:aws:kms:<region>:<account-id>:alias/aws/xray`, i.e. the request's alias re-rendered as an ARN, never resolved against KMS. (Region, account id and key uuid are per-environment values, not requirements; the requirement is the `:key/<uuid>` form of the ARN the alias resolves to.)
Why: a client reading EncryptionConfig.KeyId to identify, audit or grant on the actual CMK gets an alias ARN. Alias-to-key is not a 1:1 stable mapping, so key-identity comparisons (against DescribeKey, a key policy, or a CloudTrail record) all mismatch.
Fix: the PutEncryptionConfig handler / encryption-config state in the xray crate — resolve the supplied KeyId (alias name, alias ARN, key id or key ARN) through KMS and store/serve the resolved key ARN.
Traces: xray.encryption-config-kms-enforced-on-put-trace-segments.

## Evidence
Cause 1 — xray.create-sampling-rule-attributes-exceed-max-rejected (CreateSamplingRule with 6 Attributes, modelled max 5):
  expected: ValidationException / "1 validation error detected: Value '{probe_key_5=probe_value_5, probe_key_4=probe_value_4, probe_key_3=probe_value_3, probe_key_2=probe_value_2, probe_key_1=probe_value_1, probe_key_0=probe_value_0}' at 'samplingRule.attributes' failed to satisfy constraint: Member must have length less than or equal to 5"
  actual:   {'Code': 'ValidationException', 'Message': "1 validation error detected: Value at 'samplingRule.attributes' failed to satisfy constraint: Member must have length less than or equal to 5"}
Cause 1 — xray.get-insight-summaries-window (GetInsightSummaries States=['ACTIVE','CLOSED'], modelled max 1):
  expected: "1 validation error detected: Value '[ACTIVE, CLOSED]' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1"
  actual:   "1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 1"

Cause 2 — xray.delete-default-sampling-rule-fails:
  expected: InvalidRequestException / 'Cannot delete default sampling rule'
  actual:   {'Code': 'InvalidRequestException', 'Message': 'the service-managed `Default` sampling rule cannot be deleted'}

Cause 3 — xray.update-sampling-rule-negative-priority-rejected (SamplingRuleUpdate={RuleName, Priority: -5}):
  expected message: 'Invalid update request for sampling rule'
  actual message:   'Priority must be between 1 and 9999, not -5'

Cause 4 — xray.put-encryption-config-kms-missing-keyid-fails (PutEncryptionConfig(Type='KMS')):
  expected message: 'Key ID is required for type KMS'
  actual message:   'KeyId is required when Type is KMS: it selects the KMS key to encrypt with'

Cause 5 — xray.encryption-config-kms-enforced-on-put-trace-segments:
  PutEncryptionConfig(Type='KMS', KeyId='alias/aws/xray'); GetEncryptionConfig.EncryptionConfig.KeyId
  expected: the key ARN the alias resolves to, e.g. 'arn:aws:kms:us-west-2:000000000000:key/02bb1c80-209f-4f77-a63c-28057a1c9ae1' (the kms:DescribeKey Arn for that alias)
  actual:   'arn:aws:kms:us-west-2:000000000000:alias/aws/xray'

## Steps to Reproduce
Cause 1:
1. CreateSamplingRule with SamplingRule.Attributes holding 6 entries (max 5) -> compare the ValidationException message with AWS's, which renders the map as Value '{k=v, ...}'.
2. CreateGroup, then GetInsightSummaries(GroupName=<g>, States=['ACTIVE','CLOSED'], StartTime, EndTime) -> compare the message, which must render Value '[ACTIVE, CLOSED]'.
Cause 2:
3. DeleteSamplingRule(RuleName='Default') -> message must be exactly 'Cannot delete default sampling rule'.
Cause 3:
4. CreateSamplingRule(Priority=500, ...), then UpdateSamplingRule(SamplingRuleUpdate={RuleName, Priority: -5}) -> message must be exactly 'Invalid update request for sampling rule'; the stored Priority must stay 500 and a later Priority=501 update must succeed.
Cause 4:
5. PutEncryptionConfig(Type='KMS') with no KeyId -> message must be exactly 'Key ID is required for type KMS'; GetEncryptionConfig must be unchanged.
Cause 5:
6. kms:DescribeKey(KeyId='alias/aws/xray') and note KeyMetadata.Arn (…:key/<uuid>).
7. PutEncryptionConfig(Type='KMS', KeyId='alias/aws/xray'); poll GetEncryptionConfig until Status=ACTIVE -> EncryptionConfig.KeyId must equal that key ARN, not an …:alias/aws/xray ARN.

Replay: xray.create-sampling-rule-attributes-exceed-max-rejected, xray.get-insight-summaries-window, xray.delete-default-sampling-rule-fails, xray.update-sampling-rule-negative-priority-rejected, xray.put-encryption-config-kms-missing-keyid-fails, xray.encryption-config-kms-enforced-on-put-trace-segments.
