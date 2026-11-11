Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue__logs__s3.cfn-glue-crawler-in-place-update-crawls-retained-replacement-then-not-found cloudformation__glue__logs__s3.cfn-glue-job-in-place-update-runs-retained-replacement-then-not-found
Issue-Key: cloudformation__glue__logs__s3.cfn-glue-crawler-in-place-update-crawls-retained-replacement-then-not-found|_assert_crawler#24|AssertionError|assert got == want, f"GetCrawler must read back the template: {got!r}, want {want!r}"
Issue-Key: cloudformation__glue__logs__s3.cfn-glue-job-in-place-update-runs-retained-replacement-then-not-found|_assert_replaced_and_retained#38|AssertionError|assert _in_order(
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__glue__logs__s3: 2 net-new causes — GetCrawler echoes the Role as the ARN it was created with instead of the role name, and a replaced AWS::Glue::Job is reported with CREATE_*/DELETE_* events and deleted despite UpdateReplacePolicy Retain

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCrawler

## Description
Two net-new root causes observed on CloudFormation-deployed Glue resources. Neither failing check is marked ALREADY FILED or OWNED by an open issue.

Cause 1 of 2: GetCrawler returns Role verbatim as the IAM role ARN it was created with, where AWS returns the role's NAME.
AWS: Smithy (glue) Crawler.Role is documented as 'The Amazon Resource Name (ARN) of an IAM role that's used to access customer resources, such as Amazon Simple Storage Service (Amazon S3) data.', i.e. the member is loosely typed and the model states no normalization. Recorded behavior: CreateCrawler accepts either a role ARN or a role name, and GetCrawler reports the bare role NAME — the resource part of the ARN after the last '/' — for a crawler created with an ARN of a role in the caller's own account. Here CloudFormation's AWS::Glue::Crawler Role is 'Fn::GetAtt': [CrawlerRole, Arn], so the create sees an ARN and the read must still answer the name. Every other member of the read-back (Name, Description, DatabaseName, TablePrefix, Targets.S3Targets[].Path, SchemaChangePolicy) matches.
Emulator: GetCrawler answers Role = 'arn:aws:iam::<account>:role/<role-name>', the exact string CreateCrawler was given.
Why: a client that compares GetCrawler's Role against a role name (the form AWS reports, and the form console/CLI tooling and drift checks key on) never matches, and code that splits the value on '/' to recover a name gets a different string depending on whether the crawler was created by ARN or by name.
Fix: glue's CreateCrawler/UpdateCrawler handler (or the crawler state it writes) must normalize an own-account role ARN to its role name, so the GetCrawler serializer reports the name.
Traces: cloudformation__glue__logs__s3.cfn-glue-crawler-in-place-update-crawls-retained-replacement-then-not-found (fails on the first read-back, immediately after CREATE_COMPLETE).

Cause 2 of 2: a replacement update is reported as a fresh CREATE and UpdateReplacePolicy Retain is ignored, so the replaced AWS::Glue::Job is deleted (UpdateStack).
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}; ResourceStatus is a modeled enum including UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. An update that changes a create-only property (here the Glue job's Name) replaces the resource inside the update: AWS emits, in order for that logical id, UPDATE_IN_PROGRESS with the fixed ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (the new job name, a request value), then — because UpdateReplacePolicy is Retain — exactly one DELETE_SKIPPED carrying the OLD PhysicalResourceId and no DELETE_IN_PROGRESS/DELETE_COMPLETE. The retained old job survives the update and the later DeleteStack, so glue GetJob on the old name keeps answering while GetJob on the stack-owned name refuses with EntityNotFoundException and the message 'Job not found.' (fixed text).
Emulator: CREATE_IN_PROGRESS (reason absent, PhysicalResourceId empty string), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE on the new job name, then DELETE_IN_PROGRESS (PhysicalResourceId absent) and DELETE_COMPLETE on the OLD job name — no UPDATE_* event, no replacement reason, no DELETE_SKIPPED, and the retained job is destroyed.
Why: the stack's event record cannot distinguish a replacement from a first create, and the resource the operator explicitly asked to retain is deleted while the events report that delete as successful, so the loss is undetectable from the stack record.
Fix: the CloudFormation update engine — resource-event emission on the replacement path, and the cleanup phase, which must honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain. Not the Glue provisioner.
Traces: cloudformation__glue__logs__s3.cfn-glue-job-in-place-update-runs-retained-replacement-then-not-found.

The engine-level behaviour of Cause 2 is already recorded for other resource types in the open issues 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md, 20261111T003921Z-cloudformation-logs-a-replaced-aws-logs-loggroup-is.md and 20261110T232651Z-cloudformation-sqs-an-update-that-replaces-a-resource.md; none of their replay sets names the checks above, so these recorded units are not re-replayed by any of them.

## Evidence
Replay output (behavioral suite, section ### cloudformation__glue__logs__s3):

Cause 1 — [mismatch] cloudformation__glue__logs__s3.cfn-glue-crawler-in-place-update-crawls-retained-replacement-then-not-found
  AssertionError: GetCrawler must read back the template: {..., 'Role': 'arn:aws:iam::<account>:role/<stack>-CrawlerRole-<suffix>', ...}, want {..., 'Role': '<stack>-CrawlerRole-<suffix>', ...}
  Expected (AWS): Role is the role NAME (the ARN's resource after the last '/'). Actual (emulator): the full ARN passed to CreateCrawler. Account id, role name and suffix are per-run values; the requirement is the name-vs-ARN form.

Cause 2 — [mismatch] cloudformation__glue__logs__s3.cfn-glue-job-in-place-update-runs-retained-replacement-then-not-found
  AssertionError: the replacement must report creating a new resource, complete as '<new job>' and skip deleting '<old job>' (UpdateReplacePolicy Retain), in that order: [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new job>'), ('CREATE_COMPLETE', None, '<new job>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old job>')]
  Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id 'Job', oldest first, from DescribeStackEvents; job names are per-run request values.
  Expected (AWS), in order: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>), ('UPDATE_COMPLETE', None, '<new job>'), ('DELETE_SKIPPED', None, '<old job>'), and no DELETE_IN_PROGRESS/DELETE_COMPLETE; glue GetJob('<old job>') still answers after the update and after DeleteStack, while GetJob('<new job>') then refuses EntityNotFoundException 'Job not found.'
  Actual (emulator): the CREATE_* triple above, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old job, which is gone.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout (cloudformation, glue, logs and s3 all serve).
2. CreateStack (CAPABILITY_IAM) with an AWS::IAM::Role trusting glue.amazonaws.com, an AWS::Glue::Database, and an AWS::Glue::Crawler whose Role is {'Fn::GetAtt': ['CrawlerRole', 'Arn']}, with Description, DatabaseName, TablePrefix, Targets.S3Targets[0].Path and SchemaChangePolicy set.
3. glue GetCrawler(Name=<crawler>): expect Crawler.Role to be the role NAME (the ARN's resource after the last '/'); observe the full 'arn:aws:iam::<account>:role/<name>' string instead.
4. Re-replay cloudformation__glue__logs__s3.cfn-glue-crawler-in-place-update-crawls-retained-replacement-then-not-found.

Cause 2:
1. CreateStack (CAPABILITY_IAM) with an AWS::Glue::Job (pinned Name, pythonshell Command pointing at a script in S3, a Role, DefaultArguments, MaxCapacity, Timeout) declaring "UpdateReplacePolicy": "Retain".
2. UpdateStack with the same template but a different job Name (create-only, so a replacement). Wait for UPDATE_COMPLETE.
3. DescribeStackEvents for the 'Job' logical id: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE on the new job name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason.
4. Expect exactly one DELETE_SKIPPED carrying the OLD job name and no DELETE_* progress events; observe DELETE_IN_PROGRESS + DELETE_COMPLETE.
5. glue GetJob on the OLD name: expect the retained job to survive the update and the later DeleteStack; observe EntityNotFoundException because it was deleted.
6. Equivalently: re-replay cloudformation__glue__logs__s3.cfn-glue-job-in-place-update-runs-retained-replacement-then-not-found.
