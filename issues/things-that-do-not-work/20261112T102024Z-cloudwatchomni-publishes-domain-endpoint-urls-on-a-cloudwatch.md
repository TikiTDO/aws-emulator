Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatchomni-deep-link-and-identity-center-refusals
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatchomni-deep-link-and-identity-center-refusals
Issue-Shape-Id: com.amazonaws.cloudwatchomni#CloudWatchOmniFrontend
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.customEndpointUrls[*]
Issue-Key: cloudwatchomni|GetDomain|2xx->2xx|$.domain.domainEndpointUrl
Issue-Commit: 09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77

# cloudwatchomni publishes domain endpoint URLs on a cloudwatch-omni.global.app.aws host the emulator does not serve

> _Note: this issue was identified against the code base at commit `09fb1164602e6ebdd2bd6e78b61b67ab44fe1e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomain

## Description
Cause 1 of 1: GetDomain returns a Domain envelope whose domainEndpointUrl and customEndpointUrls[*] are minted on the real public CloudWatch Omni hostname (https://<domain-id>.cloudwatch-omni.global.app.aws and https://<domain-name>.cloudwatch-omni.global.app.aws) instead of on the address the emulator actually serves, so the URLs the response advertises are off-emulator and resolve nowhere a client of the emulator can reach.

AWS: Smithy (com.amazonaws.cloudwatchomni#Domain) models domainEndpointUrl as a required String, "The HTTPS endpoint URL for accessing the domain", and customEndpointUrls as "Additional endpoint URLs derived from the domain name"; GetDomain takes domainId (pattern ^d-[0-9a-z]{1,25}$) and returns the required `domain` member. The model does not fix the hostname — recorded behavior only for the host shape: against real AWS the recorded response carries the account's own reachable CloudWatch Omni domain host, i.e. an endpoint URL that a client can actually open to reach the domain it just created. The requirement is that the URL names a host that serves the domain being described; the concrete host name, domain id and generated domain name in any recording are per-run values, not part of the requirement.

Emulator: for a domain created through this emulator, GetDomain answers 200 with $.domain.domainEndpointUrl = 'https://d-<id>.cloudwatch-omni.global.app.aws' and $.domain.customEndpointUrls[0] = 'https://<domain-name>.cloudwatch-omni.global.app.aws' — the replay verdict on both members is host_not_own: the emulator announces an address other than the one it is reached on. It never substitutes its own data listener's authority, so the advertised endpoints point at the public AWS global app domain.

Why: a client that follows the endpoint URL the emulator itself just handed it leaves the emulator entirely — the request either fails to resolve/connect or egresses to real AWS, where the emulator-local domain does not exist. Every workflow that is supposed to chain off a created domain (opening the domain endpoint, hitting a custom endpoint URL, or handing the URL to a browser/deep-link flow) is unreachable, and the two endpoint members of the Domain envelope cannot be trusted by any caller. The same off-emulator minting shows up in CreateDomain responses (fixture cloudwatchomni-domain-and-space-singleton-lifecycle task#1/task#3), but those steps are already owned by the open issue 20261111T063558Z-cloudwatchomni-createdomain-ignores-clienttoken-requirement-and-account-singleton.md; no open issue covers the endpoint-URL host itself, which is what this issue is about.

Fix: the Domain projection/serializer in the cloudwatchomni service crate — wherever domainEndpointUrl and customEndpointUrls are composed for the domain resource (shared by CreateDomain, GetDomain and the domain list/update reads). It must build both members from the emulator's own served authority (the host:port the request arrived on, or the configured public base URL) while keeping AWS's URL shape (an https URL whose leftmost label is the domain id for domainEndpointUrl and the domain name for each custom endpoint), rather than hard-coding the cloudwatch-omni.global.app.aws suffix.

Traces: cloudwatchomni-deep-link-and-identity-center-refusals#task#7 (GetDomain, 200, $.domain.domainEndpointUrl and $.domain.customEndpointUrls[0] both host_not_own).

## Evidence
Fixture cloudwatchomni-deep-link-and-identity-center-refusals, step task#7, operation GetDomain, verdict mismatch (expected status 200, actual status 200).

Replay detail:
  $.domain.domainEndpointUrl: host_not_own (expected 'https://d-cbqr2akcef8uym5txuakqqh52.cloudwatch-omni.global.app.aws', got 'https://d-cbqr2akcef8uym5txuakqqh52.cloudwatch-omni.global.app.aws')
  $.domain.customEndpointUrls[0]: host_not_own (expected 'https://sim2real-probe-cwo-dom-66ede1d2.cloudwatch-omni.global.app.aws', got 'https://sim2real-probe-cwo-dom-66ede1d2.cloudwatch-omni.global.app.aws')

Expected: both members name a host the emulator serves — the authority the GetDomain request itself was answered on — carrying AWS's URL shape (https://<domainId>... for domainEndpointUrl, https://<domain name>... per custom endpoint).
Actual: both members name the public AWS host suffix cloudwatch-omni.global.app.aws, which the emulator does not serve; the replay comparator classifies each as host_not_own.

Model evidence: Smithy com.amazonaws.cloudwatchomni#Domain.domainEndpointUrl (required String, "The HTTPS endpoint URL for accessing the domain") and Domain.customEndpointUrls ("Additional endpoint URLs derived from the domain name"); GetDomain -> GetDomainOutput.domain (required).

Note: the domain id (d-cbqr2akcef8uym5txuakqqh52), the domain name (sim2real-probe-cwo-dom-66ede1d2) and the emulator's port are per-run values; the requirement is only that the announced host is the one serving the response.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Replay fixture 'cloudwatchomni-deep-link-and-identity-center-refusals' up to step task#7: it creates a CloudWatch Omni domain with a custom endpoint and then calls GetDomain with the returned domainId.
3. Inspect the 200 response body: $.domain.domainEndpointUrl and $.domain.customEndpointUrls[0].
4. Observe both are https URLs under the public suffix cloudwatch-omni.global.app.aws rather than under the emulator's own served authority; attempting to open either URL does not reach the emulator that produced it.
5. Expected: both URLs are rooted at the host:port the emulator serves, so an HTTPS request to them reaches this emulator's domain endpoint.
