Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: guardduty.create-custom-detection-rule-association-without-permission-denied
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty.create-custom-detection-rule-association-without-permission-denied|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: fbf28374bba60078ec7e79247fc3d719cafd12c8

# AccessDeniedException for CreateCustomDetectionRuleAssociation names a wildcard account ARN instead of the detection-rule association resource

> _Note: this issue was identified against the code base at commit `fbf28374bba60078ec7e79247fc3d719cafd12c8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCustomDetectionRuleAssociation

## Description
This issue carries 1 distinct root cause in the guardduty crate.

Cause 1 of 1: the authorization refusal of CreateCustomDetectionRuleAssociation builds its IAM no-identity-based-policy message with a catch-all service ARN (`arn:aws:guardduty:<region>:<account>:*`) in the `on resource:` slot, instead of the ARN of the resource the operation acts on.

AWS: Smithy CreateCustomDetectionRuleAssociation (POST /custom-detection-rule/association, code 200; input RuleId required, @length 1..100, pattern ^[a-z0-9]+(-[a-z0-9]+)*$; Mode required (AssociationMode enum); modeled errors AccessDeniedException httpError 403, BadRequestException 400, ConflictException 409, ResourceNotFoundException 404, InternalServerErrorException 500). Recorded behavior: a caller signed with IAM credentials that carry no identity-based policy is refused HTTP 403 AccessDeniedException before any RuleId lookup, with IAM's standard no-identity-based-policy sentence. Its fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`; its variable parts come from the request and the caller's credentials: the caller principal ARN (`arn:aws:iam::<account>:user/<user-name>`), the action (`guardduty:CreateCustomDetectionRuleAssociation`, which appears twice), and — the part that diverges — the TARGET resource ARN, which for this operation is `arn:aws:guardduty:<region>:<account>:detection-rule/custom/<RuleId>/association/*`, i.e. the detection-rule-association resource path built from the request's RuleId, not a service-wide wildcard. (The permissioned control path is already correct: the identical call by a fully-permissioned caller is refused ResourceNotFoundException `Rule not found: <RuleId>`, and that assertion passed.)

Emulator: the status (403) and code (AccessDeniedException) now match, but the `on resource:` slot is filled with `arn:aws:guardduty:<region>:<account>:*` — the account-scoped service wildcard — so the message reads `... is not authorized to perform: guardduty:CreateCustomDetectionRuleAssociation on resource: arn:aws:guardduty:<region>:<account>:* because no identity-based policy allows the guardduty:CreateCustomDetectionRuleAssociation action`. The RuleId from the request is never interpolated and the `detection-rule/custom/.../association/*` resource path is absent.

Why: the resource ARN in an AccessDenied message is the datum callers and auditors use to learn WHICH resource the denied call targeted, and it is what operators paste into a policy statement's Resource element to grant the missing permission. A client or least-privilege test asserting AWS's wording fails against the emulator; worse, tooling that lifts the ARN out of the message to author a remediation policy produces a statement granting the action on the whole service in the account rather than on the single detection-rule association, silently widening the grant.

Fix: the AccessDenied message construction on the CreateCustomDetectionRuleAssociation authorization path in the guardduty crate (the operation's handler / the shared auth-denial error constructor it calls), which must render the operation's own resource ARN from the request's RuleId rather than a service wildcard. Related open issue 20261030T120002Z-guardduty-iam-authorization-is-not-enforced-on-createcustomdetectionruleassociation.md (its Cause 1) reports the earlier state of this same path, where no permission check ran at all and the call was refused ResourceNotFoundException; its replay key is the `code == "AccessDeniedException"` assertion, which now passes, so this message-template divergence is a further, unowned defect on the same code path and should be landed there.

Traces: guardduty.create-custom-detection-rule-association-without-permission-denied.

## Evidence
Replayed check guardduty.create-custom-detection-rule-association-without-permission-denied (verdict: mismatch), assertion at check line 97 `assert message == expected_message`:

- expected (real AWS): `User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: guardduty:CreateCustomDetectionRuleAssociation on resource: arn:aws:guardduty:<region>:<account>:detection-rule/custom/<RuleId>/association/* because no identity-based policy allows the guardduty:CreateCustomDetectionRuleAssociation action`
- got (emulator): `User: arn:aws:iam::<account>:user/<user-name> is not authorized to perform: guardduty:CreateCustomDetectionRuleAssociation on resource: arn:aws:guardduty:<region>:<account>:* because no identity-based policy allows the guardduty:CreateCustomDetectionRuleAssociation action`

Fixed text of the message class (must match byte for byte): `User: `, ` is not authorized to perform: `, ` on resource: `, ` because no identity-based policy allows the `, ` action`. Variable parts, taken from the request/credentials and not from any one run: <account> (caller's account id), <user-name> (the zero-permission IAM user created by the check), <region>, <RuleId> (the request's ruleId), and the action string `guardduty:CreateCustomDetectionRuleAssociation`. The only divergence is the `on resource:` value: a service-wide `:*` where AWS names `detection-rule/custom/<RuleId>/association/*`.

Earlier assertions in the same check passed, so they are already correct and bound this defect: the permissioned caller's identical call is refused ResourceNotFoundException with `Rule not found: <RuleId>`, and the error code/status for the zero-permission caller is AccessDeniedException / 403.

## Steps to Reproduce
1. Boot the emulator and replay guardduty.create-custom-detection-rule-association-without-permission-denied.
2. Manual repro: ensure a detector exists (ListDetectors, else CreateDetector(Enable=true)). Pick RuleId = a fresh lowercase 32-hex string (matches ^[a-z0-9]+(-[a-z0-9]+)*$).
3. As the default (fully permissioned) caller: CreateCustomDetectionRuleAssociation(RuleId=<RuleId>, Mode='DRY_RUN') -> expect ResourceNotFoundException with message `Rule not found: <RuleId>` (already correct; confirms the call is not failing for an authorization reason).
4. iam.CreateUser(UserName=<fresh name>) with NO attached policy, iam.CreateAccessKey for it, and build a guardduty client signed with those credentials (poll ListDetectors until the credentials are active).
5. Issue the IDENTICAL CreateCustomDetectionRuleAssociation(RuleId=<RuleId>, Mode='DRY_RUN') with those zero-permission credentials and read Error.Message. The emulator serves `on resource: arn:aws:guardduty:<region>:<account>:*`; expect `on resource: arn:aws:guardduty:<region>:<account>:detection-rule/custom/<RuleId>/association/*` with the rest of the sentence unchanged.
6. ListCustomDetectionRuleAssociations and confirm no association exists for that RuleId (effect-absent; this part already holds).
