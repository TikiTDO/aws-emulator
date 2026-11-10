Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-catalog-list-application-providers
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-catalog-list-application-providers sso-admin-application-provider-describe-round-trip sso-admin-application-provider-not-found-describe-rejected
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.ApplicationProviders[*]
Issue-Key: sso-admin|ListApplicationProviders|2xx->2xx|$.ApplicationProviders[*].DisplayData.IconUrl
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider.ApplicationProviderArn
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider.DisplayData.Description
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider.DisplayData.DisplayName
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider.DisplayData.IconUrl
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProvider.FederationProtocol
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.ApplicationProviderArn
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.Description
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.DisplayName
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.DisplayData.IconUrl
Issue-Key: sso-admin|DescribeApplicationProvider|2xx->2xx|$.FederationProtocol
Issue-Commit: 7753983c6e495ec3612695b1336bf4936fc7b7fd

# Application-provider catalog is locally synthesized: wrong membership, wrong order and derived IconUrl

> _Note: this issue was identified against the code base at commit `7753983c6e495ec3612695b1336bf4936fc7b7fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApplicationProviders

## Description
The whole application-provider surface of this service answers from a hand-written table in the service crate (`crate::state::BUILT_IN_APPLICATION_PROVIDERS`) whose membership and order are the emulator's own, and whose `DisplayData$IconUrl` is COMPUTED from the provider id instead of being carried as published per-provider data. `ListApplicationProviders` enumerates that table and `DescribeApplicationProvider` resolves an ARN against it, so the same defect shows up on both operations. This issue bundles FOUR causes; all four must be closed.

Cause 1 of 4: ListApplicationProviders publishes a different catalog than the recorded one (fixture sso-admin-catalog-list-application-providers, step task#1).
AWS: recorded behavior only — the Smithy model fixes the SHAPE of a provider but not the catalog's membership or order (ListApplicationProviders -> ApplicationProviderList of ApplicationProvider{ApplicationProviderArn required, FederationProtocol enum SAML|OAUTH, DisplayData, ResourceServerConfig}; DisplayData{DisplayName len 1..100, IconUrl pattern ^(http|https)://.*$ len 1..768, Description len 1..128}). The recording enumerates 394 providers, and the entries it reports first are the pre-integrated SAML catalog applications: index 0 is `arn:aws:sso::aws:applicationProvider/catalog/Statuspage` with FederationProtocol `SAML` and DisplayData{DisplayName `Statuspage`, Description `Statuspage Application`, IconUrl under the static asset host}, index 1 `.../catalog/DuoNetworkGateway` (DisplayName `Duo Network Gateway`, SAML), index 2 `.../catalog/DruvaInSync` (DisplayName `DruvaInSync`, SAML).
Emulator: answers 200 with its own 122-entry table in its own order — index 0 is `arn:aws:sso::aws:applicationProvider/custom` (FederationProtocol `OAUTH`, DisplayName `SSO Custom Application Provider`), index 1 `.../athena` (OAUTH, `Amazon Athena`), index 2 `.../emr` (OAUTH, `Amazon EMR`) — i.e. the customer-managed and AWS-managed OAUTH providers first, then the `catalog/<Name>` entries in alphabetical order, so every index from 0 on carries a different provider than the recording and 272 recorded providers (indices 122..393) are absent from the response altogether. Note also that per-provider display data is re-derived rather than published: the recording's entry for `catalog/DruvaInSync` has DisplayName `DruvaInSync`, the emulator's has `Druva inSync`.
Why: a client that enumerates the catalog to let a user pick an application provider is shown a list that is neither the right set nor the right order; the provider at a given position is a different provider, pagination pages over a shorter catalog, and any provider the recording publishes but this table omits simply cannot be found (`DescribeApplicationProvider` resolves by exact provider id and refuses an unknown one with ResourceNotFoundException 404 `No application provider with arn '<arn>' was found`), so the application the client wants to connect can never be instantiated.
Fix: the catalog DATA and its order in this service's state — the published-catalog table the list handler enumerates and the describe handler resolves against — must be the catalog the recording publishes (membership, order, per-entry DisplayName/Description/FederationProtocol), not a locally curated list. The list handler itself (pure paged read over that table, in the table's own order) needs no behavior change beyond reading the corrected data.
Traces: sso-admin-catalog-list-application-providers#task#1 (ListApplicationProviders, expected 200 / actual 200, body mismatch on $.ApplicationProviders[0..393]).

Cause 2 of 4: DescribeApplicationProvider round-trip returns a provider other than the one the recorded step describes (fixture sso-admin-application-provider-describe-round-trip, step task#2).
AWS: recorded behavior only — DescribeApplicationProvider takes the required ApplicationProviderArn (pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$) and reports that provider. The recorded step describes `arn:aws:sso::aws:applicationProvider/catalog/Statuspage` and gets back 200 with ApplicationProviderArn `arn:aws:sso::aws:applicationProvider/catalog/Statuspage`, FederationProtocol `SAML`, DisplayData{DisplayName `Statuspage`, Description `Statuspage Application`}.
Emulator: answers 200 describing `arn:aws:sso::aws:applicationProvider/custom` with FederationProtocol `OAUTH`, DisplayName `SSO Custom Application Provider`, Description `SSO Custom Application Provider Application`. The step describes the provider the preceding ListApplicationProviders reported, so the catalog-order defect of Cause 1 propagates here: the emulator's list hands out the custom OAUTH provider where the recording hands out a SAML catalog application, and the describe then faithfully echoes the wrong provider.
Why: a client that lists then describes (the normal discover-then-inspect flow) is told the pre-integrated SAML application it selected is an OAUTH customer-managed provider, so it chooses the wrong federation path for it; and when the client describes a catalog ARN the table does not hold, it gets a 404 where AWS returns the provider.
Fix: same component as Cause 1 — the published-catalog data the describe handler resolves against; the describe handler's lookup-by-provider-id and its ResourceNotFoundException path are correct once the catalog holds the published entries.
Traces: sso-admin-application-provider-describe-round-trip#task#2 (DescribeApplicationProvider, expected 200 / actual 200, mismatch on $.ApplicationProviderArn, $.FederationProtocol, $.DisplayData.DisplayName, $.DisplayData.Description and their $.ApplicationProvider.* aliases).

Cause 3 of 4: DisplayData$IconUrl is synthesized from a hash of the provider id, so it never matches the published icon URL (fixture sso-admin-application-provider-not-found-describe-rejected, step task#2).
AWS: recorded behavior only — a provider's DisplayData$IconUrl is an https URL under the IAM Identity Center static asset host, of the form `https://static.global.sso.amazonaws.com/app-<digest>/icons/default.png`. Fixed text: the scheme+host, the `/app-` prefix and the `/icons/default.png` suffix. Variable part: the per-provider `<digest>`, which is published catalog data for that provider — in this step the described provider's recorded digest is `efbe0dd582ec4f87` (one recording's digest value, quoted only as the example that diverged).
Emulator: returns `https://static.global.sso.amazonaws.com/app-6cdfd271da635d49/icons/default.png` for the same provider — the first 16 hex digits of SHA-256 over the provider id, computed in `BuiltInApplicationProvider::icon_url` (see Traces). Every other member of that response matched, so the digest is the only divergence: the digest is NOT derivable from the provider id, and deriving it makes every provider's icon URL wrong while still pointing at an asset host the emulator does not serve (the replay flags this `host_not_own`).
Why: a client or portal that renders the provider icon from this URL fetches an asset that does not exist, for every provider in the catalog; the URL is also the only member a caller can use to correlate a provider with its published portal asset.
Fix: carry IconUrl as per-provider published data on the catalog record and report it verbatim from the record, rather than computing a digest in the DisplayData projection.
Traces: sso-admin-application-provider-not-found-describe-rejected#task#2 (DescribeApplicationProvider, expected 200 / actual 200, mismatch on $.DisplayData.IconUrl and $.ApplicationProvider.DisplayData.IconUrl); the derivation lives in services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs around lines 4206-4243 (`display_data`, `icon_url`, `PROVIDER_ICON_HOST`).

Cause 4 of 4: ListApplicationProviders attaches an IconUrl to entries whose recorded entries carry none (fixture sso-admin-application-provider-describe-round-trip, step task#1).
AWS: recorded behavior only — in this recorded ListApplicationProviders response NO entry carries DisplayData$IconUrl; the member is absent on all 122 entries (every other member of every entry — ApplicationProviderArn, FederationProtocol, DisplayName, Description — matched the emulator exactly, so the absence is the whole divergence). IconUrl is an optional member of DisplayData in the model, so an entry without it is a complete, valid entry.
Emulator: emits DisplayData.IconUrl on every entry, each a synthesized `https://static.global.sso.amazonaws.com/app-<sha256 prefix>/icons/default.png` (e.g. index 0 `app-6cdfd271da635d49`, index 1 `app-084328c53a9de563`), i.e. it invents an off-box asset URL where the response should have no IconUrl member at all; the replay flags each one `host_not_own` because the emulator announces a host other than the one it is reached on.
Why: a client cannot distinguish "this provider publishes no portal icon" from "this provider publishes an icon", and anything that follows the advertised URL dereferences an asset host the emulator does not serve.
Fix: the DisplayData projection shared by the list and describe handlers must report IconUrl only when the catalog record holds one (Option carried on the record) instead of unconditionally populating all three DisplayData members.
Traces: sso-admin-application-provider-describe-round-trip#task#1 (ListApplicationProviders, expected 200 / actual 200, mismatch on $.ApplicationProviders[0..121].DisplayData.IconUrl).

Related open sso-admin issues (none covers this surface): the ListInstances/InstanceMetadata.State issues and 20261024T200900Z-sso-admin-createpermissionset-validation.md concern other operations; no open issue mentions ApplicationProvider.

## Evidence
Replay of trace set simparity-wire-aws-v1 (14 fixtures, 83 steps, 79 passed, 4 mismatched, score 95.2%). All four mismatches are sso-admin application-provider steps; none carries a stub_path or owned_by.

1) sso-admin-catalog-list-application-providers#task#1 — ListApplicationProviders, expected 200, actual 200.
   expected $.ApplicationProviders[0] = {ApplicationProviderArn: 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', FederationProtocol: 'SAML', DisplayData: {DisplayName: 'Statuspage', Description: 'Statuspage Application', IconUrl: 'https://static.global.sso.amazonaws.com/app-d77638588eff9057/icons/default.png'}}
   actual   $.ApplicationProviders[0] = {ApplicationProviderArn: 'arn:aws:sso::aws:applicationProvider/custom', FederationProtocol: 'OAUTH', DisplayData: {DisplayName: 'SSO Custom Application Provider', Description: 'SSO Custom Application Provider Application', IconUrl: 'https://static.global.sso.amazonaws.com/app-6cdfd271da635d49/icons/default.png'}}
   expected [1] = catalog/DuoNetworkGateway (SAML, 'Duo Network Gateway') vs actual [1] = .../athena (OAUTH, 'Amazon Athena'); expected [2] = catalog/DruvaInSync (SAML, 'DruvaInSync') vs actual [2] = .../emr (OAUTH, 'Amazon EMR'). Diverging keys run to $.ApplicationProviders[393]; the emulator returns only indices 0..121.

2) sso-admin-application-provider-describe-round-trip#task#2 — DescribeApplicationProvider, expected 200, actual 200.
   $.ApplicationProviderArn: expected 'arn:aws:sso::aws:applicationProvider/catalog/Statuspage', got 'arn:aws:sso::aws:applicationProvider/custom'
   $.FederationProtocol: expected 'SAML', got 'OAUTH'
   $.DisplayData.DisplayName: expected 'Statuspage', got 'SSO Custom Application Provider'
   $.DisplayData.Description: expected 'Statuspage Application', got 'SSO Custom Application Provider Application'
   (same mismatches repeated on the $.ApplicationProvider.* aliases)

3) sso-admin-application-provider-not-found-describe-rejected#task#2 — DescribeApplicationProvider, expected 200, actual 200; ONLY the icon URL diverges:
   $.DisplayData.IconUrl: expected 'https://static.global.sso.amazonaws.com/app-efbe0dd582ec4f87/icons/default.png', got 'https://static.global.sso.amazonaws.com/app-6cdfd271da635d49/icons/default.png' (verdict host_not_own)
   The actual digest is sha256("custom")[..16], produced by BuiltInApplicationProvider::icon_url.

4) sso-admin-application-provider-describe-round-trip#task#1 — ListApplicationProviders, expected 200, actual 200; ONLY the icon URLs diverge, with no expected value at all:
   $.ApplicationProviders[0].DisplayData.IconUrl: expected None, got 'https://static.global.sso.amazonaws.com/app-6cdfd271da635d49/icons/default.png' (host_not_own)
   $.ApplicationProviders[1].DisplayData.IconUrl: expected None, got 'https://static.global.sso.amazonaws.com/app-084328c53a9de563/icons/default.png'
   ... same for every entry through $.ApplicationProviders[121].

Source evidence in the checkout: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs lines 4403-4561 define BUILT_IN_APPLICATION_PROVIDERS as 1 customer-managed + 8 AWS-managed providers followed by 113 alphabetical catalog/<Name> entries (122 total, 'custom' first); lines 4206-4243 define display_data()/icon_url(), which always populate DisplayName+Description+IconUrl and compute the icon digest as hex(sha256(provider id))[..16]. services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_application_providers.rs lines 184-213 enumerate that table through the same to_shape(), which is why the list and describe diverge identically.

Smithy citations (services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json): DescribeApplicationProvider input ApplicationProviderArn required, pattern ^arn:aws(-[a-z]{1,5}){0,3}:sso::aws:applicationProvider/[a-zA-Z0-9-/]+$, errors include ResourceNotFoundException (404) and ValidationException (400); ApplicationProvider{ApplicationProviderArn required, FederationProtocol, DisplayData, ResourceServerConfig}; DisplayData{DisplayName 1..100, IconUrl 1..768 pattern ^(http|https)://.*$, Description 1..128} — all three DisplayData members OPTIONAL; FederationProtocol enum = SAML|OAUTH; ListApplicationProviders paginated on NextToken/MaxResults (1..100).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (14 fixtures, 83 steps).
2. Cause 1: replay fixture sso-admin-catalog-list-application-providers, step task#1 — POST ListApplicationProviders with no MaxResults and compare $.ApplicationProviders element by element against the recording: index 0 is arn:aws:sso::aws:applicationProvider/custom (OAUTH) where the recording has .../catalog/Statuspage (SAML), and the response ends at index 121 where the recording runs to index 393.
3. Cause 2: replay fixture sso-admin-application-provider-describe-round-trip, step task#1 then task#2 — the step describes the ApplicationProviderArn the preceding list reported; the emulator describes .../custom (OAUTH, 'SSO Custom Application Provider') where the recording describes .../catalog/Statuspage (SAML, 'Statuspage'). Additionally call DescribeApplicationProvider directly with ApplicationProviderArn=arn:aws:sso::aws:applicationProvider/catalog/Statuspage and observe the emulator's catalog-dependent answer.
4. Cause 3: replay fixture sso-admin-application-provider-not-found-describe-rejected, step task#2 — every member of the DescribeApplicationProvider response matches except DisplayData.IconUrl, whose digest is hex(sha256(provider id))[..16] instead of the provider's published digest.
5. Cause 4: replay fixture sso-admin-application-provider-describe-round-trip, step task#1 — every entry of the emulator's ApplicationProviders carries a DisplayData.IconUrl pointing at static.global.sso.amazonaws.com, while no entry of the recorded response carries the IconUrl member at all.
