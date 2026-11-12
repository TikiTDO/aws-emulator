Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically|run#26|AssertionError|assert message == f"Stack with id {bare_uuid} does not exist", (
Issue-Commit: b94f7af2da1ac92db0383ac2b193617da0b34cf0

# REOPENS: ListStackResources still refuses a bare stack uuid with a pattern-constraint envelope instead of "Stack with id <id> does not exist"

> _Note: this issue was identified against the code base at commit `b94f7af2da1ac92db0383ac2b193617da0b34cf0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListStackResources

## Description
REOPENS 20261108T203843Z-cloudformation-liststackresources-refuses-a-bare-stack-uuid-with-a.md: its fix left the call refused.

The check that issue was grounded in (cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically) still fails on the same call with the same refusal wording, so the identifier pre-flight it named is still ahead of the stack lookup. This is not a new defect; it is the same cause, unfixed.

Cause 1 of 1: a stack identifier that is well formed as an identifier but names no stack — specifically the bare uuid tail of a stack id — is refused by a service-local regular-expression pre-check instead of being resolved and answered as a missing stack. Failing check id: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically.

AWS: Smithy: ListStackResources.StackName targets com.amazonaws.cloudformation#StackName, which the on-disk model defines as a plain string with NO @pattern and NO @length (smithy_query(query="shape", shape="StackName") returns {"type": "string"} and nothing more), and ListStackResources models no operation errors, so the only refusal available is the service-common ValidationError at HTTP 400. Recorded behavior: a stack is addressable by its name or by its full stack-id ARN; the uuid tail alone (stack_id.rsplit("/", 1)[-1]) is a syntactically acceptable identifier that resolves to nothing, and AWS refuses it with Error.Code ValidationError and Error.Message "Stack with id <identifier> does not exist". Fixed text: "Stack with id " … " does not exist"; the only variable part is the identifier copied verbatim from the request's StackName (never a literal from one run).

Emulator: the handler refuses before any lookup, with the right wire code (ValidationError, HTTP 400) but the constraint envelope as the message: "1 validation error detected: Value '<uuid>' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*". A uuid fails that expression because it may begin with a digit. Every earlier assertion of the same check passed (StackId ARN shape and region, name-vs-ARN equality across ListStackResources, DescribeStackResources and DescribeStackResource); only the bare-uuid refusal diverges.

Why: a client cannot tell "this stack is gone" from "you sent me garbage". Every waiter, cleanup path and existence probe that matches on the not-found wording ("does not exist" in str(exc), or an exact compare against "Stack with id <id> does not exist") treats the stack as still present and either loops to its timeout or aborts teardown. Any caller holding only a uuid — split out of a stack id, read from an event or a log line — is told its parameter is malformed rather than that the stack is absent. The pre-check is overbroad for every digit-leading identifier, not just uuids.

Fix: the stack-identifier pre-flight in the CloudFormation service state (malformed_identifier / refuse_malformed_identifier, as called at the top of the ListStackResources handler before the stack store lookup). Only an identifier CloudFormation could never resolve belongs there; one that resolves-and-misses must fall through to the handler's existing stack-does-not-exist refusal, which already carries the correct message template.

Traces: behavioral check cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically. Prior filing of the same cause: issues/things-that-now-work/20261108T203843Z-cloudformation-liststackresources-refuses-a-bare-stack-uuid-with-a.md (named services/service-com-amazonaws-cloudformation-cloudformation/src/ops/list_stack_resources.rs and src/state/stack_identifier.rs as the site).

## Evidence
From this behavioral replay, check cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically:

  AssertionError: ListStackResources on the bare stack uuid answered "1 validation error detected: Value '3e3b7137-3389-8c4e-b92e-eedac9f2784b' at 'stackNameOrId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z][-a-zA-Z0-9]*|arn:[-a-zA-Z0-9:/._+]*"

  at: assert message == f"Stack with id {bare_uuid} does not exist"

Expected (AWS): HTTP 400, Error.Code ValidationError, Error.Message "Stack with id <identifier> does not exist", where <identifier> is the StackName exactly as sent.
Actual (emulator): HTTP 400, Error.Code ValidationError (correct), Error.Message the pattern-constraint envelope quoted above (the uuid shown is this run's value and is not part of the requirement).

The assertion on the error CODE passed, so only the message text diverges.

## Steps to Reproduce
1. CreateStack with any template (the check uses one AWS::SSM::Parameter and one AWS::SQS::Queue) and wait for CREATE_COMPLETE.
2. Take the StackId the create returned and keep only the segment after the last '/' (the bare uuid).
3. Call ListStackResources(StackName=<bare uuid>).
4. Observe ValidationError with the pattern-constraint envelope; AWS answers ValidationError with "Stack with id <bare uuid> does not exist".

Re-replay: cloudformation.ec2mr-cfnres-stackid-arn-and-name-resolve-identically.
