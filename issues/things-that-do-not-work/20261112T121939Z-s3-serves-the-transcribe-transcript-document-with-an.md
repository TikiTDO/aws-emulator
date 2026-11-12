Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-polly-speech-transcribed-to-s3-transcript
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-polly-speech-transcribed-to-s3-transcript
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: s3|GetObject|2xx->2xx|$.results.audio_segments
Issue-Key: s3|GetObject|2xx->2xx|$.results.items[*]
Issue-Key: s3|GetObject|2xx->2xx|$.results.transcripts[*].transcript
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Commit: 4280f890470fc3016a7f7b2f270013172addcc68

# S3 serves the Transcribe transcript document with an empty transcript and no items or audio_segments

> _Note: this issue was identified against the code base at commit `4280f890470fc3016a7f7b2f270013172addcc68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
1 cause.

Cause 1 of 1: GetObject on the transcript document a COMPLETED transcription job reports now answers 200, but the document it serves holds an empty transcript and none of the per-token structure, so the last step of the cross-service workflow returns a transcript with no words in it.
AWS: recorded behavior only for the document's schema — the Transcribe transcript JSON is not modelled in the S3 model; S3's own contract here is that GetObject on an existing key answers 200 with the stored bytes and the object's metadata headers. The recording shows the document Transcribe itself writes to the job's output location: an envelope (jobName, accountId, status 'COMPLETED') plus `results` holding (a) `transcripts[0].transcript`, the full spoken text of the media as one string, (b) `items[]`, one entry per token in order with `id`, `type` ('pronunciation' or 'punctuation'), `alternatives[{confidence, content}]` and, for pronunciations, `start_time`/`end_time` as second-valued strings, and (c) `audio_segments[]`, the segment breakdown over those items. The cluster's diverging-key list also names $headers.content-type and $headers.x-amz-checksum-crc64nvme, i.e. the object is served as a JSON document with a checksum header matching the bytes. The transcript text, token confidences, times, bucket, key, job name and account id in the recording are this run's values and are not themselves the requirement; the requirement is that the transcript text is the speech in the media the job processed and that the items/audio_segments arrays are present and consistent with it.
Emulator: 200, but $.results.transcripts[0].transcript is '' (expected the spoken text) and $.results.items[0..9] and $.results.audio_segments are missing entirely (got None), along with the two headers above. This is deliberate in the current design, not an accident: the transcribe state's module header (services/service-com-amazonaws-transcribe-transcribe/src/state.rs, lines ~41-54) states the output document is stored at the reported location "with an EMPTY transcript and no caption, because no engine read the audio".
Why: the canonical workflow a client performs — start a transcription job, poll it to COMPLETED, fetch Transcript.TranscriptFileUri — now completes with a 200 but yields no transcription result, so nothing built on reading the transcript works. Worse than the empty string: a client that walks `results.items` or `results.audio_segments` (the normal way to get word timings, confidences or segments) indexes members that are absent rather than empty, and fails with a key/null error instead of reading an empty transcript. An AWS-shaped empty result still carries those arrays.
Fix: not GetObject's read path, which correctly serves the stored object. The write side is the defect: the transcript-document builder on the transcribe side (the `transcript_document` / `store_job_outputs` path reached from the transcription-job lifecycle in services/service-com-amazonaws-transcribe-transcribe/src/state.rs) must render the words of the media the job pointed at into `results.transcripts[0].transcript` and emit `results.items` and `results.audio_segments` consistent with that text, and must store the object with the JSON content type and a checksum header matching the bytes it writes. Related open issue "S3 GetObject 404s for the Transcribe transcript document the completed job reports" (same fixture and step) recorded the earlier state in which NO object was stored at all and the step answered 404; it does not cover this divergence, which is a 200 serving an empty document, and the replay reports this cluster as owned by no open issue.
Traces: transcribe-polly-speech-transcribed-to-s3-transcript#task#6.

## Evidence
transcribe-polly-speech-transcribed-to-s3-transcript#task#6, GetObject, expected 200 and got 200, verdict mismatch. Diverging keys: $.results.transcripts[0].transcript value_mismatch (expected the spoken text of the media — 'The quick brown fox jumps over the lazy dog.' in this recording — got ''); $.results.items[0] .. $.results.items[9] all missing (expected per-token entries such as {'id': 0, 'type': 'pronunciation', 'alternatives': [{'confidence': '0.971', 'content': 'The'}], 'start_time': '0.04', 'end_time': '0.24'} and a trailing {'id': 9, 'type': 'punctuation', 'alternatives': [{'confidence': '0.0', 'content': '.'}]}, got None); $.results.audio_segments missing (got None); and, per the cluster's key list, $headers.content-type and $headers.x-amz-checksum-crc64nvme. The particular words, confidence values and times are this run's media and are not the requirement — the requirement is a transcript of the media the job processed with the items/audio_segments arrays present and consistent with it.

## Steps to Reproduce
1. Upload the audio to a bucket, call StartTranscriptionJob on it with an OutputBucketName/OutputKey, and poll GetTranscriptionJob until TranscriptionJobStatus is COMPLETED.
2. GetObject the bucket/key the job reports in Transcript.TranscriptFileUri (transcripts/<job-name>.json).
3. The emulator answers 200 with {'jobName': ..., 'accountId': ..., 'status': 'COMPLETED', 'results': {'transcripts': [{'transcript': ''}]}} — the transcript string is empty and 'items' and 'audio_segments' are absent, where AWS serves the transcript of the media together with both arrays, under the JSON content type and a checksum header.
Replay: transcribe-polly-speech-transcribed-to-s3-transcript#task#6.
