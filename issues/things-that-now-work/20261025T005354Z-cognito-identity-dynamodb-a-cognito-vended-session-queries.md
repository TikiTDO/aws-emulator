Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__dynamodb.leading-keys-condition-scopes-query-to-the-callers-identity-id
Issue-Key: cognito-identity__dynamodb.leading-keys-condition-scopes-query-to-the-callers-identity-id|_refusal#38|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED: {result!r}")
Issue-Commit: d0dc720ebbf11903c1fa9407d1c82e53eafd4b75

# cognito-identity__dynamodb: a Cognito-vended session queries another identity's partition — the dynamodb:LeadingKeys condition on the federated role is never evaluated (1 cause)

> _Note: this issue was identified against the code base at commit `d0dc720ebbf11903c1fa9407d1c82e53eafd4b75`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Query

## Description
ONE root cause (cause 1 of 1).

AWS: the canonical Cognito multi-tenant pattern scopes a federated role's dynamodb:Query to the caller's own identity id with Condition {"ForAllValues:StringEquals": {"dynamodb:LeadingKeys": ["${cognito-identity.amazonaws.com:sub}"]}}. A Query issued under identity A's vended session for identity B's partition key is refused with HTTP 400 AccessDeniedException ("User: <assumed-role session arn> is not authorized to perform: dynamodb:Query on resource: <table arn> …"), and no items are returned. The in-file positive control — identity A querying its own partition — succeeds.
Emulator: the cross-identity Query returns HTTP 200 with identity B's item, including its payload. The condition is not evaluated at all, which is consistent with the vended credentials resolving to the emulator's ambient principal (arn:aws:iam::000000000000:user/emulator) rather than an assumed-role session carrying a cognito-identity sub, so there is no session context to evaluate ${cognito-identity.amazonaws.com:sub} against.
Why: the single most widely documented way to isolate per-user rows in DynamoDB is silently inert — one tenant's session reads another tenant's data, and a client validating that isolation against the emulator ships a cross-tenant data leak.
Fix: two components: the cognito-identity GetCredentialsForIdentity handler must mint a real assumed-role session carrying the identity's sub (so a principal and session context exist), and the DynamoDB data-plane authorization path must evaluate the role's identity-based policy, including the dynamodb:LeadingKeys condition key, against that principal.
Related but NOT covering: open issue 20261024T065715Z-dynamodb-iam-sts-dynamodb-data-plane-reads-are-never-authorized-identity.md records the DynamoDB-side authorization gap under service token dynamodb__iam__sts and does not carry this check's replay key.
Traces: cognito-identity__dynamodb.leading-keys-condition-scopes-query-to-the-callers-identity-id.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1:

cognito-identity__dynamodb.leading-keys-condition-scopes-query-to-the-callers-identity-id
  File "check_cognito_identity__dynamodb_leading_keys_condition_scopes_query_to_the_callers_identity_id.py", line 290, in run
    cross_code, cross_message = _refusal(lambda: _query(session_a, table, identity_b))
  AssertionError: expected the call to be refused, but it SUCCEEDED: [{'ItemId': {'S': 'row-1'}, 'UserId': {'S': 'us-west-2:ac18a2e0-9174-40d6-8565-8bd5556d443b'}, 'blob': {'B': b'sim2real-lk-b-…'}}]

i.e. the session vended for identity A read the row whose UserId partition key is identity B. Elsewhere in this run the caller identity under a Cognito-vended session is 'arn:aws:iam::000000000000:user/emulator' (see the cognito-identity checks), so no per-identity session context exists.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite; re-replay the check id in trace_ids.
2. Manual repro: create a DynamoDB table keyed on UserId; create an IAM role trusting cognito-identity.amazonaws.com whose policy allows dynamodb:Query on the table with Condition ForAllValues:StringEquals dynamodb:LeadingKeys = ${cognito-identity.amazonaws.com:sub}; attach it to an identity pool; obtain vended sessions for two identities A and B; write one item per identity; from A's session Query the partition UserId=<identity B> -> expect AccessDeniedException and no items; observe 200 with B's item.
