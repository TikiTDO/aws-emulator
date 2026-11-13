Issue-Source: trace-sweep-divergence
Issue-Trace: invoicing-list-invoice-summaries-and-pdf
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: invoicing-list-invoice-summaries-and-pdf
Issue-Shape-Id: com.amazonaws.invoicing#Invoicing
Issue-Key: invoicing|GetInvoicePDF|4xx->4xx|$.message
Issue-Commit: 42b0d911cbfe674c0ad13877c2fae9531eb43724

# GetInvoicePDF returns a generic ResourceNotFoundException message instead of naming the missing invoice

> _Note: this issue was identified against the code base at commit `42b0d911cbfe674c0ad13877c2fae9531eb43724`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetInvoicePDF

## Description
One cause.

Cause 1 of 1: GetInvoicePDF on an InvoiceId that no invoice is stored under answers the correct modeled error with the WRONG message text.

AWS: Smithy (services/service-com-amazonaws-invoicing-invoicing/smithy-model/com-amazonaws-invoicing-invoicing.json) models GetInvoicePDF with error ResourceNotFoundException @httpError 404, whose `message` member is a BasicString (len 0..1024, pattern ^[\s\S]*$); the shape doc is only "The resource could not be found." The model does not state the wording of the message for this operation, so the requirement comes from the recording: AWS words this error class as "Invoice <InvoiceId> does not exist" — fixed text "Invoice " + " does not exist", with the single variable part being the InvoiceId taken verbatim from the request (recorded example: InvoiceId `does-not-exist-invoice-000` yielding `Invoice does-not-exist-invoice-000 does not exist`). The recorded status is 404 and the recorded error shape is ResourceNotFoundException. Recorded behavior only for the wording; the id in the recording is a request value filling the template, not part of it.

Emulator: status 404 and the ResourceNotFoundException shape are correct, but `$.message` is the constant `The resource could not be found.` — the generic shape documentation string, carrying no reference to the requested invoice. Replay detail: `$.message: value_mismatch (expected 'Invoice does-not-exist-invoice-000 does not exist', got 'The resource could not be found.')`.

Why: a client relying on AWS's behavior cannot tell WHICH identifier was rejected. Callers that fan out GetInvoicePDF over a list of invoice ids, log the failure for operators, or regex/parse the message to extract the offending invoice id (a common pattern when the SDK surfaces only the message) get no usable signal; the emulator's constant message is indistinguishable across every missing invoice and across every other not-found resource in this service, so error-path tests and triage tooling that pass against AWS fail or silently mis-attribute against the emulator.

Fix: belongs in the GetInvoicePDF handler's not-found error construction in the invoicing service crate — the 404 branch builds its exception with the service's unnamed/generic not-found helper (`crate::state::no_such_resource_unnamed()`), which cannot embed the requested identifier. It must instead construct ResourceNotFoundException with a message rendered from the request's InvoiceId using the "Invoice {InvoiceId} does not exist" template (a resource-naming helper in `crate::state`, parameterized by the identifier, is the natural home). Status 404 and the error shape already match and must not change.

Traces: fixture `invoicing-list-invoice-summaries-and-pdf` step `task#3` (GetInvoicePDF for an unstored InvoiceId). Code: services/service-com-amazonaws-invoicing-invoicing/src/ops/get_invoice_pdf.rs lines 179-187 (the `else` branch returning `ResourceNotFoundException(crate::state::no_such_resource_unnamed())`).

Related open issues on this service that do NOT cover this message defect: 20261109T113852Z-invoicing-listinvoicesummaries-refuses-a-filtered-query-with-validationexception-instead.md (same fixture, step task#2, ListInvoiceSummaries status divergence) and 20261108T050356Z-invoicing-batchgetinvoiceprofile-answers-the-requester-s-own-account-instead.md.

## Evidence
Fixture `invoicing-list-invoice-summaries-and-pdf`, step `task#3`, operation GetInvoicePDF.

Expected (recorded AWS response): HTTP 404, ResourceNotFoundException, `$.message` = "Invoice does-not-exist-invoice-000 does not exist" — fixed text "Invoice " ... " does not exist" wrapping the InvoiceId sent in the request.

Actual (emulator): HTTP 404, ResourceNotFoundException, `$.message` = "The resource could not be found." — a constant with no request-derived part.

Replay verdict: mismatch; divergence_key `service=invoicing|GetInvoicePDF|status=404->404|keys=$.message`. Status and error code agree; only the message text diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `invoicing-list-invoice-summaries-and-pdf`.
2. At step `task#3`, call invoicing GetInvoicePDF with an InvoiceId that no invoice is stored under (the fixture uses `does-not-exist-invoice-000`; any unstored id reproduces it).
3. Observe the 404 ResourceNotFoundException body: `message` is the constant "The resource could not be found." instead of "Invoice <the InvoiceId you sent> does not exist".
