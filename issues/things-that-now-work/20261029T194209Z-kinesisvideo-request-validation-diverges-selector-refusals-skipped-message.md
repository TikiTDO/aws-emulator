Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideo-get-data-endpoint
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideo-get-data-endpoint kinesisvideo-stream-lifecycle kinesis-video-signaling-channel-tag-resource-surface kinesis-video-stream-arn-tag-resource-surface kinesisvideo-stream-listing-and-unconfigured-reads kinesis-video-stream-get-data-endpoint kinesis-video-stream-tags-lifecycle kinesisvideo-stream-tags
Issue-Shape-Id: com.amazonaws.kinesisvideo#KinesisVideo_20170930
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->2xx|$.Message
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->2xx|status
Issue-Key: kinesisvideo|DescribeStream|4xx->2xx|$.Message
Issue-Key: kinesisvideo|DescribeStream|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeStream|4xx->2xx|status
Issue-Key: kinesisvideo|TagResource|4xx->4xx|$.Message
Issue-Key: kinesisvideo|UntagResource|4xx->4xx|$.Message
Issue-Key: kinesisvideo|ListStreams|4xx->4xx|$.Message
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->4xx|$.message
Issue-Key: kinesisvideo|GetDataEndpoint|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|TagStream|4xx->4xx|$.message
Issue-Key: kinesisvideo|TagStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|ListTagsForStream|4xx->4xx|$.message
Issue-Key: kinesisvideo|ListTagsForStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: kinesisvideo|DescribeStream|4xx->4xx|$.message
Issue-Key: kinesisvideo|DescribeStream|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# kinesisvideo request validation diverges: selector refusals skipped, Message member omitted, validation text misrendered

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataEndpoint

## Description
This issue bundles 9 net-new root causes in the Kinesis Video Streams input-validation layer: two requests AWS refuses are answered 200, three constraint violations are refused with no modeled Message member at all, and four more carry an emulator-authored constraint sentence. All nine are kinesisvideo and share one component: the pre-dispatch validation of the request plus the serialization of its InvalidArgumentException envelope.

Cause 1 of 9 - GetDataEndpoint accepts a request naming both stream selectors.
AWS: Smithy GetDataEndpoint (POST /getDataEndpoint) models StreamName and StreamARN as OPTIONAL members and APIName as @required; its documentation requires exactly one of the two selectors ('Gets an endpoint for a specified stream... You must specify either the StreamName or the StreamARN'). A request that does not name exactly one is refused with HTTP 400, an x-amzn-errortype naming the modeled error (InvalidArgumentException, @httpError 400), and Message 'One of StreamName or StreamARN must be provided, but not both.' That sentence has NO variable parts - it is fixed text naming the two member names.
Emulator: answers HTTP 200 with a DataEndpoint body; no Message member and no x-amzn-errortype header are emitted at all.
Why: a caller that builds its request wrongly (passing the name AND the ARN, which in general name different resources) gets a silently chosen endpoint instead of the refusal AWS sends, so the ambiguity is resolved by the server's internal preference. Against real AWS the same code fails immediately; against the emulator it writes or reads media on whichever stream the emulator picked.
Fix: GetDataEndpoint's input validation (the selector check that runs before the handler resolves the stream).
Traces: kinesisvideo-get-data-endpoint#task#6.

Cause 2 of 9 - DescribeStream accepts a request naming both stream selectors.
AWS: Smithy DescribeStream (POST /describeStream) models StreamName and StreamARN as optional and documents 'You must specify either the StreamName or the StreamARN'; naming both (or neither) is refused with HTTP 400 InvalidArgumentException and Message 'One of StreamName or StreamARN must be provided, but not both.' - fixed text, no interpolated values.
Emulator: answers HTTP 200 with a StreamInfo projection; no Message, no x-amzn-errortype.
Why: the same ambiguity as cause 1 on the read path - a client is handed the description of a stream it did not unambiguously ask for, and the validation error it must handle against AWS never appears.
Fix: DescribeStream's input validation.
Traces: kinesisvideo-stream-lifecycle#task#5.

Cause 3 of 9 - TagResource constraint violation is refused with no Message member.
AWS: Smithy TagResource (POST /TagResource) models Tags as @required TagList with @length min 1 max 50. A list longer than 50 is refused with HTTP 400 and the constraint-violation body carrying Message '1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50'. Fixed text throughout except two template slots taken from the model: the member path ('tags', the lowerCamel form of the input member) and the limit (50, the @length max). The rejected value is NOT echoed.
Emulator: returns HTTP 400 (correct status) but the response has NO Message member - the recording's $.Message is absent.
Hypothesis: the emulator serializes this validation class under a lowercase 'message' key (as it demonstrably does for TagStream in cause 7), so the modeled 'Message' member AWS populates for this operation is missing.
Why: the status is right but the body carries no reason, so a client (or an SDK surfacing e.message) reports an empty-message error and cannot tell a tag-count violation from any other 400.
Fix: TagResource's validation + the InvalidArgumentException serializer for this operation.
Traces: kinesis-video-signaling-channel-tag-resource-surface#task#7 (also kinesis-video-stream-arn-tag-resource-surface#task#7).

Cause 4 of 9 - UntagResource constraint violation is refused with no Message member.
AWS: Smithy UntagResource (POST /UntagResource) models TagKeyList as @required with @length min 1 max 50. An empty key list is refused with HTTP 400 and Message '1 validation error detected: Value at 'tagKeyList' failed to satisfy constraint: Member must have length greater than or equal to 1' - fixed text with the member path and the min bound from the model as its only variable parts.
Emulator: HTTP 400 with no Message member in the body.
Why: same as cause 3 - the refusal is unexplained, so a caller cannot distinguish an empty tagKeyList from a malformed ARN.
Fix: UntagResource's validation + its error serializer.
Traces: kinesis-video-stream-arn-tag-resource-surface#task#9.

Cause 5 of 9 - ListStreams enum constraint violation is refused with no Message member.
AWS: Smithy ListStreams (POST /listStreams) takes StreamNameCondition, whose ComparisonOperator targets the ComparisonOperator enum with the single value BEGINS_WITH ('Currently, you can specify only the BEGINS_WITH operator'). Any other operator is refused with HTTP 400 and Message '1 validation error detected: Value at 'streamNameCondition.comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [BEGINS_WITH]'. Note the dotted nested member path and the bracketed enum value set drawn from the model.
Emulator: HTTP 400 with no Message member.
Why: the listing is refused without naming the offending field, so a client cannot correct the condition it sent.
Fix: ListStreams' validation of StreamNameCondition + its error serializer.
Traces: kinesisvideo-stream-listing-and-unconfigured-reads#task#7.

Cause 6 of 9 - GetDataEndpoint enum refusal echoes the rejected value and reorders the enum value set.
AWS: APIName is @required and targets the APIName enum. An unmodeled value is refused with HTTP 400 and message '1 validation error detected: Value at 'aPIName' failed to satisfy constraint: Member must satisfy enum value set: [GET_IMAGES, LIST_FRAGMENTS, GET_DASH_STREAMING_SESSION_URL, GET_HLS_STREAMING_SESSION_URL, GET_MEDIA_FOR_FRAGMENT_LIST, PUT_MEDIA, GET_CLIP, GET_MEDIA]'. Two requirements here: the sentence does NOT quote the submitted value, and the value set is rendered in the service's own fixed order, which is neither the model's declaration order nor alphabetical.
Emulator: '1 validation error detected: Value 'SIM2REAL_NOT_AN_API' at 'aPIName' failed to satisfy constraint: Member must satisfy enum value set: [PUT_MEDIA, GET_MEDIA, LIST_FRAGMENTS, GET_MEDIA_FOR_FRAGMENT_LIST, GET_HLS_STREAMING_SESSION_URL, GET_DASH_STREAMING_SESSION_URL, GET_CLIP, GET_IMAGES]' - it interpolates the rejected value into the fixed text and emits the model's declaration order. The same steps also diverge on x-amzn-errortype: the recording's asserted shape does not match the bare 'InvalidArgumentException' the emulator sends.
Why: clients and test suites that match on the service's validation sentence (a very common pattern for enum/constraint assertions) fail to match, and the leaked value turns a fixed sentence into a request-dependent one.
Fix: the shared constraint-violation message renderer (value echo) plus the enum-set ordering it uses.
Traces: kinesis-video-stream-get-data-endpoint#task#7 (also #task#8, kinesisvideo-get-data-endpoint#task#8).

Cause 7 of 9 - TagStream tag-count refusal echoes a synthetic '<n> entries' value.
AWS: TagStream models Tags as @required ResourceTags map with @length min 1 max 50; over-length is refused with HTTP 400 and message '1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50' - the member path and the limit are the only variable parts.
Emulator: '1 validation error detected: Value '51 entries' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50' - it injects a 'Value '<count> entries'' fragment AWS never emits. x-amzn-errortype also diverges in shape from the recording.
Why: the sentence becomes request-dependent, so message assertions and log fingerprints diverge from AWS for every over-length request.
Fix: the constraint-violation message renderer for collection @length failures.
Traces: kinesis-video-stream-tags-lifecycle#task#8 (also kinesisvideo-stream-tags#task#13).

Cause 8 of 9 - ListTagsForStream pattern refusal echoes the value and renders the regex with anchors.
AWS: ListTagsForStream.StreamARN targets ResourceARN with @pattern '^arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+$'. A non-conforming ARN is refused with HTTP 400 and message '1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+' - the rejected ARN is not quoted, and the pattern is rendered WITHOUT the leading ^ and trailing $.
Emulator: quotes the submitted ARN ('Value '<the submitted arn>' at 'streamARN' ...') and renders the anchored '^...$' form; x-amzn-errortype diverges in shape as well.
Why: two per-request deviations in one sentence, so no client can match AWS's wording; the anchored form is also not the string AWS publishes for this constraint.
Fix: the constraint-violation message renderer (value echo; strip the pattern's anchors when rendering @pattern).
Traces: kinesis-video-stream-tags-lifecycle#task#10.

Cause 9 of 9 - DescribeStream pattern refusal has the same two deviations.
AWS: DescribeStream.StreamARN carries the same ResourceARN @pattern; a malformed ARN is refused with HTTP 400 and message '1 validation error detected: Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+'.
Emulator: echoes the submitted ARN and emits the anchored pattern, as in cause 8, confirming the defect is in the shared renderer rather than one operation.
Why: as cause 8.
Fix: the shared constraint-violation message renderer.
Traces: kinesisvideo-stream-lifecycle#task#6.

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator up; 147 steps, 85 passed, score 57.8%).
Cause 1 - kinesisvideo-get-data-endpoint#task#6: expected 400 with $.Message 'One of StreamName or StreamARN must be provided, but not both.' and an x-amzn-errortype header; got 200 with neither.
Cause 2 - kinesisvideo-stream-lifecycle#task#5: expected 400 with the same Message and an errortype header; got 200 with neither.
Cause 3 - kinesis-video-signaling-channel-tag-resource-surface#task#7 (and kinesis-video-stream-arn-tag-resource-surface#task#7): status 400 matches, $.Message missing - expected "1 validation error detected: Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", got None.
Cause 4 - kinesis-video-stream-arn-tag-resource-surface#task#9: status 400 matches, $.Message missing - expected "1 validation error detected: Value at 'tagKeyList' failed to satisfy constraint: Member must have length greater than or equal to 1", got None.
Cause 5 - kinesisvideo-stream-listing-and-unconfigured-reads#task#7: status 400 matches, $.Message missing - expected "1 validation error detected: Value at 'streamNameCondition.comparisonOperator' failed to satisfy constraint: Member must satisfy enum value set: [BEGINS_WITH]", got None.
Cause 6 - kinesis-video-stream-get-data-endpoint#task#7 (also #task#8 and kinesisvideo-get-data-endpoint#task#8): $.message expected "1 validation error detected: Value at 'aPIName' failed to satisfy constraint: Member must satisfy enum value set: [GET_IMAGES, LIST_FRAGMENTS, GET_DASH_STREAMING_SESSION_URL, GET_HLS_STREAMING_SESSION_URL, GET_MEDIA_FOR_FRAGMENT_LIST, PUT_MEDIA, GET_CLIP, GET_MEDIA]", got "1 validation error detected: Value 'SIM2REAL_NOT_AN_API' at 'aPIName' failed to satisfy constraint: Member must satisfy enum value set: [PUT_MEDIA, GET_MEDIA, LIST_FRAGMENTS, GET_MEDIA_FOR_FRAGMENT_LIST, GET_HLS_STREAMING_SESSION_URL, GET_DASH_STREAMING_SESSION_URL, GET_CLIP, GET_IMAGES]"; $headers.x-amzn-errortype shape_mismatch against 'InvalidArgumentException'. The quoted 'SIM2REAL_NOT_AN_API' is the fixture's request value - the requirement is that no value be echoed at all, whatever it is.
Cause 7 - kinesis-video-stream-tags-lifecycle#task#8 (also kinesisvideo-stream-tags#task#13): expected "... Value at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50", got "... Value '51 entries' at 'tags' failed ...". '51' is this fixture's tag count; the fixed text is everything except the member path and the limit.
Cause 8 - kinesis-video-stream-tags-lifecycle#task#10: expected "... Value at 'streamARN' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z\\d-]+:kinesisvideo:[a-z0-9-]+:[0-9]+:[a-z]+/[a-zA-Z0-9_.-]+/[0-9]+", got the same sentence with the submitted ARN quoted after 'Value' and the pattern rendered as '^...$'.
Cause 9 - kinesisvideo-stream-lifecycle#task#6: identical deviation for a malformed ARN on DescribeStream.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: POST /getDataEndpoint with APIName=PUT_MEDIA and BOTH StreamName and StreamARN set (fixture kinesisvideo-get-data-endpoint, step task#6) - expect 400 InvalidArgumentException with Message 'One of StreamName or StreamARN must be provided, but not both.', observe 200.
3. Cause 2: POST /describeStream naming both StreamName and StreamARN (kinesisvideo-stream-lifecycle#task#5) - observe 200 instead of the 400.
4. Causes 3-5: POST /TagResource with 51 tags (kinesis-video-signaling-channel-tag-resource-surface#task#7), POST /UntagResource with an empty TagKeyList (kinesis-video-stream-arn-tag-resource-surface#task#9), POST /listStreams with StreamNameCondition.ComparisonOperator set to an unmodeled operator (kinesisvideo-stream-listing-and-unconfigured-reads#task#7) - each returns 400 with no Message member in the body.
5. Causes 6-7: POST /getDataEndpoint with an unmodeled APIName (kinesis-video-stream-get-data-endpoint#task#7) and POST /tagStream with 51 tags (kinesis-video-stream-tags-lifecycle#task#8) - read the message and compare to the recorded sentence.
6. Causes 8-9: POST /listTagsForStream with a stream ARN lacking the trailing /<creation-time> segment (kinesis-video-stream-tags-lifecycle#task#10) and POST /describeStream with StreamARN='arn:aws:kinesisvideo:bogus' (kinesisvideo-stream-lifecycle#task#6) - the message quotes the value and prints the anchored pattern.
