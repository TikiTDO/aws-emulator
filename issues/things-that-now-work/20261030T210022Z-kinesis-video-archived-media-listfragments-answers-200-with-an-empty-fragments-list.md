Issue-Source: trace-sweep-divergence
Issue-Trace: kinesisvideoarchivedmedia-list-fragments-addressing-and-validation
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesisvideoarchivedmedia-list-fragments-addressing-and-validation
Issue-Shape-Id: com.amazonaws.kinesisvideoarchivedmedia#AWSAcuityReader
Issue-Key: kinesis-video-archived-media|ListFragments|4xx->2xx|$
Issue-Key: kinesis-video-archived-media|ListFragments|4xx->2xx|$.Message
Issue-Key: kinesis-video-archived-media|ListFragments|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: kinesis-video-archived-media|ListFragments|4xx->2xx|status
Issue-Commit: c8ee6f0e730dbbe032dcd7cd7a48f89a4679056d

# ListFragments answers 200 with an empty Fragments list for a stream that is not found or not active instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `c8ee6f0e730dbbe032dcd7cd7a48f89a4679056d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFragments

## Description
One net-new root cause found by the wire corpus against the kinesis-video-archived-media (KVAM) data plane. It is exercised by two divergence clusters in the same fixture (a polling form and a single-shot form), and a fixer must re-replay BOTH of the bindings listed under Traces to confirm the fix.

Cause 1 of 1: ListFragments does not resolve the addressed stream against Kinesis Video Streams state before producing a result, so a request naming a stream that does not exist (or exists but is not ACTIVE) is answered as a successful, empty listing instead of being refused as not-found.

AWS: Smithy ListFragments (POST /listFragments, protocol code 200) takes the optional, mutually exclusive stream identifiers StreamName (@length 1..256, @pattern ^[a-zA-Z0-9_.-]+$) and StreamARN (@length 1..1024, @pattern ^arn:[a-z\d-]+:kinesisvideo:...$), and models ResourceNotFoundException (errorKind client, httpError 404) alongside InvalidArgumentException (400), ClientLimitExceededException (400) and NotAuthorizedException (401); ResourceNotFoundException carries a single Message member (target ErrorMessage). When the addressed stream cannot be resolved — it does not exist, or it is not in a readable/active state — the archived-media read is refused, not satisfied: HTTP 404, errorType header ResourceNotFoundException, body {"Message": "The requested stream is not found or not active."}. The message is entirely FIXED text — it has no variable part, and in particular it does not echo the StreamName or StreamARN from the request. Only the 404 status, the ResourceNotFoundException code (x-amzn-errortype) and that sentence are the requirement; the stream name used by any one recording is not. (Recorded behavior confirms this status/code/message for the unresolvable-stream case; the Smithy model supplies the error binding and its 404.)

Emulator: answers HTTP 200 with body {"Fragments": []} — a well-formed empty success page. No Message member is emitted, and no x-amzn-errortype header is set. The stream identifier in the request is therefore never checked against the Kinesis Video Streams registry at all: an unresolvable stream and a resolvable stream holding no fragments in the selected window are rendered identically. The polling form of the step retried for 61 attempts over the 60 s cap and never observed any error code, because the emulator keeps answering 200 indefinitely.

Why: 200 + empty Fragments is the ordinary 'no media in this window yet' answer, so a client that must distinguish 'this stream does not exist / is not active yet' (create it, or wait for ACTIVE, or fail fast on a typo in StreamName/StreamARN) from 'the stream is fine, keep polling for fragments' takes the wrong branch every time. Consumers that poll ListFragments until fragments appear poll forever against a stream that will never produce any, and misspelled or deleted stream identifiers are silently accepted as valid — an overly-permissive failure that lets emulated tests pass against a configuration real AWS refuses with a 404.

Fix: the ListFragments handler's stream-resolution step in the KVAM data plane (and the shared KVAM stream-resolution helper the other stream-addressed reads use) — resolve StreamName/StreamARN against the shared Kinesis Video Streams registry and check readable/active state BEFORE the fragment-index lookup, and on failure serialize ResourceNotFoundException with HTTP 404, the x-amzn-errortype header and the fixed Message sentence, rather than falling through to an empty Fragments page. Note the related open issues 20261029T202349Z-kinesis-video-archived-media-data-plane-streams-never.md (behavioral corpus; its cause 1 is the OPPOSITE symptom — GetClip/GetHLSStreamingSessionURL wrongly 404 a stream that DOES exist, and its cause 4 is ListFragments' addressing-refusal wording) and 20261029T235218Z-kinesis-video-archived-media-getimages-returns-notimplemented-501-instead-of-resourcenotfoundexception-so.md (GetImages). Neither owns these two replay steps and neither describes ListFragments answering 200 for an unresolvable stream; replay reported owned_by=[] and no stub for both clusters.

Traces: kinesisvideoarchivedmedia-list-fragments-addressing-and-validation#task#11 (polling form, representative) and kinesisvideoarchivedmedia-list-fragments-addressing-and-validation#task#12 (single-shot form).

## Evidence
Fixture kinesisvideoarchivedmedia-list-fragments-addressing-and-validation, step task#11 (verdict mismatch, service kinesis-video-archived-media, operation ListFragments): status expected 404, got 200; $.Message expected 'The requested stream is not found or not active.', got None (member missing); $headers.x-amzn-errortype expected to be present (shaped, not asserted), got None; the step's poll was capped at 60 s and reported "error code None not in ['ResourceNotFoundException'] after 61 attempt(s)" — expected body {'Message': 'The requested stream is not found or not active.'}, actual body {'Fragments': []}.

Same fixture, step task#12 (the non-polling form of the same call): status expected 404, got 200; $.Message missing (expected 'The requested stream is not found or not active.', got None); $headers.x-amzn-errortype missing.

Requirement vs per-run values: the 404 status, the ResourceNotFoundException error code in x-amzn-errortype, and the fixed message text 'The requested stream is not found or not active.' are the requirement. The stream identifier each recording sends, and the 61-attempt/60 s poll numbers, are per-run and are not.

Smithy citations: ListFragments POST /listFragments, errors [ClientLimitExceededException 400, InvalidArgumentException 400, NotAuthorizedException 401, ResourceNotFoundException 404]; ResourceNotFoundException structure, errorKind client, httpError 404, members {Message: ErrorMessage}; input StreamName @length 1..256 @pattern ^[a-zA-Z0-9_.-]+$, StreamARN @length 1..1024; output Fragments (FragmentList), NextToken.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
2. Replay fixture kinesisvideoarchivedmedia-list-fragments-addressing-and-validation, step task#11: it calls POST /listFragments against the KVAM data plane addressing a stream that is not found or not active, and polls for an error code in ['ResourceNotFoundException']. The emulator returns HTTP 200 {"Fragments": []} on every attempt, so the poll exhausts its 60 s cap with error code None.
3. Replay step task#12 of the same fixture: the same ListFragments call made once, asserting HTTP 404 with body {"Message": "The requested stream is not found or not active."} and an x-amzn-errortype header of ResourceNotFoundException. The emulator again answers HTTP 200 {"Fragments": []} with no Message member and no x-amzn-errortype header.
4. Expected on both steps: HTTP 404, x-amzn-errortype: ResourceNotFoundException, body {"Message": "The requested stream is not found or not active."}.
