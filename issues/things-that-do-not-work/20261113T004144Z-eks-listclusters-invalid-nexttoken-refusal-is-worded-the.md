Issue-Source: trace-sweep-divergence
Issue-Trace: eks-ekscc-traj-cluster-lookup-mistakes
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-ekscc-traj-cluster-lookup-mistakes
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|ListClusters|4xx->4xx|$.message
Issue-Commit: ab3e9411edd79c78c4763b290b74822ee89d0fde

# eks ListClusters invalid-nextToken refusal is worded 'The specified nextToken is invalid: unrecognized format.' instead of AWS's 'nextToken is not valid or expired.'

> _Note: this issue was identified against the code base at commit `ab3e9411edd79c78c4763b290b74822ee89d0fde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListClusters

## Description
Cause 1 of 1: ListClusters refuses an undecodable nextToken with an emulator-authored sentence instead of AWS's wording for this refusal class.

AWS: Smithy (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json): ListClusters is GET /clusters with httpQuery members maxResults (ListClustersRequestMaxResults, @range min 1 max 100), nextToken (String) and include; it is @paginated with inputToken/outputToken nextToken, items clusters, pageSize maxResults. Its modeled errors are ClientException (httpError 400), InvalidParameterException (httpError 400), ServerException (500) and ServiceUnavailableException (503) — the model does not state the refusal text, so the message itself is recorded behavior only. Recorded behavior: a nextToken that this service never minted / cannot be decoded is refused with HTTP 400 and the message 'nextToken is not valid or expired.' That sentence is ENTIRELY FIXED TEXT for this error class: it has no variable parts, it does NOT echo the rejected token, the cluster scope, maxResults or any member name, and it ends in a period. The recorded response's HTTP status (400) and its x-amzn-errortype both already match, so the status and error code are not at issue — only the message is.

Emulator: the eks emulator answers HTTP 400 with the message 'The specified nextToken is invalid: unrecognized format.' — a different sentence altogether. It describes the FAILURE MODE of the emulator's own token decoder ('unrecognized format') and uses the 'The specified <member> is invalid: ...' template this crate applies to malformed request members generally, rather than AWS's single pagination sentence. Note AWS does use the 'The specified nextToken is invalid: unrecognized format.' sentence for a DIFFERENT eks operation's token refusal (ListAssociatedAccessPolicies — see open behavioral issue 20261103T150548Z-eks-5-net-new-root-causes-in-refusal-0ae4aa.md, cause 4, where that text is the EXPECTED wording), so the defect here is that the crate serves one shared pagination sentence for every eks listing instead of the per-operation wording AWS uses; no open issue carries this step's replay key.

Why: the refusal sentence is the only operator- and test-visible part of this error that distinguishes 'your pagination token is stale or forged — restart the enumeration' from other 400s on the same operation, because ListClusters' modeled 400s (ClientException and InvalidParameterException) are shared with maxResults and include validation. A client, pagination wrapper or integration test that matches AWS's 'nextToken is not valid or expired.' never matches against the emulator, and a log-scraping or alarm rule written against real EKS output misses the event entirely. The emulator's wording also asserts a stronger fact than AWS does — it claims the token's FORMAT is unrecognized, where AWS's sentence deliberately covers both a malformed token and an EXPIRED one, so a client cannot tell from the emulator whether re-minting the token would help.

Fix: the ListClusters handler's nextToken decoding/validation path in the eks crate — emit AWS's fixed sentence 'nextToken is not valid or expired.' for the undecodable/expired-token refusal on this operation instead of the crate's shared 'The specified <member> is invalid: <reason>' pagination template. The HTTP status (400) and the error code are already correct and must not change.

Traces: eks-ekscc-traj-cluster-lookup-mistakes#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout, fixture eks-ekscc-traj-cluster-lookup-mistakes, step task#2, operation ListClusters, verdict mismatch (divergence_key service=eks|ListClusters|status=400->400|keys=$.message; affected_count 1; owned_by empty — no open issue carries this replay key).

Expected status 400, actual status 400 (status and x-amzn-errortype both match; only the body message diverges).

  $.message: value_mismatch
    expected: 'nextToken is not valid or expired.'
    actual:   'The specified nextToken is invalid: unrecognized format.'

The expected message is 100% fixed text for this error class — no request value, no state value, no token echo, no per-run id or timestamp appears in it, so this divergence is not an artifact of a generated name, clock or request id.

Smithy model consulted: services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json — ListClusters: http GET /clusters (code 200); input httpQuery maxResults (@range 1..100), nextToken (String), include; paginated {inputToken: nextToken, outputToken: nextToken, items: clusters, pageSize: maxResults}; errors ClientException (400), InvalidParameterException (400), ServerException (500), ServiceUnavailableException (503). The model carries no message text for the token refusal, hence 'recorded behavior only' for the wording.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout, and point a boto3 eks client at its data listener.
2. Call eks.list_clusters(nextToken='<any string the emulator never minted — e.g. "%%% not a real token %%%" or a truncated/garbled copy of a real token>').
3. Expect HTTP 400 with the response body message exactly 'nextToken is not valid or expired.' (fixed text, no variable parts, nothing echoed from the request).
4. Observe HTTP 400 with the message 'The specified nextToken is invalid: unrecognized format.' instead.
5. Control: eks.list_clusters() with no nextToken must still return the cluster list and, when a page boundary is reached, a usable nextToken that list_clusters accepts — i.e. only the invalid-token refusal's message is wrong, not pagination itself.
6. Replay fixture eks-ekscc-traj-cluster-lookup-mistakes step task#2 to confirm.
