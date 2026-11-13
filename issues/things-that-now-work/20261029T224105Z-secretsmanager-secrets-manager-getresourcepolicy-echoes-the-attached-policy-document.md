Issue-Source: trace-sweep-divergence
Issue-Trace: secretsmanager-large-resource-policy-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: secretsmanager-large-resource-policy-round-trip secretsmanager-resource-policy-management secretsmanager-resource-policy-public-access-error
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager|GetResourcePolicy|2xx->2xx|$.ResourcePolicy
Issue-Commit: 37be8999abc11e440bb26a3acee60c47f75abac5

# Secrets Manager GetResourcePolicy echoes the attached policy document verbatim instead of returning AWS's canonical re-serialization (1 cause)

> _Note: this issue was identified against the code base at commit `37be8999abc11e440bb26a3acee60c47f75abac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
Cause 1 of 1: every GetResourcePolicy read of a policy attached by PutResourcePolicy returns a $.ResourcePolicy document that differs from the one AWS returns for the same attached policy. One cluster, three fixtures (large-resource-policy-round-trip, resource-policy-management, resource-policy-public-access-error), so it is not specific to one policy shape or size.

AWS: Smithy: GetResourcePolicy output ResourcePolicy -> NonEmptyResourcePolicyType, @length 1..20480, documented as "A JSON-formatted string that contains the permissions policy attached to the secret." The model does not state a serialization form, so: recorded behavior only - in every recorded GetResourcePolicy response the document comes back in Secrets Manager's own canonical rendering of the policy it parsed, not as the caller's bytes. That rendering is pretty-printed with two-space indentation, a space on BOTH sides of the name separator (`"Version" : "2012-10-17"`), and spaces just inside array brackets (`[ { ... } ]`, `[ "a", "b" ]`). The requirement is that form, independent of how the caller whitespaced the document it put; the Sids, principals, actions, conditions and account ids inside the recorded document are that fixture's values, not part of the requirement.

Emulator: returns the stored string byte-for-byte as it was submitted, so whatever form the PutResourcePolicy body used is what comes back, and $.ResourcePolicy mismatches the recorded canonical document. This is deliberate in the current code: put_resource_policy.rs parses the document only to validate it and then comments "Stored as it arrived, not re-serialized from the parse above, so the document GetResourcePolicy reports is the one the caller attached", and get_resource_policy.rs returns `secret.resource_policy.clone()` with the comment "byte for byte as it was attached". Hypothesis: that choice is the entire divergence - the parse that already happens on the write path produces the structure AWS re-renders from.

Why: a client that round-trips a secret's policy - read it, edit one statement, put it back - or that diffs the live policy against the document in its source control sees a spurious difference on every read, because AWS's canonical form is what its tooling was written against. Infrastructure tooling that compares GetResourcePolicy output to its desired state reports perpetual drift and re-puts the policy on every run.

Fix: the response serialization of this operation (or, equivalently, normalizing on the PutResourcePolicy write path): render the parsed policy in Secrets Manager's canonical pretty-printed form rather than returning the caller's bytes.

Traces: secretsmanager-large-resource-policy-round-trip#task#3 (representative; the same cluster also covers secretsmanager-resource-policy-management#task#4 and secretsmanager-resource-policy-public-access-error#task#4). Emulator source: services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/get_resource_policy.rs lines 187-195 and src/ops/put_resource_policy.rs lines 171-183 and 226-228.

Related but NOT covering this: open issue 20261024T030130Z-lambda-rds-secretsmanager-secrets-manager-putresourcepolicy-is-an-unimplemented-scaffold-stub.md predates the implemented handler these steps now reach (the replayed calls answer 200, not 501) and carries no replay key for these steps; 20261025T031105Z-secretsmanager-secrets-manager-resource-based-policies-and-the-blockpublicpolicy.md covers policy EVALUATION, not the document's returned form.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 86.5%): cluster service=secretsmanager|GetResourcePolicy|status=200->200|keys=$.ResourcePolicy, verdict mismatch, affected_count 3 - secretsmanager-large-resource-policy-round-trip#task#3, secretsmanager-resource-policy-management#task#4, secretsmanager-resource-policy-public-access-error#task#4.

Both sides answer HTTP 200 and agree on ARN and Name; only $.ResourcePolicy diverges.

Expected (AWS), the canonical form - shape shown, values are this fixture's:
{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : "AllowAccountRootStatement0",\n    "Effect" : "Allow",\n    "Principal" : {\n      "AWS" : "arn:aws:iam::<account>:root"\n    },\n    "Action" : [ "secretsmanager:GetSecretValue", "secretsmanager:DescribeSecret" ],\n    "Resource" : "*",\n    "Condition" : { ... }\n  }, ... ]\n}
Note the two-space indent, the " : " name separator and the padded array brackets.

Actual (emulator): the document as the PutResourcePolicy request supplied it, returned unchanged - `resource_policy: secret.resource_policy.clone()` over a value stored "as it arrived, not re-serialized".

## Steps to Reproduce
1. CreateSecret(Name=<unique>, SecretString="v1", ClientRequestToken=<32+ chars>).
2. PutResourcePolicy(SecretId=<arn>, ResourcePolicy=<a valid policy document serialized in ANY form other than Secrets Manager's canonical one - e.g. compact `{"Version":"2012-10-17","Statement":[{...}]}`, or 4-space-indented>).
3. GetResourcePolicy(SecretId=<arn>).
4. Compare $.ResourcePolicy to AWS's: AWS returns its canonical pretty-printed rendering (two-space indent, `"Name" : value`, `[ ... ]` with inner spaces) regardless of what was submitted; the emulator returns the exact bytes submitted in step 2, so the two differ for any input that is not already in that form.
5. The same divergence appears for a large multi-statement policy (fixture secretsmanager-large-resource-policy-round-trip) and for policies carrying a wildcard principal (fixture secretsmanager-resource-policy-public-access-error), so it is independent of policy content.
