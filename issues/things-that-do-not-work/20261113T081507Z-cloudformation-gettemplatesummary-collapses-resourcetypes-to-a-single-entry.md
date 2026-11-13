Issue-Source: trace-sweep-divergence
Issue-Trace: template-count-limits
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: template-count-limits
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|GetTemplateSummary|2xx->2xx|$.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member
Issue-Commit: ba88c028fef36e1bc767f96ee2f820f05186c145

# CloudFormation GetTemplateSummary collapses ResourceTypes to a single entry for templates that declare one type many times

> _Note: this issue was identified against the code base at commit `ba88c028fef36e1bc767f96ee2f820f05186c145`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplateSummary

## Description
GetTemplateSummary answers 200, but the ResourceTypes projection in its response body is not the list AWS returns: for a template that declares the SAME resource type many times, the emulator emits a collapsed value instead of one list entry per declared resource, so the replayed member path is a type mismatch rather than a value mismatch (the member is not even a list).

This bundle carries 1 cause.

Cause 1 of 1 - ResourceTypes is de-duplicated/collapsed instead of carrying one entry per declared resource.
AWS: GetTemplateSummary's output member ResourceTypes targets the shape ResourceTypes, which the model declares as a LIST (Smithy, services/service-com-amazonaws-cloudformation-cloudformation: GetTemplateSummary.output.ResourceTypes -> target ResourceTypes, type list - not a set, no uniqueItems), and the operation models no error for this path (its only modeled error is StackSetNotFoundException, httpError 404). AWS therefore repeats a type as many times as the template declares it, in template order: the recorded 200 response for a template whose resources are all AWS::CloudFormation::WaitConditionHandle carries ResourceTypes.member as a list with one 'AWS::CloudFormation::WaitConditionHandle' string per declared resource (the recorded list runs to the template's full resource count; the exact count is a property of the request's template, not a fixed value). Status 200, no error code, no error message on this path for either AWS or the emulator.
Emulator: for the same request the emulator answers 200 but ResourceTypes.member is NOT a list - the replay reports type_mismatch at $.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member, with the recorded list of repeated AWS::CloudFormation::WaitConditionHandle entries expected and a non-list value returned. Hypothesis: the projection builds a unique SET of the template's resource types, which for a single-type template leaves exactly one entry, and the AWS-query XML serializer then renders a one-element list as a bare scalar member rather than a repeated element.
Why: callers use ResourceTypes to size and gate a deployment - counting resources against the per-stack resource limit, deciding which resource types need IAM permissions or a CAPABILITY acknowledgement, and driving per-resource progress UI. A collapsed list understates the resource count of a template by the number of repeats (for an all-one-type template it reports 1 resource however many are declared), so a client's limit precheck passes a template AWS would refuse. Worse, because the member degrades from a repeated element to a scalar, SDKs and XML consumers that expect a list on this path deserialize a single string or fail outright, so the breakage is structural and not only numeric.
Fix: the GetTemplateSummary handler's ResourceTypes projection in the cloudformation service crate - accumulate one entry per declared resource in template order instead of inserting into a uniquing collection, and keep the AWS-query serializer emitting ResourceTypes.member as a repeated list element even when it holds a single entry.
Traces: template-count-limits#task#10.

Related open issue: 20261105T232616Z-cloudformation-gettemplatesummary-synthesizes-resource-identifier-summaries-and-resourcetypes.md describes the same class of defect (its cause 5 covers a de-duplicated/reordered ResourceTypes) but it OWNS NONE of this replay key - the replay reports owned_by=[] and stub_path='' for this cluster, so this fixture/step is not exercised by that issue and must be closed here. The resolved issue 20261103T171935Z-cloudformation-template-validation-and-gettemplatesummary-diverge-on-limits.md covers neighbouring GetTemplateSummary limit/warning checks and does not cover this path either.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout.
Fixture template-count-limits, step task#10, operation GetTemplateSummary, service cloudformation, verdict=mismatch, expected_status=200, actual_status=200, stub_path='', owned_by=[].
Diverging path: $.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes.member - type_mismatch.
Expected (recorded AWS response): a LIST whose entries are all the string 'AWS::CloudFormation::WaitConditionHandle', one entry per resource declared in the submitted template.
Actual (emulator): not a list on that path - the repeated entries are absent, consistent with a uniqued single-entry collection serialized as a scalar member.
Status, error code and error message: both sides answer HTTP 200 with no error code and no error message, so the divergence is purely in the 200 body's ResourceTypes shape and cardinality.
Smithy citation: GetTemplateSummary.output.ResourceTypes -> ResourceTypes (type: list); errors: StackSetNotFoundException (client, httpError 404) only.

## Steps to Reproduce
1. Build the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus against it.
2. Go to fixture template-count-limits, step task#10: it calls GetTemplateSummary with a template body that declares the same resource type (AWS::CloudFormation::WaitConditionHandle) many times.
3. Compare $.GetTemplateSummaryResponse.GetTemplateSummaryResult.ResourceTypes against the recorded response: assert member is a repeated list element and that its length equals the number of resources declared in the submitted template, with the type repeated once per declaration in template order.
4. Observe the emulator returns 200 but ResourceTypes.member is not a list (type_mismatch), so the per-declaration repeats are lost.
5. Re-run the same step after the fix to confirm the list cardinality and ordering match the recording.
