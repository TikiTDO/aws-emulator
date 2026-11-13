Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# cacheBehaviorSettings is stored verbatim: the documented minimumTTL-with-all-headers rule and the documented allowedHTTPMethods / cachedHTTPMethods option lists are never checked

_Filed: 2026-10-29T01:46:48Z_
_Operation: CreateDistribution | Class: validation_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDistribution` (and `UpdateDistribution`, which shares the record) takes the whole `cacheBehaviorSettings` object off the request and writes it onto the distribution without reading a single member of it. Every other prose rule of this input is enforced — the origin name is resolved, `origin.responseTimeout` is held to its documented 1..60 seconds, `defaultCacheBehavior.behavior` must be present, `bundleId` must be an active published bundle, `enablePrivateOriginAccess` is refused for a non-bucket origin, `certificateName` must be an issued certificate — and the handler's own comment enumerates exactly that list of "what the model cannot express" without mentioning `cacheBehaviorSettings` at all, so nothing in the crate justifies leaving it unchecked.

Two documented rules of `CacheSettings` are therefore not applied, and the resulting record is reported straight back by `GetDistributions` / `GetDistribution`-style reads:

1. `minimumTTL` — the member's own documentation says a value of 0 MUST be specified when the distribution forwards all headers to the origin. A request carrying `forwardedHeaders: {"option": "all"}` together with `minimumTTL: 86400` is answered 200 and that combination is stored and reported.
2. `allowedHTTPMethods` / `cachedHTTPMethods` — each member documents a closed list of option strings (three for the allowed set, two for the cached set). Any other string is stored and reported: `allowedHTTPMethods: "DELETE"`, `"GET,HEAD,OPTIONS,PUT"`, or `cachedHTTPMethods: "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE"` all survive the call, so the distribution advertises a method set the service does not offer. This is the same shape of rule the crate DOES enforce for a free-form string whose domain lives only in documentation — `UpdateLoadBalancerAttribute` refuses an `attributeValue` that is not a published TLS policy name, and `UpdateBucket` refuses a `versioning` string outside its two documented values.

This is a different gap from the already-fixed CreateDistribution/validation issue about `certificateName` (which was about resolving a stored certificate record) and from the open UpdateDistribution/validation issue about `customErrorResponses[].responsePagePath`; it concerns the `cacheBehaviorSettings` object, which no path in the crate reads.

## Evidence

The model states both rules on the members themselves — smithy_query(query="shape", shape="com.amazonaws.lightsail#CacheSettings"):
```
"minimumTTL": ... "The minimum amount of time that objects stay in the distribution's cache before the
distribution forwards another request to the origin to determine whether the object has been
updated.  A value of 0 must be specified for minimumTTL if the distribution is configured to
forward all headers to the origin."

"allowedHTTPMethods": ... "The HTTP methods that are processed and forwarded to the distribution's origin.
You can specify the following options:  GET,HEAD - The distribution forwards the GET and HEAD
methods.  GET,HEAD,OPTIONS - ...  GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE - ..."

"cachedHTTPMethods": ... "The HTTP method responses that are cached by your distribution. You can
specify the following options:  GET,HEAD - ...  GET,HEAD,OPTIONS - ..."
```
(`forwardedHeaders.option` targets the modelled `ForwardValues` enum whose `all` member is "Forward all headers to your origin" — smithy_query(query="shape", shape="com.amazonaws.lightsail#HeaderObject").)

The handler stores the object unread — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_distribution.rs:420-422:
```
                default_cache_behavior: input.default_cache_behavior.clone(),
                cache_behavior_settings: input.cache_behavior_settings.clone(),
                cache_behaviors,
```
and the update path replaces it the same way — src/ops/update_distribution.rs:429-431:
```
            if let Some(cache_behavior_settings) = input.cache_behavior_settings {
                distribution.cache_behavior_settings = Some(cache_behavior_settings);
            }
```
The store reports it straight back — src/state.rs:12500-12503 and :12563:
```
    /// The cache behaviour settings the request carried, or `None` when it
    /// carried none — the model documents no default for any member of
    /// `CacheSettings`, so nothing is invented for a request that set none.
    pub cache_behavior_settings: Option<CacheSettings>,
...
            cache_behavior_settings: self.cache_behavior_settings.clone(),
```
(that comment justifies only not INVENTING defaults, not skipping the member's rules).

The same handler's own list of the prose rules it enforces never names the member — src/ops/create_distribution.rs:229-236:
```
        // What is left is what the model cannot express: the origin has to name
        // a resource, its response timeout has to be within the documented
        // range, a default cache behaviour has to say what it is, the bundle
        // has to be a plan a new distribution may use, the caller-sized lists
        // have to be within their bounds, private origin access is only for a
        // bucket origin, a certificate the request names has to be an issued
        // certificate of this account, and the name must still be free. Each is
        // checked BEFORE the first write, so a refused request creates nothing.
```
and it does apply exactly this kind of documentation-only rule a few lines down — src/ops/create_distribution.rs:244-255:
```
        // "The minimum timeout is 1 second, the maximum is 60 seconds, and the
        // default (if you don't specify otherwise) is 30 seconds."
        ...
            Some(seconds) => {
                return Err(invalid(format!(
                    "origin.responseTimeout is {seconds} seconds, outside the \
                     {min_timeout}..{max_timeout} seconds a distribution waits for its origin"
```
The crate's precedent for a closed option list carried in a free-form string — src/ops/update_load_balancer_attribute.rs:280-288:
```
        crate::state::LOAD_BALANCER_ATTRIBUTE_TLS_POLICY_NAME => {
            let policy = value.trim();
            if !crate::state::publishes_load_balancer_tls_policy(policy) {
                return Err(invalid(crate::state::unpublished_tls_policy_message(
                    "attributeValue",
                    value,
                )));
```

Test coverage: every existing test sends values inside the documented domains, so adding the checks changes no existing response — tests/all/distribution_cdn.rs:234-244 (`"minimumTTL": 0`, `"allowedHTTPMethods": "GET,HEAD,OPTIONS"`, `"cachedHTTPMethods": "GET,HEAD"`, `forwardedHeaders.option = "allow-list"`) and tests/all/distribution_update.rs:225-244 (the same object, before and after an update). No test sends `forwardedHeaders.option = "all"`, a non-zero `minimumTTL` beside it, or a method string outside the lists.

## Steps to Reproduce

1. Sign a request for us-east-1 and CreateBucket (or CreateInstances) so the distribution has an origin to name — say bucket "cdn-origin".
2. CreateDistribution with distributionName "cdn-all-headers", origin {name: "cdn-origin", regionName: "us-east-1", protocolPolicy: "http-only"}, defaultCacheBehavior {behavior: "cache"}, bundleId "small_1_0", and cacheBehaviorSettings {"minimumTTL": 86400, "forwardedHeaders": {"option": "all"}} — answered 200.
3. GetDistributions: the distribution reports cacheBehaviorSettings.minimumTTL = 86400 with forwardedHeaders.option = "all", the combination the member's documentation says must carry a minimumTTL of 0.
4. CreateDistribution (or UpdateDistribution on the record above) with cacheBehaviorSettings {"allowedHTTPMethods": "DELETE", "cachedHTTPMethods": "GET,HEAD,OPTIONS,PUT,PATCH,POST,DELETE"} — also answered 200.
5. GetDistributions: both strings are reported verbatim, although neither is one of the options the two members document, so the distribution advertises a method set the service does not offer.
