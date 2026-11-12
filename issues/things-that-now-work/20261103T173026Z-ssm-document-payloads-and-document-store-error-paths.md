Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-association-create-and-list
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-association-create-and-list ssm-association-lifecycle-listing ssm-document-create-describe-delete ssm-document-duplicate-create-error ssm-document-permission-share-and-revoke ssm-document-permission-share-workflow ssm-document-sharing-workflow ssm-automation-execution-preview-multiple-isolated ssm-automation-filter-steps-and-complete ssm-automation-lifecycle-describe-steps-and-stop ssm-automation-signal-stopstep-and-invalid-filter-error ssm-automation-stop-and-signal-error-paths ssm-execution-preview-automation-workflow ssm-start-execution-preview-basic-success ssm-document-with-metadata-and-force-delete
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.DocumentId
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.PlatformTypes[*]
Issue-Key: ssm|CreateDocument|2xx->2xx|$.DocumentDescription.Tags
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.Category
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.CategoryEnum
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.DocumentId
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.PlatformTypes[*]
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.Tags
Issue-Key: ssm|DescribeDocument|2xx->2xx|$.Document.Status
Issue-Key: ssm|DescribeDocument|4xx->2xx|$.Message
Issue-Key: ssm|DescribeDocument|4xx->2xx|$.__type
Issue-Key: ssm|DescribeDocument|4xx->2xx|status
Issue-Key: ssm|CreateDocument|4xx->4xx|$.Message
Issue-Commit: 133bbb645e9618d24e6206d20cee2b832cb9c7e9

# SSM document payloads and document-store error paths diverge from AWS

> _Note: this issue was identified against the code base at commit `133bbb645e9618d24e6206d20cee2b832cb9c7e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDocument

## Description
The DocumentDescription the emulator serves from CreateDocument and DescribeDocument is partly synthesized rather than derived from the document, and two document-store error paths are wrong. 9 CAUSES.

Cause 1 of 9 - CreateDocument's DocumentDescription omits DocumentId and Tags and reports a platform the document does not support. AWS: the 200 body carries a DocumentId (recorded behavior only - this member is not in the checked-in Smithy DocumentDescription, but the service returns it on every create), Tags as an empty list when none were supplied (Smithy: DocumentDescription.Tags, TagList), and PlatformTypes derived from the document content (Smithy: DocumentDescription.PlatformTypes -> PlatformTypeList of PlatformType [Windows, Linux, MacOS]); for the probe's document AWS reports exactly two platforms. Emulator: DocumentId absent, Tags absent, and a third PlatformTypes element 'Windows' appended - it emits the whole enum regardless of content. Why: a caller keying a document by DocumentId gets null, a caller iterating Tags must special-case null vs [], and a caller choosing targets by PlatformTypes believes a Linux/macOS-only document runs on Windows. Fix: CreateDocument handler/serializer and the document record in SSM state (persist an id and the tag list; derive PlatformTypes from the document's content). Traces: ssm-association-create-and-list#setup#1, ssm-association-lifecycle-listing#setup#1, ssm-document-create-describe-delete#task#1, ssm-document-duplicate-create-error#setup#1, ssm-document-permission-share-and-revoke#setup#1, ssm-document-permission-share-workflow#task#1, ssm-document-sharing-workflow#setup#1.

Cause 2 of 9 - DescribeDocument's Document omits DocumentId, Tags, Category and CategoryEnum and adds the same spurious platform. AWS: Document carries DocumentId, and Tags, Category and CategoryEnum as empty lists when unset (Smithy: DocumentDescription.Category/CategoryEnum, CategoryList/CategoryEnumList @length 0..3), with content-derived PlatformTypes. Emulator: all four absent and a third PlatformTypes element 'Windows'. Fix: DescribeDocument serializer - serialize empty lists for the modelled list members and share the platform derivation with cause 1. Traces: ssm-association-create-and-list#setup#2, ssm-association-lifecycle-listing#setup#2, ssm-document-create-describe-delete#task#2, ssm-document-duplicate-create-error#task#4, ssm-document-permission-share-workflow#task#2.

Cause 3 of 9 - DescribeDocument reports a just-created document as Status 'Creating' and orders PlatformTypes wrongly. AWS: a document created synchronously is Status 'Active' by the time DescribeDocument answers (Smithy: DocumentDescription.Status -> DocumentStatus), and PlatformTypes are returned in the enum's service order Windows, Linux, MacOS. Emulator: Status 'Creating' and PlatformTypes [Linux, MacOS, Windows]. Why: a client that waits for Active before using the document blocks forever, and an index-based read of PlatformTypes[0] gets the wrong platform. Fix: CreateDocument state transition (document becomes Active on create) and the PlatformTypes ordering in the serializer. Traces: ssm-automation-execution-preview-multiple-isolated#task#6.

Cause 4 of 9 - CreateDocument returns PlatformTypes in the wrong order (plus the omissions of cause 1). AWS: [Windows, Linux, MacOS]. Emulator: [Linux, MacOS, Windows]. Fix: as cause 3. Traces: ssm-automation-filter-steps-and-complete#task#1, ssm-automation-lifecycle-describe-steps-and-stop#task#1, ssm-automation-signal-stopstep-and-invalid-filter-error#task#1, ssm-automation-stop-and-signal-error-paths#setup#1, ssm-execution-preview-automation-workflow#task#1, ssm-start-execution-preview-basic-success#task#1.

Cause 5 of 9 - DescribeDocument returns PlatformTypes in the wrong order (plus the omissions of cause 2). Same AWS/emulator split as cause 4. Fix: as cause 3. Traces: ssm-automation-filter-steps-and-complete#task#2, ssm-automation-lifecycle-describe-steps-and-stop#task#2, ssm-automation-signal-stopstep-and-invalid-filter-error#task#2, ssm-automation-stop-and-signal-error-paths#setup#2, ssm-execution-preview-automation-workflow#task#2, ssm-start-execution-preview-basic-success#task#3.

Cause 6 of 9 - CreateDocument on a second document omits DocumentId and appends 'Windows'. Same fault as cause 1 on a document created with metadata. Fix: as cause 1. Traces: ssm-document-with-metadata-and-force-delete#setup#1.

Cause 7 of 9 - DescribeDocument for that document omits DocumentId, Category and CategoryEnum and appends 'Windows'. Fix: as cause 2. Traces: ssm-document-with-metadata-and-force-delete#task#2.

Cause 8 of 9 - DescribeDocument still answers 200 for a deleted document. AWS: 400 InvalidDocument (Smithy: DescribeDocument error InvalidDocument, httpError 400, member Message) with Message 'Document with name <name> does not exist.' - fixed text around the requested name. Emulator: 200 with the document body, as though DeleteDocument had not removed it. Why: a client that polls DescribeDocument to confirm deletion never observes it, and a recreate-after-delete flow sees a stale document. Fix: DeleteDocument's effect on SSM document state and DescribeDocument's not-found path. Traces: ssm-association-lifecycle-listing#teardown#10, ssm-document-create-describe-delete#task#4, ssm-document-with-metadata-and-force-delete#task#4.

Cause 9 of 9 - CreateDocument's duplicate-name message is emulator-authored and region-flavoured. AWS: 400 DocumentAlreadyExists (Smithy: CreateDocument error DocumentAlreadyExists) with $.Message 'Document with same name <name> already exists' - fixed text around the requested name, with no region. Emulator: 'Document <name> already exists in us-west-2'. Why: message-matching clients fail, and the emulator's sentence implies a region scope AWS's text does not state. Fix: CreateDocument duplicate-name error construction. Traces: ssm-document-duplicate-create-error#task#3.

## Evidence
Replay of simparity-wire-aws-v1. Representative (ssm-association-create-and-list#setup#1, CreateDocument): expected 200 / actual 200; $.DocumentDescription.DocumentId missing (expected a shaped id), $.DocumentDescription.Tags missing (expected []), $.DocumentDescription.PlatformTypes[2] extra (expected None, got 'Windows'). ssm-association-create-and-list#setup#2 (DescribeDocument): $.Document.Category, $.Document.CategoryEnum, $.Document.Tags all missing (expected []), $.Document.DocumentId missing, PlatformTypes[2] extra 'Windows'. ssm-automation-execution-preview-multiple-isolated#task#6: $.Document.Status expected 'Active', got 'Creating'; PlatformTypes[0..2] expected Windows,Linux,MacOS, got Linux,MacOS,Windows. ssm-association-lifecycle-listing#teardown#10 (DescribeDocument after delete): status expected 400, got 200; $.__type expected 'InvalidDocument' and $.Message expected 'Document with name <name> does not exist.' both absent. ssm-document-duplicate-create-error#task#3 (CreateDocument): $.Message expected 'Document with same name sim2real-probe-ssm-probe-doc-dup-2e8107b2 already exists', got 'Document sim2real-probe-ssm-probe-doc-dup-2e8107b2 already exists in us-west-2'. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (DocumentDescription - note it declares no DocumentId member, PlatformType enum [Windows, Linux, MacOS], CreateDocument/DescribeDocument errors).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1. 2. Representative: fixture ssm-association-create-and-list step setup#1 - CreateDocument and inspect DocumentDescription for DocumentId, Tags and PlatformTypes. 3. Step setup#2 - DescribeDocument and inspect Category, CategoryEnum, Tags, DocumentId, PlatformTypes. 4. Fixture ssm-automation-execution-preview-multiple-isolated step task#6 - check Status and PlatformTypes ordering; same for ssm-automation-filter-steps-and-complete steps task#1/task#2 and ssm-document-with-metadata-and-force-delete steps setup#1/task#2. 5. Fixture ssm-association-lifecycle-listing step teardown#10 - DeleteDocument then DescribeDocument; expect 400 InvalidDocument. 6. Fixture ssm-document-duplicate-create-error step task#3 - CreateDocument with an existing name; compare $.Message.
