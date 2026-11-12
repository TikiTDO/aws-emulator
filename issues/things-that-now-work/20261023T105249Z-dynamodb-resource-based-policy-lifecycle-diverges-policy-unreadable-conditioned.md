Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-tagging-and-resource-policy
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-tagging-and-resource-policy ddbctl-resource-policy-deny-vs-not-found dynamodb-resource-policy-lifecycle dynamodb-resource-policy-management
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetResourcePolicy|4xx->2xx|$.__type
Issue-Key: dynamodb|GetResourcePolicy|4xx->2xx|$.message
Issue-Key: dynamodb|GetResourcePolicy|4xx->2xx|status
Issue-Key: dynamodb|GetResourcePolicy|2xx->4xx|$.Policy
Issue-Key: dynamodb|GetResourcePolicy|2xx->4xx|status
Issue-Key: dynamodb|GetItem|2xx->4xx|$.Item
Issue-Key: dynamodb|GetItem|2xx->4xx|status
Issue-Key: dynamodb|GetItem|4xx->4xx|$.Message
Issue-Key: dynamodb|GetResourcePolicy|2xx->2xx|$.Policy
Issue-Key: dynamodb|PutResourcePolicy|2xx->2xx|$.RevisionId
Issue-Key: dynamodb|DeleteResourcePolicy|2xx->2xx|$.RevisionId
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Resource-based policy lifecycle diverges: policy unreadable, conditioned deny refuses all reads, RevisionId wrong shape

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
DynamoDB resource-based policy support diverges on every axis the corpus exercises: the absent-policy case answers success instead of an error, an attached policy cannot be read back, a policy carrying a Condition makes the emulator refuse unrelated reads, the stored document is re-serialized rather than echoed, and RevisionId is the wrong kind of value. 5 causes, all dynamodb.

Cause 1 of 5: GetResourcePolicy on a table with no policy answers 200 with an empty body instead of PolicyNotFoundException.
AWS: Smithy: GetResourcePolicy errors include PolicyNotFoundException (client error, httpError 400) and its output members Policy/RevisionId are both optional-but-present on success. The recording shows HTTP 400 with __type 'com.amazonaws.dynamodb.v20120810#PolicyNotFoundException' when the table has no resource-based policy attached.
Emulator: HTTP 200 with an empty body - no __type, no message, no Policy.
Why: a client cannot distinguish "no policy attached" from "policy attached but empty"; SDK calls that expect to catch PolicyNotFoundException instead succeed and then dereference an absent Policy, and idempotent "create policy if missing" flows skip the create.
Fix: the GetResourcePolicy handler - raise PolicyNotFoundException (400) when the table's policy slot is empty rather than returning an empty success.
Traces: dynamodb-tagging-and-resource-policy#task#8.

Cause 2 of 5: GetResourcePolicy answers 400 for a policy that was successfully put.
AWS: Smithy: GetResourcePolicy returns 200 with Policy (the ResourcePolicy string) for an attached policy. The recording shows the document put earlier in the fixture coming back verbatim with status 200.
Emulator: HTTP 400, and $.Policy missing entirely.
Why: the write path reports success but the read path denies the policy exists, so a client can neither verify nor audit the policy it just attached; any read-modify-write of the policy is impossible.
Fix: the GetResourcePolicy handler and the policy store it reads (the policy written by PutResourcePolicy must be keyed so the subsequent Get finds it - note the policy in this step carries a Condition, see cause 3).
Traces: dynamodb-tagging-and-resource-policy#task#12.

Cause 3 of 5: a resource policy containing a Condition makes the emulator refuse data-plane reads outright, including keys the policy does not deny.
AWS: Smithy: GetItem's modeled errors are ConditionalCheckFailedException, ResourceNotFoundException, ProvisionedThroughputExceededException, etc. - there is no modeled error for "policy condition unsupported". The recording shows AWS evaluating the explicit-deny statement (Action dynamodb:GetItem, Condition ForAllValues:StringEquals on dynamodb:LeadingKeys = secret): the denied leading key gets a 400 authorization failure, while a GetItem on a NON-matching key ('pk' = 'public') succeeds with 200 and returns the item {'pk': {'S': 'public'}, 'v': {'S': 'p1'}}.
Emulator: the non-denied GetItem also answers 400 with no Item; the emulator's own message states it "does not evaluate" the Condition and "refuses rather than serves what the policy may deny". So attaching any conditioned deny policy fails every GetItem on the table.
Why: a client that uses a fine-grained LeadingKeys policy - the standard DynamoDB per-tenant isolation pattern - loses access to all of its data instead of just the denied partition; a fail-closed blanket denial is not the authorization outcome AWS produces.
Fix: the resource-policy authorization check in the GetItem path (and the shared policy evaluator) - evaluate statement Conditions, and when a statement does not match the request, allow the request rather than refusing.
Traces: ddbctl-resource-policy-deny-vs-not-found#task#8 (non-denied key answered 400 instead of 200 with the item) and ddbctl-resource-policy-deny-vs-not-found#task#6 (the denied key's 400 body carries the emulator's refusal text in $.Message where a shaped authorization message is expected; the recorded text itself is only an example, the defect is the unevaluated Condition).

Cause 4 of 5: GetResourcePolicy does not echo the stored policy document; it re-serializes it and changes a Condition value's type.
AWS: Smithy: ResourcePolicy is a plain string shape, so the policy is an opaque document - AWS returns the document as stored. The recording returns the compact JSON exactly as submitted, with "dynamodb:LeadingKeys":"secret" as a scalar string.
Emulator: returns a re-printed document - whitespace after ':' and ',' inserted - and promotes the Condition value to an array: "dynamodb:LeadingKeys": ["secret"].
Why: clients byte-compare or hash the returned policy to detect drift (and some re-submit it as-is), so a re-serialized document reads as a change that nobody made; turning a scalar into a single-element list also rewrites the policy's meaning for consumers that type-check condition values.
Fix: the PutResourcePolicy/GetResourcePolicy state - store the submitted policy string verbatim and return that string, instead of parsing to a structure and re-serializing on read.
Traces: ddbctl-resource-policy-deny-vs-not-found#task#7 (cluster of 4, incl. dynamodb-resource-policy-lifecycle#task#6).

Cause 5 of 5: RevisionId is returned as a 64-character hex content digest instead of a policy revision id.
AWS: Smithy: PolicyRevisionId is a string with @length min 1 max 255, returned by PutResourcePolicy (RevisionId), GetResourcePolicy and DeleteResourcePolicy, and accepted back as ExpectedRevisionId for optimistic concurrency. The recordings show a short opaque numeric revision token (e.g. '1784568945896') that changes per revision.
Emulator: returns a 64-hex SHA-style digest of the policy document (e.g. 'da228917604c4ef00f2806cbeaa9326470788819e3990826d0252991350e62f4'). Because it is a digest of the content, re-putting an identical document yields the SAME RevisionId, so the value does not identify a revision at all.
Why: ExpectedRevisionId-based compare-and-set silently loses its guard - two sequential writes of the same document are indistinguishable, so a stale writer's update is accepted; clients that persist or length-check the revision token also see an unexpected value shape.
Fix: the PutResourcePolicy/DeleteResourcePolicy handler and policy state - issue a monotonic per-revision id on every successful write rather than hashing the document.
Traces: ddbctl-resource-policy-deny-vs-not-found#task#5 (PutResourcePolicy, cluster of 4) and dynamodb-tagging-and-resource-policy#task#11 (DeleteResourcePolicy).

Related open issues: none of the six open dynamodb issues cover the resource-policy surface (the closest, '20261022T113745Z-dynamodb-item-mutations-are-not-persisted-and-returnvalues-attributes.md', owns the item-persistence clusters only).

## Evidence
Divergence 39 (GetResourcePolicy, status 400->200): $.__type missing (expected 'com.amazonaws.dynamodb.v20120810#PolicyNotFoundException'), $.message missing, status expected 400 got 200.
Divergence 41 (GetResourcePolicy, status 200->400): $.Policy missing (expected the document put earlier in the fixture), status expected 200 got 400.
Divergence 8 (GetItem, status 200->400): status expected 200 got 400; $.Item missing (expected {'pk': {'S': 'public'}, 'v': {'S': 'p1'}}).
Divergence 6 (GetItem, 400->400): $.Message carries "... with an explicit deny in a resource-based policy (the statement carries a Condition, which this emulator does not evaluate: it refuses rather than serves what the policy may deny)".
Divergence 7 (GetResourcePolicy, 200->200): expected '{"Version":"2012-10-17",..."dynamodb:LeadingKeys":"secret"}' got '{"Version": "2012-10-17", ... "dynamodb:LeadingKeys": ["secret"]}'.
Divergence 5 (PutResourcePolicy, 200->200): $.RevisionId expected a shaped revision token, got 'da228917604c4ef00f2806cbeaa9326470788819e3990826d0252991350e62f4'; divergence 40 (DeleteResourcePolicy): expected '1784568945896', got 'a30461516c0a56a7a392d75df47bb1953009f8002d899599743c661b34e7b451'.
Model: smithy_query(operation=GetResourcePolicy) -> errors PolicyNotFoundException (400), ResourceNotFoundException (400); output Policy:ResourcePolicy (plain string), RevisionId:PolicyRevisionId @length 1..255; PutResourcePolicy input ExpectedRevisionId:PolicyRevisionId.

## Steps to Reproduce
1. Replay dynamodb-tagging-and-resource-policy#task#8: GetResourcePolicy on a table with no policy -> 200 empty body instead of 400 PolicyNotFoundException.
2. Replay dynamodb-tagging-and-resource-policy#task#12: PutResourcePolicy then GetResourcePolicy -> 400 instead of 200 with the policy.
3. Replay ddbctl-resource-policy-deny-vs-not-found#task#5..task#8: attach a deny policy with a LeadingKeys Condition, then GetItem the non-matching key ('public') -> 400 instead of 200 with the item; GetResourcePolicy (task#7) returns a re-serialized document with the condition value turned into a list; PutResourcePolicy (task#5) returns a 64-hex digest as RevisionId.
4. Replay dynamodb-tagging-and-resource-policy#task#11: DeleteResourcePolicy returns a 64-hex digest instead of a revision token.
