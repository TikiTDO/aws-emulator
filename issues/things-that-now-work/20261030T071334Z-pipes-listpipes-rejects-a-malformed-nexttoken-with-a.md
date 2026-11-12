Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-error-notfound-and-validation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-error-notfound-and-validation pipes-list-pipes-pagination
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|4xx->4xx|$.message
Issue-Commit: 533efece7fa11c0e537273065624bf5d01aa80f8

# pipes ListPipes rejects a malformed NextToken with a JSON field-list blob instead of the AWS validation message

> _Note: this issue was identified against the code base at commit `533efece7fa11c0e537273065624bf5d01aa80f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 1 cause.

Cause 1 of 1: ListPipes with a NextToken that is not a token a previous ListPipes returned.
  AWS: ListPipes (Smithy: GET /v1/pipes, NextToken httpQuery NextToken, length 1..2048, sensitive) rejects a malformed continuation token with ValidationException (Smithy: com.amazonaws.pipes#ValidationException, httpError 400, members message + fieldList). The recorded message is a single human-readable sentence pair: the fixed text "Parameter " + the offending input member's name + " is not valid. Reason: " + the reason clause + "." — for a malformed pagination token that renders as "Parameter NextToken is not valid. Reason: Malformed request." Here "Parameter", "is not valid. Reason:" and the trailing "." are fixed text, the member name (NextToken) comes from the request's rejected parameter, and the reason clause is the fixed phrase for this error class. The status (400) and the ValidationException code already match.
  Emulator: 400 ValidationException whose message member is not a sentence at all but a serialized JSON document: {"ValidationErrorFields":[{"Name":"NextToken","Message":"must be a NextToken returned by a previous ListPipes call, passed back unchanged"}]}.
  Why: message is the member every SDK surfaces to the caller and that logs, retries-on-message heuristics and human operators read; a client gets an unreadable embedded JSON blob rather than AWS's sentence, and anything matching on the documented wording (or on ValidationException.fieldList, where AWS puts per-field detail — the emulator instead stuffs its own ad-hoc ValidationErrorFields array into message) does not match. It also leaks an internal field-list schema that is not part of the pipes wire contract.
  Fix: ListPipes' input validation / error-rendering path (services/service-com-amazonaws-pipes-pipes/src/ops/list_pipes.rs) must render the ValidationException message as the AWS sentence for an invalid parameter, and put any per-field detail in the modeled fieldList member rather than inside message.
  Traces: pipes-error-notfound-and-validation#task#6 (also pipes-list-pipes-pagination#task#16).

## Evidence
Replay of simparity-wire-aws-v1, 1 mismatch cluster, 2 affected steps. divergence_key service=pipes|ListPipes|status=400->400|keys=$.message, representative pipes-error-notfound-and-validation#task#6, affected: pipes-error-notfound-and-validation#task#6, pipes-list-pipes-pagination#task#16.
Expected (AWS, recorded): HTTP 400 ValidationException, $.message == "Parameter NextToken is not valid. Reason: Malformed request."
Actual (emulator): HTTP 400 ValidationException, $.message == "{\"ValidationErrorFields\":[{\"Name\":\"NextToken\",\"Message\":\"must be a NextToken returned by a previous ListPipes call, passed back unchanged\"}]}".
Status and error code agree; only the message member diverges. Fixed text: "Parameter ", " is not valid. Reason: Malformed request."; variable part: the rejected parameter's member name, taken from the request (NextToken here).

## Steps to Reproduce
1. Replay fixture pipes-error-notfound-and-validation, step task#6: call ListPipes with a NextToken query parameter that was not produced by a previous ListPipes call.
2. Emulator answers 400 ValidationException with a JSON ValidationErrorFields document inside $.message.
3. AWS answers 400 ValidationException with $.message == "Parameter NextToken is not valid. Reason: Malformed request." (member name from the request); per-field detail belongs in the modeled fieldList member, not in message.
4. The same divergence reproduces at pipes-list-pipes-pagination#task#16.
