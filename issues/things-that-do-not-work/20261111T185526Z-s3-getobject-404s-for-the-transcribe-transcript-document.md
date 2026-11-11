Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-polly-speech-transcribed-to-s3-transcript
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-polly-speech-transcribed-to-s3-transcript
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: s3|GetObject|2xx->4xx|$.accountId
Issue-Key: s3|GetObject|2xx->4xx|$.jobName
Issue-Key: s3|GetObject|2xx->4xx|$.results
Issue-Key: s3|GetObject|2xx->4xx|$.status
Issue-Related: 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md
Issue-Related: 20261109T183045Z-s3-getobject-answers-404-nosuchkey-for-the-emr.md
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# S3 GetObject 404s for the Transcribe transcript document the completed job reports

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
1 cause.

Cause 1 of 1: the transcript document that a finished transcription job reports is not in the object store, so GetObject on it answers 404 where AWS answers 200 with the document.
AWS: S3 GetObject on an object that exists answers HTTP 200 with the object's bytes and the usual object metadata headers (content-type, etag, last-modified, accept-ranges: bytes, the checksum/SSE headers the recording carries); for a key that does not exist it answers 404 NoSuchKey. The object in question is the Transcribe transcript JSON written to the job's output location — Transcribe (not the client) writes it when the job completes, and the recording shows GetObject serving it with the documented transcript envelope: jobName, accountId, status 'COMPLETED', and results {transcripts, items, audio_segments}. Beyond the GetObject status/header contract this is recorded behavior only: the transcript document's schema is not in the S3 model.
Emulator: HTTP 404 — no body and none of the object headers — because no transcript object was ever stored at that key.
Why: the cross-service workflow a client actually performs (start a transcription job, poll it, then fetch the transcript from S3) ends in a 404 at the last step, so nothing a client reads the transcription result from works, and the 404 is indistinguishable from a genuine missing-key mistake by the caller.
Fix: not GetObject's own read path — it correctly reports an absent key. The write side is what is missing: the Transcribe service must store the job's transcript document into this emulator's object store at the Transcript.TranscriptFileUri it reports (the transcribe state's job-output write over the in-process S3 seam). In this fixture nothing was stored because the preceding StartTranscriptionJob was refused; that refusal is filed as the transcribe-side issue (title "Transcribe StartTranscriptionJob refuses media uploaded to S3...", same fixture, steps task#4/task#5/teardown#7). This step is filed separately because it is the S3-side divergence with its own replay key and must be re-replayed on its own: fixing the start does not by itself prove the transcript object is written and served.
Traces: transcribe-polly-speech-transcribed-to-s3-transcript#task#6

## Evidence
transcribe-polly-speech-transcribed-to-s3-transcript#task#6, GetObject, expected 200 got 404. Missing from the response: $.jobName (expected the transcription job's name), $.accountId, $.status (expected 'COMPLETED'), $.results (expected {'transcripts': [{'transcript': <text>}], 'items': [...], 'audio_segments': [...]}) and the object headers $headers.accept-ranges ('bytes'), $headers.content-type, $headers.etag, $headers.last-modified, $headers.x-amz-checksum-crc64nvme, $headers.x-amz-checksum-type, $headers.x-amz-server-side-encryption. The bucket, key, account id and transcript text in the recording are environment values; the requirement is that the key the job reports is served with 200 and those headers.

## Steps to Reproduce
1. Run the Polly-to-S3-to-Transcribe workflow of the fixture: upload mp3 audio to a bucket, StartTranscriptionJob on it, poll GetTranscriptionJob to COMPLETED and read Transcript.TranscriptFileUri.
2. GetObject that bucket/key (the transcripts/<job-name>.json location the job reports) — AWS answers 200 with the transcript document and the object headers; the emulator answers 404 because no transcript object exists.
Replay: transcribe-polly-speech-transcribed-to-s3-transcript#task#6.
