Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-association-create-and-list
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-association-create-and-list ssm-association-create-list-delete ssm-association-lifecycle-listing ssm-state-manager-association-workflow ssm-association-and-opsitem-error-paths
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-ssmdoc-0a8f1c27"].LastExecutionDate
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-ssmdoc-0a8f1c27"].Overview.AssociationStatusAggregatedCount
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-ssmdoc-0a8f1c27"].Overview.DetailedStatus
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-ssmdoc-0a8f1c27"].Overview.Status
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="AWS-RunShellScript"].LastExecutionDate
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="AWS-RunShellScript"].Overview.AssociationStatusAggregatedCount
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="AWS-RunShellScript"].Overview.DetailedStatus
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="AWS-RunShellScript"].Overview.Status
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-probe-ssmdoc-b0f66d1f"].LastExecutionDate
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-probe-ssmdoc-b0f66d1f"].Overview.AssociationStatusAggregatedCount
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-probe-ssmdoc-b0f66d1f"].Overview.DetailedStatus
Issue-Key: ssm|ListAssociations|2xx->2xx|$.Associations[Name="sim2real-probe-ssm-probe-ssmdoc-b0f66d1f"].Overview.Status
Issue-Key: ssm|UpdateAssociation|2xx->2xx|$.AssociationDescription.DocumentVersion
Issue-Key: ssm|UpdateAssociation|2xx->2xx|$.AssociationDescription.LastExecutionDate
Issue-Key: ssm|UpdateAssociation|2xx->2xx|$.AssociationDescription.LastSuccessfulExecutionDate
Issue-Key: ssm|UpdateAssociation|2xx->2xx|$.AssociationDescription.Overview.DetailedStatus
Issue-Key: ssm|UpdateAssociation|2xx->2xx|$.AssociationDescription.Targets
Issue-Key: ssm|DeleteAssociation|4xx->4xx|$.Message
Issue-Key: ssm|DescribeAssociation|4xx->4xx|$.Message
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# ssm association reads and writes diverge: not-found wording and ListAssociations/UpdateAssociation projection

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAssociations

## Description
The emulator's SSM State Manager association surface diverges from AWS on the LIST and UPDATE read paths and on the AssociationDoesNotExist refusal message. This bundle carries 6 causes; a fixer must close all 6 before submitting.

Related but NOT covering: open issue 20261105T101017Z-ssm-state-manager-associations-refusal-wording-response-projection.md covers CreateAssociation/DescribeAssociation projection and the deleted-association read returning 200. The causes below are different divergence clusters: ListAssociations and UpdateAssociation projection (never addressed there), and the message TEXT of a refusal that now correctly returns 400 (that issue only covered the 400-vs-200 status). Smithy model used throughout: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json.

Cause 1 of 6 - ListAssociations omits the per-association execution date and association overview detail, and reports the association as Pending forever (fixture ssm-association-create-and-list, step task#5).
AWS: Smithy ListAssociations output Associations targets AssociationList of Association. Association.LastExecutionDate targets DateTime, doc "The date on which the association was last run." Association.Overview targets AssociationOverview, whose members are Status (doc "The status of the association. Status can be: Pending, Success, or Failed."), DetailedStatus (doc "A detailed status of the association.") and AssociationStatusAggregatedCount (a map of target counts by status). The recording shows a created association, once it has run, listed with Overview.Status = "Success", Overview.DetailedStatus = "Success", an AssociationStatusAggregatedCount map present, and LastExecutionDate populated. Those values are state-derived (the timestamp is the association's own last run time, the counts come from its targets); they are not literals of one run.
Emulator: the ListAssociations 200 body carries the association entry but with LastExecutionDate, Overview.DetailedStatus and Overview.AssociationStatusAggregatedCount all absent, and Overview.Status pinned at "Pending" - the association never advances out of Pending.
Why: a client that lists associations to find which ones have converged sees every association permanently Pending with no last-run time and no per-status target counts, so convergence/wait loops never finish and compliance reporting built on ListAssociations reports nothing has ever executed.
Fix: the association state machine in the association store (advance an association past Pending and stamp its execution date) plus the ListAssociations serializer, which must project LastExecutionDate and the full AssociationOverview onto each Association entry.
Traces: ssm-association-create-and-list#task#5 (cluster also covers ssm-association-create-and-list#task#6).

Cause 2 of 6 - the same ListAssociations projection gap on an association bound to the AWS-owned document AWS-RunShellScript (fixture ssm-association-create-list-delete, step task#3).
AWS: same Smithy members as Cause 1 (Association.LastExecutionDate, Association.Overview.{Status,DetailedStatus,AssociationStatusAggregatedCount}); recorded behavior is Overview.Status "Success", Overview.DetailedStatus "Success", an AssociationStatusAggregatedCount map and a populated LastExecutionDate for the AWS-RunShellScript association.
Emulator: LastExecutionDate, Overview.DetailedStatus and Overview.AssociationStatusAggregatedCount missing; Overview.Status "Pending".
Why: as Cause 1 - and it confirms the gap is not specific to customer-created documents, so a fix that only populates the overview for self-created documents leaves this path broken.
Fix: same component as Cause 1 - the association store's status progression and the ListAssociations serializer, for AWS-owned documents too.
Traces: ssm-association-create-list-delete#task#3.

Cause 3 of 6 - the same ListAssociations projection gap in the association-lifecycle listing flow (fixture ssm-association-lifecycle-listing, step task#5).
AWS: same Smithy members and same recorded shape as Cause 1 (Overview.Status "Success", Overview.DetailedStatus "Success", AssociationStatusAggregatedCount present, LastExecutionDate populated).
Emulator: LastExecutionDate, Overview.DetailedStatus and Overview.AssociationStatusAggregatedCount missing; Overview.Status "Pending".
Why: as Cause 1; this is the third independent cluster of the same projection gap, so it must be re-replayed separately to confirm the fix.
Fix: same component as Cause 1.
Traces: ssm-association-lifecycle-listing#task#5 (cluster also covers ssm-association-lifecycle-listing#task#6).

Cause 4 of 6 - UpdateAssociation's 200 response drops DocumentVersion, both execution dates, Overview.DetailedStatus and the Targets it was just asked to set (fixture ssm-state-manager-association-workflow, step task#3).
AWS: Smithy UpdateAssociation output is AssociationDescription. That shape declares DocumentVersion (pattern ^([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$)$, doc "The document version."), LastExecutionDate ("The date on which the association was last run."), LastSuccessfulExecutionDate ("The last date on which the association was successfully run."), Overview -> AssociationOverview.DetailedStatus, and Targets ("The managed nodes targeted by the request.", a Targets list of 0..5 Key/Values entries). The recording shows UpdateAssociation echoing back the updated association in full: DocumentVersion resolved to "$DEFAULT" when the request pins no version, the two execution timestamps as state-derived values, Overview.DetailedStatus present, and Targets equal to the Targets member of the UpdateAssociation request (a tag:<key> / Values pair in the recording).
Emulator: the UpdateAssociation 200 body's AssociationDescription omits DocumentVersion, LastExecutionDate, LastSuccessfulExecutionDate, Overview.DetailedStatus and Targets entirely. Targets is the most serious: the member the caller supplied in the update is not reflected back at all.
Why: a client cannot confirm from the update response that its new target set, schedule binding or document version took effect - SDK code that reads AssociationDescription.Targets after an update to verify or to diff the association dereferences a missing member, and infrastructure tools that round-trip the description back into a subsequent update silently drop the targets.
Fix: the UpdateAssociation handler and serializer - apply the request's members to the stored association record and project the complete AssociationDescription (DocumentVersion, Targets, the execution dates and the full AssociationOverview) on the way out.
Traces: ssm-state-manager-association-workflow#task#3.

Cause 5 of 6 - DeleteAssociation's AssociationDoesNotExist message is the Smithy shape's documentation sentence instead of AWS's message (fixture ssm-association-and-opsitem-error-paths, step task#4).
AWS: Smithy DeleteAssociation declares error AssociationDoesNotExist (errorKind client, httpError 400, awsQueryError code AssociationDoesNotExist), whose single member is Message. Recorded behavior for an unknown AssociationId: HTTP 400 with Message = "Association <AssociationId> does not exist." The fixed text is "Association " ... " does not exist."; the one variable part is the association id taken verbatim from the request's AssociationId member.
Emulator: returns HTTP 400 with the right error class but Message = "The specified association doesn't exist." - that string is the AssociationDoesNotExist shape's own doc trait text, not the message AWS emits, and it names no association id.
Why: the refusal no longer identifies WHICH association was missing, so a caller deleting several associations cannot tell from the error which id failed; and any client or golden-output test asserting the AWS message text fails even though status and code are right.
Fix: the DeleteAssociation handler's not-found path - compose the AssociationDoesNotExist Message from the requested association id rather than emitting the shape's documentation sentence.
Traces: ssm-association-and-opsitem-error-paths#task#4 (cluster also covers ssm-association-create-and-list#teardown#9).

Cause 6 of 6 - DescribeAssociation's AssociationDoesNotExist message has the same defect (fixture ssm-association-create-list-delete, step teardown#5).
AWS: Smithy DescribeAssociation declares error AssociationDoesNotExist (client, httpError 400, awsQueryError code AssociationDoesNotExist), member Message. Recorded behavior for an association id that was deleted: HTTP 400 with Message = "Association <AssociationId> does not exist." - fixed text "Association " ... " does not exist.", variable part the association id from the request.
Emulator: HTTP 400 with Message = "The specified association doesn't exist." (again the shape's doc sentence, with no id).
Why: as Cause 5, on the read path that teardown and existence-check logic uses; the error carries no id, so a poller that logs or correlates refusals by association id loses that correlation.
Fix: the DescribeAssociation handler's not-found path - same message composition as Cause 5, ideally via one shared AssociationDoesNotExist constructor taking the association id.
Traces: ssm-association-create-list-delete#teardown#5 (cluster also covers ssm-association-lifecycle-listing#teardown#9, ssm-state-manager-association-workflow#teardown#14).

## Evidence
Cause 1 - ssm-association-create-and-list#task#5, ListAssociations, 200 -> 200 (2 affected steps): $.Associations[Name="<doc>"].LastExecutionDate missing (expected a timestamp, got None); $.Associations[Name="<doc>"].Overview.AssociationStatusAggregatedCount missing (expected a map, got None); $.Associations[Name="<doc>"].Overview.DetailedStatus missing (expected 'Success', got None); $.Associations[Name="<doc>"].Overview.Status value_mismatch (expected 'Success', got 'Pending').

Cause 2 - ssm-association-create-list-delete#task#3, ListAssociations, 200 -> 200 (1 affected step): $.Associations[Name="AWS-RunShellScript"].LastExecutionDate missing; .Overview.AssociationStatusAggregatedCount missing (expected {}); .Overview.DetailedStatus missing (expected 'Success'); .Overview.Status expected 'Success', got 'Pending'.

Cause 3 - ssm-association-lifecycle-listing#task#5, ListAssociations, 200 -> 200 (2 affected steps): same four keys - LastExecutionDate missing, Overview.AssociationStatusAggregatedCount missing, Overview.DetailedStatus missing (expected 'Success'), Overview.Status expected 'Success', got 'Pending'.

Cause 4 - ssm-state-manager-association-workflow#task#3, UpdateAssociation, 200 -> 200 (1 affected step): $.AssociationDescription.DocumentVersion missing (expected '$DEFAULT', got None); $.AssociationDescription.LastExecutionDate missing; $.AssociationDescription.LastSuccessfulExecutionDate missing; $.AssociationDescription.Overview.DetailedStatus missing (expected 'Creating', got None); $.AssociationDescription.Targets missing (expected the request's Targets, recorded as [{'Key': 'tag:Environment', 'Values': ['<value from request>']}], got None).

Cause 5 - ssm-association-and-opsitem-error-paths#task#4, DeleteAssociation, 400 -> 400 (2 affected steps): $.Message value_mismatch - expected 'Association <AssociationId> does not exist.' (recording used the all-zero id 00000000-0000-0000-0000-000000000000), got "The specified association doesn't exist."

Cause 6 - ssm-association-create-list-delete#teardown#5, DescribeAssociation, 400 -> 400 (3 affected steps): $.Message value_mismatch - expected 'Association <AssociationId> does not exist.', got "The specified association doesn't exist."

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay fixture ssm-association-create-and-list to step task#5 - CreateDocument, CreateAssociation, then ListAssociations; inspect the association entry's LastExecutionDate and Overview.{Status,DetailedStatus,AssociationStatusAggregatedCount}.
3. Cause 2: replay fixture ssm-association-create-list-delete to step task#3 - CreateAssociation on AWS-RunShellScript, then ListAssociations; inspect the same four keys.
4. Cause 3: replay fixture ssm-association-lifecycle-listing to step task#5 - same ListAssociations assertion on the lifecycle fixture's document.
5. Cause 4: replay fixture ssm-state-manager-association-workflow to step task#3 - UpdateAssociation on the association created in setup, supplying Targets; inspect the 200 body's AssociationDescription for DocumentVersion, LastExecutionDate, LastSuccessfulExecutionDate, Overview.DetailedStatus and Targets.
6. Cause 5: replay fixture ssm-association-and-opsitem-error-paths to step task#4 - DeleteAssociation with an AssociationId that does not exist; read the 400 body's Message and check it names the requested id.
7. Cause 6: replay fixture ssm-association-create-list-delete to step teardown#5 - DescribeAssociation after the association was deleted; read the 400 body's Message and check it names the requested id.
