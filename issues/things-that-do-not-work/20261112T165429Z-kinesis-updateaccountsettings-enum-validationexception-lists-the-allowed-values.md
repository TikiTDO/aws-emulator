Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.update-account-settings-invalid-value-fails
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.update-account-settings-invalid-value-fails|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 7394251eefd071b8c239bc91125495753c22c75a

# kinesis: UpdateAccountSettings enum ValidationException lists the allowed values alphabetically instead of in the model's declaration order

> _Note: this issue was identified against the code base at commit `7394251eefd071b8c239bc91125495753c22c75a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccountSettings

## Description
Cause 1 of 1: the enum-value set in the ValidationException message for a bad MinimumThroughputBillingCommitment.Status is rendered in sorted order rather than in the order the enum declares its members. Failing check: kinesis.update-account-settings-invalid-value-fails.

AWS: Smithy (services/service-com-amazonaws-kinesis-kinesis-20131202) models UpdateAccountSettingsInput.MinimumThroughputBillingCommitment (required) -> MinimumThroughputBillingCommitmentInput.Status (required) -> enum MinimumThroughputBillingCommitmentInputStatus with values, in declaration order, ENABLED then DISABLED; UpdateAccountSettings models ValidationException (client error, HTTP 400). For a value outside a modeled enum, AWS answers HTTP 400 ValidationException with the standard counted constraint message, whose fixed text is "<N> validation error[s] detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]" and whose variable parts are N (the number of violations), the rejected value and the member path taken from the request, and <values> — the enum's members in the ORDER THE MODEL DECLARES THEM, comma-and-space separated, i.e. "ENABLED, DISABLED" for this member. Status code and error code already match; only the value list's order diverges.

Emulator: answers ValidationException with "1 validation error detected: Value 'NEGATIVE-1' at 'minimumThroughputBillingCommitment.status' failed to satisfy constraint: Member must satisfy enum value set: [DISABLED, ENABLED]" — the two members emitted in lexicographic order instead of declaration order. Every other part of the message (count, quoted value, member path, fixed text) is correct.

Why: a client that asserts on, logs, or diffs the exact AWS message text (contract tests, golden-file comparisons, support tooling that parses the allowed-value list to present choices in the service's documented order) sees a message AWS never emits. The enum-ordering rule is global to the generic validation renderer, so any operation whose enum refusal is rendered this way will diverge the same way wherever the modelled order is not alphabetical.

Fix: the shared constraint-validation message renderer that formats "Member must satisfy enum value set" — it must emit the enum members in the order they appear in the Smithy enum definition rather than sorting them (or iterating an unordered/ordered-by-key set). The UpdateAccountSettings input validation path is where the check observes it, but the renderer is the shared component.

Traces: kinesis.update-account-settings-invalid-value-fails.

## Evidence
Check kinesis.update-account-settings-invalid-value-fails (verdict: mismatch).

Request: UpdateAccountSettings with MinimumThroughputBillingCommitment={"Status": "NEGATIVE-1"} (botocore parameter_validation disabled so the bogus enum reaches the service).

Expected (AWS): HTTP 400, Error.Code = "ValidationException", Error.Message = "1 validation error detected: Value 'NEGATIVE-1' at 'minimumThroughputBillingCommitment.status' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED]"

Actual (emulator): HTTP 400, Error.Code = "ValidationException" (correct), Error.Message = "1 validation error detected: Value 'NEGATIVE-1' at 'minimumThroughputBillingCommitment.status' failed to satisfy constraint: Member must satisfy enum value set: [DISABLED, ENABLED]"

Assertion raised: AssertionError: expected message "... enum value set: [ENABLED, DISABLED]", got "... enum value set: [DISABLED, ENABLED]".

Message anatomy — fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable from the request: the rejected value ('NEGATIVE-1') and the member path ('minimumThroughputBillingCommitment.status'). Variable from the model, and the diverging part: the member list, which must follow the Smithy enum's declaration order (ENABLED, DISABLED per MinimumThroughputBillingCommitmentInputStatus), not alphabetical order.

## Steps to Reproduce
1. Build and boot the emulator; point a kinesis client at it with botocore parameter validation disabled (Config(parameter_validation=False)) so the invalid enum is not rejected client-side.
2. Call DescribeAccountSettings and confirm MinimumThroughputBillingCommitment is present (positive control).
3. Call UpdateAccountSettings(MinimumThroughputBillingCommitment={"Status": "NEGATIVE-1"}).
4. Observe the ClientError: code is ValidationException (correct) but Error.Message ends with "enum value set: [DISABLED, ENABLED]" where AWS emits "[ENABLED, DISABLED]" (the model's declaration order).
5. Re-replay check kinesis.update-account-settings-invalid-value-fails to confirm the fix.
