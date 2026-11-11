Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-admin-auth-challenge-flow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-admin-auth-challenge-flow cognito-idp-managed-login-branding-crud cognito-idp-managed-login-branding-lifecycle cognito-idp-osc-dashboards-client-callback-diagnosis cognito-idp-ui-customization-and-risk-configuration cognito-idp-ui-customization-crud cognito-idp-ui-customization-global-and-client cognito-idp-ui-customization-lifecycle cognito-idp-ui-customization-round-trip
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$.__type
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$.message
Issue-Key: cognito-idp|AdminRespondToAuthChallenge|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets[*]
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets[*].Category
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets[*].ColorMode
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Assets[*].Extension
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.dropDown.darkMode.match.itemTextColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.pending.indicatorColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.pending.indicatorColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.darkMode.background.color
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.darkMode.background.color
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled.backgroundColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled.backgroundColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBranding|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.hover.backgroundColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Assets[*]
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Assets[*].Category
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Assets[*].ColorMode
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Assets[*].Extension
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.dropDown.darkMode.match.itemTextColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.darkMode.pending.indicatorColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.componentClasses.statusIndicator.lightMode.pending.indicatorColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.darkMode.defaults.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.idpButton.standard.lightMode.defaults.textColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.darkMode.background.color
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageFooter.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.darkMode.background.color
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.pageHeader.lightMode.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled.backgroundColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.darkMode.disabled.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled.backgroundColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.disabled.borderColor
Issue-Key: cognito-idp|DescribeManagedLoginBrandingByClient|2xx->2xx|$.ManagedLoginBranding.Settings.components.primaryButton.lightMode.hover.backgroundColor
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.CloudFrontDistribution
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.S3Bucket
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.Status
Issue-Key: cognito-idp|DescribeUserPoolDomain|2xx->2xx|$.DomainDescription.Version
Issue-Key: cognito-idp|GetUICustomization|2xx->2xx|$.UICustomization.CSSUrl
Issue-Key: cognito-idp|SetUICustomization|2xx->2xx|$.UICustomization.CSSUrl
Issue-Related: 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md
Issue-Commit: 394b978be80b5554def231b867e4dba6cd13e509

# cognito-idp: 6 net-new causes — hosted-UI/managed-login read paths omit CSSUrl and the domain description, branding defaults diverge, and a challenge-code refusal uses the wrong error class

> _Note: this issue was identified against the code base at commit `394b978be80b5554def231b867e4dba6cd13e509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge, DescribeUserPoolDomain, GetUICustomization, SetUICustomization, DescribeManagedLoginBranding, DescribeManagedLoginBrandingByClient

## Description
Six net-new causes in the cognito-idp user-pool presentation surface (hosted-UI customization, managed-login branding, user-pool domain) plus one challenge-refusal classification defect. All six are distinct root causes and each must be closed separately; the replay keys for causes 2..6 are listed in trace_ids.

None of these are covered by the open cognito-idp issues that look adjacent: 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md (pool/client/domain/MFA read payloads), 20261030T233240Z-cognito-idp-branding-and-user-pool-not-found.md (branding *refusal message* text), and 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md (challenge state machine). The replay reports every cluster below as unowned.

Cause 1 of 6: AdminRespondToAuthChallenge answers a non-matching challenge response as NotAuthorizedException instead of CodeMismatchException.
AWS: Smithy: AdminRespondToAuthChallenge binds CodeMismatchException (client error, httpError 400, documented "This exception is thrown if the provided code doesn't match what the server was expecting") as well as NotAuthorizedException (httpError 403). The recorded response for a challenge answered with a code that does not match the outstanding challenge is HTTP 400 with __type "CodeMismatchException", x-amzn-errortype "CodeMismatchException:" and message "Invalid code/session provided". That message is entirely fixed text — it has no variable parts taken from the request or from state.
Emulator: answers HTTP 400 with __type "NotAuthorizedException", x-amzn-errortype "NotAuthorizedException:" and message "Invalid session for the user." — i.e. it treats the paired (Session, ChallengeResponses) failure as an unusable session rather than as a code mismatch. Hypothesis: the handler validates/consumes the session before comparing the supplied code, so any failed answer surfaces as the session refusal.
Why: a client that branches on the error code cannot distinguish "re-prompt the user for the code" (CodeMismatchException, retryable within the same session) from "restart authentication" (NotAuthorizedException, session dead). SDK error-type matching and retry/UX logic take the wrong branch, and a correct caller loops or aborts the sign-in.
Fix: the AdminRespondToAuthChallenge handler's challenge-verification path — classify a code/response mismatch as CodeMismatchException with AWS's message, and reserve the "Invalid session for the user." NotAuthorizedException for a session that is genuinely absent, expired or already consumed.
Traces: cognito-idp-admin-auth-challenge-flow#task#5.

Cause 2 of 6: DescribeManagedLoginBranding serves a Cognito-provided default branding style whose asset list is ordered differently, is short several assets, and whose default Settings document carries different colors.
AWS: Smithy: DescribeManagedLoginBranding -> ManagedLoginBrandingType with Assets: AssetListType (@length max 40) of AssetType {Category: AssetCategoryType (required), ColorMode: ColorSchemeModeType (required), Extension (required), Bytes, ResourceId} and Settings: Document. The model does not fix the content or the order of the Cognito-provided defaults, so this is recorded behavior only: when a style is created with UseCognitoProvidedValues, AWS serves a stable, fixed default asset list and a stable default Settings document, and DescribeManagedLoginBranding returns them in that same stable order on read-back. In the recording the asset list has 38 entries, each category appears as a DARK entry immediately followed by its LIGHT entry, and the categories appear in AWS's own layout order (page-footer background, form background, page-header background, page background, email graphic, ... ) rather than alphabetically.
Emulator: returns an asset list that is sorted by Category name with LIGHT before DARK within a category (Assets[0] Category AUTH_APP_GRAPHIC/ColorMode LIGHT where AWS has PAGE_FOOTER_BACKGROUND/DARK, Assets[1] AUTH_APP_GRAPHIC/DARK vs PAGE_FOOTER_BACKGROUND/LIGHT, Assets[2] EMAIL_GRAPHIC/LIGHT vs FORM_BACKGROUND/DARK, and so on), stops short of the recorded length so Assets[30]..Assets[37] are absent entirely, misreports Extension on two entries, and fills parts of the default Settings document with different values (componentClasses.dropDown.darkMode.match.itemTextColor, componentClasses.statusIndicator.{darkMode,lightMode}.pending.indicatorColor, components.idpButton.standard.{darkMode,lightMode}.defaults.{borderColor,textColor}, components.pageFooter/pageHeader background color and borderColor, components.primaryButton.{darkMode,lightMode}.disabled.{backgroundColor,borderColor} and lightMode.hover.backgroundColor).
Why: a client that renders managed login from the described style, or that reads the default style in order to clone and edit it, gets the wrong set of asset slots and the wrong palette: assets AWS provides are missing, every index addresses a different asset than on AWS, and a round-trip of the default style silently rewrites colors. Index-based access (Assets[n]) is wrong for every n.
Fix: the managed-login branding state/defaults — the Cognito-provided default asset catalogue and default Settings document the emulator seeds, and the branding serializer's ordering of Assets (preserve the stored/AWS order instead of sorting by Category and ColorMode).
Traces: cognito-idp-managed-login-branding-crud#task#6.

Cause 3 of 6: DescribeManagedLoginBrandingByClient has the same divergent default style as the by-id read.
AWS: Smithy: DescribeManagedLoginBrandingByClient (UserPoolId + ClientId, required) -> the same ManagedLoginBrandingType. Recorded behavior only: resolving the style through the app client returns the identical style document — same 38-entry asset list in the same DARK-then-LIGHT, layout-order sequence, and the same default Settings values — as resolving it by ManagedLoginBrandingId.
Emulator: serves the by-client read from the same divergent defaults and the same Category/ColorMode sort as cause 2 (Assets[0] AUTH_APP_GRAPHIC/LIGHT instead of PAGE_FOOTER_BACKGROUND/DARK, Assets[30]..Assets[37] missing, the same Settings color divergences).
Why: the client-scoped read is the one a hosted-UI integration actually uses, so an application that resolves branding by ClientId renders with missing asset slots and the wrong palette.
Fix: same component as cause 2 (branding defaults + branding serializer ordering); confirm the by-client resolution path shares that one code path rather than duplicating the ordering.
Traces: cognito-idp-managed-login-branding-lifecycle#task#6.

Cause 4 of 6: DescribeUserPoolDomain answers 200 with an empty DomainDescription for a domain that exists.
AWS: Smithy: DescribeUserPoolDomain(Domain required) -> DescribeUserPoolDomainResponse{DomainDescription: DomainDescriptionType}, whose members are UserPoolId, AWSAccountId, Domain, S3Bucket, CloudFrontDistribution, Version, Status (DomainStatusType), CustomDomainConfig, ManagedLoginVersion, Routing. For a domain that has been created and is serving, AWS populates the description: the owning UserPoolId, the AWS account id, the Domain string as supplied, the assets S3 bucket, the CloudFront endpoint hosting it, a Version, Status ACTIVE, and the ManagedLoginVersion configured on the domain. (AWS reserves the empty DomainDescription for a domain that does not exist.)
Emulator: answers 200 with DomainDescription present but empty — UserPoolId, AWSAccountId, Domain, S3Bucket, CloudFrontDistribution, Version, Status and ManagedLoginVersion are all absent — even though the fixture created that domain on the pool earlier in the same trace. The concrete host names, bucket name, account id and version string are environment-specific and are NOT the requirement; the requirement is that the members are present and describe the existing domain, with Status ACTIVE and the pool's ManagedLoginVersion.
Why: an empty DomainDescription is AWS's "no such domain" signal, so a client that created the domain and polls DescribeUserPoolDomain concludes the domain does not exist or never becomes ACTIVE. Hosted-UI/managed-login discovery (building the sign-in URL, waiting for the domain to be ready, reading ManagedLoginVersion) dead-ends.
Fix: the DescribeUserPoolDomain handler and the user-pool-domain state it reads — resolve the stored domain record and serialize its description instead of returning an empty structure.
Traces: cognito-idp-osc-dashboards-client-callback-diagnosis#task#7.

Cause 5 of 6: GetUICustomization omits UICustomization.CSSUrl.
AWS: recorded behavior only — the Smithy UICustomizationType models UserPoolId, ClientId, ImageUrl, CSS, CSSVersion, LastModifiedDate, CreationDate and does NOT declare CSSUrl, but the recorded wire response for GetUICustomization carries a UICustomization.CSSUrl string: the URL of the stored custom CSS document under the pool's hosted-UI asset distribution, of the form https://<cloudfront-asset-host>/<userPoolId>/<clientId>/<cssVersion>/assets/CSS/custom-css.css. The host, the pool id, the client id and the CSS version are per-environment/per-request values, not requirements; the requirement is that the member is present and addresses the CSS this pool/client has stored, with its CSSVersion.
Emulator: returns UICustomization with no CSSUrl member at all (missing, got None) while answering 200.
Why: a client that fetches the published stylesheet — to verify the customization took effect, or to serve it from its own front end — has no URL to fetch and cannot tell a pool with CSS from one without.
Fix: the GetUICustomization handler's response serializer, plus the UI-customization state that must record the stored CSS's version/location so the URL can be rendered.
Traces: cognito-idp-ui-customization-and-risk-configuration#task#6.

Cause 6 of 6: SetUICustomization's echoed UICustomization also omits CSSUrl.
AWS: recorded behavior only, as in cause 5 — SetUICustomization returns the same UICustomizationType and the recorded response includes UICustomization.CSSUrl pointing at the newly stored CSS under the just-minted CSS version, so the write's own response already addresses the published stylesheet.
Emulator: answers 200 with CSSUrl absent from the echoed UICustomization.
Why: a client that uploads CSS and uses the write response's URL (rather than issuing a second GetUICustomization) never obtains a URL, so the publish step of a hosted-UI customization cannot be completed or verified.
Fix: the SetUICustomization handler's response serializer — render the same CSSUrl the read path owes (share one renderer with GetUICustomization so the two cannot drift).
Traces: cognito-idp-ui-customization-crud#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (193 fixtures, 1742 steps, 1620 passed, score 93.0%). Six unowned divergence clusters, all service=cognito-idp:

Cause 1 — fixture cognito-idp-admin-auth-challenge-flow, step task#5, AdminRespondToAuthChallenge, verdict mismatch, status 400 -> 400:
  $.__type: expected 'CodeMismatchException', got 'NotAuthorizedException'
  $.message: expected 'Invalid code/session provided', got 'Invalid session for the user.'
  $headers.x-amzn-errortype: expected 'CodeMismatchException:', got 'NotAuthorizedException:'

Cause 2 — fixture cognito-idp-managed-login-branding-crud, step task#6, DescribeManagedLoginBranding, verdict mismatch, status 200 -> 200:
  $.ManagedLoginBranding.Assets[0].Category: expected 'PAGE_FOOTER_BACKGROUND', got 'AUTH_APP_GRAPHIC'
  $.ManagedLoginBranding.Assets[0].ColorMode: expected 'DARK', got 'LIGHT'
  $.ManagedLoginBranding.Assets[1].Category: expected 'PAGE_FOOTER_BACKGROUND', got 'AUTH_APP_GRAPHIC'
  $.ManagedLoginBranding.Assets[1].ColorMode: expected 'LIGHT', got 'DARK'
  $.ManagedLoginBranding.Assets[2].Category: expected 'FORM_BACKGROUND', got 'EMAIL_GRAPHIC'
  ... every index 0..29 diverges in Category and ColorMode; Assets[24]/[25].Extension diverge
  $.ManagedLoginBranding.Assets[30] .. Assets[37]: expected present, absent from the emulator's list
  $.ManagedLoginBranding.Settings.componentClasses.dropDown.darkMode.match.itemTextColor, .componentClasses.statusIndicator.{darkMode,lightMode}.pending.indicatorColor, .components.idpButton.standard.{darkMode,lightMode}.defaults.{borderColor,textColor}, .components.pageFooter.darkMode.background.color, .components.pageFooter.lightMode.borderColor, .components.pageHeader.darkMode.background.color, .components.pageHeader.lightMode.borderColor, .components.primaryButton.{darkMode,lightMode}.disabled.{backgroundColor,borderColor}, .components.primaryButton.lightMode.hover.backgroundColor: value_mismatch

Cause 3 — fixture cognito-idp-managed-login-branding-lifecycle, step task#6, DescribeManagedLoginBrandingByClient, verdict mismatch, status 200 -> 200: identical divergence set (Assets[0] 'PAGE_FOOTER_BACKGROUND'/'DARK' vs 'AUTH_APP_GRAPHIC'/'LIGHT' through Assets[29], Assets[30]..[37] absent, same Settings color mismatches).

Cause 4 — fixture cognito-idp-osc-dashboards-client-callback-diagnosis, step task#7, DescribeUserPoolDomain, verdict mismatch, status 200 -> 200:
  $.DomainDescription.AWSAccountId: missing (expected an account id, got None)
  $.DomainDescription.CloudFrontDistribution: missing (expected the hosting CloudFront endpoint, got None)
  $.DomainDescription.Domain: missing (expected the created domain prefix, got None)
  $.DomainDescription.ManagedLoginVersion: missing (expected 1, got None)
  $.DomainDescription.S3Bucket: missing (expected the assets bucket, got None)
  $.DomainDescription.Status: missing (expected 'ACTIVE', got None)
  $.DomainDescription.UserPoolId: missing (expected the owning pool id, got None)
  $.DomainDescription.Version: missing (expected <shaped-not-asserted>, got None)

Cause 5 — fixture cognito-idp-ui-customization-and-risk-configuration, step task#6, GetUICustomization, verdict mismatch, status 200 -> 200:
  $.UICustomization.CSSUrl: missing (expected 'https://<asset-host>/<userPoolId>/<clientId>/<cssVersion>/assets/CSS/custom-css.css', got None)
  cluster also covers cognito-idp-ui-customization-crud#task#5, cognito-idp-ui-customization-global-and-client#task#{5,7,8}, cognito-idp-ui-customization-lifecycle#task#{6,8}, cognito-idp-ui-customization-round-trip#task#6 (8 steps).

Cause 6 — fixture cognito-idp-ui-customization-crud, step task#4, SetUICustomization, verdict mismatch, status 200 -> 200:
  $.UICustomization.CSSUrl: missing (expected 'https://<asset-host>/<userPoolId>/<clientId>/{{css_version}}/assets/CSS/custom-css.css', got None)
  cluster also covers cognito-idp-ui-customization-global-and-client#task#{4,6}, cognito-idp-ui-customization-lifecycle#task#{5,7}, cognito-idp-ui-customization-round-trip#task#5 (6 steps).

Model evidence: Smithy AdminRespondToAuthChallenge errors include CodeMismatchException (client, 400); DomainDescriptionType declares UserPoolId/AWSAccountId/Domain/S3Bucket/CloudFrontDistribution/Version/Status/CustomDomainConfig/ManagedLoginVersion/Routing; AssetType.Category is AssetCategoryType and AssetType.ColorMode is ColorSchemeModeType, both @required, in AssetListType @length max 40; UICustomizationType does not declare CSSUrl (recorded wire behavior only).

## Steps to Reproduce
1. git clone the project and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus) and look at the cognito-idp clusters.
3. Cause 1: replay cognito-idp-admin-auth-challenge-flow step task#5 — drive AdminInitiateAuth to a challenge, then call AdminRespondToAuthChallenge with the returned Session and a challenge response that does not match. Observe 400 NotAuthorizedException "Invalid session for the user." instead of 400 CodeMismatchException "Invalid code/session provided" (and x-amzn-errortype CodeMismatchException:).
4. Cause 2: replay cognito-idp-managed-login-branding-crud step task#6 — CreateManagedLoginBranding with UseCognitoProvidedValues, then DescribeManagedLoginBranding. Compare the Assets list length/order (Category + ColorMode per index) and the Settings document against the recording.
5. Cause 3: replay cognito-idp-managed-login-branding-lifecycle step task#6 — same style, read via DescribeManagedLoginBrandingByClient with the app client id; same asset-order/length and Settings divergences.
6. Cause 4: replay cognito-idp-osc-dashboards-client-callback-diagnosis step task#7 — CreateUserPoolDomain for the pool, then DescribeUserPoolDomain with that Domain. Observe 200 with an empty DomainDescription instead of a populated one with Status ACTIVE.
7. Cause 5: replay cognito-idp-ui-customization-and-risk-configuration step task#6 — SetUICustomization with CSS, then GetUICustomization. Observe UICustomization with no CSSUrl member.
8. Cause 6: replay cognito-idp-ui-customization-crud step task#4 — the SetUICustomization response itself carries no CSSUrl.
9. All six causes must be fixed and all six fixture#step bindings (issue_trace/issue_step plus the five in trace_ids) re-replayed before closing.
