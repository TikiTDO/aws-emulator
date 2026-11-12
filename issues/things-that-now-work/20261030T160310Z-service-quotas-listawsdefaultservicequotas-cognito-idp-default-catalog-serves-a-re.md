Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-7CDAF993"].Description
Issue-Related: 20261029T203936Z-service-quotas-listawsdefaultservicequotas-default-catalog-omits-quotas-cognito-identity-cognito.md
Issue-Commit: 39254a0284e83d27e6c32b4bb6c2c66aaf2815af

# ListAWSDefaultServiceQuotas cognito-idp default catalog serves a re-worded Description for L-7CDAF993 and still omits the rest of the Cognito User Pools quota set

> _Note: this issue was identified against the code base at commit `39254a0284e83d27e6c32b4bb6c2c66aaf2815af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
The emulator's Service Quotas default-quota catalog is a hand-seeded table. For ServiceCode 'cognito-idp' it answers HTTP 200 but the Quotas list neither matches AWS's default set nor carries AWS's per-quota text. This issue is ONE cause: the seeded cognito-idp catalog row set and row content.

Cause 1 of 1 - ServiceCode cognito-idp (Amazon Cognito User Pools) default catalog.

AWS: ListAWSDefaultServiceQuotas with ServiceCode 'cognito-idp' returns the full Cognito User Pools default-quota set. Smithy (services/service-com-amazonaws-servicequotas-servicequotasv20190624 model): ListAWSDefaultServiceQuotas -> ListAWSDefaultServiceQuotasResponse { Quotas: list<ServiceQuota>, NextToken }; ServiceQuota { ServiceCode (pattern ^[a-zA-Z][a-zA-Z0-9-]{1,63}$), ServiceName, QuotaArn, QuotaCode (pattern ^[a-zA-Z][a-zA-Z0-9-]{1,128}$), QuotaName, Value (double, range 0..10000000000), Unit, Adjustable (default false), GlobalQuota (default false), UsageMetric, Period, Description (@length 0..350) }. The model does not enumerate quota codes or their text, so the CATALOG CONTENT is recorded behavior only and the recording is the requirement. Real AWS codes have the form L-XXXXXXXX (eight hex digits). The recording for cognito-idp carries, among ~33 entries, L-7D6E8ED3 (QuotaName 'Rate of UserAccountRecovery requests', Adjustable false, Unit None, Value 30.0, Period {PeriodUnit SECOND, PeriodValue 1}, UsageMetric {MetricNamespace 'AWS/Usage', MetricName 'CallCount', MetricStatisticRecommendation 'Sum', MetricDimensions Service 'Cognito User Pool' / Resource 'UserAccountRecovery' / Type 'API' / Class 'None'}) and L-5987B8A0 (QuotaName 'Rate of UserCreation requests', Adjustable true, Value 50.0, same Period/UsageMetric shape with Resource 'UserCreation'), plus L-B7575496, L-026ADBA3, L-04F7DFB8, L-12C4D74A, L-181E40D0, L-1B44D826, L-259E3368, L-2D4120F0, L-3F0DE77D, L-55545DC8, L-574C86AE, L-5EAB0605, L-60A0B411, L-6621E65D, L-66E6DF30, L-681BB884, L-71267F98, L-74D3DD04, L-770A44F8, L-A01C9633, L-A412573D, L-A585C375, L-BB3E7CCF, L-BC27731B, L-CFFBE34A, L-D6BD5178, L-F122EFFD and L-F21F8BB4. For the category rate quotas AWS's Description is a fixed sentence template whose only variable part is the category name: 'The maximum combined call rate (requests per second) for all API operations in the <Category> category. All operations within a category share the same quota. For the list of included operations, see https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations.' (adjustable categories word it as 'The maximum combined call rate (requests per second) you can request for all API operations in the <Category> category. Provision your rate limit within this quota from the Amazon Cognito console or APIs. For the list of included operations, see ...'). L-7CDAF993 is one of those category rate quotas, and its Description must be AWS's recorded sentence for its category, not a paraphrase.

Emulator: answers 200, but (a) for $.Quotas[QuotaCode="L-7CDAF993"] it now emits an emulator-authored, shortened Description instead of AWS's sentence - the replay reports $.Quotas[QuotaCode="L-7CDAF993"].Description as value_mismatch, i.e. the entry exists with the wrong text; (b) it still omits 31 further L- codes outright (each reported 'missing (expected {...}, got None)'), including L-7D6E8ED3 and L-5987B8A0 above; and (c) it serves two invented camel-case entries AWS never returns, QuotaCode 'UserPoolsPerAccount' and QuotaCode 'AppClientsPerUserPool', which do not correspond to any AWS quota code.

Why: a client enumerating Cognito User Pools limits gets a successful but wrong catalog. It sees no L- code it can pass to GetAWSDefaultServiceQuota / GetServiceQuota / RequestServiceQuotaIncrease for 31 of the service's quotas, it sees two codes AWS would reject with NoSuchResourceException, and for L-7CDAF993 it prints quota text that is not AWS's - the Description is the human-readable string a console, CLI or quota dashboard displays, and the Period/UsageMetric blocks that a CloudWatch AWS/Usage dashboard needs are absent for every omitted row.

Fix: the service-quotas default-quota catalog seed data that the ListAWSDefaultServiceQuotas handler reads (the cognito-idp rows), not the operation's wire framing or serializer - both the missing rows and the text of the L-7CDAF993 row, plus removal of the invented UserPoolsPerAccount / AppClientsPerUserPool rows.

Related open issue, which does NOT cover this cluster: 20261029T203936Z-service-quotas-listawsdefaultservicequotas-default-catalog-omits-quotas-cognito-identity-cognito.md carries Issue-Key entries for cognito-idp codes including $.Quotas[QuotaCode="L-7CDAF993"] as a whole-entry omission. The replay now reports this cluster with owned_by EMPTY, because the cluster's key set changed: L-7CDAF993 is no longer missing, it is present with a wrong Description ($.Quotas[QuotaCode="L-7CDAF993"].Description), which that issue's keys do not cover. Closing that issue therefore would not close this step.

Traces: service-quotas-catalog-list-aws-default-service-quotas-cognito-idp#task#1 (cluster affected_count 1).

## Evidence
Replay of fixture service-quotas-catalog-list-aws-default-service-quotas-cognito-idp, step task#1 (ListAWSDefaultServiceQuotas, ServiceCode 'cognito-idp'): expected_status 200, actual_status 200 - every difference is in the response body, verdict mismatch, stub_path empty, owned_by empty.

Divergence detail (verbatim from the replay):
- $.Quotas[QuotaCode="L-7CDAF993"].Description: value_mismatch - the emulator serves its own shortened wording where the recording carries AWS's category sentence.
- $.Quotas[QuotaCode="L-7D6E8ED3"]: missing (expected {'Adjustable': False, 'Description': 'The maximum combined call rate (requests per second) for all API operations in the UserAccountRecovery category. All operations within a category share the same quota. For the list of included operations, see https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations.', 'GlobalQuota': False, 'Period': {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::cognito-idp/L-7D6E8ED3', 'QuotaCode': 'L-7D6E8ED3', 'QuotaName': 'Rate of UserAccountRecovery requests', 'ServiceCode': 'cognito-idp', 'ServiceName': 'Amazon Cognito User Pools', 'Unit': 'None', 'UsageMetric': {'MetricDimensions': {'Class': 'None', 'Resource': 'UserAccountRecovery', 'Service': 'Cognito User Pool', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, 'Value': 30.0}, got None)
- $.Quotas[QuotaCode="L-5987B8A0"]: missing (expected {'Adjustable': True, 'Description': 'The maximum combined call rate (requests per second) you can request for all API operations in the UserCreation category. Provision your rate limit within this quota from the Amazon Cognito console or APIs. For the list of included operations, see https://docs.aws.amazon.com/cognito/latest/developerguide/limits.html#category_operations.', 'GlobalQuota': False, 'Period': {'PeriodUnit': 'SECOND', 'PeriodValue': 1}, 'QuotaArn': 'arn:aws:servicequotas:us-west-2::cognito-idp/L-5987B8A0', 'QuotaCode': 'L-5987B8A0', 'QuotaName': 'Rate of UserCreation requests', 'ServiceCode': 'cognito-idp', 'ServiceName': 'Amazon Cognito User Pools', 'Unit': 'None', 'UsageMetric': {'MetricDimensions': {'Class': 'None', 'Resource': 'UserCreation', 'Service': 'Cognito User Pool', 'Type': 'API'}, 'MetricName': 'CallCount', 'MetricNamespace': 'AWS/Usage', 'MetricStatisticRecommendation': 'Sum'}, 'Value': 50.0}, got None)
- Same 'missing' verdict for L-026ADBA3, L-04F7DFB8, L-12C4D74A, L-181E40D0, L-1B44D826, L-259E3368, L-2D4120F0, L-3F0DE77D, L-55545DC8, L-574C86AE, L-5EAB0605, L-60A0B411, L-6621E65D, L-66E6DF30, L-681BB884, L-71267F98, L-74D3DD04, L-770A44F8, L-A01C9633, L-A412573D, L-A585C375, L-B7575496, L-BB3E7CCF, L-BC27731B, L-CFFBE34A, L-D6BD5178, L-F122EFFD, L-F21F8BB4.
- Emulator-only keys reported for the same response: $.Quotas[QuotaCode="UserPoolsPerAccount"] and $.Quotas[QuotaCode="AppClientsPerUserPool"].

Full divergence key: service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200|keys=$.Quotas[QuotaCode="AppClientsPerUserPool"],...,$.Quotas[QuotaCode="L-7CDAF993"].Description,...,$.Quotas[QuotaCode="UserPoolsPerAccount"].

The region in the QuotaArn values above is the recorded environment's; the requirement is the ARN template arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode> together with each entry's QuotaCode, QuotaName, Description, Unit, Value, Adjustable, GlobalQuota and its Period/UsageMetric block where the recorded quota has them.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-cognito-idp, step task#1 - ListAWSDefaultServiceQuotas with ServiceCode 'cognito-idp'.
3. The response is HTTP 200, but compare its Quotas list against the recording: $.Quotas[QuotaCode="L-7CDAF993"].Description differs from AWS's category sentence, 31 L- codes (L-7D6E8ED3, L-5987B8A0, L-B7575496, ... ) are absent, and two entries AWS never returns (UserPoolsPerAccount, AppClientsPerUserPool) are present.
4. A fix must make this fixture step replay clean: every recorded cognito-idp quota present with the recorded QuotaCode/QuotaName/Description/Unit/Value/Adjustable/GlobalQuota and Period/UsageMetric, and no invented entries.

## Resolution (what landed, and the one leaf that is still open)

The cognito-idp catalog now publishes, under AWS's own codes, every quota of that service this checkout can cite: the category rates L-574C86AE, L-5987B8A0, L-7D6E8ED3, L-B7575496 and L-D6BD5178, and the two RESOURCE COUNTS this report's cluster is about — L-66E6DF30 ("User pools per account", 1000, adjustable) and L-7CDAF993 ("Resource servers per user pool", 25, adjustable). A count is not a rate, so neither carries the one-second window or the category call count. The invented camel-case rows (UserPoolsPerAccount, AppClientsPerUserPool) are published by nothing, and the codes resolve on every plane that resolves a code, so GetAWSDefaultServiceQuota / GetServiceQuota / RequestServiceQuotaIncrease answer for L-7CDAF993 instead of refusing it.

STILL OPEN, and deliberately not papered over:

* `$.Quotas[QuotaCode="L-7CDAF993"].Description` — the recorded TEXT of that one member. Replay shows every other member of the row agreeing with the recording (it is the only key reported for the code), but no evidence in this checkout states the string itself: the replay withholds recorded values, and the Evidence above quotes verbatim expected records only for the rows reported `missing`. Its claim that the text is the CATEGORY sentence does not hold — the recorded QuotaName for the code is "Resource servers per user pool" (quoted from the replay's expected record by the later cluster report 20261111T131231Z…), and a category-rate row would have diverged on QuotaName, Value, Period and UsageMetric too. So the row publishes NO `Description` rather than wording authored in this emulator: a published description is the quota's own text, and this catalog publishes none it cannot cite.
* The 24 further cognito-idp codes the recording carries (L-026ADBA3 … L-F21F8BB4) name quotas no source here states, so they are still published by nothing. They are the keys of the open report 20261111T131231Z-service-quotas-listawsdefaultservicequotas-default-catalog-incomplete-for-cognito-identity-cognito.md, and the fixture step is not clean until they are cited.
