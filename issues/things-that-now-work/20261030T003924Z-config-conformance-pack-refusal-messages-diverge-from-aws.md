Issue-Source: trace-sweep-divergence
Issue-Trace: config-conformance-pack-error-paths
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-conformance-pack-error-paths config-conformance-pack-invalid-template config-conformance-pack-nonexistent-errors config-conformance-pack-not-found-errors
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|PutConformancePack|4xx->4xx|$.message
Issue-Key: config|DescribeConformancePacks|4xx->4xx|$.message
Issue-Key: config|DescribeConformancePackCompliance|4xx->4xx|$.message
Issue-Key: config|GetConformancePackComplianceDetails|4xx->4xx|$.message
Issue-Key: config|GetConformancePackComplianceSummary|4xx->4xx|$.message
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# Config conformance-pack refusal messages diverge from AWS wording

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConformancePack

## Description
Five distinct causes on the AWS Config conformance-pack refusal path, all in this service. In every one the HTTP status (400) and the error code match AWS, but the emulator serves its own descriptive prose instead of the service's fixed message text, so no client that matches on the message sees what AWS produces. FIVE CAUSES must be closed.

Cause 1 of 5 - PutConformancePack rejects an unsupported-resource template with emulator-authored prose (fixture config-conformance-pack-error-paths, step task#1).
AWS: PutConformancePack models ConformancePackTemplateValidationException (client, httpError 400, defaulted) (Smithy: PutConformancePack.errors). The recording shows that a TemplateBody declaring a resource type other than AWS::Config::ConfigRule / AWS::Config::RemediationConfiguration is refused with 400 and that error code, and the message is a fixed template: the fixed text "Invalid conformance pack template provided. Errors found: [" + the list of template errors + "]. Fix your template and try again." The only variable part is the bracketed error list (in this recording the single entry "Unsupported resource type(s) provided in the template"); the offending logical id and type are NOT named.
Emulator: same 400 and code, but the message is its own explanation naming the logical id and type: "Template passed in the input parameter is invalid: the resource '<logicalId>' is of type '<type>', and a conformance pack template declares only AWS::Config::ConfigRule and AWS::Config::RemediationConfiguration".
Why: a client or CI assertion that matches AWS's fixed sentence (or parses the bracketed Errors found list) never matches, so template-validation failures are indistinguishable from any other 400.
Fix: PutConformancePack's template validation - emit the fixed AWS sentence with the bracketed error list, instead of the handler's own diagnostic string.

Cause 2 of 5 - DescribeConformancePacks' not-found message is the wrong sentence and the wrong name rendering (fixture config-conformance-pack-error-paths, step task#2).
AWS: DescribeConformancePacks models NoSuchConformancePackException (client, 400) - "You specified one or more conformance packs that do not exist" (Smithy: DescribeConformancePacks.errors, NoSuchConformancePackException). The recording shows the message is fixed text "No such conformance pack exists. Conformance Pack(s): [" + the requested names + "]", i.e. for this operation the names are rendered inside square brackets; the names come from the request.
Emulator: same 400 and code, message "One or more conformance packs are not valid. Name(s): <names>".
Why: clients keying on AWS's "No such conformance pack exists." sentence treat the response as an unrecognised failure; the bracketed-name rendering a parser expects is also absent.
Fix: the conformance-pack not-found error construction used by DescribeConformancePacks' handler.

Cause 3 of 5 - DescribeConformancePackCompliance uses the same wrong sentence (fixture config-conformance-pack-error-paths, step task#3).
AWS: DescribeConformancePackCompliance models NoSuchConformancePackException (client, 400) (Smithy: DescribeConformancePackCompliance.errors). The recording shows the fixed text "No such conformance pack exists. Conformance Pack(s): " + the requested pack name, with NO square brackets around the name on this operation.
Emulator: "One or more conformance packs are not valid. Name(s): <name>".
Why: as cause 2; additionally the bracket/no-bracket difference between the list and single-pack operations is lost, so a client cannot rely on either rendering.
Fix: same not-found construction, invoked from DescribeConformancePackCompliance's handler, with the unbracketed name rendering.

Cause 4 of 5 - GetConformancePackComplianceDetails uses the same wrong sentence (fixture config-conformance-pack-error-paths, step task#4).
AWS: 400 NoSuchConformancePackException with fixed text "No such conformance pack exists. Conformance Pack(s): " + the requested pack name (recorded behavior; the model states only the error class).
Emulator: "One or more conformance packs are not valid. Name(s): <name>".
Why: as cause 3.
Fix: same construction, from GetConformancePackComplianceDetails' handler.

Cause 5 of 5 - GetConformancePackComplianceSummary uses the same wrong sentence (fixture config-conformance-pack-error-paths, step task#5).
AWS: 400 NoSuchConformancePackException with fixed text "No such conformance pack exists. Conformance Pack(s): " + the requested pack name (recorded behavior).
Emulator: "One or more conformance packs are not valid. Name(s): <name>".
Why: as cause 3.
Fix: same construction, from GetConformancePackComplianceSummary's handler.

No existing open issue covers the conformance-pack surface; the only open config wire issue, 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.md, is about delivery channels and DeletePendingAggregationRequest.

Traces: config-conformance-pack-error-paths#task#1 (PutConformancePack), #task#2 (DescribeConformancePacks), #task#3 (DescribeConformancePackCompliance), #task#4 (GetConformancePackComplianceDetails), #task#5 (GetConformancePackComplianceSummary). The same wrong sentence also fails in config-conformance-pack-invalid-template#task#2, config-conformance-pack-nonexistent-errors#task#1..#task#4 and config-conformance-pack-not-found-errors#task#2..#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, score 69.9%).

Cause 1 - PutConformancePack, config-conformance-pack-error-paths#task#1 (400 on both sides):
  $.message expected 'Invalid conformance pack template provided. Errors found: [Unsupported resource type(s) provided in the template]. Fix your template and try again.'
  got "Template passed in the input parameter is invalid: the resource 'NotAConfigRule' is of type 'AWS::S3::Bucket', and a conformance pack template declares only AWS::Config::ConfigRule and AWS::Config::RemediationConfiguration"

Cause 2 - DescribeConformancePacks, config-conformance-pack-error-paths#task#2 (400 on both sides, 4 steps in cluster):
  $.message expected 'No such conformance pack exists. Conformance Pack(s): [sim2real-probe-config-cfpack-missing-9c2bde4c]'
  got 'One or more conformance packs are not valid. Name(s): sim2real-probe-config-cfpack-missing-9c2bde4c'

Cause 3 - DescribeConformancePackCompliance, config-conformance-pack-error-paths#task#3 (400 on both sides, 3 steps in cluster):
  $.message expected 'No such conformance pack exists. Conformance Pack(s): sim2real-probe-config-cfpack-missing-9c2bde4c'
  got 'One or more conformance packs are not valid. Name(s): sim2real-probe-config-cfpack-missing-9c2bde4c'

Cause 4 - GetConformancePackComplianceDetails, config-conformance-pack-error-paths#task#4: identical expected/actual pair as cause 3 (3 steps in cluster).

Cause 5 - GetConformancePackComplianceSummary, config-conformance-pack-error-paths#task#5: identical expected/actual pair as cause 3 (3 steps in cluster).

The pack names above are per-run fixture values; the requirement is the fixed sentence plus the name taken from the request.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutConformancePack errors include ConformancePackTemplateValidationException (400); DescribeConformancePacks and DescribeConformancePackCompliance both model NoSuchConformancePackException (400).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: config-conformance-pack-error-paths#task#1 - PutConformancePack with a TemplateBody containing an AWS::S3::Bucket resource; compare $.message against AWS's 'Invalid conformance pack template provided. Errors found: [...]' sentence.
4. Cause 2: config-conformance-pack-error-paths#task#2 - DescribeConformancePacks naming a pack that does not exist.
5. Cause 3: config-conformance-pack-error-paths#task#3 - DescribeConformancePackCompliance on the same absent pack.
6. Cause 4: config-conformance-pack-error-paths#task#4 - GetConformancePackComplianceDetails on the same absent pack.
7. Cause 5: config-conformance-pack-error-paths#task#5 - GetConformancePackComplianceSummary on the same absent pack.
8. Re-replay all five fixture#step bindings (plus config-conformance-pack-invalid-template, config-conformance-pack-nonexistent-errors and config-conformance-pack-not-found-errors) to confirm the fix.
