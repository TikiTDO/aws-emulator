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
Issue-Commit: 1dbd6c9a628cf270d6e2528da937a9703bd8df7f

# Textract synchronous Blocks do not describe the submitted document: geometry, text and analysis block set all diverge

> _Note: this issue was identified against the code base at commit `1dbd6c9a628cf270d6e2528da937a9703bd8df7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectDocumentText

## Description
Replaying the simparity-wire-aws-v1 wire corpus against the emulator booted from this checkout, both synchronous Textract document-analysis steps of fixture `textract-sync-text-detection-and-document-analysis` answer HTTP 200 (the recorded status) but the body's `Blocks` array does not describe the document that was submitted: the per-block geometry is synthesized from a fixed layout grid rather than measured from the page, the recognized `Text` and `Confidence` values differ, and on the analysis path the whole block set (count, BlockType, EntityTypes, Relationships) is missing the FORMS/TABLES structure the recording carries. This issue bundles 2 distinct causes, both in the textract crate; a fixer must close BOTH before submitting.

Cause 1 of 2 - DetectDocumentText: Block$Geometry is a synthetic fixed-step layout, Block$Text and Block$Confidence are not the page's.
AWS: Smithy (services/service-com-amazonaws-textract-textract/smithy-model/com-amazonaws-textract-textract.json): DetectDocumentText input {Document required}, output {DocumentMetadata, Blocks: BlockList, DetectDocumentTextModelVersion}. Block members: BlockType (BlockType enum, PAGE/LINE/WORD for text detection), Text ("The word or line of text that's recognized by Amazon Textract"), Confidence (Percent, @range 0..100, "the accuracy of the recognized text and the accuracy of the geometry points around the recognized text"), Geometry {BoundingBox, Polygon, RotationAngle}. BoundingBox is documented as ratios of the overall page: Left/Top are the top-left corner over page width/height, Width/Height the box dimensions over the page dimensions; Point X/Y are likewise ratios, and Polygon is "within the bounding box, a fine-grained polygon around the recognized item"; Geometry$RotationAngle is "a numerical value corresponding to the rotation of the WORD block. Possible values are 0, 90, 180, and 270". So the requirement is: the PAGE block's BoundingBox spans the page content as actually laid out, each LINE/WORD block's BoundingBox and Polygon are that item's measured position on the page (the Polygon tracking the item's real outline, not merely restating the four bbox corners), the RotationAngle reflects the page/word rotation, Text is the characters drawn in those pixels, and Confidence is the recognizer's score for that item. These are per-document measurements, not constants.
Emulator: every block's geometry comes back on a coarse fixed grid independent of the document. For the recorded page the PAGE block (Blocks[0]) is reported as Left 0.0356 / Top 0.2333 / Width 0.2733 / Height 0.1667 where the recording has Left 0.0118 / Top 0.1004 / Width 0.8972 / Height 0.8755 - i.e. the emulator gives the PAGE block a single line-sized box instead of the page's content extent. Its Polygon is just the four axis-aligned corners of that same box ((0.0356,0.2333), (0.3089,0.2333), (0.3089,0.4000), (0.0356,0.4000)) while the recording's polygon is a distinct, rotated four-point outline ((0.9090,0.9760), (0.0118,0.9760), (0.0118,0.1004), (0.9090,0.1004)), so no rotation is represented and the fine-grained polygon carries no information the bounding box does not. The same pattern repeats for every subsequent block: Blocks[1] Left 0.0356/Top 0.2333/Width 0.2733/Height 0.1667 vs recorded 0.0325/0.2160/0.2760/0.1829, and the x-coordinates advance in a constant 0.2733 step with Top/Height pinned to multiples of 0.1667, which is a layout generator rather than a measurement. Alongside it Blocks[1].Confidence is 81.98 where the recording has 99.93, and Blocks[1].Text, Blocks[2].Text and Blocks[2].Geometry.RotationAngle also diverge, so the recognized characters and the rotation are wrong on the same blocks.
Why: a client calls DetectDocumentText to locate and read text. It gets a 200 with a structurally valid BlockList, so nothing in its schema validation or error handling fires, yet every coordinate it crops or highlights points at the wrong region of the page (and the PAGE block understates the document extent by ~5x in each dimension), the text it indexes is not the text in the document, and the confidence it thresholds on is not the recognizer's. Redaction, highlighting and bounding-box-driven extraction all silently operate on the wrong pixels.
Emulator fix: the geometry and text derivation behind DetectDocumentText - the recognizer and block builder in the textract crate's state module (crate::state::ocr / read_submitted_document, feeding crate::state::blocks), and the Geometry construction it emits. BoundingBox/Polygon must be computed from the recognized items' measured positions on the submitted page (with the PAGE block spanning the page's content), the Polygon must be the item's outline rather than a copy of the bbox corners, RotationAngle must follow the detected rotation, and Text/Confidence must come from the recognition of those pixels. No per-run value from the recording is a requirement - the requirement is that the numbers are derived from the submitted document.
Traces: textract-sync-text-detection-and-document-analysis#task#1 (DetectDocumentText, expected 200, actual 200, 200-level body mismatch across $.Blocks[*].Geometry.BoundingBox.{Left,Top,Width,Height}, $.Blocks[*].Geometry.Polygon[*].{X,Y}, $.Blocks[*].Geometry.RotationAngle, $.Blocks[*].Confidence and $.Blocks[*].Text).

Cause 2 of 2 - AnalyzeDocument: the analysis block set is truncated and carries no KEY_VALUE_SET entity/relationship structure.
AWS: Smithy: AnalyzeDocument input {Document required, FeatureTypes required, HumanLoopConfig, QueriesConfig, AdaptersConfig}, output {DocumentMetadata, Blocks: BlockList, HumanLoopActivationOutput, AnalyzeDocumentModelVersion}. BlockType enum includes KEY_VALUE_SET, TABLE, CELL, MERGED_CELL, SELECTION_ELEMENT, QUERY, QUERY_RESULT besides PAGE/LINE/WORD; Block$EntityTypes (EntityType enum: KEY, VALUE, COLUMN_HEADER, ...) is documented as the way to tell whether a KEY_VALUE_SET block is the KEY or the VALUE; Block$Relationships (RelationshipType enum: VALUE, CHILD, COMPLEX_FEATURES, MERGED_CELL, ANSWER, TABLE, ...) "describe how blocks are related to each other. For example, a LINE block object contains a CHILD relationship type with the WORD blocks that make up the line". So for a document analysis AWS returns the full page decomposition - a PAGE block, one LINE block per line and one WORD block per word, plus the feature blocks - wired together by CHILD relationships, with each KEY_VALUE_SET block tagged KEY or VALUE and the KEY block carrying a VALUE relationship to its value block.
Emulator: the response contains only 7 blocks where the recording contains 27 - $.Blocks[7] through $.Blocks[26] are absent entirely, so most of the page's LINE/WORD decomposition and all of the feature blocks beyond the first few are never emitted. Within the blocks it does emit the classification is wrong: $.Blocks[3].BlockType and $.Blocks[5].BlockType differ from the recording, $.Blocks[5].EntityTypes and $.Blocks[3].TextType diverge, $.Blocks[5].Relationships[0].Type diverges, $.Blocks[5].Relationships[1] is missing, and $.Blocks[6].Relationships is missing altogether - i.e. the KEY/VALUE entity tagging and the VALUE/CHILD relationship graph that make the FORMS result navigable are not built. The same synthetic fixed-step geometry as Cause 1 is present on every block it does emit (Blocks[0] Left 0.0333/Top 0.2333/Width 0.3778/Height 0.1733 against recorded 0.0091/0.0118/0.9776/0.9819; Blocks[1].Confidence 69.46 against recorded 94.70) and Block$Text diverges on the same blocks.
Why: a client asks for AnalyzeDocument precisely to walk the block graph - find the KEY_VALUE_SET blocks, read EntityTypes to tell key from value, follow the VALUE relationship to the value block and its CHILD words. It receives a 200 whose block list is short and whose relationships are missing, so its traversal finds no fields and it concludes the document has no form content, while a block-count or id-based consistency check sees dangling or absent references. Nothing distinguishes this from a genuinely empty document.
Emulator fix: AnalyzeDocument's block assembly in the textract crate's state module - crate::state::blocks and the feature derivations it calls (form_blocks/table_blocks and the relationship/EntityTypes wiring), plus the response serializer that emits Relationships and EntityTypes. The handler must emit the complete page decomposition (PAGE + every LINE + every WORD, linked by CHILD) for the submitted document and, for each requested FeatureType, the feature blocks with their KEY/VALUE EntityTypes and VALUE/CHILD relationships, rather than a fixed short prefix of blocks.
Traces: textract-sync-text-detection-and-document-analysis#task#2 (AnalyzeDocument, expected 200, actual 200, 200-level body mismatch with $.Blocks[7]..$.Blocks[26] missing and BlockType/EntityTypes/Relationships/TextType diverging on $.Blocks[3], $.Blocks[5], $.Blocks[6]).

Relation to already-open issues: both replayed steps came back with an empty `owned_by`, so no open issue carries their replay key. The open behavioral issues 20261029T184456Z-textract-document-analysis-returns-garbled-ocr-text-and.md, 20261030T113700Z-textract-detectdocumenttext-positive-control-after-a-rejected-corrupt-image.md and 20261110T200812Z-textract-analyzedocument-tables-cell-blocks-carry-garbled-text.md describe the garbled recognized text and the empty FORMS/TABLES/QUERIES/expense derivations from the behavioral suite; they are grounded only in behavioral check ids and cover neither the synthetic fixed-grid Geometry/Polygon/RotationAngle of Cause 1 nor the truncated block set and missing EntityTypes/Relationships graph of Cause 2, and they carry no wire fixture/step a fixer could re-replay for these two steps.

## Evidence
Trace set simparity-wire-aws-v1 replayed against the emulator booted from this checkout: emulator_up true, bootstrap_signal false (/__admin/health answered 200 and the :41311 data listener accepted), 6 fixtures replayed, 38 steps: 27 passed, 11 mismatched, 0 errored, 0 blocked, score 71.1%, 2 divergence clusters - both textract, both in fixture textract-sync-text-detection-and-document-analysis, neither with a stub_path and neither with an owner.

Cause 1 - [mismatch] textract-sync-text-detection-and-document-analysis#task#1, service textract, operation DetectDocumentText, expected status 200, actual status 200, body mismatch:
  $.Blocks[0].Geometry.BoundingBox.Height: expected 0.875543475151062, got 0.1666666716337204
  $.Blocks[0].Geometry.BoundingBox.Left:   expected 0.011839513666927814, got 0.035555556416511536
  $.Blocks[0].Geometry.BoundingBox.Top:    expected 0.10043054074048996, got 0.23333333432674408
  $.Blocks[0].Geometry.BoundingBox.Width:  expected 0.8971883058547974, got 0.2733333706855774
  $.Blocks[0].Geometry.Polygon[0].X/Y:     expected 0.9090278148651124 / 0.975973904132843, got 0.035555556416511536 / 0.23333333432674408
  $.Blocks[0].Geometry.Polygon[1].X/Y:     expected 0.011839591898024082 / 0.9759740233421326, got 0.30888891220092773 / 0.23333333432674408
  $.Blocks[0].Geometry.Polygon[2].X/Y:     expected 0.011839513666927814 / 0.10043059289455414, got 0.30888891220092773 / 0.4000000059604645
  $.Blocks[0].Geometry.Polygon[3].X/Y:     expected 0.9090277552604676 / 0.10043054074048996, got 0.035555556416511536 / 0.4000000059604645
  $.Blocks[1].Confidence: expected 99.9255599975586, got 81.9842300415039
  $.Blocks[1].Geometry.BoundingBox.{Height,Left,Top,Width}: expected 0.1828515976667404 / 0.03252305090427399 / 0.21599966287612915 / 0.27597329020500183, got 0.1666666716337204 / 0.035555556416511536 / 0.23333333432674408 / 0.2733333706855774
  further diverging keys in the same cluster: $.Blocks[1].Geometry.Polygon[0..3].{X,Y}, $.Blocks[1].Text, $.Blocks[2].Confidence, $.Blocks[2].Geometry.BoundingBox.{Height,Left,Top,Width}, $.Blocks[2].Geometry.Polygon[0..3].{X,Y}, $.Blocks[2].Geometry.RotationAngle, $.Blocks[2].Text, $.Blocks[3]...
  Note the emulator's returned Polygon for Blocks[0] is exactly the four axis-aligned corners of its own BoundingBox (Left/Top, Left+Width/Top, Left+Width/Top+Height, Left/Top+Height), and Left advances in a constant 0.2733 step with Top/Height pinned to 0.1667 multiples across blocks - a generated grid, not a measurement.

Cause 2 - [mismatch] textract-sync-text-detection-and-document-analysis#task#2, service textract, operation AnalyzeDocument, expected status 200, actual status 200, body mismatch:
  $.Blocks[0].Geometry.BoundingBox.{Height,Left,Top,Width}: expected 0.9818877577781676 / 0.009066186845302582 / 0.011760788038372992 / 0.9776000380516052, got 0.1733333319425583 / 0.03333333507180214 / 0.23333333432674408 / 0.3777777850627899
  $.Blocks[0].Geometry.Polygon[0..3].{X,Y}: expected ((0.0103,0.0118),(0.9867,0.0134),(0.9859,0.9936),(0.0091,0.9934)), got ((0.0333,0.2333),(0.4111,0.2333),(0.4111,0.4067),(0.0333,0.4067))
  $.Blocks[1].Confidence: expected 94.70218658447266, got 69.462158203125
  $.Blocks[1].Geometry.BoundingBox.Width: expected 0.3802343904972077, got 0.07000000774860382
  absent in the emulator's response: $.Blocks[7], $.Blocks[8], $.Blocks[9], $.Blocks[10] ... $.Blocks[26] (recording carries 27 blocks, emulator returned 7)
  classification/graph keys diverging within the returned prefix: $.Blocks[3].BlockType, $.Blocks[3].TextType, $.Blocks[5].BlockType, $.Blocks[5].EntityTypes, $.Blocks[5].Relationships[0].Type, $.Blocks[5].Relationships[1] (missing), $.Blocks[6].BlockType, $.Blocks[6].EntityTypes, $.Blocks[6].Relationships (missing), plus $.Blocks[1..6].Text and the same fixed-grid Geometry divergence as Cause 1.

No divergence in either cluster is a per-run artifact: no request id, host, port, generated id or timestamp is among the diverging keys - every one of them is a geometry ratio, a confidence, a recognized text, a block type/entity type/relationship or a missing block.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator from it, and replay the simparity-wire-aws-v1 wire corpus (6 fixtures, 38 steps). Confirm emulator_up is true and bootstrap_signal is false, so the data listener is serving.
2. Cause 1: replay fixture `textract-sync-text-detection-and-document-analysis`, step `task#1` - a DetectDocumentText call over the fixture's document. The status matches (200); compare the response body's Blocks against the recorded body. Observe that the PAGE block's BoundingBox is a single line-sized box instead of the page's content extent, that every block's Polygon is just the four corners of its own BoundingBox, that Left/Top/Width/Height advance on a constant grid (0.2733 horizontal step, 0.1667 vertical multiples) independent of the document, and that Confidence, Text and Geometry.RotationAngle diverge on the same blocks.
3. Cause 2: replay the same fixture's step `task#2` - an AnalyzeDocument call with FeatureTypes over the fixture's document. Count the Blocks in the response: the emulator returns 7 against the recording's 27, so $.Blocks[7]..$.Blocks[26] are missing. Then inspect the blocks it does return and observe that BlockType and TextType are misclassified at indices 3, 5 and 6, that EntityTypes does not carry the recorded KEY/VALUE tagging, and that Relationships is short at index 5 (Relationships[1] missing) and absent at index 6, so the VALUE/CHILD graph cannot be walked.
4. After fixing, re-replay BOTH textract-sync-text-detection-and-document-analysis#task#1 and textract-sync-text-detection-and-document-analysis#task#2; both must pass before submitting.
