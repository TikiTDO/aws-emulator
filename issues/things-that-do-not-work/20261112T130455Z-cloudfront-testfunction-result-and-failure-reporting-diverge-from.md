Issue-Source: trace-sweep-divergence
Issue-Trace: cfdeep-test-function-derived-output
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfdeep-test-function-derived-output
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|TestFunction|2xx->2xx|$.TestResult.FunctionErrorMessage
Issue-Key: cloudfront|TestFunction|2xx->2xx|$.TestResult.FunctionOutput
Issue-Key: cloudfront|TestFunction|5xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|TestFunction|5xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|TestFunction|5xx->4xx|$.ErrorResponse.Error.Type
Issue-Key: cloudfront|TestFunction|5xx->4xx|status
Issue-Commit: de066d12162a803aee16eb82b570789675c83c24

# CloudFront TestFunction result and failure reporting diverge from AWS

> _Note: this issue was identified against the code base at commit `de066d12162a803aee16eb82b570789675c83c24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestFunction

## Description
CloudFront TestFunction (POST /2020-05-31/function/{Name}/test) returns a TestResult that is shaped differently from AWS's on both the success and the function-error path, and classifies an unparseable EventObject as a client argument error instead of the modeled server-side test failure. 3 causes, all on the same operation.

Cause 1 of 3 - the TestResult omits FunctionErrorMessage when the test succeeded (fixture cfdeep-test-function-derived-output, step task#2).
AWS: recorded behavior only - the Smithy model marks TestResult.FunctionErrorMessage (target sensitiveStringType, @sensitive) as an optional member documented as "If the result of testing the function was an error, this field contains the error message", so the model does not state presence. The recording of a successful TestFunction (HTTP 200) shows AWS emitting the FunctionErrorMessage member with an EMPTY value rather than leaving it out, i.e. the member is always serialized in the XML TestResult payload and carries the empty string when nothing went wrong.
Emulator: the member is absent from the TestResult payload entirely (replay reports "$.TestResult.FunctionErrorMessage: missing (expected '', got None)"), status 200 otherwise matching.
Why: an SDK or client that reads result.FunctionErrorMessage unconditionally - e.g. to display it, or to length-check it to decide whether the test errored - sees a null/absent field instead of an empty string. Code that distinguishes "no error" (empty) from "field not returned" takes the wrong branch, and strongly typed consumers that expect the element to exist in the XML can fail to map the response.
Fix: the TestFunction response serializer for the TestResult shape - always serialize FunctionErrorMessage, with an empty value when the tested function ran without error, instead of skipping the member when the stored value is empty/None.
Traces: cfdeep-test-function-derived-output#task#2.

Cause 2 of 3 - a function that throws reports the raw runtime message and no FunctionOutput (fixture cfdeep-test-function-derived-output, step task#3).
AWS: recorded behavior only - the model documents FunctionErrorMessage as the error message and FunctionOutput as "The event object returned by the function", with no wording constraint. The recorded 200 response for a function whose handler throws shows AWS reporting the error wrapped in a fixed sentence: the fixed text is "The CloudFront function associated with the CloudFront distribution is invalid or could not run. Error: " followed by the variable part, which is the message the tested function's runtime raised (in this recording the function's own thrown text, taken from the request's function code and EventObject - not a literal requirement). The same recorded response still carries FunctionOutput, serialized as the empty JSON object "{}", because the function produced no event object.
Emulator: FunctionErrorMessage carries only the bare runtime message with none of AWS's fixed prefix (replay: expected 'The CloudFront function associated with the CloudFront distribution is invalid or could not run. Error: sim2real boom on /boom', got 'sim2real boom on /boom'), and FunctionOutput is omitted from the payload altogether ("$.TestResult.FunctionOutput: missing (expected '{}', got None)").
Why: the diagnostic a developer sees while iterating on CloudFront Functions loses the part that identifies WHAT failed (that the function associated with the distribution is invalid or could not run), leaving an unqualified string; and tooling that parses FunctionOutput as JSON on every test run gets null instead of a parseable empty object, so JSON.parse-style handling throws rather than yielding {}.
Fix: the TestFunction handler's result assembly plus the TestResult serializer - prefix the captured runtime error with AWS's fixed sentence when building FunctionErrorMessage, and always emit FunctionOutput ("{}" when the function returned no event object).
Traces: cfdeep-test-function-derived-output#task#3.

Cause 3 of 3 - an EventObject that is not valid JSON is refused 400 InvalidArgument instead of 500 TestFunctionFailed (fixture cfdeep-test-function-derived-output, step task#4).
AWS: Smithy: TestFunction errors include both InvalidArgument (client, httpError 400) and TestFunctionFailed (shape com.amazonaws.cloudfront#TestFunctionFailed, errorKind server, httpError 500, doc "The CloudFront function failed."). The recorded response for a TestFunction whose EventObject body is not parseable is HTTP 500 with ErrorResponse.Error.Type "Receiver", Code "TestFunctionFailed" and Message "The test returned an unexpected result." - that message is entirely fixed text with no variable parts. So AWS routes an event object it cannot run the function against to the server-side test-failure error, not to argument validation.
Emulator: HTTP 400 with Error.Type "Sender", Error.Code "InvalidArgument" and Error.Message "An argument is invalid: the event object is not valid JSON" - wrong status class, wrong fault type, wrong error code, and a message that is not AWS's wording for any of these error classes.
Why: the fault is attributed to the caller rather than to the service, so an SDK's retry and error-classification logic diverges: TestFunctionFailed is a 5xx server error that SDKs treat as retryable/throttle-adjacent and surface as a service exception, while InvalidArgument is a terminal client error. A client with a catch for TestFunctionFailed (the modeled way to detect a failed test) never matches, and a client that aborts on 4xx stops instead of retrying.
Fix: the TestFunction handler's EventObject handling / error mapping - an EventObject that cannot be used to run the function must raise the modeled TestFunctionFailed error (500, Receiver, fixed message "The test returned an unexpected result.") rather than the generic InvalidArgument argument-validation path.
Traces: cfdeep-test-function-derived-output#task#4.

No open issue covers TestFunction: the nearest CloudFront refusal/message issues (20261105T231732Z-cloudfront-refusals-carry-the-wrong-http-status-and.md and 20261105T231732Z-cloudfront-validation-and-pagination-marker-messages-do-not.md) carry other operations' replay keys and do not own any of these three steps.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, /__admin/health 200):

Cause 1 - cfdeep-test-function-derived-output#task#2, TestFunction, expected 200 / actual 200:
  $.TestResult.FunctionErrorMessage: missing (expected '', got None)

Cause 2 - cfdeep-test-function-derived-output#task#3, TestFunction, expected 200 / actual 200:
  $.TestResult.FunctionErrorMessage: value_mismatch (expected 'The CloudFront function associated with the CloudFront distribution is invalid or could not run. Error: sim2real boom on /boom', got 'sim2real boom on /boom')
  $.TestResult.FunctionOutput: missing (expected '{}', got None)
  (fixed text = 'The CloudFront function associated with the CloudFront distribution is invalid or could not run. Error: '; the trailing 'sim2real boom on /boom' is the variable part supplied by the tested function in this recording)

Cause 3 - cfdeep-test-function-derived-output#task#4, TestFunction, expected 500 / actual 400:
  status: expected 500, got 400
  $.ErrorResponse.Error.Type: value_mismatch (expected 'Receiver', got 'Sender')
  $.ErrorResponse.Error.Code: value_mismatch (expected 'TestFunctionFailed', got 'InvalidArgument')
  $.ErrorResponse.Error.Message: value_mismatch (expected 'The test returned an unexpected result.', got 'An argument is invalid: the event object is not valid JSON')

Model grounding (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json):
  TestFunction: POST /2020-05-31/function/{Name}/test, code 200; input Name (@httpLabel, required), IfMatch (If-Match header, required), Stage, EventObject (blob, required, @sensitive, length 0..40960); output TestResult (@httpPayload).
  errors: InvalidArgument(400, client), InvalidIfMatchVersion(400, client), NoSuchFunctionExists(404, client), TestFunctionFailed(500, server), UnsupportedOperation(400, client).
  TestResult members: FunctionSummary, ComputeUtilization, FunctionExecutionLogs, FunctionErrorMessage (sensitiveStringType), FunctionOutput (sensitiveStringType).
  TestFunctionFailed: errorKind server, httpError 500, doc 'The CloudFront function failed.'

## Steps to Reproduce
1. git clone the project and boot the emulator built from the checkout.
2. Replay fixture cfdeep-test-function-derived-output.
3. Step task#1 creates a CloudFront function; step task#2 calls TestFunction with a valid JSON EventObject against a function that runs successfully -> observe the 200 TestResult payload has no FunctionErrorMessage member at all, where AWS emits the member with an empty value.
4. Step task#3 calls TestFunction against a function whose handler throws -> observe FunctionErrorMessage is the bare runtime message without AWS's fixed prefix 'The CloudFront function associated with the CloudFront distribution is invalid or could not run. Error: ', and FunctionOutput is absent instead of '{}'.
5. Step task#4 calls TestFunction with an EventObject that is not valid JSON -> observe HTTP 400 ErrorResponse Error{Type=Sender, Code=InvalidArgument, Message='An argument is invalid: the event object is not valid JSON'} instead of AWS's HTTP 500 ErrorResponse Error{Type=Receiver, Code=TestFunctionFailed, Message='The test returned an unexpected result.'}.
