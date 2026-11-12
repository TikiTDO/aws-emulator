Issue-Source: trace-sweep-divergence
Issue-Trace: textract-sync-text-detection-and-document-analysis
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: textract-sync-text-detection-and-document-analysis
Issue-Shape-Id: com.amazonaws.textract#Textract
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*]
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Confidence
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Height
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Left
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Top
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Width
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].X
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].Y
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Geometry.RotationAngle
Issue-Key: textract|DetectDocumentText|2xx->2xx|$.Blocks[*].Text
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*]
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].BlockType
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Confidence
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].EntityTypes
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Height
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Left
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Top
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.BoundingBox.Width
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].X
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.Polygon[*].Y
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Geometry.RotationAngle
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Relationships
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Relationships[*]
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Relationships[*].Type
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].Text
Issue-Key: textract|AnalyzeDocument|2xx->2xx|$.Blocks[*].TextType
Issue-Commit: 4280f890470fc3016a7f7b2f270013172addcc68

# Textract wire replay: derived Blocks do not describe the submitted document (text, confidence, page geometry) and the analysis block set is truncated

> _Note: this issue was identified against the code base at commit `4280f890470fc3016a7f7b2f270013172addcc68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
Replaying the wire corpus simparity-wire-aws-v1 against the emulator booted from this checkout, both synchronous Textract read steps of the fixture textract-sync-text-detection-and-document-analysis answer HTTP 200 (the recorded status) but the Blocks they return do not describe the document that was submitted: the recognised text, the per-block Confidence and the blocks' Geometry all diverge from the recorded AWS response for the SAME request bytes, and on the AnalyzeDocument step the block set is additionally much shorter than the recorded one and the analysis-only members (BlockType/TextType, EntityTypes, Relationships) diverge. Because the status is 200 and the payload is structurally valid, a client sees no failure at all. This issue bundles 2 distinct root causes, both in the textract crate; a fixer must close BOTH before submitting.

Cause 1 of 2 — DetectDocumentText: the PAGE/LINE/WORD blocks report text, confidence and geometry that are not measurements of the submitted page (fixture textract-sync-text-detection-and-document-analysis, step task#1).
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText output {DocumentMetadata, Blocks, DetectDocumentTextModelVersion}; Block$Text is "The word or line of text that's recognized by Amazon Textract", Block$Confidence (target Percent, @range 0..100) is "The confidence score that Amazon Textract has in the accuracy of the recognized text and the accuracy of the geometry points around the recognized text", Block$Geometry is "The location of the recognized text on the image"; BoundingBox members Left/Top/Width/Height are documented as RATIOS of the overall document page (origin at the upper-left), and Polygon is "within the bounding box, a fine-grained polygon around the recognized item"; BlockType PAGE "Contains a list of the LINE Block objects that are detected on a document page". So for a given page AWS reports, for each detected item, the characters actually drawn, a confidence in the accuracy of that read, and a box/polygon located where that item sits on the page — and the PAGE block spans the page's content, which for a text page covers substantially the whole page. The requirement is that these describe whatever document is submitted, not one recording's numbers.
Emulator: status matches (200) but the content does not. The replay reports value mismatches on every geometry member of the PAGE block and on the text, confidence and geometry of the LINE/WORD blocks that follow it. The PAGE block's box is a small fragment of the page (Left/Top/Width/Height = 0.0356/0.2333/0.2733/0.1667 — about 27% of the page width by 17% of its height) where the recorded AWS PAGE block spans essentially the page (0.0118/0.1004/0.8972/0.8755); its Polygon diverges on all four points accordingly. The first text block's Confidence is 81.98 where AWS recorded 99.93, and Blocks[1].Text, Blocks[2].Text (and Blocks[2].Geometry.RotationAngle) diverge too. The pattern — a correct block skeleton whose strings are wrong, whose confidences are depressed, and whose page extent covers only the sliver of ink the reader matched — is the shared recogniser under-reading the page: crate::state::ocr measures the ink it can match to a glyph and "reports nothing for ink it could not match" (module docs of services/service-com-amazonaws-textract-textract/src/state.rs), and the PAGE box is then derived from only that matched ink, so unmatched marks are missing both from the text and from the page's extent.
Why: a client calls DetectDocumentText to read a document. It gets a 200 with a well-formed PAGE/LINE/WORD tree that passes its schema checks, yet the strings are characters that were never on the page, the confidence it uses as a quality gate is understated for a clean printed page, and the boxes it uses to crop, highlight or sort text point at the wrong region of the page. Nothing in the response signals the failure, so downstream indexing, redaction and layout all operate on text and coordinates the document does not contain.
Fix: the glyph recognition and ink measurement in the service's OCR module (crate::state::ocr, reached from crate::state::read_submitted_document / read_content), and the geometry derivation above it in crate::state (blocks / bounding_box) — matched glyphs must resolve to the characters drawn, the confidence must reflect that read, and the PAGE block's extent must cover the page's marks rather than only the fragment the reader matched.
Traces: textract-sync-text-detection-and-document-analysis#task#1.

Cause 2 of 2 — AnalyzeDocument: the analysis response carries far fewer blocks than AWS recorded, and the analysis-only block members diverge (fixture textract-sync-text-detection-and-document-analysis, step task#2).
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. BlockType enum includes PAGE, LINE, WORD, KEY_VALUE_SET, TABLE, CELL, MERGED_CELL, SELECTION_ELEMENT, QUERY, QUERY_RESULT; Block$EntityTypes documents KEY and VALUE for FORMS analysis (and COLUMN_HEADER etc. for TABLES); Block$Relationships is "a list of relationship objects that describe how blocks are related to each other", with the KEY block carrying a VALUE relationship and CHILD relationships to its words; Block$TextType reports printed vs handwritten. An AnalyzeDocument answer therefore contains the detected text blocks PLUS the feature-derived blocks, which is why the recorded response for this request carries blocks through index 26.
Emulator: the replay reports whole-element divergences at $.Blocks[7] through $.Blocks[26] (no sub-path under them) — the emulator's Blocks list has no counterpart element at those positions, i.e. the analysis answer is truncated to roughly a quarter of the recorded block set — together with member mismatches on the blocks it does return: $.Blocks[3].BlockType and $.Blocks[3].TextType, $.Blocks[5].EntityTypes, $.Blocks[5].Relationships[0].Type and $.Blocks[5].Relationships[1], $.Blocks[6].Relationships, plus the same text/confidence/geometry divergences as Cause 1 on Blocks[0..6] (PAGE box 0.0333/0.2333/0.3778/0.1733 against the recorded 0.0091/0.0118/0.9776/0.9819; Blocks[1].Confidence 69.46 against 94.70). So the feature-derived blocks AWS returns for this document are largely absent, and those that are present are classified and related differently: the missing elements are the KEY_VALUE_SET / table / child blocks and their relationships, which the emulator derives from the recognised character text (crate::state::blocks -> form_blocks / labelled_split and table_runs / cell_texts) rather than from the page's recognised words and layout.
Why: a client asks for AnalyzeDocument instead of DetectDocumentText precisely to get the feature blocks and the relationship graph. It receives HTTP 200 with a block list that is a fraction of the real one, so its traversal finds no key/value or cell blocks to follow, and the EntityTypes/Relationships it does find point somewhere else — it concludes the document has no fields or tables rather than that the service failed to analyse it.
Fix: AnalyzeDocument's feature derivation in crate::state (blocks, form_blocks / labelled_split, table_runs / cell_texts / table_blocks, query_blocks) must emit one block per detected item with its KEY/VALUE EntityTypes and VALUE/CHILD Relationships derived from the recognised words' geometry, and the block classification (BlockType/TextType) in that same derivation must match the item it describes; it sits downstream of the recogniser in Cause 1, so it cannot be confirmed fixed until that one is.
Traces: textract-sync-text-detection-and-document-analysis#task#2.

Related open issues, none of which covers these replay keys (both divergences came back with an empty owned_by): 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md and 20261110T200812Z-textract-analyzedocument-tables-cell-blocks-carry-garbled-text.md are behavioral-corpus issues keyed on check ids (textract.detect-document-text-reads-rendered-text, textract.analyze-document-combined-features-tables-and-forms, ...) and carry no wire fixture/step binding, and 20261030T113700Z-textract-detectdocumenttext-positive-control-after-a-rejected-corrupt-image.md covers a different, post-refusal control path. The wire evidence here adds the PAGE-extent and block-count divergences against a recorded AWS response for the same request.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout: emulator_up=true (/__admin/health 200, data listener accepting), bootstrap_signal=false, 6 fixtures replayed, 38 steps: 27 passed, 11 mismatched, 0 errored, 0 blocked, score 71.1%. Both reported divergence clusters are textract, both on fixture textract-sync-text-detection-and-document-analysis, both with stub_path empty and owned_by empty.

Cause 1 — step task#1, operation DetectDocumentText, verdict mismatch, expected_status 200, actual_status 200. Diverging paths (value_mismatch, expected -> actual):
  $.Blocks[0].Geometry.BoundingBox.Height 0.875543475151062 -> 0.1666666716337204
  $.Blocks[0].Geometry.BoundingBox.Left   0.011839513666927814 -> 0.035555556416511536
  $.Blocks[0].Geometry.BoundingBox.Top    0.10043054074048996 -> 0.23333333432674408
  $.Blocks[0].Geometry.BoundingBox.Width  0.8971883058547974 -> 0.2733333706855774
  $.Blocks[0].Geometry.Polygon[0..3].X/Y  all four points diverge (e.g. Polygon[0].X 0.9090278148651124 -> 0.12444444745779037)
  $.Blocks[1].Confidence                  99.9255599975586 -> 81.9842300415039
  $.Blocks[1].Geometry.BoundingBox.*, $.Blocks[1].Geometry.Polygon[*], $.Blocks[1].Text
  $.Blocks[2].Confidence, $.Blocks[2].Geometry.BoundingBox.*, $.Blocks[2].Geometry.Polygon[*], $.Blocks[2].Geometry.RotationAngle, $.Blocks[2].Text, and on into $.Blocks[3].
The PAGE block (Blocks[0]) is the clearest reading: AWS recorded a box spanning the page (Left 0.0118, Top 0.1004, Width 0.8972, Height 0.8755); the emulator reports Left 0.0356, Top 0.2333, Width 0.2733, Height 0.1667. No status or error-code divergence — the failure is entirely in response content.

Cause 2 — step task#2, operation AnalyzeDocument, verdict mismatch, expected_status 200, actual_status 200. Diverging paths:
  whole-element divergences, no sub-path, at $.Blocks[7], $.Blocks[8], $.Blocks[9], $.Blocks[10] ... $.Blocks[26] — the emulator's Blocks list carries no matching element at those indices, where the recorded AWS response carries blocks through index 26;
  $.Blocks[3].BlockType, $.Blocks[3].TextType;
  $.Blocks[5].EntityTypes, $.Blocks[5].Relationships[0].Type, $.Blocks[5].Relationships[1];
  $.Blocks[6].Relationships;
  plus the Cause-1 pattern on Blocks[0..6]: $.Blocks[0].Geometry.BoundingBox.Height 0.9818877577781676 -> 0.1733333319425583, Left 0.009066186845302582 -> 0.03333333507180214, Top 0.011760788038372992 -> 0.23333333432674408, Width 0.9776000380516052 -> 0.3777777850627899, all four Polygon points, and $.Blocks[1].Confidence 94.70218658447266 -> 69.462158203125, $.Blocks[1].Text, $.Blocks[2].Text, $.Blocks[2].Geometry.RotationAngle.

The specific floats, strings and confidences above are one recording's values for one document; the requirement they witness is that each block's Text is the characters drawn, its Confidence reflects that read, its BoundingBox/Polygon locate it on the page as a ratio of page size, and that an AnalyzeDocument answer contains the feature-derived blocks and relationships for the document submitted.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Replay the wire trace set simparity-wire-aws-v1 (fixture textract-sync-text-detection-and-document-analysis).
3. Cause 1: step task#1 issues DetectDocumentText with the fixture's recorded Document bytes. Compare the returned Blocks against the recorded response: the PAGE block's BoundingBox/Polygon, the LINE/WORD Confidence, Text and Geometry (and RotationAngle) all diverge while the status stays 200. Expect the PAGE box to span the page's marks and each text block's Text/Confidence/Geometry to describe the item read.
4. Cause 2: step task#2 issues AnalyzeDocument with the recorded Document and FeatureTypes. Count the Blocks in the response and compare index by index with the recorded response: indices 7..26 have no counterpart, and Blocks[3].BlockType/TextType, Blocks[5].EntityTypes/Relationships and Blocks[6].Relationships differ. Expect one block per detected item, with the feature blocks' EntityTypes (KEY/VALUE) and VALUE/CHILD Relationships present.
5. After the fix, re-replay both steps named in Issue-Trace/Issue-Step and Issue-Trace-Ids — textract-sync-text-detection-and-document-analysis#task#1 and textract-sync-text-detection-and-document-analysis#task#2 — and both must pass.
