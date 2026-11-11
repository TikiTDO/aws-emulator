Issue-Source: trace-sweep-divergence
Issue-Trace: rekognition-image-analysis-detect-operations
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rekognition-image-analysis-detect-operations rekognition-refarch-rekreup-detect-labels-s3object-mistakes
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*]
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*].Aliases
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*].Categories
Issue-Key: rekognition|DetectLabels|2xx->2xx|$.Labels[*].Instances
Issue-Key: rekognition|DetectLabels|4xx->2xx|$.Code
Issue-Key: rekognition|DetectLabels|4xx->2xx|$.Message
Issue-Key: rekognition|DetectLabels|4xx->2xx|$.__type
Issue-Key: rekognition|DetectLabels|4xx->2xx|status
Issue-Commit: d1185311883c3d926af6a235180686b5555f406c

# DetectLabels diverges: Label shape omits Aliases/Categories/Instances and conflicting Image sources accepted

> _Note: this issue was identified against the code base at commit `d1185311883c3d926af6a235180686b5555f406c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectLabels

## Description
FOUR net-new causes in rekognition DetectLabels, all on the same operation: its label synthesis produces image-colour placeholders rather than object/scene labels, its Label serializer drops three modeled members, and its request validation does not reject an Image that carries both an S3 source and inline bytes. Count: 4 causes — Cause 1 of 4 .. Cause 4 of 4 below; each must be closed before the fix is complete.

--- Cause 1 of 4: label set is image colours, not detected objects/scenes, and is far shorter than AWS's ---
AWS: Smithy (DetectLabels, com.amazonaws.rekognition): output member Labels targets list<Label>; Label has members Name, Confidence (Percent, @range 0..100), Instances (list<Instance>), Parents (list<Parent>), Aliases (list<LabelAlias>), Categories (list<LabelCategory>). Recorded behavior for this image: HTTP 200 with a ten-plus-entry Labels list whose Name values are object/scene concepts ("Face", "Head", "Hardhat", "Snow", "Stencil", "Animal", "Beak", "Kiwi Bird", ...), each entry carrying Aliases, Categories (e.g. {"Name": "Person Description"}, {"Name": "Apparel and Accessories"}, {"Name": "Nature and Outdoors"}), Instances (bounding boxes present for common object labels such as apparel, people, pets) and Parents naming the ancestor labels.
Emulator: HTTP 200 with only three entries, and their Name values are image colours — "White", "Monochrome", "Black" — each shaped {Name, Confidence, Parents: []} with no Aliases, no Categories and no Instances. Labels[3..] are absent entirely.
Why: a client that keys on label names ("does this image contain a Hardhat / a Person"), buckets results by Categories[].Name, or draws Instances[].BoundingBox receives nothing it can act on; a list of dominant colours is not a detection result, so every downstream label-driven branch takes the wrong path while the call still looks successful.
Fix: the DetectLabels handler's label-synthesis path in the rekognition service crate (the code that derives Labels from the image) plus the Label serializer, which must emit all six modeled members.
Traces: rekognition-image-analysis-detect-operations#task#3.

--- Cause 2 of 4: DetectLabels returns an empty Labels list for an S3Object image ---
AWS: Smithy: DetectLabels output Labels: list<Label>. Recorded behavior: HTTP 200 with at least one Label present, shaped {Aliases: [], Categories: [{Name: ...}], Confidence: <float>, Instances: [], Name: <string>, Parents: []} (the fixture asserts the shape, not the values).
Emulator: HTTP 200 but Labels[0] is missing — the list is empty, so no label is reported at all for an image the service accepted.
Why: a caller cannot distinguish "image analysed, nothing recognised" from "analysis not performed"; label-gated workflows (tagging, moderation routing, index population) silently no-op.
Fix: the DetectLabels handler's label-synthesis path for S3Object-sourced images in the rekognition service crate.
Traces: rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#5.

--- Cause 3 of 4: Label members Aliases, Categories and Instances are omitted instead of serialized ---
AWS: Smithy: Label.Aliases (LabelAliases), Label.Categories (LabelCategories), Label.Instances (Instances) are modeled list members; recorded behavior shows them present on every returned Label — Aliases: [], Instances: [] when there is nothing to report, and Categories: [{"Name": <category>}] naming the label's category.
Emulator: the returned Label carries only Name, Confidence and Parents; $.Labels[0].Aliases, $.Labels[0].Categories and $.Labels[0].Instances are all absent (not empty lists — keys missing). The emulator additionally appends a colour label (Labels[1] = {Name: "Monochrome", Confidence: 100.0, Parents: []}) that AWS does not return for this request, the same placeholder-label defect as Cause 1.
Why: a strongly-typed SDK client deserializes the missing lists as null rather than empty, so any iteration over label.Categories / label.Instances / label.Aliases throws instead of looping zero times — the response is not merely incomplete, it breaks clients that follow the model.
Fix: the Label serializer in the rekognition service crate's DetectLabels response path — always emit Aliases, Categories and Instances (empty list when there is nothing to report).
Traces: rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#6.

--- Cause 4 of 4: an Image carrying both S3Object and Bytes is accepted instead of rejected ---
AWS: Smithy: DetectLabels.Image (required) targets Image, a structure with exactly two alternative sources, Bytes (ImageBlob, @length 1..5242880) and S3Object; its documentation states the image is provided "either as bytes or an S3 object". Supplying both violates that, and DetectLabels models InvalidParameterException (errorKind client, httpError 400, "Input parameter violated a constraint. Validate your parameter before calling the API operation again."). Recorded behavior: HTTP 400 with $.__type = "InvalidParameterException", $.Code = "InvalidParameterException" and $.Message = "Cannot provide both S3 Location and Image Bytes." — that message is entirely fixed text for this error class; it interpolates nothing from the request or state.
Emulator: HTTP 200 with a normal DetectLabels result; no __type, no Code, no Message — the conflicting request is analysed as if it were valid.
Why: a client that mistakenly populates both sources (a common porting bug, exactly what this fixture exercises) never learns which source was used and gets a success for a request AWS rejects, so the error branch it must handle in production is untested and the ambiguity in which image was analysed goes unnoticed.
Fix: DetectLabels input validation in the rekognition service crate — reject an Image with both Bytes and S3Object set (and the reciprocal neither-set case) with InvalidParameterException before dispatching to analysis.
Traces: rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#7.

Relation to already-open issues: 20261102T103806Z-rekognition-detectlabels-constraint-violation-message-renders-minconfidence-as-an.md covers only the MinConfidence float formatting in the generic validation message and does not cover any cause above; 20261106T074501Z-rekognition-recognizecelebrities-...md and 20261106T205525Z-rekognition-searchfacesbyimage-...md are different operations. None of the four causes here is owned by an open issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 68.1%), four net-new rekognition divergences, all DetectLabels:

Cause 1 — rekognition-image-analysis-detect-operations#task#3, expected 200 / actual 200:
  $.Labels[0]: expected {Aliases: [], Categories: [{Name: 'Person Description'}], Confidence: 83.115..., Instances: [], Name: 'Face', Parents: [{Name: 'Head'}, {Name: 'Person'}]} — got {Name: 'White', Confidence: 100.0, Parents: []}
  $.Labels[1]: expected Name 'Head' (Categories [{Name: 'Person Description'}], Parents [{Name: 'Person'}]) — got {Name: 'Monochrome', Confidence: 97.502..., Parents: []}
  $.Labels[2]: expected Name 'Hardhat' (Categories [{Name: 'Apparel and Accessories'}], Instances [{BoundingBox: {...}, Confidence: 67.86...}], Parents [{Name: 'Clothing'}, {Name: 'Helmet'}]) — got {Name: 'Black', Confidence: 74.979..., Parents: []}
  $.Labels[3..9]: expected 'Snow', 'Stencil', 'Animal', 'Beak', 'Kiwi Bird', ... — got None (list ends at index 2)

Cause 2 — rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#5, expected 200 / actual 200:
  $.Labels[0]: missing (expected {Aliases: [], Categories: [{Name: <shaped-not-asserted>}], Confidence: <shaped-not-asserted>, Instances: [], Name: <shaped-not-asserted>, Parents: []}, got None)

Cause 3 — rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#6, expected 200 / actual 200:
  $.Labels[0].Aliases: missing (expected [], got None)
  $.Labels[0].Categories: missing (expected [{Name: <shaped-not-asserted>}], got None)
  $.Labels[0].Instances: missing (expected [], got None)
  $.Labels[1]: extra (expected None, got {Name: 'Monochrome', Confidence: 100.0, Parents: []})

Cause 4 — rekognition-refarch-rekreup-detect-labels-s3object-mistakes#task#7, expected 400 / actual 200:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.Code: missing (expected 'InvalidParameterException', got None)
  $.Message: missing (expected 'Cannot provide both S3 Location and Image Bytes.', got None)

Model evidence (smithy_query on services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/com-amazonaws-rekognition-rekognitionservice.json):
  DetectLabels input {Image (required), MaxLabels, MinConfidence (Percent @range 0..100), Features, Settings}; output {Labels: list<Label>, OrientationCorrection, LabelModelVersion, ImageProperties}; errors include InvalidParameterException (client, httpError 400).
  Label members: Name, Confidence (Percent @range 0..100), Instances, Parents, Aliases, Categories.
  Image members: Bytes (ImageBlob @length 1..5242880), S3Object — documented as "the input image either as bytes or an S3 object".

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture rekognition-image-analysis-detect-operations, step task#3 — DetectLabels on the analysis image. Observe 200 with three colour labels ('White', 'Monochrome', 'Black') lacking Aliases/Categories/Instances, where AWS returns ten-plus object/scene labels with all six Label members populated.
4. Cause 2: replay fixture rekognition-refarch-rekreup-detect-labels-s3object-mistakes, step task#5 — DetectLabels with a valid S3Object image. Observe 200 with an empty Labels list where AWS returns at least one fully shaped Label.
5. Cause 3: same fixture, step task#6 — DetectLabels. Observe the returned Label omits the Aliases, Categories and Instances keys entirely (null, not []) and an extra 'Monochrome' colour label is appended.
6. Cause 4: same fixture, step task#7 — DetectLabels with Image containing BOTH S3Object and Bytes. Observe HTTP 200 with a normal result; AWS answers HTTP 400 with __type/Code 'InvalidParameterException' and Message 'Cannot provide both S3 Location and Image Bytes.'
7. Re-replay all four fixture#step bindings after the fix; each must pass independently.
