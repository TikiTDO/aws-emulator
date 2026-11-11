Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.crawler-metrics-reflect-run-state glue.delete-crawler-while-running-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|StartCrawler|EntityNotFoundException
Issue-Key: glue.delete-crawler-while-running-rejected|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 41e8aa3f6ef6702105d7e1e39f8f06aa9334e793

# glue: 2 net-new causes — StartCrawler on a deleted crawler returns GetCrawler's refusal message, and DeleteCrawler on a RUNNING crawler returns an emulator-authored CrawlerRunningException message

> _Note: this issue was identified against the code base at commit `41e8aa3f6ef6702105d7e1e39f8f06aa9334e793`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Two net-new root causes (Cause 1 of 2, Cause 2 of 2) in Glue's crawler refusal messages. Each cause has its own failing check id; both must be closed before this issue is submitted.

Cause 1 of 2 — StartCrawler for a crawler that does not exist answers with GetCrawler's wording instead of its own. Failing check: glue.crawler-metrics-reflect-run-state.
AWS: Smithy: StartCrawler declares EntityNotFoundException (client error, HTTP 400) alongside CrawlerRunningException and OperationTimeoutException. Recorded behavior fixes the message per operation: GetCrawler for an absent crawler says 'Crawler entry with name <name> does not exist', while StartCrawler for the same absent crawler says 'Crawler with name <name> does not exist'. In both the only variable part is <name>, the Name from the request; the rest is fixed text, and the two operations' fixed texts differ ('Crawler entry with name' vs 'Crawler with name').
Emulator: StartCrawler returns the correct code (EntityNotFoundException) but GetCrawler's text — 'Crawler entry with name <name> does not exist' — for the StartCrawler call, i.e. one shared message is used for both operations.
Why: a client that keys on the refusal text (log assertions, runbooks, error-matching in a retry wrapper, test fixtures recorded against AWS) sees the wrong sentence for StartCrawler; the message is part of the API contract for this error class.
Fix: the StartCrawler handler's not-found refusal must author its own message rather than reusing the crawler-lookup/GetCrawler one.
Traces: glue.crawler-metrics-reflect-run-state; services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs (and the shared crawler-lookup helper it borrows the message from).

Cause 2 of 2 — DeleteCrawler while the crawler is RUNNING refuses with the wrong message. Failing check: glue.delete-crawler-while-running-rejected.
AWS: Smithy: DeleteCrawler declares CrawlerRunningException (client error, HTTP 400). Recorded behavior: the message is "Cannot delete crawler<name> while it's running" — fixed text 'Cannot delete crawler' immediately followed, with no separating space, by the crawler Name from the request, then the fixed text " while it's running". The missing space is AWS's own wording, not a transcription slip, and must be reproduced; the effect is absent (the crawler survives, and DeleteCrawler succeeds once the crawler is READY again).
Emulator: the code is right (CrawlerRunningException) but the message is emulator-authored: 'Crawler with name <name> is running'.
Why: same contract break as Cause 1 — a client matching the documented refusal text for a delete-while-running conflict does not recognize the emulator's sentence.
Fix: the DeleteCrawler handler's running-crawler refusal message.
Traces: glue.delete-crawler-while-running-rejected; services/service-com-amazonaws-glue-awsglue/src/ops/delete_crawler.rs.

Related but NOT covering these two: open issue 20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md collects other Glue refusal-message divergences (GetTable/GetDatabase, GetDashboardUrl and others); neither StartCrawler's not-found message nor DeleteCrawler's CrawlerRunningException message is carried by it, and the replay did not attribute these checks to it.

## Evidence
Cause 1 — expected (AWS): after delete_crawler, get_crawler(Name=<c>) raises EntityNotFoundException with Message 'Crawler entry with name <c> does not exist' (this leg passes), and start_crawler(Name=<c>) raises EntityNotFoundException with Message 'Crawler with name <c> does not exist'. Actual (emulator):
  [mismatch] glue.crawler-metrics-reflect-run-state
  AssertionError: unexpected StartCrawler message: 'Crawler entry with name sim2real-glue-crawler-126c0e92c1e5 does not exist'

Cause 2 — expected: delete_crawler on a RUNNING crawler raises CrawlerRunningException with Message "Cannot delete crawler<name> while it's running". Actual:
  [mismatch] glue.delete-crawler-while-running-rejected
  AssertionError: expected message "Cannot delete crawlersim2real-glue-crd-57af80d00498 while it's running", got 'Crawler with name sim2real-glue-crd-57af80d00498 is running'
(The names shown are this run's generated crawler names; the requirement is the fixed text with the request's Name substituted.)

## Steps to Reproduce
Cause 1: create a crawler over a readable S3 prefix, start it, wait for State=READY with LastCrawl present, then glue.delete_crawler(Name=<c>); call glue.get_crawler(Name=<c>) (message 'Crawler entry with name <c> does not exist') and then glue.start_crawler(Name=<c>) — observe StartCrawler returning the GetCrawler sentence instead of 'Crawler with name <c> does not exist'.
Cause 2: create a crawler over a readable S3 prefix; glue.start_crawler; poll glue.get_crawler until State=='RUNNING'; call glue.delete_crawler(Name=<c>) — observe CrawlerRunningException with Message 'Crawler with name <c> is running' instead of "Cannot delete crawler<c> while it's running".
