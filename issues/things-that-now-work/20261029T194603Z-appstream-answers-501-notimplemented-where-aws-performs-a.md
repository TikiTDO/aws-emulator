Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.create-application-nonexistent-appblock-rejected appstream.streaming-url-rejected-until-fleet-running appstream.appblockbuilder-invalid-role-rejected appstream.concurrent-user-creation-duplicate-name
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.create-application-nonexistent-appblock-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: appstream.streaming-url-rejected-until-fleet-running|run#15|AssertionError|assert code == "OperationNotPermittedException", (
Issue-Key: appstream.appblockbuilder-invalid-role-rejected|run#8|AssertionError|assert code == "InvalidRoleException", (
Issue-Key: appstream.concurrent-user-creation-duplicate-name|run#10|AssertionError|assert len(successes) == racers, f"expected all {racers} racers to succeed, got: {results}"
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# appstream answers 501 NotImplemented where AWS performs a modeled refusal or an idempotent create: CreateApplication, CreateStreamingURL, CreateAppBlockBuilder, CreateUser (4 causes)

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication

## Description
Four AppStream operations answer HTTP 501 `NotImplemented` ("operation <Op> is scaffolded but not implemented in this emulator") for requests on which AWS returns a specific modeled error — or, for CreateUser, a specific SUCCESS. A 501 is neither: it is not one of the operation's modeled errors, so no client's error handling can match it. Each cause names the behavior the handler must carry, not merely "implement the op". Note on overlap: separate scaffold-stub issues are filed for the CreateAppBlockBuilder and CreateUser handlers (they block other checks in this suite via a propagated 501); causes 3 and 4 below are NOT those issues — they record the request-validation and idempotency semantics those handlers must have, which the stub issues do not describe, and they are grounded in two further failing checks whose replay keys those issues do not carry.

Cause 1 of 4: CreateApplication with an AppBlockArn that references no existing app block.
AWS: Smithy: CreateApplication.AppBlockArn is @required (Arn, pattern ^arn:aws...$) and the operation models ResourceNotFoundException (httpError 404) — "The specified resource was not found." Recorded behavior: 404 ResourceNotFoundException with message `The App block <AppBlockArn> for account <accountId> does not exist.` Fixed text: "The App block ", " for account ", " does not exist." Variable parts: <AppBlockArn> echoed from the request, <accountId> the caller's account. The syntactically valid ARN passes the modeled pattern, so the existence check is a handler gate, and no Application may be created.
Emulator: 501 NotImplemented (`operation CreateApplication is scaffolded but not implemented in this emulator`); the handler is a scaffold stub (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_application.rs, SCAFFOLDED marker).
Why: a client cannot create applications at all, and code that distinguishes "bad app-block reference" (404 ResourceNotFoundException, retry with a real ARN) from a server fault sees a 501 it treats as the emulator being broken, or retries forever.
Fix: the CreateApplication handler — implement the create plus its app-block existence gate and its ResourceNotFoundException message.
Traces: appstream.create-application-nonexistent-appblock-rejected.

Cause 2 of 4: CreateStreamingURL while the target fleet is not RUNNING.
AWS: Smithy: CreateStreamingURL models OperationNotPermittedException (httpError 400) — "The attempted operation is not permitted" — alongside ResourceNotFoundException/ResourceNotAvailableException. Recorded behavior: a STOPPED fleet associated with the stack yields 400 OperationNotPermittedException with message `The fleet <FleetName> is not running. The stack must have a running fleet to get a streaming URL` (no trailing period). Fixed text: "The fleet ", " is not running. The stack must have a running fleet to get a streaming URL"; variable part: <FleetName> from the request. No URL is issued and DescribeSessions shows no session for that UserId.
Emulator: 501 NotImplemented; create_streaming_url.rs is a scaffold stub whose handle() returns `Scaffolded::not_implemented("CreateStreamingURL")` (line ~173).
Why: the state gate that makes streaming-URL issuance safe is absent, so a caller can neither get the modeled refusal while the fleet is stopped nor a URL once it is running; any flow that waits for RUNNING and then streams is unexercisable.
Fix: the CreateStreamingURL handler — fleet/stack lookup, the fleet-state gate returning OperationNotPermittedException, and the StreamingURL/Expires output.
Traces: appstream.streaming-url-rejected-until-fleet-running.

Cause 3 of 4: CreateAppBlockBuilder with an IamRoleArn AppStream cannot assume.
AWS: Smithy: CreateAppBlockBuilder.IamRoleArn is an Arn member and the operation models InvalidRoleException (httpError 400) — the role-assumability gate. Recorded behavior: 400 InvalidRoleException with message `WorkSpaces Applications encountered an error because your IAM role '<IamRoleArn>' is invalid` (the ARN quoted in single quotes, no trailing period); no builder is created, so DescribeAppBlockBuilders on that name then answers 404 ResourceNotFoundException `The appblockbuilder <Name> for account <accountId> does not exist.`
Emulator: 400-expecting caller receives 501 NotImplemented instead, so neither the refusal nor the "no builder created" effect is observable.
Why: callers retry CreateAppBlockBuilder on InvalidRoleException while an IAM role propagates (the suite's own role-propagation retry keys on exactly that code); a 501 breaks that loop and makes a misconfigured role indistinguishable from an emulator fault.
Fix: the CreateAppBlockBuilder handler — implement the create and the IamRoleArn assumability gate returning InvalidRoleException with AWS's sentence. See also the filed scaffold-stub issue for this operation's handler, which covers only that the handler is a stub.
Traces: appstream.appblockbuilder-invalid-role-rejected.

Cause 4 of 4: concurrent CreateUser on one UserName+AuthenticationType must all succeed (idempotent create).
AWS: Smithy: CreateUser requires UserName and AuthenticationType and models ResourceAlreadyExistsException (httpError 400), but recorded behavior shows CreateUser is IDEMPOTENT for an identical UserName+AuthenticationType: six concurrent calls all return 200, none is refused, and afterwards exactly one user exists for that name with Enabled=true (DescribeUsers). Recorded behavior only — the model does not state the idempotency.
Emulator: all six concurrent calls return 501 NotImplemented, so the identity is never allocated and DescribeUsers can show nothing.
Why: user provisioning is unusable, and a client that treats "create user" as safely repeatable (the AWS contract) gets a non-modeled 501 on every attempt instead of a 200; a later conditional implementation must also not double-allocate the identity.
Fix: the CreateUser handler — idempotent insert keyed on (UserName, AuthenticationType) that is safe under concurrent calls, with the user readable from DescribeUsers as Enabled. See also the filed scaffold-stub issue for this operation's handler, which covers only that the handler is a stub.
Traces: appstream.concurrent-user-creation-duplicate-name.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (appstream 1/24 passed). All four failures are the emulator answering `(NotImplemented) ... operation <Op> is scaffolded but not implemented in this emulator`:

1) appstream.create-application-nonexistent-appblock-rejected [mismatch]
   AssertionError: expected ResourceNotFoundException for a nonexistent AppBlockArn, got 'NotImplemented'
   (expected message 'The App block <app-block-arn> for account <accountId> does not exist.')

2) appstream.streaming-url-rejected-until-fleet-running [mismatch]
   AssertionError: CreateStreamingURL against a STOPPED fleet must be rejected with OperationNotPermittedException; got 'NotImplemented'
   (expected message 'The fleet <fleet-name> is not running. The stack must have a running fleet to get a streaming URL')

3) appstream.appblockbuilder-invalid-role-rejected [mismatch]
   AssertionError: CreateAppBlockBuilder with a non-assumable IamRoleArn returned 'NotImplemented', expected 'InvalidRoleException'
   (expected message "WorkSpaces Applications encountered an error because your IAM role '<role-arn>' is invalid")

4) appstream.concurrent-user-creation-duplicate-name [mismatch]
   AssertionError: expected all 6 racers to succeed, got: [('error','NotImplemented') x6]

Stub markers in the checkout confirm the cause: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_application.rs and .../create_streaming_url.rs both carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and their handle() bodies end in `Err(crate::scaffolded::Scaffolded::not_implemented(...))` (create_streaming_url.rs line 173).

Resource names, ARNs and account numbers above are placeholders for per-run values; the requirements are the status, the modeled error code and the message template.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateApplication(Name=A, IconS3Location=..., LaunchPath=..., Platforms=[WINDOWS_SERVER_2019], InstanceFamilies=[GENERAL_PURPOSE], AppBlockArn=arn:aws:appstream:<region>:<account>:app-block/does-not-exist) -> emulator answers 501 NotImplemented; AWS answers 404 ResourceNotFoundException.
3. CreateStack(Name=S); CreateFleet(Name=F, FleetType=ELASTIC, ...); AssociateFleet(F,S); with F still STOPPED call CreateStreamingURL(StackName=S, FleetName=F, UserId=u) -> 501; AWS answers 400 OperationNotPermittedException.
4. CreateAppBlockBuilder(Name=B, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, VpcConfig=..., IamRoleArn=arn:aws:iam::<account>:role/does-not-exist) -> 501; AWS answers 400 InvalidRoleException.
5. Fire 6 concurrent CreateUser(UserName=u@example.com, AuthenticationType=USERPOOL, MessageAction=SUPPRESS) -> all 501; AWS returns 200 for every racer and DescribeUsers then shows exactly one Enabled user.
6. Or replay the four checks in trace_ids.
