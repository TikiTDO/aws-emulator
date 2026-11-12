Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-tag-resource-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-tag-resource-lifecycle opensearchserverless-tag-resource-errors
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|TagResource|2xx->4xx|$
Issue-Key: aoss|TagResource|2xx->4xx|status
Issue-Key: aoss|ListTagsForResource|2xx->4xx|$.tags
Issue-Key: aoss|ListTagsForResource|2xx->4xx|status
Issue-Key: aoss|UntagResource|2xx->4xx|$
Issue-Key: aoss|UntagResource|2xx->4xx|status
Issue-Key: aoss|TagResource|4xx->4xx|$.message
Issue-Key: aoss|ListTagsForResource|4xx->4xx|$.message
Issue-Key: aoss|UntagResource|4xx->4xx|$.message
Issue-Key: aoss|TagResource|4xx->4xx|$.__type
Issue-Commit: a04a180e27a1be47457c995aec938403d1945997

# aoss tagging plane cannot resolve a collection-group ARN and misclassifies a malformed ARN (7 causes)

> _Note: this issue was identified against the code base at commit `a04a180e27a1be47457c995aec938403d1945997`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
SEVEN net-new root causes in the aoss tagging plane, from replaying the wire corpus simparity-wire-aws-v1. Three of them make tagging a collection group impossible at all (an accepted AWS call is refused 400); three reword the not-found refusal; one misclassifies a malformed ARN. Each cause is independent and must be closed on its own.

=== Cause 1 of 7: TagResource cannot resolve a LIVE collection-group ARN, so a collection group can never be tagged ===
AWS: Smithy: TagResource takes resourceArn (Arn, @length 1..1011 — any aoss resource ARN) plus tags (0..50) and models ResourceNotFoundException only for a resource that does NOT exist. Recorded behavior: TagResource on the ARN of a collection group that CreateCollectionGroup just returned answers HTTP 200 with an EMPTY JSON body `{}`.
Emulator: HTTP 400 `{"__type":"ResourceNotFoundException","message":"Resource <collection-group ARN> is not found"}` for a collection group that exists.
Why: no tag can ever be written to a collection group, so tag round-trip, tag-based cost allocation and tag-based access control are unavailable on that resource type, and the caller is told the resource does not exist when it does.
Fix: the tagging plane's resourceArn resolution in the opensearchserverless crate — resolve a collection-group (and every other taggable) ARN to its stored record, not only collection ARNs. NOTE: open issue 20261028T185156Z-opensearchserverless-tagresource-wrong-behavior.md describes this resolution gap from a static read and open behavioral issue 20261029T191222Z-opensearchserverless-10-net-new-causes-in-the-refusal.md (Cause 5) from the behavioral suite; this replay step was NOT marked owned by either, so it is recorded here to be re-replayed and may close with whichever fix lands first.
Traces: opensearchserverless-tag-resource-lifecycle#task#2.

=== Cause 2 of 7: ListTagsForResource cannot read a live collection group's tags ===
AWS: Smithy: ListTagsForResource takes resourceArn and answers tags (list of Tag{key,value}); it models ResourceNotFoundException only for a resource that does not exist. Recorded behavior: after tagging a live collection group, the read answers HTTP 200 with the tag set that was written, in the order written, and after an UntagResource it answers 200 with the remaining tags.
Emulator: HTTP 400 ResourceNotFoundException with the same `Resource <ARN> is not found` body — $.tags is absent, so no tag state is observable for a collection group.
Why: even if tags were written another way, they could not be read back; a client reconciling desired against actual tags on a collection group gets an error rather than a tag list.
Fix: the same resourceArn resolution, on the ListTagsForResource read path.
Traces: opensearchserverless-tag-resource-lifecycle#task#3 (same cluster: opensearchserverless-tag-resource-lifecycle#task#5).

=== Cause 3 of 7: UntagResource cannot remove a tag from a live collection group ===
AWS: Smithy: UntagResource takes resourceArn + tagKeys and answers an empty body. Recorded behavior: removing a key from a live collection group answers HTTP 200 with `{}`.
Emulator: HTTP 400 ResourceNotFoundException `Resource <ARN> is not found`.
Why: a tag can neither be written nor withdrawn, so the whole tag lifecycle of a collection group is unavailable.
Fix: the same resourceArn resolution, on the UntagResource write path.
Traces: opensearchserverless-tag-resource-lifecycle#task#4.

=== Cause 4 of 7: TagResource's not-found message renders the ARN where AWS names the resource KIND and id ===
AWS: recorded behavior (the message is not in the model): for a resourceArn whose resource does not exist, the refusal is HTTP 400 ResourceNotFoundException with `Collection Group ID <id> is not found` — fixed text `Collection Group ID ` + the id PARSED OUT of the request's ARN + ` is not found`. AWS names the resource kind in words and renders only the id, never the whole ARN.
Emulator: `Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found` — the literal ARN from the request, and the generic word `Resource` instead of the kind.
Why: the code is the generic not-found, so the message is the discriminator; a client matching AWS's sentence (or extracting the id from it) fails on the emulator's.
Fix: the not-found refusal message of the TagResource handler.
Traces: opensearchserverless-tag-resource-errors#task#1.

=== Cause 5 of 7: ListTagsForResource's not-found message has the same defect ===
AWS: recorded behavior: HTTP 400 ResourceNotFoundException `Collection Group ID <id> is not found`, the id parsed from the request's ARN.
Emulator: `Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found`.
Why: same as Cause 4 — a read against a missing resource reports an unrecognisable sentence.
Fix: the not-found refusal message of the ListTagsForResource handler.
Traces: opensearchserverless-tag-resource-errors#task#2 (same cluster: opensearchserverless-tag-resource-lifecycle#teardown#7).

=== Cause 6 of 7: UntagResource's not-found message has the same defect ===
AWS: recorded behavior: HTTP 400 ResourceNotFoundException `Collection Group ID <id> is not found`.
Emulator: `Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found`.
Why: same as Cause 4.
Fix: the not-found refusal message of the UntagResource handler.
Traces: opensearchserverless-tag-resource-errors#task#3.

=== Cause 7 of 7: a malformed resourceArn is reported as a missing resource instead of a validation failure ===
AWS: Smithy: resourceArn targets Arn and TagResource models both ValidationException and ResourceNotFoundException. Recorded behavior: a resourceArn that is not an ARN at all is refused HTTP 400 with __type `com.amazon.aoss#ValidationException` and message `Resource ARN: <resourceArn> is invalid` — fixed text `Resource ARN: ` + the request's resourceArn verbatim + ` is invalid`.
Emulator: HTTP 400 with __type `ResourceNotFoundException` and message `Resource <resourceArn> is not found`.
Why: the two conditions are not interchangeable — a malformed ARN is a caller bug that must never be retried and is surfaced to the user, while a not-found is a state condition a client may poll or treat as already-deleted. The emulator sends every malformed-ARN caller down the not-found branch.
Fix: the tagging plane's resourceArn parsing — refuse a syntactically invalid ARN with ValidationException and that message, before any state lookup (applies equally to UntagResource and ListTagsForResource).
Traces: opensearchserverless-tag-resource-errors#task#4.

## Evidence
All seven clusters come from the simparity-wire-aws-v1 replay.
Cause 1 — opensearchserverless-tag-resource-lifecycle#task#2 TagResource: expected status 200 with body {}; got 400 {'__type': 'ResourceNotFoundException', 'message': 'Resource arn:aws:aoss:<region>:<account>:collection-group/<id> is not found'} for a group created earlier in the same fixture (the id in the recording is that run's minted value).
Cause 2 — opensearchserverless-tag-resource-lifecycle#task#3 ListTagsForResource: expected status 200 with $.tags [{'key': 'env', 'value': 'probe'}, {'key': 'team', 'value': 'search'}]; got 400 and $.tags missing. Same cluster: #task#5 (the read after UntagResource).
Cause 3 — opensearchserverless-tag-resource-lifecycle#task#4 UntagResource: expected status 200 with body {}; got 400 ResourceNotFoundException with the same 'Resource <ARN> is not found' body.
Cause 4 — opensearchserverless-tag-resource-errors#task#1 TagResource: $.message expected 'Collection Group ID <id> is not found'; got 'Resource arn:aws:aoss:us-west-2:<account>:collection-group/<id> is not found'.
Cause 5 — opensearchserverless-tag-resource-errors#task#2 ListTagsForResource: same expected/actual pair; cluster also covers opensearchserverless-tag-resource-lifecycle#teardown#7.
Cause 6 — opensearchserverless-tag-resource-errors#task#3 UntagResource: same expected/actual pair.
Cause 7 — opensearchserverless-tag-resource-errors#task#4 TagResource with a non-ARN string: $.__type expected 'com.amazon.aoss#ValidationException', got 'ResourceNotFoundException'; $.message expected 'Resource ARN: <the request's resourceArn> is invalid', got 'Resource <the request's resourceArn> is not found' (the probe string itself is the request's value; the fixed text around it is the requirement).

## Steps to Reproduce
Boot the emulator from this checkout and replay the aoss wire fixtures, or with an SDK:
Causes 1-3: create_collection_group(name=<g>), then tag_resource(resourceArn=<the ARN the create returned>, tags=[{'key':'env','value':'probe'},{'key':'team','value':'search'}]) -> expect 200 {}; list_tags_for_resource(resourceArn=<same>) -> expect 200 with both tags in the order written; untag_resource(resourceArn=<same>, tagKeys=['team']) -> expect 200 {}; list_tags_for_resource again -> expect 200 with the remaining tag.
Causes 4-6: tag_resource / list_tags_for_resource / untag_resource on 'arn:aws:aoss:<region>:<account>:collection-group/<40 chars of [a-z0-9] never created>' -> inspect the message.
Cause 7: tag_resource(resourceArn='not-a-real-arn', tags=[{'key':'k','value':'v'}]) -> inspect the error code and message.
