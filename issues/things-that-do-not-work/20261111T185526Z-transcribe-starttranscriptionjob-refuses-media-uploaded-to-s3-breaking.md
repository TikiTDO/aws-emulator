Issue-Source: trace-sweep-divergence
Issue-Trace: transcribe-polly-speech-transcribed-to-s3-transcript
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transcribe-polly-speech-transcribed-to-s3-transcript transcribe-vocabulary-filter-lifecycle
Issue-Shape-Id: com.amazonaws.transcribe#Transcribe
Issue-Key: transcribe|StartTranscriptionJob|2xx->4xx|$.TranscriptionJob
Issue-Key: transcribe|StartTranscriptionJob|2xx->4xx|status
Issue-Key: transcribe|GetTranscriptionJob|2xx->4xx|$
Issue-Key: transcribe|GetTranscriptionJob|2xx->4xx|$.TranscriptionJob
Issue-Key: transcribe|GetTranscriptionJob|2xx->4xx|$.TranscriptionJob.TranscriptionJobStatus
Issue-Key: transcribe|GetTranscriptionJob|2xx->4xx|status
Issue-Key: transcribe|DeleteTranscriptionJob|2xx->4xx|status
Issue-Key: transcribe|GetVocabularyFilter|2xx->2xx|$.DownloadUri
Issue-Commit: b37a548368ed88ff8e9aed060c39391f1ed9f4cf

# Transcribe StartTranscriptionJob refuses media uploaded to S3, breaking the job lifecycle, and GetVocabularyFilter DownloadUri names a host the emulator does not serve

> _Note: this issue was identified against the code base at commit `b37a548368ed88ff8e9aed060c39391f1ed9f4cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTranscriptionJob

## Description
Four net-new transcribe divergences. Causes 1-3 are the whole transcription-job lifecycle of one cross-service workflow (speech synthesized to Amazon S3, then transcribed): the start is refused, so the poll never finds a job and the teardown delete is refused too. Cause 4 is independent: the vocabulary-filter DownloadUri is built on a host this emulator is not reached on. 4 causes.

Cause 1 of 4: StartTranscriptionJob is refused (400) for media that was uploaded into this emulator's own Amazon S3.
AWS: Smithy StartTranscriptionJob — input Media (required) with MediaFileUri, TranscriptionJobName @pattern ^[0-9a-zA-Z._-]+$ @length 1-200, output TranscriptionJob, errors BadRequestException(400)/ConflictException(409)/LimitExceededException(429)/InternalFailureException(500); operation doc: "you must first upload your media file into an Amazon S3 bucket; you can then specify the Amazon S3 location of the file using the Media parameter". A start that names an existing S3 object and a valid name/LanguageCode is ACCEPTED: HTTP 200 with TranscriptionJob carrying TranscriptionJobName (the requested name), TranscriptionJobStatus IN_PROGRESS, LanguageCode, MediaFormat, Media.MediaFileUri echoed from the request, and CreationTime/StartTime. The recording shows exactly that shape.
Emulator: HTTP 400 BadRequestException; no TranscriptionJob is returned and no job record is stored (the recorded 200 body is absent entirely). The emulator's own StartTranscriptionJob handler refuses a start only when its cross-service read of the media location answers "no such object" (crate::state::resolve_media_objects -> MediaVerdict::NoSuchObject -> bad_request(crate::state::URI_NAMES_NO_OBJECT), "The URI that you provided doesn't point to an S3 object. Make sure that the object exists and try your request again."), so the refusal means that read reported the media absent even though the preceding steps of the same fixture put it in this emulator's S3 (the earlier S3/Polly steps of the fixture passed).
Why: the canonical documented workflow — upload audio to S3, start a job on it, poll it, fetch the transcript — cannot start at all. Every client of this service begins with this call, and it gets a hard validation error naming a missing object for an object that is present, so the failure also misdiagnoses itself.
Fix: StartTranscriptionJob's media validation in the transcribe handler and the cross-service object-existence seam it calls (crate::state::resolve_media_objects and the in-process read into this emulator's Amazon S3). Hypothesis (stated by neither the model nor the recording, so the fixer must establish which side fails): either the object the preceding synthesis/upload step wrote is not visible to transcribe's read of S3 (wrong bucket/key derivation, wrong region scoping, or a callback that never reaches the S3 service), or that step stored nothing and the write side is at fault. Confirm by reading the same s3://<bucket>/<key> back through S3 GetObject immediately before the start.
Traces: transcribe-polly-speech-transcribed-to-s3-transcript#task#4

Cause 2 of 4: GetTranscriptionJob never reports a job for the name that was started — the poll expires.
AWS: Smithy GetTranscriptionJob — input TranscriptionJobName (required), output TranscriptionJob, errors BadRequestException(400)/NotFoundException(404)/LimitExceededException(429)/InternalFailureException(500). A started job is readable immediately and reaches a terminal status: the recording polls to HTTP 200 with TranscriptionJob.TranscriptionJobStatus COMPLETED, Media.MediaFileUri echoed, LanguageCode, MediaFormat, MediaSampleRateHertz, Settings (ChannelIdentification/ShowAlternatives false), CreationTime/StartTime/CompletionTime, and Transcript.TranscriptFileUri naming the stored transcript document.
Emulator: HTTP 400 BadRequestException with Message "The requested job couldn't be found. Check the job name and try your request again." for every attempt — 19 attempts over the 180s poll cap — because Cause 1 stored no record. Fixed text of that message: the whole sentence pair; variable parts: none (the job name is not interpolated).
Why: a client that starts a job and waits for it hangs to its own timeout and then sees a client-error refusal naming its own job as unknown; a workflow that branches on job status can never progress.
Fix: follows from Cause 1 (the job record must exist and its clock must carry it IN_PROGRESS -> COMPLETED with the transcript location it serves); GetTranscriptionJob's lookup path in the transcribe handler must then find it. Re-replay this step separately — it is a distinct recorded step and passing Cause 1 does not by itself demonstrate the poll completes.
Traces: transcribe-polly-speech-transcribed-to-s3-transcript#task#5

Cause 3 of 4: DeleteTranscriptionJob refuses a delete AWS accepts.
AWS: Smithy DeleteTranscriptionJob — input TranscriptionJobName (required), output Unit, errors BadRequestException(400)/LimitExceededException(429)/InternalFailureException(500). The recorded teardown is HTTP 200 with an empty body: the delete is accepted.
Emulator: HTTP 400 — there is no job under that name to delete (again downstream of Cause 1).
Why: teardown of a workflow fails hard, so cleanup scripts abort and the name is never released; a client cannot distinguish this from real misuse.
Fix: follows from Cause 1 plus DeleteTranscriptionJob's lookup/guard path in the transcribe handler. Re-replay this step separately.
Traces: transcribe-polly-speech-transcribed-to-s3-transcript#teardown#7

Cause 4 of 4: GetVocabularyFilter.DownloadUri points at the real AWS S3 host instead of the endpoint the emulator is reached on.
AWS: Smithy GetVocabularyFilter output DownloadUri -> Uri, @length 1-2000, @pattern ^(s3://|http(s*)://).+$, doc "The Amazon S3 location where the custom vocabulary filter is stored; use this URI to view or download the custom vocabulary filter." The requirement is therefore that the reported location is FETCHABLE and serves the stored filter file; the recording does not pin a literal value (it is shaped-not-asserted), only that the URI resolves to the service under test. Status and shape otherwise match (HTTP 200).
Emulator: HTTP 200 whose DownloadUri is https://s3.<region>.amazonaws.com/aws-transcribe-<region>-prod/<account>/vocabulary-filter/<filter-name> — a host outside this emulator (replay verdict host_not_own). The emulator announces an address other than the one it is reached on, so the filter file it claims to serve at that URI cannot be fetched from it.
Why: "use this URI to view or download" is the member's whole purpose; a client that follows it is sent to real AWS S3 (or to a name it cannot resolve) instead of to the emulator that holds the built filter, so download, round-trip-after-update and any integration that re-reads the stored words all break. Same host defect is reported for both the post-create read and the post-update read of this lifecycle.
Fix: the DownloadUri construction in the transcribe service state (vocabulary_download_uri, which formats "https://s3.{region}.amazonaws.com/{bucket}/{key}") must build the location on this emulator's own externally reachable endpoint — the host/port the request arrived on — keeping the bucket/key it actually stores the file under; the same constructor feeds the vocabulary and medical-vocabulary DownloadUri and the service-managed transcript output URI, so fix it once there.
Traces: transcribe-vocabulary-filter-lifecycle#task#4 (and the second read of the same cluster, transcribe-vocabulary-filter-lifecycle#task#6)

Not covered by any open issue: the open issue 20261029T200807Z-transcribe-refusal-wording-is-reworded-on-four-operations.md owns four transcribe message-wording divergences and one DeleteVocabulary refusal; none of the four causes above is among its replay keys.

## Evidence
Cause 1 (transcribe-polly-speech-transcribed-to-s3-transcript#task#4, StartTranscriptionJob): status expected 200, got 400; $.TranscriptionJob missing — expected {'TranscriptionJobName': '<the started name>', 'TranscriptionJobStatus': 'IN_PROGRESS', 'LanguageCode': 'en-US', 'MediaFormat': 'mp3', 'Media': {'MediaFileUri': 's3://<the bucket the audio was uploaded to>/<key>.mp3'}, 'CreationTime': <shaped>, 'StartTime': <shaped>}, got None. The preceding upload steps of the same fixture passed, so the media object was accepted by this emulator's S3.
Cause 2 (transcribe-polly-speech-transcribed-to-s3-transcript#task#5, GetTranscriptionJob): status expected 200, got 400; body got {'__type': 'BadRequestException', 'Message': "The requested job couldn't be found. Check the job name and try your request again."}; $.TranscriptionJob.TranscriptionJobStatus poll: not in ['COMPLETED'] after 19 attempt(s) (poll capped at 180s), expected COMPLETED with Transcript.TranscriptFileUri, MediaSampleRateHertz and Settings present.
Cause 3 (transcribe-polly-speech-transcribed-to-s3-transcript#teardown#7, DeleteTranscriptionJob): status expected 200, got 400.
Cause 4 (transcribe-vocabulary-filter-lifecycle#task#4 and #task#6, GetVocabularyFilter, 200 -> 200): $.DownloadUri host_not_own — got 'https://s3.us-west-2.amazonaws.com/aws-transcribe-us-west-2-prod/000000000000/vocabulary-filter/<filter-name>'; the expected value is shaped-not-asserted, i.e. any URI the emulator itself serves. The account id, region, filter name and bucket in that string are environment values, not the requirement; the requirement is the HOST.
Source: emulator code path for Cause 1 is services/service-com-amazonaws-transcribe-transcribe/src/ops/start_transcription_job.rs lines 493-501 (resolve_media_objects -> URI_NAMES_NO_OBJECT); for Cause 4 services/service-com-amazonaws-transcribe-transcribe/src/state.rs lines 4012-4026 (vocabulary_download_uri / service_managed_bucket).

## Steps to Reproduce
1. Create an S3 bucket and put an mp3 object in it through this emulator (the fixture synthesizes speech with Polly into s3://<bucket>/<uuid>.mp3); confirm the object reads back with S3 GetObject.
2. StartTranscriptionJob(TranscriptionJobName=<new name>, LanguageCode='en-US', MediaFormat='mp3', Media={MediaFileUri:'s3://<bucket>/<uuid>.mp3'}) — AWS answers 200 with TranscriptionJobStatus IN_PROGRESS; the emulator answers 400 BadRequestException.
3. Poll GetTranscriptionJob(TranscriptionJobName=<that name>) until COMPLETED — AWS reaches COMPLETED with Transcript.TranscriptFileUri; the emulator answers 400 "The requested job couldn't be found..." until the poll cap.
4. DeleteTranscriptionJob(TranscriptionJobName=<that name>) — AWS answers 200 with an empty body; the emulator answers 400.
5. CreateVocabularyFilter(VocabularyFilterName=<new>, LanguageCode='en-US', Words=[...]), then GetVocabularyFilter on it (and again after UpdateVocabularyFilter) and inspect DownloadUri: it names https://s3.<region>.amazonaws.com/... instead of the endpoint the call was made to, so fetching it does not reach this emulator.
Replay: transcribe-polly-speech-transcribed-to-s3-transcript#task#4, #task#5 and #teardown#7, and transcribe-vocabulary-filter-lifecycle#task#4 (plus #task#6).
