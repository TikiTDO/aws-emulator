Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-66E6DF30"].Description
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-7CDAF993"].Description
Issue-Related: 20261111T131231Z-service-quotas-listawsdefaultservicequotas-default-catalog-incomplete-for-cognito-identity-cognito.md
Issue-Commit: fca1310c8a9c6ed690bfde34d31f406914f28a21

# cognito-idp default quota catalog omits 24 quota rows and blanks two Descriptions

> _Note: this issue was identified against the code base at commit `fca1310c8a9c6ed690bfde34d31f406914f28a21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
Cause 1 of 1: ListAWSDefaultServiceQuotas for ServiceCode="cognito-idp" returns HTTP 200 with a Quotas list that is missing almost the whole default-quota catalog for that service code, and two of the rows it does return are missing their Description member.

AWS: Smithy model com.amazonaws.servicequotas#ServiceQuotasV20190624, operation ListAWSDefaultServiceQuotas (input ListAWSDefaultServiceQuotasRequest.ServiceCode @required, @length 1..63, @pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$; output ListAWSDefaultServiceQuotasResponse.Quotas -> ServiceQuotaListDefinition of com.amazonaws.servicequotas#ServiceQuota; paginated inputToken=NextToken outputToken=NextToken items=Quotas pageSize=MaxResults). The operation returns the AWS-published DEFAULT quota for every quota the named service code defines, one ServiceQuota per quota code, each carrying ServiceCode, ServiceName, QuotaArn, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota and, where AWS publishes one, Description (@length 0..350) plus Period and UsageMetric for rate quotas. For a service code that exists, the catalog is complete and fixed: it is reference data, not account state, so no request parameter and no account condition can shrink it. The exact per-code membership of the catalog is recorded behavior only (the model does not enumerate quota codes); the recording for cognito-idp is the authority for which codes and which member values must appear.

Emulator: the response is HTTP 200, so the status and the envelope are right, but the Quotas list is largely empty of the expected rows. Replay reports 24 quota codes present in the recorded response and entirely absent from the emulator's (missing, got None) — L-026ADBA3, L-04F7DFB8, L-12C4D74A, L-181E40D0, L-1B44D826, L-259E3368, L-2D4120F0, L-3F0DE77D, L-55545DC8, L-5EAB0605, L-60A0B411, L-6621E65D, L-681BB884, L-71267F98, L-74D3DD04, L-770A44F8, L-A01C9633, L-A412573D, L-A585C375, L-BB3E7CCF, L-BC27731B, L-CFFBE34A, L-F122EFFD, L-F21F8BB4 — and 2 further codes that ARE emitted but with Description absent: L-66E6DF30 (expected Description "The maximum number of user pools that you can create in this account per region.") and L-7CDAF993 (expected Description "The maximum number of resource servers per user pool. A resource server is a server for access-protected resources."). The missing rows include the per-category API rate quotas, each of which AWS returns with its full shape, e.g. L-60A0B411 QuotaName "Rate of UserPoolUpdate requests", Adjustable false, GlobalQuota false, Unit "None", Value 15.0, Period {PeriodUnit SECOND, PeriodValue 1}, UsageMetric {MetricNamespace AWS/Usage, MetricName CallCount, MetricStatisticRecommendation Sum, MetricDimensions {Service "Cognito User Pool", Resource "UserPoolUpdate", Type "API", Class "None"}}, and QuotaArn of the form arn:aws:servicequotas:<region>::cognito-idp/<QuotaCode> (region comes from the request's endpoint, the account segment is empty for a default quota — the recording's us-west-2 is an example, not a requirement).

Why: a client that enumerates Service Quotas to discover what limits a service has concludes that Amazon Cognito User Pools defines only a couple of quotas and has no API rate quotas at all. Quota-inventory and quota-increase tooling then cannot find the quota code it needs (a subsequent GetAWSDefaultServiceQuota / RequestServiceQuotaIncrease for e.g. L-60A0B411 has no discoverable code), capacity-planning and compliance reports under-report limits, and the two rows that are returned without Description break any UI or report that renders the quota description for the user. Because the response is a successful 200 with a short list rather than an error, nothing signals to the caller that the catalog is incomplete — the data is silently wrong.

Fix: the seeded default-quota reference data behind ListAWSDefaultServiceQuotas in the service-quotas service crate (services/service-com-amazonaws-servicequotas-servicequotasv20190624) — the per-ServiceCode quota catalog that the handler looks the service code up in, not the ListAWSDefaultServiceQuotas serializer, which already emits a well-formed 200 envelope. The cognito-idp entry must carry every quota code AWS publishes for that code with each ServiceQuota fully populated (QuotaName, Description, Value, Unit, Adjustable, GlobalQuota, QuotaArn, and Period/UsageMetric for rate quotas), and the two existing rows must carry their Description. The same catalog backs GetAWSDefaultServiceQuota and the applied-quota listing, so filling it in fixes those lookups too.

Note on an existing issue: open issue 20261111T131231Z-service-quotas-listawsdefaultservicequotas-default-catalog-incomplete-for-cognito-identity-cognito names cognito-idp in its title and lists the bare fixture id service-quotas-catalog-list-aws-default-service-quotas-cognito-idp in Issue-Trace-Ids, but that token carries no #step half, so this cluster's replay key is not bound to it and replay reports the cluster as unowned (owned_by empty). This issue carries the binding so the cognito-idp step is actually re-replayed when the catalog is fixed.

Traces: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp#task#1 (26 diverging JSON paths under $.Quotas, verdict mismatch, expected 200 / actual 200).

## Evidence
Fixture service-quotas-catalog-list-aws-default-service-quotas-cognito-idp, step task#1, verdict mismatch. Expected status 200, actual status 200 — the divergence is entirely in the body.

Expected (recorded AWS response) vs actual (emulator), as reported by replay:
- $.Quotas[QuotaCode="L-66E6DF30"].Description: expected 'The maximum number of user pools that you can create in this account per region.', got None
- $.Quotas[QuotaCode="L-7CDAF993"].Description: expected 'The maximum number of resource servers per user pool. A resource server is a server for access-protected resources.', got None
- $.Quotas[QuotaCode="L-60A0B411"]: expected {'Adjustable': False, 'Description': 'The maximum combined call rate (requests per second) for all API operations in the UserPoolUpdate category...', 'GlobalQuota': False, 'Period': {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::cognito-idp/L-60A0B411', 'QuotaCode': 'L-60A0B411', 'QuotaName': 'Rate of UserPoolUpdate requests', 'ServiceCode': 'cognito-idp', 'ServiceName': 'Amazon Cognito User Pools', 'Unit': 'None', 'UsageMetric': {'MetricDimensions': {'Class': 'None', 'Resource': 'UserPoolUpdate', 'Service': 'Cognito User Pool', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, 'Value': 15.0}, got None
- $.Quotas[QuotaCode="L-12C4D74A"]: expected the equivalent full row for 'Rate of UserPoolClientUpdate requests per account', got None
- 22 further quota codes missing entirely: L-026ADBA3, L-04F7DFB8, L-181E40D0, L-1B44D826, L-259E3368, L-2D4120F0, L-3F0DE77D, L-55545DC8, L-5EAB0605, L-6621E65D, L-681BB884, L-71267F98, L-74D3DD04, L-770A44F8, L-A01C9633, L-A412573D, L-A585C375, L-BB3E7CCF, L-BC27731B, L-CFFBE34A, L-F122EFFD, L-F21F8BB4

Divergence key: service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="L-026ADBA3"],...,$.Quotas[QuotaCode="L-F21F8BB4"] (26 paths).

Model evidence: Smithy ListAWSDefaultServiceQuotas -> ListAWSDefaultServiceQuotasResponse.Quotas: ServiceQuotaListDefinition; ServiceQuota members ServiceCode/ServiceName/QuotaArn/QuotaCode/QuotaName/Value/Unit/Adjustable(default false)/GlobalQuota(default false)/UsageMetric/Period/Description(@length 0..350). Per-code catalog membership is recorded behavior only.

The only values that vary per environment here are the region inside QuotaArn; the quota codes, names, descriptions, units, values, Adjustable/GlobalQuota flags, Period and UsageMetric are fixed AWS reference data and are requirements.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it (the replay harness boots it and serves the data listener).
2. Replay the wire fixture corpus simparity-wire-aws-v1, or replay just fixture service-quotas-catalog-list-aws-default-service-quotas-cognito-idp, step task#1.
3. That step issues the Service Quotas ListAWSDefaultServiceQuotas call with ServiceCode="cognito-idp" (POST to the service-quotas endpoint, X-Amz-Target ServiceQuotasV20190624.ListAWSDefaultServiceQuotas, body {"ServiceCode":"cognito-idp"}).
4. Observe HTTP 200 with a Quotas array that contains neither the 24 quota codes listed in the evidence nor a Description on rows L-66E6DF30 and L-7CDAF993.
5. Compare against the recorded AWS response for the same step: every one of the 26 reported JSON paths under $.Quotas is present there.
