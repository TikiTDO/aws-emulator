Issue-Source: trace-sweep-divergence
Issue-Trace: translate-text-translation-job-errors
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: translate-text-translation-job-errors
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate|StartTextTranslationJob|4xx->2xx|$.Message
Issue-Key: translate|StartTextTranslationJob|4xx->2xx|$.__type
Issue-Key: translate|StartTextTranslationJob|4xx->2xx|status
Issue-Key: translate|StartTextTranslationJob|4xx->4xx|status
Issue-Key: translate|DescribeTextTranslationJob|4xx->4xx|$.Message
Issue-Key: translate|DescribeTextTranslationJob|4xx->4xx|status
Issue-Key: translate|StopTextTranslationJob|4xx->4xx|$.Message
Issue-Key: translate|StopTextTranslationJob|4xx->4xx|status
Issue-Commit: 0aa1df021fcab924dd462ae3dfcde463e64681ac

# Translate text-translation-job error paths answer the wrong status and message

> _Note: this issue was identified against the code base at commit `0aa1df021fcab924dd462ae3dfcde463e64681ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTextTranslationJob

## Description
Four net-new causes in the Amazon Translate asynchronous text-translation-job surface (StartTextTranslationJob / DescribeTextTranslationJob / StopTextTranslationJob). They are the same shared fault: the emulator does not reproduce AWS's error identity for the batch-job operations — one precondition is not validated at all (the request succeeds where AWS rejects it), and every other failing precondition is mapped onto the generic ResourceNotFoundException/404 shape instead of the 400 client-request error class and message text AWS emits on the wire. A fixer must close all FOUR before submitting.

This is the same CLASS of defect as the already-open issue 20261029T184304Z-translate-error-responses-use-wrong-status-and-message.md (wrong error class and generic shape-doc message on Translate's terminology/parallel-data paths) and 20261103T092830Z-translate-translatedocument-not-found-error-answers-http-404-where.md (TranslateDocument not-found answering 404), but neither of those issues covers any text-translation-job operation and neither carries these replay keys, so these four causes are unowned and filed here.

Cause 1 of 4 - StartTextTranslationJob with a DataAccessRoleArn that Translate cannot assume: no validation at all, the job is accepted.
AWS: Smithy: StartTextTranslationJob.DataAccessRoleArn is @required, targets IamRoleArn with @length min 20 max 2048 and @pattern ^arn:aws(-[^:]+)?:iam::[0-9]{12}:role/.+$; the operation binds InvalidRequestException (client, httpError 400) alongside InvalidParameterValueException 400, ResourceNotFoundException 404, UnsupportedLanguagePairException 400 and InternalServerException 500. The status and error identity come from the recorded response: HTTP 400 with `__type` = `InvalidRequestException` and body member `Message` = the template `Translate is not authorized to assume role: <roleArn>. Please update the role's trust policy.` Fixed text: `Translate is not authorized to assume role: ` and `. Please update the role's trust policy.`; the single variable part is the `DataAccessRoleArn` taken verbatim from the request. The role arn in the recording is an environment value that fills the template, not part of it.
Emulator: answers HTTP 200 with a StartTextTranslationJobResponse — no `__type`, no `Message` — i.e. it accepts the job and allocates a JobId for a role it cannot use.
Why: a client that submits a batch job with a role whose trust policy is wrong gets a success and a JobId from the emulator, so it proceeds to poll a job AWS would never have created; the authorization defect is masked at submit time instead of surfacing as a 400 InvalidRequestException the caller can act on. Code paths that assert on the trust-policy error are never exercised.
Fix: the StartTextTranslationJob handler's pre-flight validation in the translate service crate — it must check the submitted data-access role against the state's notion of an assumable role and raise InvalidRequestException/400 with the trust-policy message template before any job is recorded in state.
Traces: translate-text-translation-job-errors#task#2.

Cause 2 of 4 - StartTextTranslationJob rejecting a missing referenced resource with HTTP 404 instead of 400.
AWS: Smithy: StartTextTranslationJob binds both ResourceNotFoundException (httpError 404) and the 400 client errors InvalidRequestException / InvalidParameterValueException, so the model alone does not pick the status; the recorded response does: HTTP 400. The asserted body members of the recorded error match what the emulator emits, so the error message/identity is already right — only the HTTP status diverges.
Emulator: answers HTTP 404 for the same request.
Why: a client branching on the HTTP status (or on the status-derived retry/throttle classification in an AWS SDK) treats the response as a resource-not-found rather than the invalid-request class AWS reports, so error handling and conformance assertions on StartTextTranslationJob take the wrong branch even though the body text is right.
Fix: the error-class-to-status mapping used by the StartTextTranslationJob handler's failure path — the job-submit precondition failure must be emitted with the 400 client-request error class, not the 404 ResourceNotFoundException binding.
Traces: translate-text-translation-job-errors#task#3.

Cause 3 of 4 - DescribeTextTranslationJob on a JobId that does not exist: wrong status AND generic message.
AWS: Smithy: DescribeTextTranslationJob.JobId is @required, targets JobId with @length min 1 max 32 and @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$; the operation's modeled errors are InternalServerException 500, ResourceNotFoundException 404 and TooManyRequestsException 429. The model therefore does not state this status — recorded behavior only: AWS answers HTTP 400 with body member `Message` = `Request failed due to job not found.` That message is entirely FIXED text — no part of it is interpolated from the request or from state, and in particular AWS does NOT echo the JobId back.
Emulator: answers HTTP 404 with the generic ResourceNotFoundException prose `The resource you are looking for has not been found. Review the resource you're looking for and see if a different resource will accomplish your needs before retrying the revised request.` — the Smithy shape's own documentation text, not an AWS wire message.
Why: a client polling a job it did not create, or polling after state was lost, sees a 404/ResourceNotFoundException where AWS raises a 400 client error with a job-specific sentence; status-based branching and any assertion on the job-not-found message both diverge, and the generic prose tells the caller nothing about the job.
Fix: the DescribeTextTranslationJob handler's job-lookup miss path — raise the 400 client-request error class with the fixed `Request failed due to job not found.` message instead of falling through to the shared generic 404 ResourceNotFoundException.
Traces: translate-text-translation-job-errors#task#4.

Cause 4 of 4 - StopTextTranslationJob on a JobId that does not exist: same wrong status and generic message.
AWS: Smithy: StopTextTranslationJob.JobId is @required (JobId, @length 1..32, same @pattern); modeled errors are InternalServerException 500, ResourceNotFoundException 404, TooManyRequestsException 429 — again the model does not state the status, so recorded behavior only: HTTP 400 with `Message` = `Request failed due to job not found.`, the identical fully-fixed sentence used by DescribeTextTranslationJob, with no variable parts.
Emulator: answers HTTP 404 with the same generic ResourceNotFoundException shape-doc sentence as Cause 3.
Why: cancelling an unknown or already-reaped job yields the wrong status and error class, so a caller cannot distinguish AWS's job-not-found client error from a missing-resource 404, and idempotent stop/cleanup logic built against AWS misbehaves.
Fix: the StopTextTranslationJob handler's job-lookup miss path — the same 400 error class and fixed message as Cause 3; both operations should share one job-not-found error constructor rather than the generic not-found 404.
Traces: translate-text-translation-job-errors#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, steps_total=39, steps_passed=15, score_pct=38.5). All four causes come from fixture translate-text-translation-job-errors; none carried a stub_path or an owned_by, so none is covered by an existing issue.

Cause 1 - translate-text-translation-job-errors#task#2, StartTextTranslationJob: status expected 400, got 200; $.__type expected 'InvalidRequestException', got None (member absent); $.Message expected "Translate is not authorized to assume role: <dataAccessRoleArn>. Please update the role's trust policy.", got None (member absent). Divergence key service=translate|StartTextTranslationJob|status=400->200|keys=$.Message,$.__type,status.

Cause 2 - translate-text-translation-job-errors#task#3, StartTextTranslationJob: status expected 400, got 404; no body member diverged (keys=status only). Divergence key service=translate|StartTextTranslationJob|status=400->404|keys=status.

Cause 3 - translate-text-translation-job-errors#task#4, DescribeTextTranslationJob: status expected 400, got 404; $.Message expected 'Request failed due to job not found.', got "The resource you are looking for has not been found. Review the resource you're looking for and see if a different resource will accomplish your needs before retrying the revised request." Divergence key service=translate|DescribeTextTranslationJob|status=400->404|keys=$.Message,status.

Cause 4 - translate-text-translation-job-errors#task#5, StopTextTranslationJob: status expected 400, got 404; $.Message expected 'Request failed due to job not found.', got the same generic ResourceNotFoundException sentence as Cause 3. Divergence key service=translate|StopTextTranslationJob|status=400->404|keys=$.Message,status.

The job ids, role arns and account id appearing in the recorded requests/messages are per-environment values that fill the templates; the statuses, the error codes (`InvalidRequestException` for Cause 1) and the message templates above are the requirement. 'Request failed due to job not found.' has no variable part at all.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus) and look at fixture translate-text-translation-job-errors.
3. Cause 1: step task#2 - call StartTextTranslationJob with valid InputDataConfig/OutputDataConfig/SourceLanguageCode/TargetLanguageCodes/ClientToken but a DataAccessRoleArn whose trust policy does not allow translate.amazonaws.com to assume it. Observe HTTP 200 with a JobId; AWS answers HTTP 400, __type=InvalidRequestException, Message="Translate is not authorized to assume role: <roleArn>. Please update the role's trust policy."
4. Cause 2: step task#3 - re-submit StartTextTranslationJob with the fixture's missing-referenced-resource request. Observe HTTP 404; AWS answers HTTP 400 with the same body the emulator already produces, so compare the status only.
5. Cause 3: step task#4 - call DescribeTextTranslationJob with a JobId that was never created. Observe HTTP 404 and the generic ResourceNotFoundException shape-doc sentence; AWS answers HTTP 400 with Message exactly 'Request failed due to job not found.'
6. Cause 4: step task#5 - call StopTextTranslationJob with that same unknown JobId. Observe HTTP 404 + generic sentence; AWS answers HTTP 400 with Message exactly 'Request failed due to job not found.'
7. Re-replay translate-text-translation-job-errors#task#2, #task#3, #task#4 and #task#5 after fixing; all four must pass.
