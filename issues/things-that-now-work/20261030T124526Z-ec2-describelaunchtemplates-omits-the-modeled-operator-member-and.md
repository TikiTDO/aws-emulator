Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-t12-post-delete-not-found-probes
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-t12-post-delete-not-found-probes ec2-launch-template-versioning
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeLaunchTemplates|4xx->4xx|$
Issue-Key: ec2|DescribeLaunchTemplates|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DescribeLaunchTemplates|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeLaunchTemplates|2xx->2xx|$.DescribeLaunchTemplatesResponse.launchTemplates.item.operator
Issue-Key: ec2|DescribeLaunchTemplates|2xx->2xx|$.DescribeLaunchTemplatesResponse.launchTemplates.item.defaultVersionNumber
Issue-Key: ec2|DescribeLaunchTemplates|2xx->2xx|$.DescribeLaunchTemplatesResponse.launchTemplates.item.latestVersionNumber
Issue-Commit: e72ab6d9ecc28f469f99eceb42a3c20602b3ead9

# ec2 DescribeLaunchTemplates omits the modeled operator member and misreports not-found lookups

> _Note: this issue was identified against the code base at commit `e72ab6d9ecc28f469f99eceb42a3c20602b3ead9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLaunchTemplates

## Description
DescribeLaunchTemplates diverges on both of its paths: the error path classifies a well-formed but unknown launch-template id as malformed instead of not-found, and the success path drops a modeled response member. Four causes.

Cause 1 of 4 (fixture ec2-t12-post-delete-not-found-probes, step task#15): lookup of a deleted launch template by id.
AWS: recorded behavior only - DescribeLaunchTemplates has no modeled errors (Smithy: DescribeLaunchTemplates errors: []), and the recording shows HTTP 400 with Response.Errors.Error.Code 'InvalidLaunchTemplateId.NotFound' and Message 'At least one of the launch templates specified in the request does not exist.' The message is entirely fixed text - it names no id - and the code distinguishes a syntactically valid id that does not resolve from a syntactically invalid one ('InvalidLaunchTemplateId.Malformed'), which is only for ids that do not match the lt-<17 hex> form.
Emulator: HTTP 400 with Code 'InvalidLaunchTemplateId.Malformed' and Message 'The following launch template ids are malformed: <id>' for an id that is well-formed and was accepted by an earlier create in the same fixture; the step polls for the not-found code and never sees it (56 attempts).
Why: SDK retry/wait helpers and delete-then-confirm flows branch on the error code: they treat ...NotFound as 'the resource is gone' (success) and ...Malformed as a client bug that is never retried, so a caller waiting for deletion to settle loops forever or aborts, and the message text misleads the user into thinking the id was mistyped.
Fix: the DescribeLaunchTemplates handler's id resolution - a request id that matches the launch-template id syntax but is absent from state must raise InvalidLaunchTemplateId.NotFound with the fixed not-found message; reserve the Malformed code (and its id-listing message) for ids that fail the syntax check.

Cause 2 of 4 (fixture ec2-t12-post-delete-not-found-probes, step task#16): same lookup by name.
AWS: HTTP 400, Code 'InvalidLaunchTemplateId.NotFound', Message 'At least one of the launch templates specified in the request does not exist.' - the same fixed text, with no name interpolated, when LaunchTemplateNames names a template that does not exist.
Emulator: returns the correct status and code but the message "The launch template name '<name>' does not exist" - a different sentence that injects the requested name as a variable part where AWS has none.
Why: callers and test suites that match on the documented message text, and users reading the error, see a message AWS never emits for this error class.
Fix: the DescribeLaunchTemplates handler's name resolution must emit the same fixed not-found message as the id path.

Cause 3 of 4 (fixture ec2-t12-post-delete-not-found-probes, step task#13): successful describe omits operator.
AWS: Smithy: LaunchTemplate.Operator (xmlName 'operator', target OperatorResponse) is a modeled member of each launchTemplates item, and the recording shows it present with managed 'false' for a customer-created template.
Emulator: the launchTemplates item has no operator element at all.
Why: a client that reads operator.managed to tell customer-managed from service-managed templates gets a missing element and either crashes on the dereference or silently classifies every template as unknown.
Fix: the DescribeLaunchTemplates serializer must emit the modeled operator member for every item.

Cause 4 of 4 (fixture ec2-launch-template-versioning, step task#4): same omission plus stale version counters.
AWS: operator present as in Cause 3; LaunchTemplate.defaultVersionNumber and latestVersionNumber (Smithy: both modeled, type long) report the template's current version state, which the fixture advanced by creating a second version.
Emulator: operator absent, and defaultVersionNumber/latestVersionNumber both '1' where the recording has '2'.
Why: a caller that launches from $Latest/$Default, or that waits for a new version to become visible, operates on a version that the emulator pretends does not exist.
Fix: the operator omission belongs in the serializer as in Cause 3. The stale counters are downstream of CreateLaunchTemplateVersion answering 501 in this fixture (open issue 20261022T235108Z-ec2-createlaunchtemplateversion-stub.md), so once that operation exists the launch-template state must bump latestVersionNumber (and defaultVersionNumber when the new version is made default) - verify this step after that stub is implemented rather than patching the counters in the read path.

Related open issues that do NOT cover these causes: 20261029T211352Z-ec2-deletelaunchtemplate-refuses-an-existing-template-with-http.md (DeleteLaunchTemplate, different operation) and 20261029T200817Z-ec2-read-paths-omit-modeled-members-and-serve.md (owns the DescribeRegions geographySet omission only; the operator omission here is unowned).

## Evidence
Replay (trace set simparity-wire-aws-v1), fixture ec2-t12-post-delete-not-found-probes: step task#15 expected 400/'InvalidLaunchTemplateId.NotFound' with Message 'At least one of the launch templates specified in the request does not exist.', got 400/'InvalidLaunchTemplateId.Malformed' with Message 'The following launch template ids are malformed: lt-...' and the divergence records "poll: error code 'InvalidLaunchTemplateId.Malformed' not in ['InvalidLaunchTemplateId.NotFound'] after 56 attempt(s)"; step task#16 - '$.Response.Errors.Error.Message: value_mismatch (expected "At least one of the launch templates specified in the request does not exist.", got "The launch template name ... does not exist")'; step task#13 - '$.DescribeLaunchTemplatesResponse.launchTemplates.item.operator: missing (expected {managed: false}, got None)'. Fixture ec2-launch-template-versioning step task#4 - '$...launchTemplates.item.defaultVersionNumber: value_mismatch (expected 2, got 1)', '$...latestVersionNumber: value_mismatch (expected 2, got 1)', '$...operator: missing'. Smithy model: LaunchTemplate members operator (OperatorResponse), defaultVersionNumber, latestVersionNumber; DescribeLaunchTemplates errors: [].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture ec2-t12-post-delete-not-found-probes: create a launch template, delete it, then call DescribeLaunchTemplates with its id (step task#15) and with its name (step task#16). Expect 400 InvalidLaunchTemplateId.NotFound with 'At least one of the launch templates specified in the request does not exist.' in both cases; observe the Malformed code for the id probe and the name-specific message for the name probe.
3. In the same fixture, step task#13: DescribeLaunchTemplates for an existing template and check the launchTemplates item for the operator element.
4. Replay fixture ec2-launch-template-versioning step task#4 and compare defaultVersionNumber/latestVersionNumber and operator (this step also needs CreateLaunchTemplateVersion implemented).
