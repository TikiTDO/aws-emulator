Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors ssm.parameter-version-selector-and-label-move ssm.global-infrastructure-region-public-parameters-name-us-east-1 ssm.aws-owned-restart-ec2-instance-automation-document-is-described-and-listed ssm.run-command-and-automation-execution-ids-resolve-only-in-their-own-api ssm.maintenance-window-execution-lookup-nonexistent-rejected ssm.delete-association-nonexistent-rejected ssm.maintenance-window-dates-echo-and-invalid-schedule-rejected ssm.tag-calls-reject-missing-window-and-mismatched-resource-type
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors|run#14|AssertionError|assert got_names == [alpha, bravo, charlie], (
Issue-Key: ssm.parameter-version-selector-and-label-move|run#18|AssertionError|assert pinned["Name"] == name, (
Issue-Key: ssm.global-infrastructure-region-public-parameters-name-us-east-1|run#3|AssertionError|assert regions.get(name) == ("String", code), (
Issue-Key: ssm|DescribeDocument|InvalidDocument
Issue-Key: ssm.run-command-and-automation-execution-ids-resolve-only-in-their-own-api|_refused#12|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ssm.maintenance-window-execution-lookup-nonexistent-rejected|run#11|AssertionError|assert exec_code == "DoesNotExistException", (
Issue-Key: ssm.delete-association-nonexistent-rejected|run#5|AssertionError|assert message == f"Association {bogus_id} does not exist.", message
Issue-Key: ssm.maintenance-window-dates-echo-and-invalid-schedule-rejected|run#17|AssertionError|assert (code, message) == (
Issue-Key: ssm.tag-calls-reject-missing-window-and-mismatched-resource-type|run#11|AssertionError|assert (code, message) == (
Issue-Commit: dcee3c715dd1f76e7da2147c1b920fea18ed74e4

# ssm: 8 net-new causes — parameter Name loses its leading '/', missing global-infrastructure and AWS-owned-document catalogs, a Run Command id resolving as an automation execution, and four mis-worded/mis-coded refusals

> _Note: this issue was identified against the code base at commit `dcee3c715dd1f76e7da2147c1b920fea18ed74e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameters

## Description
Eight net-new root causes in the Systems Manager surface. None is covered by an existing open issue; cause 5 shares an operation with the auto-filed scaffold-stub issue for `GetMaintenanceWindowExecution` (services/service-com-amazonaws-ssm-amazonssm/src/ops/get_maintenance_window_execution.rs), which was attributed to only one check and not to this one.

Cause 1 of 8: GetParameter/GetParameters echo a parameter Name with its leading '/' stripped.
AWS: Smithy: Parameter.Name targets PSParameterName (@length 1..2048) and is "The name of the parameter". Parameter Store echoes the name as stored. A hierarchical name is stored with a leading '/', so a parameter created as `/<path>/<leaf>` is echoed as `/<path>/<leaf>` — byte-identical to the name in PutParameter — and when the request used a selector form (`<name>:<version>` or `<name>:<label>`), the echoed `Name` is the BASE name and the selector is reported separately in `Selector`.
Emulator: the echoed `Name` omits the leading '/': asking GetParameters for `['/<n>-c','/<n>-b','/<n>-a']` returns names `['<n>-a','<n>-b','<n>-c']`, and GetParameter on a selector form returns `Name` = `<n>` with no leading '/'.
Why: a client that keys a cache/map by the name it asked for, or that re-sends an echoed Name to DeleteParameter/AddTagsToResource/GetParameterHistory, gets a name that does not resolve — SSM requires the leading '/' for any hierarchical name, so the round trip breaks.
Fix: the GetParameter/GetParameters serializer (the Parameter projection) must echo the stored name verbatim, including its leading '/'.
Traces: ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors, ssm.parameter-version-selector-and-label-move

Cause 2 of 8: the `/aws/service/global-infrastructure/regions` public parameter tree is empty.
AWS: documented public parameters (https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-public-parameters-global-infrastructure.html): GetParametersByPath on `/aws/service/global-infrastructure/regions` lists one String parameter per Region, each named `/aws/service/global-infrastructure/regions/<region-code>` with `Type` `String` and `Value` equal to that Region code, and `/aws/service/global-infrastructure/regions/<region-code>/longName` carries the Region's long name (for `us-east-1`, `US East (N. Virginia)`).
Emulator: GetParametersByPath on that path returns 0 parameters, so no Region is listed and `us-east-1` resolves to nothing.
Why: tooling that enumerates Regions or maps a Region code to its long name through Parameter Store (a documented, widely used pattern) gets an empty list and silently degrades to no Regions.
Fix: the public-parameter catalog backing GetParametersByPath/GetParameter must include the global-infrastructure regions tree.
Traces: ssm.global-infrastructure-region-public-parameters-name-us-east-1

Cause 3 of 8: the AWS-owned Automation runbook `AWS-RestartEC2Instance` does not exist.
AWS: documented AWS-owned runbook (https://docs.aws.amazon.com/systems-manager-automation-runbooks/latest/userguide/automation-aws-restartec2instance.html). DescribeDocument(Name="AWS-RestartEC2Instance") succeeds with `Document.Owner` = `Amazon`, `Document.DocumentType` = `Automation`, `Document.Status` = `Active`; ListDocuments filtered to Owner `Amazon` and DocumentType `Automation` lists it. Smithy: DescribeDocument's modeled refusal for an absent name is `InvalidDocument` (HTTP 400).
Emulator: `InvalidDocument` — `Document AWS-RestartEC2Instance doesn't exist`. The AWS-owned Automation catalog does not carry it.
Why: any client that describes or starts a stock AWS-* runbook (the normal way Automation is used) is refused as though the account were broken, and the refusal is indistinguishable from a genuinely missing custom document.
Fix: the SSM document store's seeded AWS-owned document catalog (Owner `Amazon`, DocumentType `Automation`), surfaced by DescribeDocument/ListDocuments.
Traces: ssm.aws-owned-restart-ec2-instance-automation-document-is-described-and-listed

Cause 4 of 8: GetAutomationExecution resolves a Run Command command id as an automation execution.
AWS: Smithy: GetAutomationExecution.AutomationExecutionId is @length 36..36 and the operation's modeled client error is `AutomationExecutionNotFoundException` (doc: "There is no automation execution information for the requested automation execution ID"). Run Command command ids and Automation execution ids are disjoint namespaces even though both are 36-character UUIDs, so GetAutomationExecution given a SendCommand `CommandId` is refused HTTP 400 `AutomationExecutionNotFoundException` with the message `The input automation execution does not exist.` (fixed text, no variable part). Symmetrically CancelCommand given an automation execution id answers `InvalidCommandId` / `Command id <id> does not exist.` where `<id>` is the id sent.
Emulator: GetAutomationExecution(AutomationExecutionId=<a Run Command CommandId>) returns HTTP 200 with a fully populated `AutomationExecution` (AutomationExecutionStatus `Success`, StepExecutions, Mode, ExecutedBy, MaxConcurrency, MaxErrors), i.e. the two id spaces are one shared table.
Why: a client that probes which API an opaque id belongs to — or that passes a command id to the wrong API by mistake — is told the automation exists and reads invented step/status data instead of being refused, so error handling that branches on AutomationExecutionNotFoundException never fires.
Fix: the GetAutomationExecution handler's lookup must be scoped to the automation-execution store only (and the Run Command store kept separate), refusing ids it does not own.
Traces: ssm.run-command-and-automation-execution-ids-resolve-only-in-their-own-api

Cause 5 of 8: GetMaintenanceWindowExecution answers NotImplemented instead of its modeled refusal.
AWS: Smithy: GetMaintenanceWindowExecution.WindowExecutionId is @length 36..36 with pattern `^[0-9a-fA-F]{8}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{4}\-[0-9a-fA-F]{12}$`, and the operation's only modeled client error is `DoesNotExistException` (HTTP 400, doc: "Error returned when the ID specified for a resource, such as a maintenance window or patch baseline, doesn't exist") with the message `Resource does not exist in the data store.` (fixed text, no variable part).
Emulator: error code `NotImplemented` — the operation is a scaffold stub, so the modeled refusal is unreachable and the sibling Describe listings for a nonexistent execution cannot be exercised either.
Why: a client catching DoesNotExistException to treat a window execution as absent instead sees an unmodeled NotImplemented that botocore surfaces as an unexpected server-side fault, so the not-found branch is never taken.
Fix: implement the GetMaintenanceWindowExecution handler (services/service-com-amazonaws-ssm-amazonssm/src/ops/get_maintenance_window_execution.rs) over the maintenance-window-execution state, refusing unknown ids with DoesNotExistException.
Traces: ssm.maintenance-window-execution-lookup-nonexistent-rejected

Cause 6 of 8: DeleteAssociation's not-found message is emulator-authored.
AWS: Smithy: DeleteAssociation's modeled client error is `AssociationDoesNotExist` (HTTP 400). For an AssociationId that names no association the message is `Association <association-id> does not exist.` — fixed text `Association ` ... ` does not exist.` with the AssociationId from the request as its only variable part.
Emulator: the code is right but the message is the shape's documentation string, `The specified association doesn't exist.` — it names no id and is not AWS's wording.
Why: the message is part of the contract clients log, match and show to operators; a message that omits the id makes a failure in a multi-association teardown unattributable, and any assertion or log scraper keyed to AWS's wording breaks.
Fix: the DeleteAssociation handler's refusal construction (the AssociationDoesNotExist message template).
Traces: ssm.delete-association-nonexistent-rejected

Cause 7 of 8: CreateMaintenanceWindow's malformed-cron refusal message is emulator-authored.
AWS: Smithy: CreateMaintenanceWindow.Schedule is required, @length 1..256. A cron expression that does not parse is refused HTTP 400 `ValidationException` whose message names the parse position and the specific rule violated: `Invalid CRON expression: error at position <n>, details: <detail>.` — for `cron(0 25 ? * MON *)` the recorded message is `Invalid CRON expression: error at position 1, details: '?' can only be specified for Day-of-Month or Day-of-Week.`. Fixed text: `Invalid CRON expression: error at position `, `, details: `; variable parts: the position and the rule detail derived from the submitted expression. No window is created, so DescribeMaintenanceWindows filtered to that Name returns [].
Emulator: the code `ValidationException` is right but the message is a generic `The schedule is invalid. Verify your cron or rate expression and try again.` — it names neither a position nor the violated rule.
Why: a client (or a human) debugging a rejected schedule gets no indication of which field of the expression is wrong, and assertions/log matching on AWS's wording fail.
Fix: the CreateMaintenanceWindow handler's Schedule validation — parse the cron/rate expression and build the position-and-detail ValidationException message.
Traces: ssm.maintenance-window-dates-echo-and-invalid-schedule-rejected

Cause 8 of 8: AddTagsToResource under a mismatched ResourceType answers the wrong error code and an empty message.
AWS: Smithy: AddTagsToResource takes required ResourceType (enum ResourceTypeForTagging) and ResourceId, and models both `ValidationException` and `InvalidResourceId`. A ResourceId whose form does not match the declared ResourceType's own id pattern is refused by a FORMAT check ahead of any existence lookup: HTTP 400 `ValidationException` with the message `resourceId should be in format <pattern> for resource type <ResourceType>` — fixed text `resourceId should be in format ` ... ` for resource type `, with the type's id pattern and the ResourceType value from the request as its variable parts (for `PatchBaseline`: `resourceId should be in format pb-[0-9a-f]{17} for resource type PatchBaseline`). A well-formed id that simply does not exist is the separate `InvalidResourceId` (whose shape models NO message member, so the message is empty). The refused call must leave the resource's tags unchanged.
Emulator: a maintenance-window id sent under ResourceType `PatchBaseline` is refused `InvalidResourceId` with an empty message — the existence-flavoured code, not the format-flavoured ValidationException, and with no indication of the expected id form.
Why: a client cannot distinguish "you named the wrong ResourceType" from "that id does not exist", and gets no hint of the id format the type requires; retry logic that treats InvalidResourceId as a transient/absent resource retries a request that can never succeed.
Fix: the AddTagsToResource handler's input validation — pattern-check ResourceId against the declared ResourceType before the existence lookup and raise ValidationException with the format message.
Traces: ssm.tag-calls-reject-missing-window-and-mismatched-resource-type

## Evidence
Cause 1 — GetParameters, expected names as asked (with the leading '/'): AssertionError: GetParameters must list results in alphabetical order by name whatever order they were requested in; asked for ['/sim2real-behav-batchorder-c754612052dd-c', '/sim2real-behav-batchorder-c754612052dd-b', '/sim2real-behav-batchorder-c754612052dd-a'] and got ['sim2real-behav-batchorder-c754612052dd-a', 'sim2real-behav-batchorder-c754612052dd-b', 'sim2real-behav-batchorder-c754612052dd-c'] — the ordering is in fact alphabetical; every returned Name has lost its leading '/'. Selector form: AssertionError: the returned Name must be the base name, not the selector form: 'sim2real-behav-probe-selector-1d8f4611c90f' (created as '/sim2real-behav-probe-selector-...'). The generated suffixes are per-run fillers.

Cause 2 — expected: GetParametersByPath('/aws/service/global-infrastructure/regions') lists /aws/service/global-infrastructure/regions/us-east-1 as Type 'String', Value 'us-east-1'. Actual: AssertionError: ... got None among 0 parameters.

Cause 3 — expected: DescribeDocument(Name='AWS-RestartEC2Instance') -> (Owner, DocumentType, Status) == ('Amazon', 'Automation', 'Active'). Actual: botocore.errorfactory.InvalidDocument: An error occurred (InvalidDocument) when calling the DescribeDocument operation: Document AWS-RestartEC2Instance doesn't exist.

Cause 4 — expected: GetAutomationExecution(AutomationExecutionId=<SendCommand CommandId>) -> ClientError AutomationExecutionNotFoundException, 'The input automation execution does not exist.'. Actual: AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'AutomationExecution': {'AutomationExecutionId': '00aee12f-...', 'DocumentName': 'sim2real-behav-idspace-...', 'AutomationExecutionStatus': 'Success', 'StepExecutions': [{'StepName': 'pause', 'Action': 'aws:sleep', 'StepStatus': 'Success', ...}], 'Mode': 'Auto', 'ExecutedBy': 'arn:aws:iam::000000000000:user/emulator', 'MaxConcurrency': '10', 'MaxErrors': '1'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (ids/timestamps are per-run fillers).

Cause 5 — expected: GetMaintenanceWindowExecution(WindowExecutionId=<random UUID>) -> DoesNotExistException, 'Resource does not exist in the data store.'. Actual: AssertionError: expected DoesNotExistException from GetMaintenanceWindowExecution for a nonexistent WindowExecutionId, got NotImplemented.

Cause 6 — expected message: 'Association <association-id> does not exist.'. Actual: AssertionError: The specified association doesn't exist. (code AssociationDoesNotExist was correct).

Cause 7 — expected: ('ValidationException', "Invalid CRON expression: error at position 1, details: '?' can only be specified for Day-of-Month or Day-of-Week.") for Schedule='cron(0 25 ? * MON *)'. Actual: AssertionError: CreateMaintenanceWindow with a malformed cron gave 'ValidationException': 'The schedule is invalid. Verify your cron or rate expression and try again.'

Cause 8 — expected: ('ValidationException', 'resourceId should be in format pb-[0-9a-f]{17} for resource type PatchBaseline') for AddTagsToResource(ResourceType='PatchBaseline', ResourceId=<an mw- id>). Actual: AssertionError: AddTagsToResource with a mismatched ResourceType gave 'InvalidResourceId': ''. (The sibling leg — ListTagsForResource on a missing 'mw-0123456789abcdef0' answering InvalidResourceId with an empty message — passed.)

## Steps to Reproduce
Cause 1: PutParameter three String parameters named '/<p>-a', '/<p>-b', '/<p>-c'; call GetParameters(Names=['/<p>-c','/<p>-b','/<p>-a']) and read Parameters[*].Name. Expect the names exactly as sent; observe the leading '/' stripped. Then PutParameter '/<p>2' twice, label version 1, and GetParameter(Name='/<p>2:1') — expect Name '/<p>2' with Selector '1'; observe '<p>2' without the slash.

Cause 2: ssm.get_parameters_by_path(Path='/aws/service/global-infrastructure/regions') (paginated). Expect a parameter per Region, including /aws/service/global-infrastructure/regions/us-east-1 (String, value 'us-east-1'); observe 0 parameters.

Cause 3: ssm.describe_document(Name='AWS-RestartEC2Instance'). Expect Owner 'Amazon', DocumentType 'Automation', Status 'Active'; observe InvalidDocument "Document AWS-RestartEC2Instance doesn't exist". Also ssm.list_documents filtered Owner=Amazon, DocumentType=Automation.

Cause 4: create a schemaVersion 2.2 command document, SendCommand against it and keep the CommandId; call ssm.get_automation_execution(AutomationExecutionId=<that CommandId>). Expect AutomationExecutionNotFoundException; observe HTTP 200 with a populated AutomationExecution. Mirror: start an Automation execution and call ssm.cancel_command(CommandId=<that execution id>), expecting InvalidCommandId 'Command id <id> does not exist.'

Cause 5: ssm.get_maintenance_window_execution(WindowExecutionId=str(uuid.uuid4())). Expect DoesNotExistException; observe NotImplemented.

Cause 6: ssm.delete_association(AssociationId=<a random UUID that names no association>). Expect AssociationDoesNotExist with message 'Association <that id> does not exist.'; observe 'The specified association doesn't exist.'

Cause 7: ssm.create_maintenance_window(Name=<n>, Schedule='cron(0 25 ? * MON *)', Duration=3, Cutoff=1, AllowUnassociatedTargets=True). Expect ValidationException "Invalid CRON expression: error at position 1, details: '?' can only be specified for Day-of-Month or Day-of-Week."; observe the generic schedule message. Then DescribeMaintenanceWindows filtered to that Name must be [].

Cause 8: create a maintenance window and take its WindowId; call ssm.add_tags_to_resource(ResourceType='PatchBaseline', ResourceId=<that mw- id>, Tags=[{'Key':'owner','Value':'beta'}]). Expect ValidationException 'resourceId should be in format pb-[0-9a-f]{17} for resource type PatchBaseline'; observe InvalidResourceId with an empty message. Confirm the window's tags are unchanged.
