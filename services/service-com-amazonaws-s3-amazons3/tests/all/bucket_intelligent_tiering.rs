// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's S3 INTELLIGENT-TIERING
//! CONFIGURATIONS: `PutBucketIntelligentTieringConfiguration` records one under the id the
//! request names and `ListBucketIntelligentTieringConfigurations` reports the ones the
//! bucket carries.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle suite owns), so what is pinned is the wire behaviour — status, body,
//! error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * A bucket carries MANY configurations, each identified by its own id: "Puts a S3
//!   Intelligent-Tiering configuration to the specified bucket. You can have up to 1,000 S3
//!   Intelligent-Tiering configurations per bucket"
//!   (`PutBucketIntelligentTieringConfiguration`'s own documentation), so a put under a new
//!   id ADDS one and a put under an id already carried replaces that one alone. A successful
//!   put answers [`PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS`] with an
//!   empty body.
//! * The payload IS the configuration: what a put names is what the listing reports back,
//!   member for member, and the configuration's own `Id` is the id the request identifies
//!   ("The ID used to identify the S3 Intelligent-Tiering configuration").
//! * The listing reports the configurations the bucket carries, in ascending id order, and
//!   PAGES: `IsTruncated` "indicates whether the returned list … is complete. A value of
//!   true indicates that the list is not complete and the NextContinuationToken will be
//!   provided for a subsequent request", and the request's `ContinuationToken` comes back
//!   on the response.
//! * A tiering's `Days` is bounded by what that member documents: "The minimum number of
//!   days specified for Archive Access tier must be at least 90 days and Deep Archive
//!   Access tier must be at least 180 days. The maximum can be up to 2 years (730 days)" —
//!   a value outside that range answers `InvalidAccessTier` (HTTP 400), since the range is
//!   the access tier's own, and an `Id` past the published 64-character maximum answers
//!   `InvalidConfigId` (HTTP 400), while a `Status` or an `AccessTier` outside its enum or
//!   a payload `Id` that is not the one the request identifies is refused with
//!   `InvalidArgument` (HTTP 400) — and nothing is recorded in any of those cases.
//! * A call naming a bucket this emulator does not hold answers `NoSuchBucket` (HTTP 404);
//!   one naming an `ExpectedBucketOwner` that is not the account's answers HTTP 403
//!   `AccessDenied`.
//! * The listing is a REPORT: two identical listings of an unchanged bucket answer the
//!   same thing.
//! * A configuration is also addressed ON ITS OWN by the id that identifies it:
//!   `GetBucketIntelligentTieringConfiguration` reports the one container the bucket
//!   carries under that id — the same entry the listing carries for it — and
//!   `DeleteBucketIntelligentTieringConfiguration` drops that one alone (HTTP 204, empty
//!   body), leaving every other configuration the bucket carries in place.
//! * A per-id call naming a configuration the bucket does not carry — one never put, one
//!   already deleted, or one only another bucket carries — answers `NoSuchConfiguration`
//!   (HTTP 404) and changes nothing.
//! * A per-id read reports the container the LATEST put under that id recorded, in full —
//!   the payload IS the configuration, so a replacement that drops a filter or a tiering
//!   reads back without it — and it reaches a configuration by its id whether or not the
//!   first page of the listing happens to carry that one.
//! * Once every configuration a bucket carried has been deleted, the bucket carries NONE:
//!   the listing reports an untruncated, EMPTY page — `IntelligentTieringConfigurationList`
//!   is `xmlFlattened` with `xmlName` `IntelligentTieringConfiguration`
//!   (`smithy_query(query="operation",operation="ListBucketIntelligentTieringConfigurations")`),
//!   so a page carrying no configuration carries no container at all, `IsTruncated` is
//!   false and no `NextContinuationToken` is handed back — whatever the bucket carried
//!   before and in whatever order the deletes arrived, and configurations another bucket
//!   carries under the same ids are left alone.

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_intelligent_tiering_configuration::PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    INVALID_ACCESS_TIER_CODE, INVALID_CONFIG_ID_CODE,
    MALFORMED_CONTINUATION_TOKEN_CODE, MALFORMED_XML_CODE,
    MAX_CONFIGURATION_ID_CHARS, MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET,
    NO_SUCH_BUCKET_CODE, NO_SUCH_CONFIGURATION_CODE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The two values `IntelligentTieringAccessTier` takes, and the fewest days `Tiering$Days`
/// documents for each: "the minimum number of days specified for Archive Access tier must
/// be at least 90 days and Deep Archive Access tier must be at least 180 days"
/// (`smithy_query(query="doc",shape="Tiering",member="Days")`, whose enum values come from
/// `smithy_query(query="shape",shape="IntelligentTieringAccessTier")`).
const ACCESS_TIER_FLOORS: [(&str, i32); 2] = [("ARCHIVE_ACCESS", 90), ("DEEP_ARCHIVE_ACCESS", 180)];

/// "The maximum can be up to 2 years (730 days)" — the same documentation, for either
/// tier.
const MAX_DAYS: i32 = 730;

/// One `Tiering` of a configuration: the days of no access and the tier they transition
/// to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TieringRow {
    days: i32,
    access_tier: String,
}

impl TieringRow {
    fn new(days: i32, access_tier: &str) -> Self {
        Self {
            days,
            access_tier: access_tier.to_string(),
        }
    }

    fn xml(&self) -> String {
        format!(
            "<Tiering><Days>{}</Days><AccessTier>{}</AccessTier></Tiering>",
            self.days, self.access_tier
        )
    }
}

/// An `IntelligentTieringFilter`, in the three forms the shape allows: a `Prefix`, a single
/// `Tag`, or an `And` operator carrying a prefix and tags
/// (`smithy_query(query="shape",shape="IntelligentTieringFilter")`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FilterRow {
    prefix: Option<String>,
    tag: Option<(String, String)>,
    and_prefix: Option<String>,
    and_tags: Vec<(String, String)>,
    /// Whether the filter carries an `And` container at all, so a filter whose `And` names
    /// only a prefix is distinguishable from one naming that prefix directly.
    has_and: bool,
}

impl FilterRow {
    fn prefix(prefix: &str) -> Self {
        Self {
            prefix: Some(prefix.to_string()),
            ..Self::default()
        }
    }

    fn tag(key: &str, value: &str) -> Self {
        Self {
            tag: Some((key.to_string(), value.to_string())),
            ..Self::default()
        }
    }

    fn and(prefix: Option<&str>, tags: &[(&str, &str)]) -> Self {
        Self {
            and_prefix: prefix.map(str::to_string),
            and_tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            has_and: true,
            ..Self::default()
        }
    }

    fn xml(&self) -> String {
        let mut body = String::from("<Filter>");
        if let Some(prefix) = &self.prefix {
            body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
        }
        if let Some((key, value)) = &self.tag {
            body.push_str(&format!(
                "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
            ));
        }
        if self.has_and {
            body.push_str("<And>");
            if let Some(prefix) = &self.and_prefix {
                body.push_str(&format!("<Prefix>{prefix}</Prefix>"));
            }
            for (key, value) in &self.and_tags {
                body.push_str(&format!(
                    "<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"
                ));
            }
            body.push_str("</And>");
        }
        body.push_str("</Filter>");
        body
    }
}

/// One `IntelligentTieringConfiguration`, as a request carries it and as the listing brings
/// it back.
///
/// The same type on both sides, so a round-trip assertion compares what was sent with what
/// was reported rather than with a hand-spelled expectation. The members are the ones
/// `smithy_query(query="shape",shape="IntelligentTieringConfiguration")` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ConfigurationRow {
    id: String,
    filter: Option<FilterRow>,
    status: String,
    tierings: Vec<TieringRow>,
}

impl ConfigurationRow {
    /// A configuration with the two required scalars and one tiering at its tier's floor.
    fn enabled(id: &str) -> Self {
        Self {
            id: id.to_string(),
            filter: None,
            status: "Enabled".to_string(),
            tierings: vec![TieringRow::new(
                ACCESS_TIER_FLOORS[0].1,
                ACCESS_TIER_FLOORS[0].0,
            )],
        }
    }

    fn with_status(mut self, status: &str) -> Self {
        self.status = status.to_string();
        self
    }

    fn with_filter(mut self, filter: FilterRow) -> Self {
        self.filter = Some(filter);
        self
    }

    fn with_tierings(mut self, tierings: Vec<TieringRow>) -> Self {
        self.tierings = tierings;
        self
    }

    /// The configuration as XML, in the member order the shape declares. `Tierings` is
    /// `xmlFlattened` with `xmlName` `Tiering`, so each entry is a bare `<Tiering>`.
    fn xml(&self) -> String {
        let mut body = format!("<IntelligentTieringConfiguration xmlns=\"{XMLNS}\">");
        body.push_str(&format!("<Id>{}</Id>", self.id));
        if let Some(filter) = &self.filter {
            body.push_str(&filter.xml());
        }
        body.push_str(&format!("<Status>{}</Status>", self.status));
        for tiering in &self.tierings {
            body.push_str(&tiering.xml());
        }
        body.push_str("</IntelligentTieringConfiguration>");
        body
    }
}

/// `PUT /<bucket>?intelligent-tiering&id=<id>` — `PutBucketIntelligentTieringConfiguration`
/// with `configuration` as its payload.
async fn put_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    configuration: &ConfigurationRow,
) -> Answer {
    put_payload(router, bucket, id, &configuration.xml(), &[]).await
}

/// The same put with the payload spelled out and any extra bound headers, for a container
/// no [`ConfigurationRow`] can express.
async fn put_payload(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?intelligent-tiering&id={id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>?intelligent-tiering` — `ListBucketIntelligentTieringConfigurations`,
/// optionally continuing at `token`.
async fn list_configurations(
    router: &MultiServiceRouter,
    bucket: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
) -> Answer {
    let uri = match token {
        None => format!("/{bucket}?intelligent-tiering"),
        Some(token) => format!("/{bucket}?intelligent-tiering&continuation-token={token}"),
    };
    send(router, "GET", &uri, headers, "").await
}

/// The configurations a listing body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// replication suite beside this one reads a container, rather than by slicing the body.
fn reported_configurations(answer: &Answer) -> Vec<ConfigurationRow> {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut configurations: Vec<ConfigurationRow> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                match name.as_str() {
                    "IntelligentTieringConfiguration" => {
                        configurations.push(ConfigurationRow::default())
                    }
                    "Filter" => {
                        if let Some(configuration) = configurations.last_mut() {
                            configuration.filter = Some(FilterRow::default());
                        }
                    }
                    "And" => {
                        if let Some(filter) = configurations
                            .last_mut()
                            .and_then(|configuration| configuration.filter.as_mut())
                        {
                            filter.has_and = true;
                        }
                    }
                    "Tiering" => {
                        if let Some(configuration) = configurations.last_mut() {
                            configuration
                                .tierings
                                .push(TieringRow::new(0, ""));
                        }
                    }
                    "Tag" => {
                        let under_and = path.iter().any(|element| element == "And");
                        if let Some(filter) = configurations
                            .last_mut()
                            .and_then(|configuration| configuration.filter.as_mut())
                        {
                            if under_and {
                                filter.and_tags.push((String::new(), String::new()));
                            } else {
                                filter.tag = Some((String::new(), String::new()));
                            }
                        }
                    }
                    _ => {}
                }
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let under_and = path.iter().any(|element| element == "And");
                let here: Vec<&str> = path.iter().map(String::as_str).collect();
                let Some(configuration) = configurations.last_mut() else {
                    continue;
                };
                // The member a value belongs to is its PATH, not its tag: `Prefix` sits
                // both directly on a `Filter` and inside its `And`, and `Tag` likewise.
                match here[here.len().saturating_sub(2)..] {
                    ["IntelligentTieringConfiguration", "Id"] => configuration.id = value,
                    ["IntelligentTieringConfiguration", "Status"] => configuration.status = value,
                    ["Tiering", "Days"] => {
                        if let Some(tiering) = configuration.tierings.last_mut() {
                            tiering.days = value.parse().unwrap_or_default();
                        }
                    }
                    ["Tiering", "AccessTier"] => {
                        if let Some(tiering) = configuration.tierings.last_mut() {
                            tiering.access_tier = value;
                        }
                    }
                    ["Filter", "Prefix"] | ["And", "Prefix"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                filter.and_prefix = Some(value);
                            } else {
                                filter.prefix = Some(value);
                            }
                        }
                    }
                    ["Tag", "Key"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                if let Some(tag) = filter.and_tags.last_mut() {
                                    tag.0 = value;
                                }
                            } else if let Some(tag) = filter.tag.as_mut() {
                                tag.0 = value;
                            }
                        }
                    }
                    ["Tag", "Value"] => {
                        if let Some(filter) = configuration.filter.as_mut() {
                            if under_and {
                                if let Some(tag) = filter.and_tags.last_mut() {
                                    tag.1 = value;
                                }
                            } else if let Some(tag) = filter.tag.as_mut() {
                                tag.1 = value;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    configurations
}

/// Whether a listing reports itself truncated.
fn is_truncated(answer: &Answer) -> bool {
    answer
        .element("IsTruncated")
        .unwrap_or_else(|| panic!("a listing reports IsTruncated, got {:?}", body_of(answer)))
        == "true"
}

/// The listing body as text, for an assertion message.
fn body_of(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).into_owned()
}

/// Stand up a bucket named `name`, which is what a configuration may be put on.
async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created, got {}",
        created.status
    );
}

/// The table of configurations every row of which must round-trip through the bucket
/// unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: which `Status`
/// the configuration carries; whether it names no filter, a prefix filter, a single tag, or
/// an `And` operator over a prefix and one or more tags; how MANY tierings it carries,
/// which tier each names, and whether a tiering's `Days` sits at its tier's documented
/// floor, in between, or at the published maximum.
fn round_trip_rows() -> Vec<(&'static str, ConfigurationRow)> {
    let (archive, archive_floor) = ACCESS_TIER_FLOORS[0];
    let (deep, deep_floor) = ACCESS_TIER_FLOORS[1];
    vec![
        (
            "the required members only, archive access at its floor",
            ConfigurationRow::enabled("minimal"),
        ),
        (
            "a disabled configuration",
            ConfigurationRow::enabled("disabled").with_status("Disabled"),
        ),
        (
            "deep archive access at its own floor",
            ConfigurationRow::enabled("deep-floor")
                .with_tierings(vec![TieringRow::new(deep_floor, deep)]),
        ),
        (
            "both tiers, the second at the published maximum",
            ConfigurationRow::enabled("both-tiers").with_tierings(vec![
                TieringRow::new(archive_floor, archive),
                TieringRow::new(MAX_DAYS, deep),
            ]),
        ),
        (
            "a prefix filter and a mid-range number of days",
            ConfigurationRow::enabled("prefix-filtered")
                .with_filter(FilterRow::prefix("logs/2026/"))
                .with_tierings(vec![TieringRow::new(365, archive)]),
        ),
        (
            "a single-tag filter",
            ConfigurationRow::enabled("tag-filtered")
                .with_filter(FilterRow::tag("archive", "yes")),
        ),
        (
            "an And operator over a prefix and two tags",
            ConfigurationRow::enabled("and-filtered")
                .with_filter(FilterRow::and(
                    Some("cold/"),
                    &[("tier", "deep"), ("team", "analytics")],
                ))
                .with_tierings(vec![TieringRow::new(deep_floor, deep)]),
        ),
        (
            "an And operator over tags alone",
            ConfigurationRow::enabled("and-tags-only")
                .with_filter(FilterRow::and(
                    None,
                    &[("retain", "long"), ("team", "ops")],
                ))
                .with_status("Disabled"),
        ),
        (
            // The published id ceiling itself: the longest id S3 records, which must round
            // trip like any other.
            "an id exactly as long as the published maximum",
            ConfigurationRow::enabled(&"i".repeat(MAX_CONFIGURATION_ID_CHARS)),
        ),
    ]
}

#[tokio::test]
async fn a_configuration_round_trips_through_the_bucket() {
    let router = router();

    for (index, (case, configuration)) in round_trip_rows().into_iter().enumerate() {
        let name = format!("itc-round-trip-{index}");
        bucket(&router, &name).await;

        let put = put_configuration(&router, &name, &configuration.id, &configuration).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: a successful put carries no body, got {:?}",
            body_of(&put)
        );

        let listed = list_configurations(&router, &name, None, &[]).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: expected the listing to answer 200, got {} ({})",
            listed.status,
            body_of(&listed)
        );
        let reported = reported_configurations(&listed);
        assert_eq!(
            reported,
            vec![configuration.clone()],
            "case {case:?}: expected the configuration that was put, got {reported:?}"
        );
        assert!(
            !is_truncated(&listed),
            "case {case:?}: a single-configuration listing is complete, got {:?}",
            body_of(&listed)
        );

        // A listing is a REPORT: reading it again answers the same thing.
        let again = reported_configurations(&list_configurations(&router, &name, None, &[]).await);
        assert_eq!(
            again, reported,
            "case {case:?}: a second listing must report the same configurations, got {again:?}"
        );
    }
}

#[tokio::test]
async fn a_bucket_carries_one_configuration_per_id() {
    let router = router();
    let name = "itc-many-ids";
    bucket(&router, name).await;

    let empty = list_configurations(&router, name, None, &[]).await;
    assert_eq!(
        empty.status,
        200,
        "a bucket carrying no configuration still lists, got {} ({})",
        empty.status,
        body_of(&empty)
    );
    let nothing = reported_configurations(&empty);
    assert!(
        nothing.is_empty(),
        "a bucket carrying no configuration reports none, got {nothing:?}"
    );

    // Put them in an order that is NOT the id order, so what is asserted is the listing's
    // own ordering rather than the order of arrival.
    let arrival = ["zeta", "alpha", "mu"];
    for id in arrival {
        let put = put_configuration(&router, name, id, &ConfigurationRow::enabled(id)).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    let listed = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    let ids: Vec<&str> = listed
        .iter()
        .map(|configuration| configuration.id.as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["alpha", "mu", "zeta"],
        "case {arrival:?}: expected every configuration once, in ascending id order, got {ids:?}"
    );

    // A put under an id the bucket already carries replaces THAT configuration and leaves
    // the others exactly as they were.
    let replacement = ConfigurationRow::enabled("mu")
        .with_status("Disabled")
        .with_filter(FilterRow::prefix("replaced/"));
    let again = put_configuration(&router, name, "mu", &replacement).await;
    assert_eq!(
        again.status,
        PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        "case \"replace mu\": expected the put to answer \
         {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
        again.status,
        body_of(&again)
    );
    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert_eq!(
        after,
        vec![
            ConfigurationRow::enabled("alpha"),
            replacement,
            ConfigurationRow::enabled("zeta"),
        ],
        "case \"replace mu\": expected the replacement in place of mu alone, got {after:?}"
    );
}

#[tokio::test]
async fn the_listing_pages_over_the_configurations() {
    let router = router();
    let name = "itc-paging";
    bucket(&router, name).await;

    // Enough configurations that the listing cannot report them in one page: the ids are
    // zero-padded so ascending id order is also ascending numeric order.
    let held = 101;
    for index in 0..held {
        let id = format!("configuration-{index:04}");
        let put = put_configuration(&router, name, &id, &ConfigurationRow::enabled(&id)).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    let first = list_configurations(&router, name, None, &[]).await;
    assert_eq!(
        first.status,
        200,
        "case \"first page\": expected 200, got {} ({})",
        first.status,
        body_of(&first)
    );
    let first_page = reported_configurations(&first);
    assert!(
        first_page.len() < held && !first_page.is_empty(),
        "case \"first page\": expected a partial page of the {held} configurations, got {}",
        first_page.len()
    );
    assert!(
        is_truncated(&first),
        "case \"first page\": expected IsTruncated true, got {:?}",
        body_of(&first)
    );
    assert!(
        first.element("ContinuationToken").is_none(),
        "case \"first page\": a request carrying no token is answered with none, got {:?}",
        first.element("ContinuationToken")
    );
    let token = first.element("NextContinuationToken").unwrap_or_else(|| {
        panic!(
            "case \"first page\": a truncated listing hands back a token, got {:?}",
            body_of(&first)
        )
    });

    let second = list_configurations(&router, name, Some(&token), &[]).await;
    assert_eq!(
        second.status,
        200,
        "case \"second page\": expected 200, got {} ({})",
        second.status,
        body_of(&second)
    );
    let second_page = reported_configurations(&second);
    assert!(
        !is_truncated(&second),
        "case \"second page\": the rest fits in one page, got {:?}",
        body_of(&second)
    );
    assert_eq!(
        second.element("ContinuationToken").as_deref(),
        Some(token.as_str()),
        "case \"second page\": expected the request's token reported back, got {:?}",
        second.element("ContinuationToken")
    );
    assert!(
        second.element("NextContinuationToken").is_none(),
        "case \"second page\": a complete listing hands back no token, got {:?}",
        second.element("NextContinuationToken")
    );

    // The two pages together are every configuration, each exactly once and in order.
    let paged: Vec<String> = first_page
        .iter()
        .chain(second_page.iter())
        .map(|configuration| configuration.id.clone())
        .collect();
    let expected: Vec<String> = (0..held)
        .map(|index| format!("configuration-{index:04}"))
        .collect();
    assert_eq!(
        paged, expected,
        "case \"both pages\": expected every configuration once, in id order, got {paged:?}"
    );

    // A token this listing never issued is not a cursor.
    let refused = list_configurations(&router, name, Some("not-a-token"), &[]).await;
    assert_eq!(
        refused.status,
        400,
        "case \"forged token\": expected 400, got {} ({})",
        refused.status,
        body_of(&refused)
    );
    assert_eq!(
        refused.error_code(),
        MALFORMED_CONTINUATION_TOKEN_CODE,
        "case \"forged token\": expected {MALFORMED_CONTINUATION_TOKEN_CODE}, got {}",
        refused.error_code()
    );
}

/// One refused-put row: the case, the id the request identifies, the payload it carries and
/// the error code the refusal must answer with — `None` where the value is refused at the
/// request boundary against the model (an enum member outside its value set), whose code is
/// the framework's own, so the row pins the 400 and that nothing was recorded.
type RefusalRow<'a> = (&'a str, String, String, Option<&'static str>);

/// The puts that must be refused, spanning the rule each refusal states: the payload id
/// must be the one the request identifies and no longer than the published maximum,
/// `Status` and `AccessTier` must name a value of their enum, and `Days` must sit inside
/// the range the access tier it names publishes — one row just below each tier's floor and
/// one just past the maximum.
fn refusal_rows() -> Vec<RefusalRow<'static>> {
    let (archive, archive_floor) = ACCESS_TIER_FLOORS[0];
    let (deep, deep_floor) = ACCESS_TIER_FLOORS[1];
    // One character past the published id ceiling, with the payload's own `Id` the same, so
    // the row is refused on the LENGTH rather than on the two ids disagreeing.
    let over_long = "i".repeat(MAX_CONFIGURATION_ID_CHARS + 1);
    vec![
        (
            "a payload naming another id",
            "wanted".to_string(),
            ConfigurationRow::enabled("other").xml(),
            Some(MALFORMED_XML_CODE),
        ),
        (
            "two tierings naming the same access tier",
            "duplicate-tier".to_string(),
            ConfigurationRow::enabled("duplicate-tier")
                .with_tierings(vec![
                    TieringRow::new(archive_floor, archive),
                    TieringRow::new(archive_floor + 30, archive),
                ])
                .xml(),
            Some(INVALID_ACCESS_TIER_CODE),
        ),
        (
            "deep archive access due no later than archive access",
            "deep-not-after".to_string(),
            ConfigurationRow::enabled("deep-not-after")
                .with_tierings(vec![
                    TieringRow::new(deep_floor, archive),
                    TieringRow::new(deep_floor, deep),
                ])
                .xml(),
            Some(INVALID_ACCESS_TIER_CODE),
        ),
        (
            "an And operator over a single predicate",
            "lone-predicate".to_string(),
            ConfigurationRow::enabled("lone-predicate")
                .with_filter(FilterRow::and(None, &[("retain", "long")]))
                .xml(),
            Some("InvalidAnd"),
        ),
        (
            "an And operator naming one tag key twice",
            "duplicate-key".to_string(),
            ConfigurationRow::enabled("duplicate-key")
                .with_filter(FilterRow::and(
                    Some("cold/"),
                    &[("team", "one"), ("team", "two")],
                ))
                .xml(),
            Some("InvalidAnd"),
        ),
        (
            "a filter prefix spelled empty",
            "empty-prefix".to_string(),
            ConfigurationRow::enabled("empty-prefix")
                .with_filter(FilterRow::prefix(""))
                .xml(),
            Some("InvalidPrefix"),
        ),
        (
            "a status outside the enum",
            "bad-status".to_string(),
            ConfigurationRow::enabled("bad-status")
                .with_status("Paused")
                .xml(),
            None,
        ),
        (
            "an access tier outside the enum",
            "bad-tier".to_string(),
            ConfigurationRow::enabled("bad-tier")
                .with_tierings(vec![TieringRow::new(200, "FROZEN_ACCESS")])
                .xml(),
            None,
        ),
        (
            "archive access one day below its floor",
            "archive-too-soon".to_string(),
            ConfigurationRow::enabled("archive-too-soon")
                .with_tierings(vec![TieringRow::new(archive_floor - 1, archive)])
                .xml(),
            Some(INVALID_ACCESS_TIER_CODE),
        ),
        (
            "deep archive access one day below its floor",
            "deep-too-soon".to_string(),
            ConfigurationRow::enabled("deep-too-soon")
                .with_tierings(vec![TieringRow::new(deep_floor - 1, deep)])
                .xml(),
            Some(INVALID_ACCESS_TIER_CODE),
        ),
        (
            "one day past the published maximum",
            "too-late".to_string(),
            ConfigurationRow::enabled("too-late")
                .with_tierings(vec![TieringRow::new(MAX_DAYS + 1, deep)])
                .xml(),
            Some(INVALID_ACCESS_TIER_CODE),
        ),
        (
            "an id one character past the published maximum",
            over_long.clone(),
            ConfigurationRow::enabled(&over_long).xml(),
            Some(INVALID_CONFIG_ID_CODE),
        ),
    ]
}

#[tokio::test]
async fn a_configuration_outside_its_published_bounds_is_refused() {
    let router = router();
    let name = "itc-refusals";
    bucket(&router, name).await;

    // One configuration the bucket carries throughout, so each refusal can be seen to have
    // left the bucket exactly as it was.
    let held = ConfigurationRow::enabled("held");
    let put = put_configuration(&router, name, &held.id, &held).await;
    assert_eq!(
        put.status,
        PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        "the held configuration must be recorded, got {} ({})",
        put.status,
        body_of(&put)
    );

    for (case, id, payload, code) in refusal_rows() {
        let refused = put_payload(&router, name, &id, &payload, &[]).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} ({})",
            refused.status,
            body_of(&refused)
        );
        if let Some(code) = code {
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}: expected {code}, got {}",
                refused.error_code()
            );
        }

        let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
        assert_eq!(
            after,
            vec![held.clone()],
            "case {case:?}: a refused put records nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn both_operations_refuse_a_bucket_that_is_not_the_accounts() {
    let router = router();
    let name = "itc-ownership";
    bucket(&router, name).await;

    // A bucket this emulator does not hold: both the put and the listing answer
    // `NoSuchBucket`.
    let configuration = ConfigurationRow::enabled("absent-bucket");
    for (case, answer) in [
        (
            "put on an absent bucket",
            put_configuration(&router, "itc-no-such-bucket", "absent-bucket", &configuration).await,
        ),
        (
            "list of an absent bucket",
            list_configurations(&router, "itc-no-such-bucket", None, &[]).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            404,
            "case {case:?}: expected 404, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            NO_SUCH_BUCKET_CODE,
            "case {case:?}: expected {NO_SUCH_BUCKET_CODE}, got {}",
            answer.error_code()
        );
    }

    // An expected owner that is not the account's: HTTP 403 `AccessDenied` from either
    // operation.
    let other_account = [("x-amz-expected-bucket-owner", "209876543210")];
    for (case, answer) in [
        (
            "put naming another owner",
            put_payload(
                &router,
                name,
                &configuration.id,
                &configuration.xml(),
                &other_account,
            )
            .await,
        ),
        (
            "list naming another owner",
            list_configurations(&router, name, None, &other_account).await,
        ),
    ] {
        assert_eq!(
            answer.status,
            403,
            "case {case:?}: expected 403, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {}",
            answer.error_code()
        );
    }

    // The refused put recorded nothing.
    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert!(
        after.is_empty(),
        "case \"put naming another owner\": a refused put records nothing, got {after:?}"
    );
}

#[tokio::test]
async fn the_per_bucket_ceiling_is_the_published_one() {
    // The ceiling the published documentation names, so a change to the constant cannot
    // quietly make the emulator hold a different number of configurations.
    assert_eq!(
        MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET, 1_000,
        "a bucket may carry up to 1,000 S3 Intelligent-Tiering configurations, got {MAX_INTELLIGENT_TIERING_CONFIGURATIONS_PER_BUCKET}"
    );
    // The published id ceiling, likewise: an S3 Intelligent-Tiering configuration id is up
    // to 64 characters long.
    assert_eq!(
        MAX_CONFIGURATION_ID_CHARS, 64,
        "a configuration id may carry up to 64 characters, got {MAX_CONFIGURATION_ID_CHARS}"
    );
}

/// The text S3 sends with `InvalidConfigId`, written out here rather than read from the
/// crate so that a change to the handler's wording fails this suite — the way the
/// subresource-message suite beside this one pins its published texts.
const PUBLISHED_INVALID_CONFIG_ID_MESSAGE: &str =
    "Config Id should not be more than 1 char and less than 64 chars in length";

/// The published id-length bound, tested at each end and one past it.
///
/// The bound's wording — "Config Id should be more than 1 character and less than 64
/// characters in length." — is not literal at either end: a 64-character id IS recorded, so
/// `64` is the inclusive maximum, and by the same reading a one-character id is inside the
/// bound while an empty one is not.
#[tokio::test]
async fn an_id_outside_the_published_length_bound_is_refused() {
    let router = router();
    let name = "itc-id-length";
    bucket(&router, name).await;

    // Inside the bound at each end: the shortest id and the longest one.
    for (case, length) in [
        ("the shortest id inside the bound", 1usize),
        (
            "an id exactly at the published maximum",
            MAX_CONFIGURATION_ID_CHARS,
        ),
    ] {
        let id = "i".repeat(length);
        let configuration = ConfigurationRow::enabled(&id);
        let put = put_configuration(&router, name, &id, &configuration).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
        let read = reported_configuration(&get_configuration(&router, name, &id, &[]).await);
        assert_eq!(
            read, configuration,
            "case {case:?}: the configuration must read back, got {read:?}"
        );
        let deleted = delete_configuration(&router, name, &id, &[]).await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: expected the delete to answer 204, got {} ({})",
            deleted.status,
            body_of(&deleted)
        );
    }

    // One past each end: nothing is recorded, and the refusal carries the fixed wording
    // published for the code, which names neither the id nor the bucket.
    for (case, length) in [
        ("an empty id", 0usize),
        (
            "an id one character past the published maximum",
            MAX_CONFIGURATION_ID_CHARS + 1,
        ),
    ] {
        let id = "i".repeat(length);
        let refused = put_payload(
            &router,
            name,
            &id,
            &ConfigurationRow::enabled(&id).xml(),
            &[],
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?}: expected 400, got {} ({})",
            refused.status,
            body_of(&refused)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_CONFIG_ID_CODE,
            "case {case:?}: expected {INVALID_CONFIG_ID_CODE}, got {}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            PUBLISHED_INVALID_CONFIG_ID_MESSAGE,
            "case {case:?}: expected {PUBLISHED_INVALID_CONFIG_ID_MESSAGE:?}, got {:?}",
            refused.error_message()
        );

        let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
        assert!(
            after.is_empty(),
            "case {case:?}: a refused put records nothing, got {after:?}"
        );
    }
}

/// One out-of-range tiering: the access tier it names, the days it asks for and the bound
/// the refusal must state.
struct DayBoundRow {
    case: &'static str,
    access_tier: &'static str,
    days: i32,
    bound: String,
}

#[tokio::test]
async fn a_tiering_outside_its_tiers_day_range_states_the_bound_it_missed() {
    let router = router();
    let name = "itc-day-bounds";
    bucket(&router, name).await;

    // Both tiers, both ends: the floor is the tier's own (90 days for one, 180 for the
    // other) and the maximum is the same for either, so the refusal names the tier it was
    // asked for and the bound that tier publishes.
    let mut rows: Vec<DayBoundRow> = Vec::new();
    for (access_tier, floor) in ACCESS_TIER_FLOORS {
        rows.push(DayBoundRow {
            case: "one day below the tier's floor",
            access_tier,
            days: floor - 1,
            bound: format!("should not be less than {floor}"),
        });
        rows.push(DayBoundRow {
            case: "one day past the published maximum",
            access_tier,
            days: MAX_DAYS + 1,
            bound: format!("should not be more than {MAX_DAYS}"),
        });
    }

    for row in &rows {
        let DayBoundRow {
            case,
            access_tier,
            days,
            bound,
        } = row;
        let id = format!("day-bound-{access_tier}-{days}");
        let configuration = ConfigurationRow::enabled(&id)
            .with_tierings(vec![TieringRow::new(*days, access_tier)]);
        let refused = put_configuration(&router, name, &id, &configuration).await;
        assert_eq!(
            refused.status,
            400,
            "case {case:?} on {access_tier}: expected 400, got {} ({})",
            refused.status,
            body_of(&refused)
        );
        assert_eq!(
            refused.error_code(),
            INVALID_ACCESS_TIER_CODE,
            "case {case:?} on {access_tier}: expected {INVALID_ACCESS_TIER_CODE}, got {}",
            refused.error_code()
        );
        let expected = format!("Days specified in {access_tier} tier {bound}.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?} on {access_tier}: expected {expected:?}, got {:?}",
            refused.error_message()
        );

        // A refused put records nothing.
        let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
        assert!(
            after.is_empty(),
            "case {case:?} on {access_tier}: a refused put records nothing, got {after:?}"
        );
    }
}

/// `GET /<bucket>?intelligent-tiering&id=<id>` —
/// `GetBucketIntelligentTieringConfiguration`, which addresses ONE configuration by its own
/// id rather than the bucket's whole set.
async fn get_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?intelligent-tiering&id={id}"),
        headers,
        "",
    )
    .await
}

/// `DELETE /<bucket>?intelligent-tiering&id=<id>` —
/// `DeleteBucketIntelligentTieringConfiguration`, likewise per-id.
async fn delete_configuration(
    router: &MultiServiceRouter,
    bucket: &str,
    id: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/{bucket}?intelligent-tiering&id={id}"),
        headers,
        "",
    )
    .await
}

/// The single configuration a per-id read reports.
///
/// `GetBucketIntelligentTieringConfigurationOutput$IntelligentTieringConfiguration` is the
/// `@httpPayload`, so the body IS one `<IntelligentTieringConfiguration>` container — read
/// with the same parser the listing is read with, so what is compared is the container, not
/// the envelope it arrived in.
fn reported_configuration(answer: &Answer) -> ConfigurationRow {
    let reported = reported_configurations(answer);
    assert_eq!(
        reported.len(),
        1,
        "a per-id read reports exactly one configuration, got {reported:?} from {:?}",
        body_of(answer)
    );
    reported.into_iter().next().expect("one configuration")
}

#[tokio::test]
async fn each_configuration_is_read_back_under_its_own_id() {
    let router = router();
    let name = "itc-per-id-read";
    bucket(&router, name).await;

    // Every row of the round-trip table on ONE bucket, so each per-id read is seen to
    // address its own configuration while the others are carried alongside it.
    let rows = round_trip_rows();
    for (case, configuration) in &rows {
        let put = put_configuration(&router, name, &configuration.id, configuration).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    // The listing is the other reader: a per-id read must report exactly the entry the
    // listing carries for that id, member for member.
    let listed = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert_eq!(
        listed.len(),
        rows.len(),
        "the bucket must carry every configuration that was put, got {listed:?}"
    );

    for (case, configuration) in &rows {
        let read = get_configuration(&router, name, &configuration.id, &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the per-id read to answer 200, got {} ({})",
            read.status,
            body_of(&read)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            &reported, configuration,
            "case {case:?}: expected the configuration that was put, got {reported:?}"
        );
        let from_listing = listed
            .iter()
            .find(|entry| entry.id == configuration.id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must carry an entry for this id, got {listed:?}")
            });
        assert_eq!(
            &reported, from_listing,
            "case {case:?}: the per-id read and the listing entry must agree, got \
             {reported:?} and {from_listing:?}"
        );

        // A read is a REPORT: reading again answers the same thing, and the bucket still
        // carries every configuration it did.
        let again = reported_configuration(&get_configuration(&router, name, &configuration.id, &[]).await);
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same configuration, got {again:?}"
        );
    }

    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert_eq!(
        after, listed,
        "the per-id reads must leave the bucket's configurations as they were, got {after:?}"
    );
}

#[tokio::test]
async fn deleting_a_configuration_removes_that_one_alone() {
    let router = router();
    let name = "itc-per-id-delete";
    bucket(&router, name).await;

    let rows = round_trip_rows();
    for (case, configuration) in &rows {
        let put = put_configuration(&router, name, &configuration.id, configuration).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    // Delete them one at a time, each one a different FORM of configuration, so what is
    // pinned is "the id this request names, and only that one" rather than one shape of
    // payload.
    let mut remaining: Vec<ConfigurationRow> =
        rows.iter().map(|(_, row)| row.clone()).collect();
    remaining.sort_by(|left, right| left.id.cmp(&right.id));

    for (case, configuration) in &rows {
        let deleted = delete_configuration(&router, name, &configuration.id, &[]).await;
        assert_eq!(
            deleted.status,
            204,
            "case {case:?}: expected the delete to answer 204, got {} ({})",
            deleted.status,
            body_of(&deleted)
        );
        assert!(
            deleted.body.is_empty(),
            "case {case:?}: a successful delete carries no body, got {:?}",
            body_of(&deleted)
        );

        remaining.retain(|row| row.id != configuration.id);

        // The deleted id is gone from both readers; every other configuration is untouched.
        let read = get_configuration(&router, name, &configuration.id, &[]).await;
        assert_eq!(
            read.status,
            404,
            "case {case:?}: a deleted configuration is no longer there, got {} ({})",
            read.status,
            body_of(&read)
        );
        assert_eq!(
            read.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: expected {NO_SUCH_CONFIGURATION_CODE}, got {}",
            read.error_code()
        );

        let listing = list_configurations(&router, name, None, &[]).await;
        let reported = reported_configurations(&listing);
        assert_eq!(
            reported, remaining,
            "case {case:?}: expected the configurations that were not deleted, got {reported:?}"
        );

        if remaining.is_empty() {
            // Nothing left: the listing reports no configuration container at all, rather
            // than an entry for one the bucket no longer carries.
            assert!(
                !body_of(&listing).contains("<IntelligentTieringConfiguration"),
                "case {case:?}: a bucket carrying none reports no configuration container, \
                 got {:?}",
                body_of(&listing)
            );
        }

        // A delete is not idempotent on the wire: the second one has nothing to delete.
        let repeated = delete_configuration(&router, name, &configuration.id, &[]).await;
        assert_eq!(
            repeated.status,
            404,
            "case {case:?}: a repeated delete has nothing to delete, got {} ({})",
            repeated.status,
            body_of(&repeated)
        );
        assert_eq!(
            repeated.error_code(),
            NO_SUCH_CONFIGURATION_CODE,
            "case {case:?}: expected {NO_SUCH_CONFIGURATION_CODE}, got {}",
            repeated.error_code()
        );
    }

    // And the bucket takes a configuration again afterwards, so the deletes left it usable.
    let fresh = ConfigurationRow::enabled("after-the-deletes");
    let put = put_configuration(&router, name, &fresh.id, &fresh).await;
    assert_eq!(
        put.status,
        PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        "a bucket whose configurations were all deleted still takes one, got {} ({})",
        put.status,
        body_of(&put)
    );
    let read = reported_configuration(&get_configuration(&router, name, &fresh.id, &[]).await);
    assert_eq!(
        read, fresh,
        "the configuration put after the deletes must read back, got {read:?}"
    );
}

/// Which per-id operation a refusal row drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PerIdOperation {
    Read,
    Delete,
}

/// One per-id request that must be refused, and the status and wire code it must answer.
struct PerIdRefusal<'a> {
    case: &'a str,
    /// Carried as data rather than inferred from [`Self::case`], so renaming a case cannot
    /// change which operation the row exercises.
    operation: PerIdOperation,
    bucket: &'a str,
    id: &'a str,
    headers: &'a [(&'a str, &'a str)],
    status: u16,
    code: &'a str,
}

#[tokio::test]
async fn a_per_id_call_naming_nothing_the_bucket_carries_is_refused() {
    let router = router();
    let name = "itc-per-id-refusals";
    bucket(&router, name).await;
    // A second bucket carrying its OWN configuration, so an id is seen to be scoped to the
    // bucket that carries it.
    let neighbour = "itc-per-id-neighbour";
    bucket(&router, neighbour).await;

    let held = ConfigurationRow::enabled("held");
    for on in [name, neighbour] {
        let put = put_configuration(&router, on, &held.id, &held).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "the held configuration must be recorded on {on:?}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }
    let only_on_the_neighbour = ConfigurationRow::enabled("neighbour-only");
    let put = put_configuration(
        &router,
        neighbour,
        &only_on_the_neighbour.id,
        &only_on_the_neighbour,
    )
    .await;
    assert_eq!(
        put.status,
        PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        "the neighbour's own configuration must be recorded, got {} ({})",
        put.status,
        body_of(&put)
    );

    // An expected owner that is not this account's.
    let other_account: &[(&str, &str)] = &[("x-amz-expected-bucket-owner", "209876543210")];
    let none: &[(&str, &str)] = &[];

    // The conditions differ along WHICH one refuses the call: an id the bucket never
    // carried, an id only another bucket carries, a bucket this emulator does not hold, and
    // an owner that is not the account's — each driven through the read AND the delete.
    let conditions = [
        (
            "an id the bucket never carried",
            name,
            "never-put",
            none,
            404,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "an id only another bucket carries",
            name,
            only_on_the_neighbour.id.as_str(),
            none,
            404,
            NO_SUCH_CONFIGURATION_CODE,
        ),
        (
            "a bucket this emulator does not hold",
            "itc-per-id-no-such-bucket",
            held.id.as_str(),
            none,
            404,
            NO_SUCH_BUCKET_CODE,
        ),
        (
            "an owner that is not the account's",
            name,
            held.id.as_str(),
            other_account,
            403,
            "AccessDenied",
        ),
    ];
    let mut rows: Vec<PerIdRefusal> = Vec::new();
    for (case, on, id, headers, status, code) in conditions {
        for operation in [PerIdOperation::Read, PerIdOperation::Delete] {
            rows.push(PerIdRefusal {
                case,
                operation,
                bucket: on,
                id,
                headers,
                status,
                code,
            });
        }
    }

    for row in &rows {
        let PerIdRefusal {
            case,
            operation,
            bucket: on,
            id,
            headers,
            status,
            code,
        } = row;
        let answer = match operation {
            PerIdOperation::Read => get_configuration(&router, on, id, headers).await,
            PerIdOperation::Delete => delete_configuration(&router, on, id, headers).await,
        };
        assert_eq!(
            answer.status, *status,
            "case {case:?} via {operation:?}: expected HTTP {status}, got {} ({})",
            answer.status,
            body_of(&answer)
        );
        assert_eq!(
            answer.error_code(),
            *code,
            "case {case:?} via {operation:?}: expected {code}, got {}",
            answer.error_code()
        );
        // The wording is a constant for the condition: nothing the request named reaches
        // it.
        for fragment in [*on, *id] {
            assert!(
                !answer.error_message().contains(fragment),
                "case {case:?} via {operation:?}: the message must carry nothing from the \
                 request, but {fragment:?} is in {:?}",
                answer.error_message()
            );
        }

        // A refused call deletes nothing and reports nothing: both buckets still carry
        // exactly what they did.
        let still_held = get_configuration(&router, name, &held.id, &[]).await;
        assert_eq!(
            still_held.status,
            200,
            "case {case:?} via {operation:?}: a refused call must leave the held \
             configuration, got {} ({})",
            still_held.status,
            body_of(&still_held)
        );
        assert_eq!(
            reported_configuration(&still_held),
            held,
            "case {case:?} via {operation:?}: the held configuration must be unchanged"
        );
        let neighbours =
            reported_configurations(&list_configurations(&router, neighbour, None, &[]).await);
        assert_eq!(
            neighbours.len(),
            2,
            "case {case:?} via {operation:?}: the neighbouring bucket must keep both its \
             configurations, got {neighbours:?}"
        );
    }
}

/// The order one row of [`deleting_every_configuration_leaves_the_bucket_carrying_none`]
/// deletes the configurations the bucket carried in.
///
/// A delete is addressed by id alone, so the order the deletes arrive in must not change
/// where the bucket ends up: these are the orders the rows walk.
#[derive(Debug, Clone, Copy)]
enum DeletionOrder {
    /// The order the puts recorded them in, which is not the store's own order.
    AsPut,
    AscendingId,
    DescendingId,
}

#[tokio::test]
async fn deleting_every_configuration_leaves_the_bucket_carrying_none() {
    let router = router();

    // The rows differ along what the rule generalises over: HOW MANY configurations the
    // bucket carried, which FORM each one took (the round-trip table's rows, prefix- and
    // tag-filtered containers among them), and the ORDER the deletes arrived in.
    let table = round_trip_rows();
    let rows: Vec<(usize, DeletionOrder)> = vec![
        (1, DeletionOrder::AsPut),
        (2, DeletionOrder::DescendingId),
        (3, DeletionOrder::AscendingId),
        (table.len(), DeletionOrder::AsPut),
        (table.len(), DeletionOrder::DescendingId),
    ];

    for (index, (count, order)) in rows.into_iter().enumerate() {
        let case = format!("{count} configuration(s), deleted {order:?}");
        let name = format!("itc-empty-after-delete-{index}");
        bucket(&router, &name).await;
        // A second bucket carrying the SAME ids, so what is pinned is "the configurations
        // of the bucket the request names" rather than "every configuration under that id".
        let neighbour = format!("itc-empty-after-delete-neighbour-{index}");
        bucket(&router, &neighbour).await;

        let carried: Vec<ConfigurationRow> = table
            .iter()
            .take(count)
            .map(|(_, configuration)| configuration.clone())
            .collect();
        for configuration in &carried {
            for on in [&name, &neighbour] {
                let put = put_configuration(&router, on, &configuration.id, configuration).await;
                assert_eq!(
                    put.status,
                    PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
                    "case {case:?}: expected the put of {:?} on {on} to answer \
                     {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
                    configuration.id,
                    put.status,
                    body_of(&put)
                );
            }
        }

        let mut deletes = carried.clone();
        match order {
            DeletionOrder::AsPut => {}
            DeletionOrder::AscendingId => deletes.sort_by(|left, right| left.id.cmp(&right.id)),
            DeletionOrder::DescendingId => deletes.sort_by(|left, right| right.id.cmp(&left.id)),
        }

        for (position, configuration) in deletes.iter().enumerate() {
            let deleted = delete_configuration(&router, &name, &configuration.id, &[]).await;
            assert_eq!(
                deleted.status,
                204,
                "case {case:?}: expected the delete of {:?} to answer 204, got {} ({})",
                configuration.id,
                deleted.status,
                body_of(&deleted)
            );
            assert!(
                deleted.body.is_empty(),
                "case {case:?}: a successful delete of {:?} carries no body, got {:?}",
                configuration.id,
                body_of(&deleted)
            );

            // Every configuration not yet deleted is still reported, in ascending id order:
            // each delete drops exactly the one it names.
            let mut expected: Vec<ConfigurationRow> = deletes[position + 1..].to_vec();
            expected.sort_by(|left, right| left.id.cmp(&right.id));
            let listing = list_configurations(&router, &name, None, &[]).await;
            let reported = reported_configurations(&listing);
            assert_eq!(
                reported, expected,
                "case {case:?}: after deleting {:?} the bucket must carry {expected:?}, got \
                 {reported:?}",
                configuration.id
            );
        }

        // The last delete left the bucket carrying none: one untruncated, EMPTY page.
        let listing = list_configurations(&router, &name, None, &[]).await;
        assert_eq!(
            listing.status,
            200,
            "case {case:?}: a bucket carrying no configuration still lists, got {} ({})",
            listing.status,
            body_of(&listing)
        );
        assert!(
            !body_of(&listing).contains("<IntelligentTieringConfiguration"),
            "case {case:?}: a page carrying no configuration carries no container, got {:?}",
            body_of(&listing)
        );
        assert!(
            !is_truncated(&listing),
            "case {case:?}: an empty page is complete, got {:?}",
            body_of(&listing)
        );
        assert!(
            listing.element("NextContinuationToken").is_none(),
            "case {case:?}: a complete page hands back no cursor, got {:?}",
            body_of(&listing)
        );

        // Each id is gone per-id as well, and a repeated delete has nothing left to delete.
        for configuration in &carried {
            let read = get_configuration(&router, &name, &configuration.id, &[]).await;
            assert_eq!(
                read.status,
                404,
                "case {case:?}: {:?} must be gone, got {} ({})",
                configuration.id,
                read.status,
                body_of(&read)
            );
            assert_eq!(
                read.error_code(),
                NO_SUCH_CONFIGURATION_CODE,
                "case {case:?}: expected {NO_SUCH_CONFIGURATION_CODE} for {:?}, got {}",
                configuration.id,
                read.error_code()
            );
            let repeated = delete_configuration(&router, &name, &configuration.id, &[]).await;
            assert_eq!(
                repeated.status,
                404,
                "case {case:?}: a repeated delete of {:?} has nothing to delete, got {} ({})",
                configuration.id,
                repeated.status,
                body_of(&repeated)
            );
            assert_eq!(
                repeated.error_code(),
                NO_SUCH_CONFIGURATION_CODE,
                "case {case:?}: expected {NO_SUCH_CONFIGURATION_CODE} for the repeated delete \
                 of {:?}, got {}",
                configuration.id,
                repeated.error_code()
            );
        }

        // The neighbouring bucket kept every one of them: the deletes were scoped to the
        // bucket they named.
        let mut untouched = carried.clone();
        untouched.sort_by(|left, right| left.id.cmp(&right.id));
        let neighbours =
            reported_configurations(&list_configurations(&router, &neighbour, None, &[]).await);
        assert_eq!(
            neighbours, untouched,
            "case {case:?}: the neighbouring bucket must still carry {untouched:?}, got \
             {neighbours:?}"
        );
    }
}

#[tokio::test]
async fn a_per_id_read_reports_the_latest_configuration_recorded_under_that_id() {
    let router = router();
    let name = "itc-per-id-replacement";
    bucket(&router, name).await;

    // A second id the bucket carries throughout, so each replacement is seen to address the
    // id its put named and nothing else.
    let untouched = ConfigurationRow::enabled("untouched");
    let put = put_configuration(&router, name, &untouched.id, &untouched).await;
    assert_eq!(
        put.status,
        PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
        "the untouched configuration must be recorded, got {} ({})",
        put.status,
        body_of(&put)
    );

    let id = "replaced";
    let (archive, archive_floor) = ACCESS_TIER_FLOORS[0];
    let (deep, deep_floor) = ACCESS_TIER_FLOORS[1];
    // Each row replaces the one before it under the SAME id, and the rows differ along what
    // the container may carry: its `Status`, which form of `Filter` it names (none, a
    // prefix, a single tag, an `And` over a prefix and a tag), how many tierings it carries
    // and where each one's `Days` sits in its tier's published range — including a row that
    // DROPS the filter and the extra tiering a previous row added, so what is pinned is the
    // latest container in full rather than a merge of what the id ever carried.
    let rows: Vec<(&str, ConfigurationRow)> = vec![
        ("the required members only", ConfigurationRow::enabled(id)),
        (
            "a prefix filter added",
            ConfigurationRow::enabled(id).with_filter(FilterRow::prefix("cold/")),
        ),
        (
            "disabled, with a single-tag filter",
            ConfigurationRow::enabled(id)
                .with_status("Disabled")
                .with_filter(FilterRow::tag("archive", "yes")),
        ),
        (
            "two tierings, the second at the published maximum",
            ConfigurationRow::enabled(id).with_tierings(vec![
                TieringRow::new(archive_floor, archive),
                TieringRow::new(MAX_DAYS, deep),
            ]),
        ),
        (
            "an And filter over a prefix and a tag, deep archive at its floor",
            ConfigurationRow::enabled(id)
                .with_filter(FilterRow::and(Some("cold/"), &[("tier", "deep")]))
                .with_tierings(vec![TieringRow::new(deep_floor, deep)]),
        ),
        (
            "back to the required members only",
            ConfigurationRow::enabled(id),
        ),
    ];

    for (case, configuration) in &rows {
        let put = put_configuration(&router, name, id, configuration).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {case:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );

        let read = get_configuration(&router, name, id, &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the per-id read to answer 200, got {} ({})",
            read.status,
            body_of(&read)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            &reported, configuration,
            "case {case:?}: expected the container the latest put recorded, got {reported:?}"
        );

        // A replacement is not an addition: the bucket still carries exactly the two ids,
        // and the other one reads back as it was put.
        let mut expected = vec![configuration.clone(), untouched.clone()];
        expected.sort_by(|left, right| left.id.cmp(&right.id));
        let listed = reported_configurations(&list_configurations(&router, name, None, &[]).await);
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the replacement beside the untouched configuration, got \
             {listed:?}"
        );
        let other = reported_configuration(&get_configuration(&router, name, &untouched.id, &[]).await);
        assert_eq!(
            other, untouched,
            "case {case:?}: the other id must be untouched, got {other:?}"
        );
    }
}

#[tokio::test]
async fn a_per_id_read_reaches_a_configuration_the_first_listing_page_does_not_carry() {
    let router = router();
    let name = "itc-per-id-beyond-the-page";
    bucket(&router, name).await;

    // More configurations than one listing page reports, so the ids the rows read span the
    // page break: a per-id read addresses ONE configuration by the id that identifies it,
    // so where the listing happens to break is nothing to it. The ids are zero-padded, so
    // ascending id order is also ascending numeric order.
    let held = 101usize;
    for index in 0..held {
        let id = format!("configuration-{index:04}");
        let put = put_configuration(&router, name, &id, &ConfigurationRow::enabled(&id)).await;
        assert_eq!(
            put.status,
            PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            "case {id:?}: expected the put to answer \
             {PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS}, got {} ({})",
            put.status,
            body_of(&put)
        );
    }

    let first = list_configurations(&router, name, None, &[]).await;
    let first_page = reported_configurations(&first);
    assert!(
        !first_page.is_empty() && first_page.len() < held,
        "the {held} configurations must not fit in one page, got {}",
        first_page.len()
    );
    let on_the_first_page = first_page.len();

    for (case, index) in [
        ("the first id of all", 0),
        ("the last id the first page carries", on_the_first_page - 1),
        ("the first id past the page break", on_the_first_page),
        ("the last id of all", held - 1),
    ] {
        let id = format!("configuration-{index:04}");
        let read = get_configuration(&router, name, &id, &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?} ({id:?}): expected the per-id read to answer 200, got {} ({})",
            read.status,
            body_of(&read)
        );
        let reported = reported_configuration(&read);
        assert_eq!(
            reported,
            ConfigurationRow::enabled(&id),
            "case {case:?} ({id:?}): expected the configuration that was put, got {reported:?}"
        );
    }

    // A read is a REPORT: an id the bucket does not carry is refused and configures
    // nothing, so the bucket carries exactly what it did and the id stays absent.
    let missing = format!("configuration-{held:04}");
    let refused = get_configuration(&router, name, &missing, &[]).await;
    assert_eq!(
        refused.status,
        404,
        "case \"an id the bucket does not carry\": expected 404, got {} ({})",
        refused.status,
        body_of(&refused)
    );
    assert_eq!(
        refused.error_code(),
        NO_SUCH_CONFIGURATION_CODE,
        "case \"an id the bucket does not carry\": expected {NO_SUCH_CONFIGURATION_CODE}, got {}",
        refused.error_code()
    );
    let after = reported_configurations(&list_configurations(&router, name, None, &[]).await);
    assert_eq!(
        after, first_page,
        "a refused read must leave the bucket's configurations as they were, got {after:?}"
    );
    let again = get_configuration(&router, name, &missing, &[]).await;
    assert_eq!(
        again.status,
        404,
        "case \"an id the bucket does not carry, read twice\": expected 404, got {} ({})",
        again.status,
        body_of(&again)
    );
}
