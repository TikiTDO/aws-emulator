Issue-Source: trace-sweep-divergence
Issue-Trace: glue-asset-crud-with-forms
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-asset-crud-with-forms glue-attachment-lifecycle glue-glossary-term-lifecycle-asset-association
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetAsset|2xx->2xx|$.Forms.catalog::Resource.Content
Issue-Key: glue|GetAsset|2xx->2xx|$.Attachments.attach_sensitivity_f4e8ee5d
Issue-Key: glue|GetAsset|2xx->2xx|$.MetadataForms[*]
Issue-Commit: 3ca9558fed9445543f97a972598d878b9e498dd9

# Glue GetAsset drops stored form content, attachment entries and metadata forms

> _Note: this issue was identified against the code base at commit `3ca9558fed9445543f97a972598d878b9e498dd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAsset

## Description
GetAsset answers 200 with the asset's form and attachment COLLECTIONS now present, but their payloads hollowed out: the form entry carries no Content, an attached form never appears as an entry in Attachments at all, and MetadataForms comes back empty where the asset holds one. So an asset's written metadata documents are unreadable even though the envelope that should carry them is served. 2 causes, both in the GetAsset read projection.

Cause 1 of 2: a form written onto an asset is read back as an entry with no Content.
  AWS: Smithy: GetAsset -> GetAssetOutput models Forms and Attachments as AssetFormMap, and IterableForms as IterableFormMap. AssetFormMap maps AssetFormKey (the qualified form name) to AssetFormEntry, and AssetFormEntry models exactly two members: FormTypeId ('The identifier of the form type that defines this form's schema', length 1..256) and Content ('The JSON content of the form, conforming to the schema of the specified form type'). Content is therefore the member that carries the document the client supplied at write time, and AWS returns it for every form the asset holds. The recorded 200 body for an asset put with one catalog form carries a Forms entry under the qualified form name whose value has BOTH FormTypeId (the form type's id) and Content (the exact JSON document string passed in the write request).
  Emulator: status 200 matches and the Forms map and its key are now present, but the entry's Content member is absent (replay detail: `$.Forms.catalog::Resource.Content: missing (expected '<the form JSON document written by the fixture>', got None)`). FormTypeId and the surrounding members no longer diverge — only the document itself is dropped.
  Why: Content is the whole point of a form. A client that writes a metadata document and reads it back with GetAsset to recover it gets a nil/absent member instead of the document it just stored, so the asset's metadata is write-only: no round-trip, no edit-then-save flow, no validation of a form against its form type. Because FormTypeId IS served, the response looks structurally healthy and the loss is silent — a consumer sees 'a form of type X exists' and concludes the document is empty rather than unserved.
  Fix: the GetAsset handler's output projection for form entries — it must read the stored form document out of the asset record and serialise it into AssetFormEntry.Content alongside FormTypeId, for every entry of Forms (and of Attachments and IterableForms, which share the AssetFormEntry shape).
  Traces: glue-asset-crud-with-forms#task#4 (representative; cluster also covers glue-attachment-lifecycle#task#8 and glue-glossary-term-lifecycle-asset-association#task#7).

Cause 2 of 2: an attachment added to an asset never appears in Attachments, and the asset's metadata form is reported as an empty collection.
  AWS: Smithy: GetAssetOutput.Attachments is an AssetFormMap — 'Additional attachments on the asset for more context, keyed by attachment name' — so after an attachment is added under a given name, the 200 body MUST carry an Attachments entry keyed by that attachment name whose value is an AssetFormEntry with FormTypeId (the id of the form type the attachment was created against) and Content (the attachment's JSON document). The recording additionally shows AWS serialising a MetadataForms list that carries one entry per metadata form held on the asset; MetadataForms is not declared by the on-disk model, so for that member: recorded behavior only. In the recording the asset's Forms entry also carries its Content, as in Cause 1.
  Emulator: status 200 matches, but the named attachment is missing from the Attachments map entirely, MetadataForms is served with no entries where the asset holds one, and the Forms entry's Content is absent (replay detail: `$.Attachments.<attachmentName>: missing (expected {'Content': '<attachment JSON>', 'FormTypeId': '<formTypeId>'}, got None)`, `$.Forms.catalog::Resource.Content: missing (...)`, `$.MetadataForms[0]: missing (expected {}, got None)`). Attachments is thus present-but-empty, which is AWS's representation for an asset with NO attachments — so an asset with one attachment is indistinguishable from an asset with none.
  Why: the attachment lifecycle cannot be observed at all. A client that attaches a document and then lists or reads the asset's attachments to confirm the write, to render them, or to delete one by key, sees an empty map and concludes the attachment was never created — but a re-attach may then conflict, and a delete-by-key cannot be issued because the key is unknown. The same holds for MetadataForms: an empty list means 'none', so the asset's metadata forms are invisible to any consumer that iterates them.
  Fix: the GetAsset handler's output projection plus the asset state it reads — attachments recorded by the attach path must be stored against the asset and emitted as keyed Attachments entries (with FormTypeId and Content), and the asset's metadata forms must be projected into MetadataForms, rather than both being serialised as empty collections.
  Traces: glue-attachment-lifecycle#task#6.

Neither cluster is owned: the replay reports owned_by empty for both, and no stub blocks GetAsset. Open issue 20261108T010218Z-glue-getasset-reads-omit-the-asset-type-and.md ('Glue GetAsset reads omit the asset type and every form collection') described the earlier, wider omission in which Type, Forms, AssetForms, Attachments, IterableForms, MetadataForms and GlossaryTerms were ALL absent; the collections and the asset type are now served, so that issue does not cover this narrower residual — the entry-level Content, the missing Attachments entry and the empty MetadataForms — and the replay does not bind these steps to it. Open issue 20261029T201859Z-glue-job-job-run-session-and-connection-reads.md concerns job/session/connection reads, not the asset projection.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Service glue, operation GetAsset, both clusters verdict 'mismatch' with expected status 200 and actual status 200 — the divergence is entirely in the response body.

Cause 1 — fixture glue-asset-crud-with-forms, step task#4, affected_count 3 (cluster also covers glue-attachment-lifecycle#task#8 and glue-glossary-term-lifecycle-asset-association#task#7):
  $.Forms.catalog::Resource.Content: missing (expected '{"name":"Customer Table","description":"Raw customer records"}', got None)
  The form key 'catalog::Resource' and the quoted JSON document are this recording's values filling the template — the requirement is that the entry's Content member is present and reproduces, byte for byte, the document the preceding write supplied for that form name.

Cause 2 — fixture glue-attachment-lifecycle, step task#6, affected_count 1:
  $.Attachments.attach_sensitivity_f4e8ee5d: missing (expected {'Content': '{"sensitivity":"HIGH","classification":"PII"}', 'FormTypeId': 'FormTypeClassf4e8ee5d'}, got None)
  $.Forms.catalog::Resource.Content: missing (expected '{"name":"Orders Table","description":"Order transactions"}', got None)
  $.MetadataForms[0]: missing (expected {}, got None)
  The attachment name 'attach_sensitivity_f4e8ee5d' and form type id 'FormTypeClassf4e8ee5d' are per-run generated names from this recording; the requirement is that an attachment written under some name is read back as an Attachments entry keyed by that name, carrying that attachment's FormTypeId and Content, and that MetadataForms carries one entry per metadata form the asset holds instead of none.

Model evidence: smithy_query(operation="GetAsset", model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json") — GetAssetOutput members include Forms (AssetFormMap), Attachments (AssetFormMap) and IterableForms (IterableFormMap). smithy_query(shape="AssetFormMap,AssetFormEntry,AssetFormKey") — AssetFormMap: key AssetFormKey (string), value AssetFormEntry; AssetFormEntry is a structure of FormTypeId ('The identifier of the form type that defines this form's schema') and Content ('The JSON content of the form, conforming to the schema of the specified form type'). MetadataForms is not declared in the on-disk model and is evidenced by the recording only.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay fixture glue-asset-crud-with-forms and stop at step task#4 (Cause 1): the fixture creates a form type and an asset type, puts an asset carrying a form under a qualified form name with a JSON document, then calls GetAsset on it. Inspect the 200 body — Forms[<formName>] is present with FormTypeId but its Content member is absent, so the document written in the preceding step cannot be read back.
3. Replay fixture glue-attachment-lifecycle to step task#6 (Cause 2): the fixture puts an asset with a form and then adds an attachment under an attachment name against a second form type, then calls GetAsset. Inspect the 200 body — the Attachments map contains no entry for that attachment name, MetadataForms is served with no entries, and Forms[<formName>].Content is absent.
4. Confirm the collections themselves are present-but-empty (not omitted), so an asset holding one attachment/metadata form is indistinguishable from one holding none.
5. Re-replay glue-asset-crud-with-forms#task#4, glue-attachment-lifecycle#task#6, glue-attachment-lifecycle#task#8 and glue-glossary-term-lifecycle-asset-association#task#7 to confirm a fix.
