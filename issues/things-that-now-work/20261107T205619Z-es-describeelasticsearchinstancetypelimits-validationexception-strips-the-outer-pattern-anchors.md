Issue-Source: trace-sweep-divergence
Issue-Trace: elasticsearch-service-domain-validation-and-instance-type-limits-errors
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticsearch-service-domain-validation-and-instance-type-limits-errors
Issue-Shape-Id: com.amazonaws.elasticsearchservice#AmazonElasticsearchService2015
Issue-Key: es|DescribeElasticsearchInstanceTypeLimits|4xx->4xx|$.message
Issue-Commit: f1f9f5b66979f27b8680cd8e6119d18c6179a071

# es DescribeElasticsearchInstanceTypeLimits ValidationException strips the outer @pattern anchors from the elasticsearchVersion refusal message

> _Note: this issue was identified against the code base at commit `f1f9f5b66979f27b8680cd8e6119d18c6179a071`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeElasticsearchInstanceTypeLimits

## Description
1 net-new root cause on the `es` service token (the Elasticsearch Service / OpenSearch Service control plane served on endpoint prefix `es`), on the legacy 2015-01-01 instance-type-limits path. The status (400) and the error class (ValidationException) are right; the rendered `message` text is not AWS's, because the constraint formatter trims the first and last character of the quoted @pattern when they are `^` and `$`. On a multi-alternative pattern that mangles the expression rather than merely unanchoring it: only the FIRST alternative loses its `^` and only the LAST loses its `$`, while the interior anchors survive, so the emulator quotes a regex that is not a de-anchored form of the trait and is not any expression the service states.

Cause 1 of 1 — DescribeElasticsearchInstanceTypeLimits with an elasticsearchVersion that violates the member @pattern (fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors, step task#2).

AWS: Smithy (com.amazonaws.elasticsearchservice): DescribeElasticsearchInstanceTypeLimits GET /2015-01-01/es/instanceTypeLimits/{ElasticsearchVersion}/{InstanceType}; input ElasticsearchVersion @httpLabel @required targeting ElasticsearchVersionString @pattern("^[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}$"); errors ValidationException (client, HTTP 400), BaseException (client, 400, defaulted), InvalidTypeException, LimitExceededException, ResourceNotFoundException, InternalException. The recording shows the standard aggregate constraint template: `1 validation error detected: Value '<submitted version>' at 'elasticsearchVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`. Fixed text: `1 validation error detected: `, `Value '<value>' at '<member path>' failed to satisfy constraint: `, `Member must satisfy regular expression pattern: `. Variable parts: the echoed value (from the request URI label), the member path (`elasticsearchVersion`, lower-camel) and the quoted pattern — which is taken from the model trait and reproduced EXACTLY as the trait writes it, every alternative anchored: `^[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}$`.

Emulator: answers 400 ValidationException with the same template and the same member path, but quotes the pattern as `[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}` — the leading `^` of the first alternative and the trailing `$` of the last alternative are gone, the three interior anchors are kept. The replay reports `$.message` as the ONLY diverging key (status 400 and the error class agree).

Why: this message is the only place the API states the accepted engine-version syntax. A client, SDK test or human that compiles the quoted expression to pre-validate versions gets a regex that accepts values AWS refuses (anything merely ENDING in a bare `M.NN`, and any string whose tail matches `OS_<n>.<nn>` without terminating there) while its first alternative is unanchored at the start — i.e. an accept set that is neither AWS's nor a consistent de-anchored variant. Any assertion on the refusal text fails, and a user shown the message is told a syntax the service does not use.

Fix: the @pattern branch of the shared constraint-violation message formatter in this service's request validation, reached from the legacy 2015-01-01 httpLabel validation entry point for this operation — it must render the trait verbatim instead of trimming outer `^`/`$` characters. NOTE that the correct behaviour is per-member accurate rendering, not a blanket keep-or-strip: open issue 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md documents that the `domainName` member is quoted by AWS WITHOUT anchors on both the 2015-01-01 and 2021-01-01 domain paths, so a global 'always keep anchors' change regresses those steps.

RELATED OPEN WORK (does NOT cover this replay key — neither issue names fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors or step task#2 in its Issue-Trace/Issue-Trace-Ids/Issue-Key lines): 20261101T193857Z-es-validationexception-message-text-diverges-pattern-anchors-stripped.md (the same anchor-stripping defect on the VpcEndpoint id members and the modern DescribeInstanceTypeLimits instanceType enum set) and 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md (domainName/enum rendering and validation ordering). This step is a distinct operation (the legacy DescribeElasticsearchInstanceTypeLimits), a distinct member (elasticsearchVersion) and the first case where the trimming corrupts an ALTERNATION pattern rather than just dropping its outer anchors.

The submitted version string in the recording ('99.99') is this recording's input filling the template; the requirement is the template's fixed text plus the model trait it quotes.

Traces: elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#2.

## Evidence
Cause 1 — elasticsearch-service-domain-validation-and-instance-type-limits-errors#task#2, DescribeElasticsearchInstanceTypeLimits, expected status 400 / actual status 400, diverging key $.message only (divergence_key service=es|DescribeElasticsearchInstanceTypeLimits|status=400->400|keys=$.message, affected_count 1):
  expected "1 validation error detected: Value '99.99' at 'elasticsearchVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}$"
  got      "1 validation error detected: Value '99.99' at 'elasticsearchVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}"

Diff is exactly the first character (`^`) and the last character (`$`) of the quoted expression; the interior `$|^` anchors are rendered.

Smithy cited: com.amazonaws.elasticsearchservice#DescribeElasticsearchInstanceTypeLimits — GET /2015-01-01/es/instanceTypeLimits/{ElasticsearchVersion}/{InstanceType}; input member ElasticsearchVersion @httpLabel @required -> ElasticsearchVersionString @pattern("^[0-9]{1}\\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\\.[0-9]{1,2}$|^OS_[0-9]{1,2}\\.[0-9]{1,2}$"); modeled errors include ValidationException (client, httpError 400).

Fixed text of the message: "1 validation error detected: Value '" … "' at '" … "' failed to satisfy constraint: Member must satisfy regular expression pattern: " … . Values from the request/model: the echoed version string ('99.99' here, taken from the request URI label), the member path 'elasticsearchVersion', and the pattern string, taken verbatim from the @pattern trait. The error count (1) is derived from the number of violated constraints.

## Steps to Reproduce
1. Replay fixture elasticsearch-service-domain-validation-and-instance-type-limits-errors, step task#2: GET /2015-01-01/es/instanceTypeLimits/99.99/<an instance type> — i.e. an ElasticsearchVersion label that violates the member @pattern.
2. Observe HTTP 400 with error class ValidationException (both correct) and diff the `message` body member against the recording.
3. The quoted regular expression must be the @pattern trait verbatim — `^[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$|^OS_[0-9]{1,2}\.[0-9]{1,2}$` — with the leading `^` and trailing `$` present. The emulator currently drops both.
4. Re-check that the domainName refusal texts owned by 20261102T091103Z-es-request-validation-diverges-refusal-text-on-domain.md still render UNANCHORED after the fix, so the change is per-member and not a blanket anchor policy.
5. This step must pass before the issue is closed.
