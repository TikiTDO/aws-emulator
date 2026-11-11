Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__sts.execute-statement-partiql-select-index-qualified-scoped-to-index-arn-denied dynamodb__iam__sts.search-vectors-table-read-role-denied-index-arn-grant-allowed dynamodb__iam__sts.execute-statement-select-full-table-scan-denied-keyed-select-allowed
Issue-Key: dynamodb__iam__sts.execute-statement-partiql-select-index-qualified-scoped-to-index-arn-denied|_error#39|AssertionError|raise AssertionError(f"call was expected to be DENIED but succeeded: {result!r}")
Issue-Key: dynamodb__iam__sts.search-vectors-table-read-role-denied-index-arn-grant-allowed|expect_error#34|AssertionError|assert got_message == message, f"unexpected message {got_message!r}"
Issue-Key: dynamodb__iam__sts|ExecuteStatement|AccessDeniedException
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# dynamodb: 2 net-new causes — index-qualified PartiQL and SearchVectors are authorized against the table ARN instead of the index ARN, and the dynamodb:FullTableScan condition key is never evaluated

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Two net-new root causes in DynamoDB's IAM authorization of PartiQL and vector-search reads. Both must be closed before this issue is submitted.

Cause 1 of 2: an operation that names a secondary index is authorized against the TABLE ARN, not the index ARN.
AWS: DynamoDB's IAM resource for an index-qualified request is the index's own ARN, `arn:<partition>:dynamodb:<region>:<account>:table/<table-name>/index/<index-name>` — the table ARN does not cover it and the index ARN does not cover the table. This holds for an ExecuteStatement PartiQL statement whose FROM target is `"<table>"."<index>"` (AWS docs, "IAM policies with PartiQL for DynamoDB": dynamodb:PartiQLSelect on the index ARN for an index-qualified SELECT) and for SearchVectors with an IndexName (action dynamodb:SearchVectors). Consequently: a role allowed `dynamodb:PartiQLSelect` only on the table ARN is REFUSED the index-qualified SELECT, with 400 AccessDeniedException and the message `User: <assumed-role session ARN> is not authorized to perform: dynamodb:PartiQLSelect on resource: <index ARN> because no identity-based policy allows the dynamodb:PartiQLSelect action` — the fixed text is everything but the principal ARN (from the signing session), the action (from the request) and the resource ARN, which must be the INDEX ARN. Symmetrically, a role allowed `dynamodb:SearchVectors` on nothing is refused naming the index ARN, and a role allowed it on the index ARN succeeds.
Emulator: it resolves the request's resource to the table ARN in both directions. The table-ARN-scoped role's index-qualified SELECT is ALLOWED and returns items (`{'Items': [{'pk': {'S': 'seed'}, 'gsi_pk': {'S': 'seed'}}]}`) where AWS refuses it; and the SearchVectors denial is emitted naming `arn:aws:dynamodb:<region>:<account>:table/<table>` instead of `.../table/<table>/index/<index>`, so even the refusal it does produce names the wrong resource.
Why: a least-privilege policy that deliberately withholds index access is silently bypassed — a caller scoped to the base table can read the whole index through PartiQL, which is a real privilege escalation for anyone using index-ARN scoping to separate projections. In the opposite direction, a correctly index-scoped grant cannot be validated because the denial message names a resource the policy was never written about.
Fix: the request-to-resource-ARN derivation used by DynamoDB's IAM authorization for index-qualified requests (the PartiQL ExecuteStatement handler's statement-target resolution and the SearchVectors handler's IndexName resolution), plus the resource ARN rendered into the AccessDeniedException message.
Traces: dynamodb__iam__sts.execute-statement-partiql-select-index-qualified-scoped-to-index-arn-denied, dynamodb__iam__sts.search-vectors-table-read-role-denied-index-arn-grant-allowed.

Cause 2 of 2: the `dynamodb:FullTableScan` condition key is never evaluated, so a conditioned Deny applies to every request.
AWS: `dynamodb:FullTableScan` is a Bool condition key DynamoDB publishes for PartiQL requests (AWS docs, "IAM policies with PartiQL for DynamoDB"). It is true only for a statement that scans the whole table and false for a statement keyed by the partition key. So an identity policy of Allow dynamodb:PartiQLSelect on the table ARN plus Deny dynamodb:PartiQLSelect on the same ARN with `Condition: {"Bool": {"dynamodb:FullTableScan": "true"}}` must refuse `SELECT * FROM "<table>"` (no key condition) and ALLOW `SELECT * FROM "<table>" WHERE pk = ?`.
Emulator: the condition key is not produced for the request, so the Bool condition does not evaluate to false and the Deny is applied unconditionally. The keyed SELECT is refused 400 AccessDeniedException `User: <session ARN> is not authorized to perform: dynamodb:PartiQLSelect on resource: <table ARN> with an explicit deny in an identity-based policy`.
Why: the documented way to forbid full-table scans while permitting keyed reads is inverted into a blanket denial — the policy's intended workload stops working entirely, and a client cannot distinguish a condition the emulator ignored from a policy it got wrong.
Fix: the DynamoDB authorization context for PartiQL requests — publish dynamodb:FullTableScan (derived from whether the statement carries a key condition) into the condition-key set the policy evaluator sees.
Traces: dynamodb__iam__sts.execute-statement-select-full-table-scan-denied-keyed-select-allowed.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### dynamodb__iam__sts` section (8/15 passed).

Cause 1:
[mismatch] dynamodb__iam__sts.execute-statement-partiql-select-index-qualified-scoped-to-index-arn-denied
  AssertionError: call was expected to be DENIED but succeeded: {'Items': [{'pk': {'S': 'seed'}, 'gsi_pk': {'S': 'seed'}}]}
  The check attaches to the role only: {"Effect":"Allow","Action":"dynamodb:PartiQLSelect","Resource":<table_arn>} and then runs ExecuteStatement with Statement='SELECT * FROM "<table>"."<index>" WHERE gsi_pk = ?'; the expected refusal is ('AccessDeniedException', 'User: <principal> is not authorized to perform: dynamodb:PartiQLSelect on resource: <table_arn>/index/<index> because no identity-based policy allows the dynamodb:PartiQLSelect action').

[mismatch] dynamodb__iam__sts.search-vectors-table-read-role-denied-index-arn-grant-allowed
  AssertionError: unexpected message 'User: arn:aws:sts::000000000000:assumed-role/sim2real-vecgsi-denied-<id>/sim2real-vecgsi-<id> is not authorized to perform: dynamodb:SearchVectors on resource: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-vecgsi-<id> because no identity-based policy allows the dynamodb:SearchVectors action'
  The check's expectation names index_arn = f"{table_arn}/index/{index_name}" for the same action and principal; only the resource ARN segment diverges. (Account/region/name/id values above are per-run.)

Cause 2:
[mismatch] dynamodb__iam__sts.execute-statement-select-full-table-scan-denied-keyed-select-allowed
  AssertionError: keyed SELECT under the full-table-scan-deny policy failed with 'AccessDeniedException': 'User: arn:aws:sts::000000000000:assumed-role/sim2real-xsvc-ddbql-fts-<id>/sim2real-ddbql-<id> is not authorized to perform: dynamodb:PartiQLSelect on resource: arn:aws:dynamodb:us-west-2:000000000000:table/sim2real-xsvc-ddbql-fts-<id> with an explicit deny in an identity-based policy'
  Role policy in the check: Allow dynamodb:PartiQLSelect on <table_arn>, plus Deny dynamodb:PartiQLSelect on <table_arn> with Condition {"Bool": {"dynamodb:FullTableScan": "true"}}. The refused statement is 'SELECT * FROM "<table>" WHERE pk = ?' — keyed, so FullTableScan is false and the Deny must not apply.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. Run the behavioral suite and look at the `dynamodb__iam__sts` section.
3. Cause 1a: CreateTable with a GSI; attach to a role an inline policy allowing only dynamodb:PartiQLSelect on the TABLE ARN; assume it and call ExecuteStatement with Statement='SELECT * FROM "<table>"."<index>" WHERE <gsi_pk> = ?'. AWS refuses 400 AccessDeniedException naming <table_arn>/index/<index>; the emulator returns Items. Replay dynamodb__iam__sts.execute-statement-partiql-select-index-qualified-scoped-to-index-arn-denied (it also checks the mirror case: an index-ARN-only grant must allow the index-qualified SELECT and refuse the unqualified one naming the table ARN).
4. Cause 1b: create a table with a vector index; assume a role that has table reads but no dynamodb:SearchVectors, and call SearchVectors(TableName, IndexName, SearchVector, TopK). Read the resource ARN in the AccessDeniedException message — it must be the index ARN. Replay dynamodb__iam__sts.search-vectors-table-read-role-denied-index-arn-grant-allowed.
5. Cause 2: attach Allow dynamodb:PartiQLSelect on the table ARN plus a Deny on the same ARN conditioned Bool dynamodb:FullTableScan=true; assume the role and run both 'SELECT * FROM "<table>"' (must be denied) and 'SELECT * FROM "<table>" WHERE pk = ?' (must be allowed). Replay dynamodb__iam__sts.execute-statement-select-full-table-scan-denied-keyed-select-allowed.
