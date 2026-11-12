Issue-Source: trace-sweep-divergence
Issue-Trace: glue-integration-property-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-integration-property-error-paths glue-integration-resource-and-table-properties-errors glue-integration-resource-property-round-trip
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|DeleteIntegrationResourceProperty|4xx->4xx|$.Message
Issue-Key: glue|DeleteIntegrationResourceProperty|4xx->4xx|$.__type
Issue-Commit: e2a7b75abf70ab47f92f0d33522cc824ac8688ce

# Glue DeleteIntegrationResourceProperty refusals diverge: wrong error class and emulator-authored not-found message (2 causes)

> _Note: this issue was identified against the code base at commit `e2a7b75abf70ab47f92f0d33522cc824ac8688ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIntegrationResourceProperty

## Description
This issue bundles 2 net-new causes on the delete leg of the zero-ETL integration-property family. Shared fault: DeleteIntegrationResourceProperty refuses with the emulator's own generic ARN-kind/target-resolution check instead of the operation's not-found rule, so the message is emulator-authored in both causes and the error class is wrong in one. The already-open issue 20261029T201859Z-glue-integration-resource-and-table-property-errors-and.md ('Glue integration resource and table property errors and ARNs diverge from AWS') describes the same fault for the Create/Get legs and 20261030T000205Z-glue-delete-path-refusals-carry-emulator-authored-messages.md covers DeleteIntegrationTableProperties/DeletePartition; neither carries these DeleteIntegrationResourceProperty replay keys, so the delete leg of this operation is unowned and filed here.

Cause 1 of 2: DeleteIntegrationResourceProperty for a Glue connection ARN that carries no integration resource property.
AWS: Smithy: DeleteIntegrationResourceProperty input ResourceArn (String512, @required, @length 1..512); modeled errors include EntityNotFoundException (client, httpError 400, member Message: 'A message describing the problem'), InvalidInputException and ValidationException. Recorded behavior: status 400 with __type EntityNotFoundException and Message following the template 'Failed to find the resource property with arn <resourceArn> in the table' — the fixed text is 'Failed to find the resource property with arn ' and ' in the table'; the only variable part is the ResourceArn copied from the request.
Emulator: status 400 and the right class, but Message 'The target resource <resourceArn> is not found.' — wording the service never emits for this operation (it is the emulator's generic target-resolution sentence).
Why: a client (or an assertion/log scraper) matching AWS's not-found wording for a missing integration resource property does not recognise the refusal, and the message no longer says that it was the resource *property* that was absent rather than the resource itself.
Fix: the DeleteIntegrationResourceProperty handler's not-found refusal message (emit the operation's own template rather than the shared target-resolution message).
Traces: glue-integration-property-error-paths#task#2.

Cause 2 of 2: DeleteIntegrationResourceProperty for a Glue Data Catalog table ARN (a resource kind that holds no integration resource property).
AWS: Smithy: same operation/errors as cause 1. Recorded behavior: status 400 with __type EntityNotFoundException and Message on the same template 'Failed to find the resource property with arn <resourceArn> in the table' (fixed text as above, ResourceArn taken from the request). AWS resolves the ARN form first and then reports the missing property; it does not reject the ARN kind.
Emulator: status 400 with __type InvalidInputException and Message 'ResourceArn "<resourceArn>" is not the ARN of a Glue connection or a Glue database' — the generic ARN-kind precondition fires instead of the not-found classification.
Why: the wrong fault class changes client control flow: InvalidInputException reads as a permanent, client-side request defect, so an SDK caller that would otherwise treat EntityNotFoundException as 'nothing to delete' (an idempotent-delete / create-then-retry path) aborts instead.
Fix: DeleteIntegrationResourceProperty error classification — resolve ResourceArn, then refuse missing properties with EntityNotFoundException and the operation's message template, rather than pre-empting with the shared ARN-kind validator.
Traces: glue-integration-resource-and-table-properties-errors#task#3 (also glue-integration-resource-property-round-trip#task#2).

## Evidence
Replay of simparity-wire-aws-v1 (154 fixtures, 1292 steps; emulator up, /__admin/health 200):
1. glue-integration-property-error-paths#task#2 — DeleteIntegrationResourceProperty, expected status 400 / actual 400: $.Message value_mismatch — expected 'Failed to find the resource property with arn arn:aws:glue:us-west-2:000000000000:connection/<connectionName> in the table', got 'The target resource arn:aws:glue:us-west-2:000000000000:connection/<connectionName> is not found.' ($.__type matched: EntityNotFoundException).
2. glue-integration-resource-and-table-properties-errors#task#3 — DeleteIntegrationResourceProperty, expected status 400 / actual 400: $.__type value_mismatch — expected 'EntityNotFoundException', got 'InvalidInputException'; $.Message value_mismatch — expected 'Failed to find the resource property with arn arn:aws:glue:us-west-2:000000000000:table/<databaseName>/<tableName> in the table', got 'ResourceArn "arn:aws:glue:us-west-2:000000000000:table/<databaseName>/<tableName>" is not the ARN of a Glue connection or a Glue database'. Same cluster: glue-integration-resource-property-round-trip#task#2.
In the message templates above, 'Failed to find the resource property with arn ' / ' in the table' is fixed text; the ARN (and the account, region, connection/database/table names inside it) is the request's ResourceArn. The concrete names and account id in the recording are that run's values and are not part of the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Re-replay these steps: glue-integration-property-error-paths#task#2 and glue-integration-resource-and-table-properties-errors#task#3 (plus glue-integration-resource-property-round-trip#task#2 in the same cluster).
4. For each, call DeleteIntegrationResourceProperty with a ResourceArn that is a well-formed Glue connection ARN with no integration resource property (cause 1) and with a Glue Data Catalog table ARN (cause 2); compare the response $.__type and $.Message against the AWS behavior quoted per cause.

## Resolution
Both causes are FIXED. The behaviour had already been corrected when this issue was picked up: the delete no longer pre-empts its own lookup with the family's shared ARN-kind/target-resolution reading of `ResourceArn`, so a Glue ARN of any resource kind reaches the record lookup and a miss is the modelled `EntityNotFoundException` carrying the operation's own record-miss template (`services/service-com-amazonaws-glue-awsglue/src/ops/delete_integration_resource_property.rs`, with `names_a_glue_resource` and `removed_resource_property_miss` in `src/state/integration_properties.rs`); the sibling issue 20261029T201859Z-glue-integration-resource-and-table-property-errors-and.md carried that change for the Create/Get legs. Replay of the three fixtures named above passes, with both Issue-Key lines reported fixed.

The rule is now pinned by tests, so neither cause can regress: `tests/all/integration_resource_properties.rs` asserts the class and the exact sentence over a table of Glue ARNs spanning resource kind, whether the resource is held, the account/Region/partition the ARN carries and the form of its resource portion, and `tests/all/integration_resource_property_delete_misses.rs` asserts the same miss at the declared length bounds of that member, with the request boundary's constraint refusal past each bound.
