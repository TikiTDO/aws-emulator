Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-resource-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-resource-policy-lifecycle kinesis-resource-policy-malformed-document kinesis-resource-policy-stream-not-active
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|PutResourcePolicy|4xx->4xx|$.message
Issue-Key: kinesis|GetResourcePolicy|2xx->2xx|$.Policy
Issue-Key: kinesis|GetResourcePolicy|4xx->4xx|$.__type
Issue-Key: kinesis|GetResourcePolicy|4xx->4xx|$.message
Issue-Key: kinesis|PutResourcePolicy|4xx->2xx|$
Issue-Key: kinesis|PutResourcePolicy|4xx->2xx|status
Issue-Key: kinesis|DeleteResourcePolicy|4xx->2xx|$
Issue-Key: kinesis|DeleteResourcePolicy|4xx->2xx|status
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# kinesis: resource-policy operations skip stream-state gating and author their own policy text (5 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy, GetResourcePolicy, DeleteResourcePolicy

## Description
FIVE net-new root causes in the kinesis resource-policy surface (PutResourcePolicy / GetResourcePolicy / DeleteResourcePolicy), all replayed from the wire corpus simparity-wire-aws-v1. Each cause below must be closed before this issue is submitted. Three of them are the same missing precondition seen from three different handlers (no stream-state gate), one is an invented refusal sentence, and one is a policy-document round-trip defect.

Related but NOT covering: 20261029T200552Z-kinesis-getresourcepolicy-refuses-a-stream-that-has-no.md files GetResourcePolicy on an ACTIVE stream that has NO policy attached (should be 200 with Policy "{}"). Cause 3 here is a different precondition: a stream that is not ACTIVE at all, which AWS refuses with ResourceInUseException before any policy lookup happens. 20261029T200552Z-kinesis-9-net-new-root-causes-resource-policy.md covers Deny enforcement on the data plane and an invalid Effect; neither covers the causes below. 20261027T011654Z-kinesis-putresourcepolicy-stub.md is the original scaffold stub; the handler now answers, but with the divergences below.

=== Cause 1 of 5: the invalid-policy-JSON refusal message is emulator-authored instead of AWS's fixed sentence ===
AWS: Smithy models PutResourcePolicy input { ResourceARN (required, @length 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$), StreamId, Policy (required) } with an empty output, and its modeled errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException (all HTTP 400). Shape Policy is a plain `string`, so the JSON-validity check is a service-side policy-document check, not a model constraint. Recorded behavior: a Policy string that is not parseable JSON is refused with HTTP 400 and the message `Policy validation error: This policy contains invalid Json`. That message is entirely FIXED text — it has no variable parts, it does not quote the offending document, the offset or the member name. The emulator already returns the correct HTTP status and the correct error code for this case; only the sentence diverges.
Emulator: HTTP 400 with the right error code but the message `Policy must be a JSON policy document: the value sent is not valid JSON.` — a sentence AWS never emits for this error class.
Why: a client that branches on, logs, or surfaces the refusal text for a malformed policy sees wording no AWS SDK or console ever produces; test suites and operator runbooks that assert the AWS sentence pass against AWS and fail against the emulator, and the emulator's text implies a generic request-shape problem rather than policy-document validation.
Fix: the message template in PutResourcePolicy's policy-document validation in the kinesis service crate (the ops handler for PutResourcePolicy, its policy-validation path), which must emit the fixed AWS sentence for an unparseable policy.
Traces: kinesis-resource-policy-lifecycle#task#3, kinesis-resource-policy-malformed-document#task#3.

=== Cause 2 of 5: the stored policy document is re-encoded on read instead of being returned as the opaque string it was submitted as ===
AWS: Smithy models GetResourcePolicy output { Policy (required) } where Policy is shape `string`. Recorded behavior: the policy document reads back BYTE-FOR-BYTE as it was submitted to PutResourcePolicy — the recorded response carries the compact form the request sent (no space after `:` or `,`, original member order). AWS stores the policy as an opaque document and does not normalize, reformat or re-order it.
Emulator: GetResourcePolicy answers 200 but returns a RE-SERIALIZED document: every `":"` and `","` separator has gained a space (`{"Version": "2012-10-17", "Statement": [{"Sid": ...`) where the submitted document had none. Hypothesis: the handler parses the policy into a JSON value when storing or reading it and re-serializes with a pretty/spaced encoder, which also makes member order non-authoritative.
Why: the policy document is a string to every caller. Code that compares the read-back policy to the document it wrote (drift detection), hashes or signs it, or diffs it in a read-modify-write loop sees a spurious change on every single read, so idempotent policy management reports a permanent diff and may rewrite the policy endlessly. Any assertion of exact round-trip fidelity (including whitespace inside string values) is unreproducible.
Fix: the kinesis resource-policy state and the GetResourcePolicy/PutResourcePolicy serializers must persist and return the Policy member as the raw string received, with no JSON parse-and-re-emit round trip (validity checking may parse a copy, but must not replace the stored bytes).
Traces: kinesis-resource-policy-malformed-document#task#5.

=== Cause 3 of 5: GetResourcePolicy applies no stream-state precondition, so a non-ACTIVE stream is reported as a missing policy ===
AWS: Smithy ResourceInUseException (client error, HTTP 400) is documented as "The resource is not available for this operation. For successful operation, the resource must be in the ACTIVE state." and is a modeled error of GetResourcePolicy. Recorded behavior: GetResourcePolicy against a stream that exists but is not yet ACTIVE is refused with HTTP 400, error code `ResourceInUseException`, message `Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state <state>` — fixed text `Stream `, ` under account `, ` not ACTIVE or UPDATING, instead in state `; variable parts the stream name taken from the request's ResourceARN, the caller's 12-digit account id, and the stream's current state from service state. The state check precedes any policy lookup.
Emulator: HTTP 400 with error code `ResourceNotFoundException` and message `Policy for resource <resource-arn> under account <account-id> not found.` — the state gate is absent entirely, so the call falls through to the policy store and reports the absence of a policy on a stream that is merely still being created.
Why: the two conditions demand opposite client actions. ResourceInUseException means "retry once the stream reaches ACTIVE"; ResourceNotFoundException means "the thing you named does not exist, stop". A client that polls on ResourceInUseException aborts instead, and a client that treats not-found as "no policy yet" starts from an empty document and then writes a policy against a stream whose real state it never learned.
Fix: GetResourcePolicy's handler in the kinesis service crate must resolve the ResourceARN to the stream, gate on its state before touching the policy store, and raise ResourceInUseException with the AWS template above when that state is neither ACTIVE nor UPDATING.
Traces: kinesis-resource-policy-stream-not-active#task#2.

=== Cause 4 of 5: PutResourcePolicy accepts a write against a non-ACTIVE stream and answers 200 instead of refusing ===
AWS: PutResourcePolicy models ResourceInUseException (HTTP 400) — "the resource must be in the ACTIVE state". Recorded behavior: PutResourcePolicy on a stream that is not ACTIVE is REFUSED with HTTP 400, code `ResourceInUseException`, message `Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state <state>` (same template and same fixed/variable split as Cause 3). No policy is attached.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body — and the policy is attached to a stream that is still in a non-ACTIVE state.
Why: this is a silent write where AWS performs none. A client gets a success for a mutation AWS rejects, so its retry/ordering logic (wait for ACTIVE, then attach the policy) is never exercised against the emulator, and the emulator's and AWS's resource state diverge from that point on — the emulator reports a policy on a resource AWS would have left unpoliced. Code that relies on the refusal as a readiness probe loops zero times and proceeds too early.
Fix: add the stream-state precondition to PutResourcePolicy's handler in the kinesis service crate, ahead of the state mutation, raising ResourceInUseException with the AWS message template.
Traces: kinesis-resource-policy-stream-not-active#task#3.

=== Cause 5 of 5: DeleteResourcePolicy accepts a delete against a non-ACTIVE stream and answers 200 instead of refusing ===
AWS: DeleteResourcePolicy models ResourceInUseException (HTTP 400) with the same ACTIVE-state requirement. Recorded behavior: DeleteResourcePolicy on a stream that is not ACTIVE is REFUSED with HTTP 400, code `ResourceInUseException`, message `Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state <state>` — fixed text as in Cause 3, variables the stream name from the request ARN, the account id and the current state.
Emulator: the call SUCCEEDS — HTTP 200 with an empty body — deleting (or no-op deleting) policy state on a stream whose state AWS would have refused on.
Why: same class of harm as Cause 4 but on the destructive path: a client is told a policy removal took effect in a window where AWS refuses it, so the emulator cannot be used to exercise the retry-until-ACTIVE contract and its post-call state does not match AWS's.
Fix: add the stream-state precondition to DeleteResourcePolicy's handler in the kinesis service crate, ahead of the state mutation, raising ResourceInUseException with the AWS message template.
Traces: kinesis-resource-policy-stream-not-active#task#4.

## Evidence
All evidence replayed from the wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 118/153 steps passed, score 77.1%). Stream names, ARNs and the account id below are per-run values that fill the message templates; the requirements are the HTTP status, the error code, and the fixed text of the message.

Cause 1 — PutResourcePolicy, fixtures kinesis-resource-policy-lifecycle#task#3 and kinesis-resource-policy-malformed-document#task#3 (status 400 matched, error code matched, $.message diverged):
  expected $.message: 'Policy validation error: This policy contains invalid Json'
  actual   $.message: 'Policy must be a JSON policy document: the value sent is not valid JSON.'

Cause 2 — GetResourcePolicy, fixture kinesis-resource-policy-malformed-document#task#5 (status 200 matched, $.Policy diverged):
  expected $.Policy: '{"Version":"2012-10-17","Statement":[{"Sid":"...","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"kinesis:DescribeStreamSummary","Resource":"<stream-arn>"}]}'  (compact, exactly the bytes submitted)
  actual   $.Policy: '{"Version": "2012-10-17", "Statement": [{"Sid": "...", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::<account>:root"}, "Action": "kinesis:DescribeStreamSummary", "Resource": "<stream-arn>"}]}'  (a space inserted after every ':' and ',' separator)

Cause 3 — GetResourcePolicy on a stream in state CREATING, fixture kinesis-resource-policy-stream-not-active#task#2 (status 400 matched, code and message diverged):
  expected $.__type: 'ResourceInUseException'; $.message: 'Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state CREATING'
  actual   $.__type: 'ResourceNotFoundException'; $.message: 'Policy for resource <stream-arn> under account <account-id> not found.'

Cause 4 — PutResourcePolicy on a stream in state CREATING, fixture kinesis-resource-policy-stream-not-active#task#3:
  expected: HTTP 400 with body { '__type': 'ResourceInUseException', 'message': 'Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state CREATING' }
  actual:   HTTP 200 with an empty body {} (the write was accepted)

Cause 5 — DeleteResourcePolicy on a stream in state CREATING, fixture kinesis-resource-policy-stream-not-active#task#4:
  expected: HTTP 400 with body { '__type': 'ResourceInUseException', 'message': 'Stream <stream-name> under account <account-id> not ACTIVE or UPDATING, instead in state CREATING' }
  actual:   HTTP 200 with an empty body {} (the delete was accepted)

Smithy citations used: PutResourcePolicy / GetResourcePolicy / DeleteResourcePolicy input { ResourceARN required @length 1..2048 pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$, StreamId @length 1..24 pattern ^[a-z0-9]{20}-[a-z0-9]{3}$ } (+ Policy required on Put), GetResourcePolicy output { Policy required }, shape Policy = string; modeled errors on all three: AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException, ResourceNotFoundException, all HTTP 400; ResourceInUseException doc: "The resource is not available for this operation. For successful operation, the resource must be in the ACTIVE state."

## Steps to Reproduce
Causes 1 and 2 (stream ACTIVE):
1. kinesis CreateStream(StreamName=<n>, ShardCount=1); poll DescribeStreamSummary until StreamStatus == ACTIVE; take StreamARN.
2. PutResourcePolicy(ResourceARN=<StreamARN>, Policy='not-json') -> observe HTTP 400 with the message 'Policy must be a JSON policy document: the value sent is not valid JSON.' instead of AWS's fixed 'Policy validation error: This policy contains invalid Json'. (Cause 1; replay kinesis-resource-policy-lifecycle#task#3 and kinesis-resource-policy-malformed-document#task#3.)
3. PutResourcePolicy(ResourceARN=<StreamARN>, Policy=<a valid policy serialized COMPACTLY, i.e. json.dumps(doc, separators=(',',':'))>) -> 200.
4. GetResourcePolicy(ResourceARN=<StreamARN>) and compare the returned Policy string byte-for-byte with the string sent in step 3 -> observe a space inserted after every ':' and ',' instead of the submitted bytes. (Cause 2; replay kinesis-resource-policy-malformed-document#task#5.)

Causes 3, 4 and 5 (stream NOT ACTIVE):
5. kinesis CreateStream(StreamName=<m>, ShardCount=1) and do NOT wait — act while DescribeStreamSummary still reports StreamStatus == CREATING; build <StreamARN> for <m>.
6. GetResourcePolicy(ResourceARN=<StreamARN>) -> observe 400 ResourceNotFoundException 'Policy for resource <arn> under account <account> not found.' instead of 400 ResourceInUseException 'Stream <m> under account <account> not ACTIVE or UPDATING, instead in state CREATING'. (Cause 3; replay kinesis-resource-policy-stream-not-active#task#2.)
7. PutResourcePolicy(ResourceARN=<StreamARN>, Policy=<a valid policy>) -> observe HTTP 200 with an empty body instead of the 400 ResourceInUseException above. (Cause 4; replay kinesis-resource-policy-stream-not-active#task#3.)
8. DeleteResourcePolicy(ResourceARN=<StreamARN>) -> observe HTTP 200 with an empty body instead of the 400 ResourceInUseException above. (Cause 5; replay kinesis-resource-policy-stream-not-active#task#4.)
9. Full replay: replay_wire_corpus and confirm kinesis-resource-policy-lifecycle, kinesis-resource-policy-malformed-document and kinesis-resource-policy-stream-not-active report no divergent steps.
