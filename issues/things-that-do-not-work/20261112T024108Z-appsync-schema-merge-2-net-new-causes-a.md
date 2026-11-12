Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.e2e-schema-merge-cross-source-query-flow appsync.e2e-schema-merge-source-to-merged-propagation-flow appsync.e2e-source-api-merge-then-disassociate-reverts-types
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.e2e-schema-merge-cross-source-query-flow|run#27|AssertionError|assert "greetingTwo" in merged_query_def, merged_query_def
Issue-Key: appsync.e2e-schema-merge-source-to-merged-propagation-flow|_poll_until#35|Timeout|time.sleep(interval_s)
Issue-Key: appsync.e2e-source-api-merge-then-disassociate-reverts-types|_wait_type_removed#31|Timeout|time.sleep(5)
Issue-Commit: 36913b39d33301b1264c8d78e90a930464327a08

# AppSync schema merge: 2 net-new causes — a second source API's merge replaces a shared root type instead of combining its fields, and disassociation never withdraws the merged types from the Merged API

> _Note: this issue was identified against the code base at commit `36913b39d33301b1264c8d78e90a930464327a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSchemaMerge

## Description
Two independent net-new root causes in the emulator's AppSync Merged-API schema-merge state machine, both under the suite heading `appsync`. BOTH must be closed before this issue is submitted. Neither is an unimplemented scaffold stub (StartSchemaMerge, AssociateSourceGraphqlApi/AssociateMergedGraphqlApi, DisassociateSourceGraphqlApi/DisassociateMergedGraphqlApi, GetType and ListTypesByAssociation are all implemented in this checkout), and neither is carried by any open issue's replay keys.

Cause 1 of 2 — when two source APIs are merged into one Merged API, the type name they share (the root `Query` type) is REPLACED by whichever merge ran last instead of carrying both sources' fields.
AWS: a Merged API combines the schemas of every source API associated with it. Smithy: StartSchemaMerge POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/merge -> sourceApiAssociationStatus (SourceApiAssociationStatus); GetType GET /v1/apis/{apiId}/types/{typeName} -> Type.definition; ListTypesByAssociation GET /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/types. Recorded behavior: after two associations each reach MERGE_SUCCESS, each association's own ListTypesByAssociation view shows only its own source's field, while GetType(apiId=<mergedApiId>, typeName="Query", format="SDL") on the Merged API answers 200 with a SINGLE `Query` type whose definition contains the fields contributed by BOTH sources (the root operation types Query/Mutation/Subscription are field-unioned across sources; a genuine field-name collision is what AWS turns into MERGE_FAILED, which is not the case here — the two fields have different names).
Emulator: both merges report MERGE_SUCCESS and both association-scoped listings are right, but the Merged API's own `Query` type holds only the fields of the source whose merge settled last: `type Query {\n  greetingOne: String\n}` — the other source's field is absent. Replay: AssertionError on `assert "greetingTwo" in merged_query_def` with the definition above. The merged type map is keyed by type NAME and a name already held simply takes the incoming definition, so one source's root type overwrites the other's rather than its fields being combined.
Why: a client that merges two source APIs sees a Merged API that advertises MERGE_SUCCESS for every association while serving only one source's root fields. Any query spanning both sources fails validation against the served schema, and which source survives depends on merge settle order, so the loss is silent and non-deterministic.
Fix: the schema-merge routine in the AppSync state (the `merge_types` composition invoked from the due-merge settle path in the service's state module) must COMBINE the field sets of a type name contributed by several source APIs — in particular the root operation types — rather than inserting the last merge's definition over the held one; a real field-name collision is the case that must drive the association to MERGE_FAILED. File/line citations: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state/schema_merge.rs lines 73-122 (`merge_types`, the `merged.types.extend(merged_types)` at line 120 replaces per-name).
Traces: appsync.e2e-schema-merge-cross-source-query-flow.

Cause 2 of 2 — disassociating a source API leaves its merged types on the Merged API forever, so the Merged API's schema never reverts.
AWS: Smithy: DisassociateMergedGraphqlApi DELETE /v1/sourceApis/{sourceApiIdentifier}/mergedApiAssociations/{associationId} and DisassociateSourceGraphqlApi (the same delete addressed from the Merged API) -> sourceApiAssociationStatus; both model NotFoundException (httpError 404). Recorded behavior: once the deletion settles, GetSourceApiAssociation for that associationId is refused 404 NotFoundException with the message "No source API association was found" (fixed text, no request interpolation) AND the Merged API loses the types that association contributed: GetType(apiId=<mergedApiId>, typeName=<the source's type>, format="SDL") is then refused 404 NotFoundException with the fixed message "Type not found.", and the Merged API's type set returns to exactly the set it held before that source was ever merged in.
Emulator: the association record itself is dropped on schedule (the status goes DELETION_SCHEDULED and the record is retained-out once DELETION_SPAN elapses), but nothing un-merges the schema: the types the merge installed on the Merged API stay readable indefinitely, so GetType on the Merged API keeps answering 200 with the disassociated source's definition and the Merged API's type set never returns to its pre-merge baseline. Both checks exhaust their whole budget inside exactly that poll and are reported as "Check timed out after 60.0s" — the deletion itself settles in well under a second, and the association-gone poll that precedes the type poll converges, so the non-converging condition is the type withdrawal alone.
Why: a client that disassociates a source API from a Merged API believes the Merged API's surface shrank back; instead the Merged API keeps serving and listing types from an API it is no longer associated with, and a client polling for the withdrawal (the documented way to observe a disassociation completing) never terminates. Re-associating a different source under the same type names then also inherits stale definitions.
Fix: the shared association-delete rule and the merge settle path must withdraw the contributed types when an association's deletion is materialised — the Merged API's type state has to be recomposed from the associations that remain, so an association's types disappear with it. File/line citations: services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state.rs `delete_source_api_association` (lines 1845+) and `settle_associations` (lines 1925-1933, which only `retain`s away the record), with services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/state/schema_merge.rs offering only the forward `merge_types` and no inverse; the handlers themselves (src/ops/disassociate_merged_graphql_api.rs, src/ops/disassociate_source_graphql_api.rs) delegate to that rule.
Traces: appsync.e2e-schema-merge-source-to-merged-propagation-flow, appsync.e2e-source-api-merge-then-disassociate-reverts-types.

Related open issues that do NOT carry these three checks' replay keys (so they do not cover the two causes above): 20261109T041647Z-appsync-control-plane-9-net-new-causes-merge.md (its cause 6 is that a MERGE_SUCCESS wrote NO types at all on the merged API — types ARE written now, so what is left is the multi-source composition of cause 1 and the missing withdrawal of cause 2), 20261029T200818Z-appsync-source-api-association-projection-and-merge-status.md (association projection and status detail text) and 20261102T115141Z-appsync-disassociatesourcegraphqlapi-refuses-a-well-formed-unknown-associationid.md (the unknown-associationId refusal class, not the schema effect of a successful disassociation).

Per-run values (api ids, association ids, generated API/type names, merge timestamps, request ids) are not requirements; the statuses, the error classes and messages and the composition/withdrawal rules are.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout; all three checks are listed under the service heading `appsync`.

1) appsync.e2e-schema-merge-cross-source-query-flow [mismatch]: two source APIs (fields `greetingOne` and `greetingTwo`) associated to one MERGED API with a merge execution role; both associations reached MERGE_SUCCESS (`assert status1 == "MERGE_SUCCESS"` and `status2` both passed) and both ListTypesByAssociation scoped views carried their own source's field (`assert "greetingOne" in query_def_1` / `assert "greetingTwo" in query_def_2` passed). The decisive read then failed: `GetType(apiId=<mergedApiId>, typeName="Query", format="SDL")["type"]["definition"]` returned
    type Query {
      greetingOne: String
    }
AssertionError at check line 72: `assert "greetingTwo" in merged_query_def`. Expected a single Query definition containing BOTH greetingOne and greetingTwo.

2) appsync.e2e-schema-merge-source-to-merged-propagation-flow [mismatch]: "Check timed out after 60.0s". The check creates a source API with a distinctive type, StartSchemaCreation->SUCCESS, a MERGED API + merge execution role, AssociateMergedGraphqlApi, waits for a terminal status, StartSchemaMerge, asserts the type is visible on the merged API, then calls DisassociateMergedGraphqlApi and polls (a) GetSourceApiAssociation until NotFoundException "No source API association was found" and (b) GetType(apiId=<mergedApiId>, typeName=<sourceType>, format="SDL") until NotFoundException "Type not found." (poll budget 60s, interval 5s). Poll (b) can never converge: GetType keeps answering 200 with the merged definition.

3) appsync.e2e-source-api-merge-then-disassociate-reverts-types [mismatch]: "Check timed out after 60.0s". Same shape with AssociateSourceGraphqlApi(mergeType=MANUAL_MERGE) + StartSchemaMerge; after the merge the type `Widget4` is present both on the merged API and in the association-scoped listing, then DisassociateSourceGraphqlApi is issued, the association-gone wait is followed by `_wait_type_removed(appsync, merged_api_id, "Widget4", timeout_s=120)` and a final `assert reverted_types == baseline_types` — the type is never removed, so the wait burns the check's budget.

Emulator code evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice):
- src/state/schema_merge.rs:73-122 `merge_types` builds the incoming type map keyed by type name and finishes with `merged.types.extend(merged_types)` — "a name the Merged API already holds ... takes the definition this merge carries". For the root `Query` type that two sources both declare this is a wholesale replacement, not a field union (cause 1).
- src/state.rs:1925-1933 `settle_associations` only `retain`s away associations whose deletion came due and then settles schema creations; src/state.rs:1429-1434 `schedule_deletion` clears the owed merge and sets DELETION_SCHEDULED; DELETION_SPAN and MERGE_SPAN are both 500ms (src/state.rs:2725, 2744), so no span explains the timeouts. There is no inverse of `merge_types` anywhere, i.e. nothing removes a disassociated source's types from the Merged API (cause 2).

Model evidence (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/smithy-model): StartSchemaMerge POST /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/merge, output sourceApiAssociationStatus, errors BadRequestException(400)/ConcurrentModificationException(409)/NotFoundException(404)/UnauthorizedException(401)/InternalFailureException(500). DisassociateMergedGraphqlApi DELETE /v1/sourceApis/{sourceApiIdentifier}/mergedApiAssociations/{associationId}, same error set. GetType GET /v1/apis/{apiId}/types/{typeName} (format required query), output Type, errors include NotFoundException(404). ListTypesByAssociation GET /v1/mergedApis/{mergedApiIdentifier}/sourceApiAssociations/{associationId}/types, paginated on nextToken/types.

## Steps to Reproduce
Boot the emulator from this checkout and point appsync, iam and sts clients at it.

Cause 1:
1. CreateGraphqlApi(name=srcA, authenticationType="API_KEY"); StartSchemaCreation(apiId=srcA, definition="schema { query: Query }\ntype Query { greetingOne: String }\n") and wait for GetSchemaCreationStatus == SUCCESS.
2. Same again for srcB with `type Query { greetingTwo: String }`.
3. CreateRole trusted by appsync.amazonaws.com with an inline policy allowing appsync:SourceGraphQL on both source API ARNs (and "/*").
4. CreateGraphqlApi(name=merged, authenticationType="API_KEY", apiType="MERGED", mergedApiExecutionRoleArn=<that role>).
5. AssociateSourceGraphqlApi (or AssociateMergedGraphqlApi) for each source, then StartSchemaMerge for each association, and poll GetSourceApiAssociation until both report MERGE_SUCCESS.
6. GetType(apiId=<mergedApiId>, typeName="Query", format="SDL"). Expected: one definition containing BOTH greetingOne and greetingTwo. The emulator returns only the field of the source whose merge settled last.

Cause 2:
1. CreateGraphqlApi(name=src, authenticationType="API_KEY"); StartSchemaCreation with "type Widget4 { id: ID! name: String }\ntype Query { getWidget4(id: ID!): Widget4 }\nschema { query: Query }\n"; wait for SUCCESS.
2. Create the merge execution role and a MERGED API as above; record the merged API's baseline type-name set (ListTypes).
3. AssociateSourceGraphqlApi(mergedApiIdentifier, sourceApiIdentifier, sourceApiAssociationConfig={"mergeType": "MANUAL_MERGE"}), then StartSchemaMerge, then poll GetSourceApiAssociation until MERGE_SUCCESS. GetType(apiId=<mergedApiId>, typeName="Widget4", format="SDL") now answers 200 (correct).
4. DisassociateSourceGraphqlApi(mergedApiIdentifier, associationId) (or DisassociateMergedGraphqlApi(sourceApiIdentifier, associationId)). Poll GetSourceApiAssociation until it is refused 404 NotFoundException "No source API association was found" — this converges.
5. Poll GetType(apiId=<mergedApiId>, typeName="Widget4", format="SDL"). Expected: 404 NotFoundException "Type not found.", and the merged API's type-name set back to the step-2 baseline. The emulator keeps answering 200 with the merged definition indefinitely.

Equivalent replay: re-run the three check ids in trace_ids from suite simparity-behavioral-aws-v1.
