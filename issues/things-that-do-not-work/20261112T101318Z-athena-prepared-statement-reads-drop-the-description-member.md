Issue-Source: trace-sweep-divergence
Issue-Trace: athena-prepared-statement-parameterized-query-workflow
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-prepared-statement-parameterized-query-workflow
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetPreparedStatement|2xx->2xx|$.PreparedStatement.Description
Issue-Key: athena|BatchGetPreparedStatement|2xx->2xx|$.PreparedStatements[*].Description
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# athena prepared-statement reads drop the Description member set by CreatePreparedStatement

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPreparedStatement

## Description
Athena's prepared-statement store does not retain (or does not serialize) the optional Description supplied to CreatePreparedStatement, so every read path returns a PreparedStatement structure with Description absent. Two distinct read operations are affected; both causes must be closed.

Cause 1 of 2: GetPreparedStatement omits PreparedStatement.Description.
AWS: Smithy (com.amazonaws.athena) - CreatePreparedStatement input has optional member Description (target DescriptionString, @length min 1 max 1024, "The description of the prepared statement"); GetPreparedStatement output is PreparedStatement, whose members are StatementName, QueryStatement, WorkGroupName, Description (DescriptionString, @length 1..1024) and LastModifiedTime. AWS echoes back the description string exactly as it was supplied on create, so a statement created with a description reads back with that same Description value.
Emulator: the GetPreparedStatement 200 response carries StatementName/QueryStatement/WorkGroupName/LastModifiedTime but PreparedStatement.Description is missing entirely (replay: "$.PreparedStatement.Description: missing (expected 'parameterized test statement', got None)" - the expected value is the description the fixture passed to CreatePreparedStatement; only the presence and round-trip of the member is the requirement, not that literal string).
Why: a client that stores human-readable metadata on a prepared statement loses it on read. Console/CLI listings render an empty description, and round-trip tooling that reads a statement, edits the query and calls UpdatePreparedStatement silently erases the description because the read never returned it. An SDK model field that is documented to be returned reads as unset, which is indistinguishable from "created without a description".
Fix: the prepared-statement state (CreatePreparedStatement/UpdatePreparedStatement must persist the Description member) and the GetPreparedStatement serializer (must emit Description when present). Most likely the field is dropped at create time, which would explain both causes from one place.
Traces: athena-prepared-statement-parameterized-query-workflow#task#5.

Cause 2 of 2: BatchGetPreparedStatement omits Description on each returned statement.
AWS: Smithy (com.amazonaws.athena) - BatchGetPreparedStatement output member PreparedStatements targets PreparedStatementDetailsList, whose element structure carries the same Description member (DescriptionString, @length 1..1024). Each element of PreparedStatements for a statement created with a description includes that Description.
Emulator: the 200 response's PreparedStatements[0] omits Description (replay: "$.PreparedStatements[0].Description: missing (expected 'parameterized test statement', got None)"). Status and the rest of the element shape match; only the Description member is absent.
Why: the batch read is the path tools use to hydrate many statements at once, so the same metadata loss happens in bulk - a UI listing statements with descriptions shows none of them, and any diff/sync tool built on the batch read concludes every statement has no description.
Fix: the BatchGetPreparedStatement handler/serializer, sharing the same PreparedStatement projection as GetPreparedStatement, over state that retains Description.
Traces: athena-prepared-statement-parameterized-query-workflow#task#7.

Neither cause is covered by an existing open issue: 20261104T163600Z-athena-listpreparedstatements-stub.md is about ListPreparedStatements being an unimplemented stub, and 20261109T123922Z-athena-cloudformation-aws-athena-preparedstatement-is-not-a.md is about the CloudFormation resource type; both leave the Description round-trip on the Get/BatchGet read paths unaddressed.

## Evidence
Fixture athena-prepared-statement-parameterized-query-workflow (CreatePreparedStatement is called earlier in the same fixture with a Description, then the statement is read back).

Cause 1 - step task#5, GetPreparedStatement, expected 200 / actual 200:
  $.PreparedStatement.Description: missing (expected 'parameterized test statement', got None)
Expected: the PreparedStatement structure includes Description holding the string passed to CreatePreparedStatement.
Actual: the member is absent from the response body.

Cause 2 - step task#7, BatchGetPreparedStatement, expected 200 / actual 200:
  $.PreparedStatements[0].Description: missing (expected 'parameterized test statement', got None)
Expected: each element of PreparedStatements includes Description holding the created description.
Actual: the member is absent from every element.

Model basis: smithy_query operation=CreatePreparedStatement shows input member Description (DescriptionString, @length min 1 max 1024); smithy_query shape=PreparedStatement shows member Description (DescriptionString, @length 1..1024, doc "The description of the prepared statement"), returned by GetPreparedStatement (output PreparedStatement) and by BatchGetPreparedStatement (output PreparedStatements -> PreparedStatementDetailsList).
No status or error-code divergence is involved; both are 200 responses with a dropped member. The quoted description text is the fixture's request value, not part of the requirement - the requirement is that whatever Description the create request carried is returned verbatim by both read paths.

## Steps to Reproduce
1. Replay fixture athena-prepared-statement-parameterized-query-workflow against the emulator (replay_wire_corpus).
2. Cause 1: at step task#5, CreatePreparedStatement has already been called in the fixture with a non-empty Description; call GetPreparedStatement with that StatementName and WorkGroup and inspect the 200 body - PreparedStatement.Description is absent instead of echoing the created description.
3. Cause 2: at step task#7, call BatchGetPreparedStatement with that statement name and WorkGroup and inspect PreparedStatements[0] - Description is absent there too.
4. Equivalent manual check: CreatePreparedStatement(WorkGroup=<wg>, StatementName=<name>, QueryStatement='SELECT ?', Description='any text'), then GetPreparedStatement and BatchGetPreparedStatement for the same name; both must return Description='any text'.
