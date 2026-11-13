// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the link slice (`CreateLink` / `GetLinks`), driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: a link is only ever read back through `GetLinks`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::global_network_lifecycle::{
    error_code, expected_scoped_arn, ids_of, is_minted_id, Emulator, PAST_SETTLE,
};

/// One row of the create/round-trip table: the members the request carries, with
/// `Value::Null` standing for "this row omits the member".
struct LinkCase {
    name: &'static str,
    bandwidth: Value,
    description: Value,
    link_type: Value,
    provider: Value,
    tags: Value,
}

impl LinkCase {
    /// The `CreateLink` body this row sends: `Bandwidth` always (the model makes it
    /// required), every other member only when the row carries it.
    fn body(&self) -> Value {
        let mut body = serde_json::Map::new();
        body.insert("Bandwidth".to_string(), self.bandwidth.clone());
        for (member, value) in [
            ("Description", &self.description),
            ("Type", &self.link_type),
            ("Provider", &self.provider),
            ("Tags", &self.tags),
        ] {
            if !value.is_null() {
                body.insert(member.to_string(), value.clone());
            }
        }
        Value::Object(body)
    }

    /// The `Tags` a read must answer: the list the row sent, or the empty list when it
    /// sent none (a link always has a tag list, possibly empty).
    fn expected_tags(&self) -> Value {
        if self.tags.is_null() {
            json!([])
        } else {
            self.tags.clone()
        }
    }
}

/// The rows of the create/round-trip table. They differ along the form of `Bandwidth`
/// (both speeds, one speed, neither, zero, the largest `Integer`) and along the size,
/// character set and presence of each free-form descriptor — everything the storage rule
/// generalises over. `Bandwidth`'s two members are both optional
/// (`smithy_query shape="Bandwidth"`), and 256 is the modelled maximum of `Description`
/// (`ConstrainedString`, len 0..256) while 128 is the documented maximum of `Type` and
/// `Provider` (`smithy_query query="doc" shape="CreateLinkRequest" member="Provider"`).
fn link_cases() -> Vec<LinkCase> {
    vec![
        LinkCase {
            name: "both speeds and every descriptor",
            bandwidth: json!({ "UploadSpeed": 100, "DownloadSpeed": 200 }),
            description: json!("primary uplink"),
            link_type: json!("broadband"),
            provider: json!("AnyCompany Networks"),
            tags: json!([{ "Key": "env", "Value": "test" }, { "Key": "tier", "Value": "primary" }]),
        },
        LinkCase {
            name: "upload speed only",
            bandwidth: json!({ "UploadSpeed": 50 }),
            description: Value::Null,
            link_type: Value::Null,
            provider: Value::Null,
            tags: Value::Null,
        },
        LinkCase {
            name: "download speed only",
            bandwidth: json!({ "DownloadSpeed": 1 }),
            description: json!("downstream only"),
            link_type: json!("mpls"),
            provider: Value::Null,
            tags: json!([]),
        },
        LinkCase {
            name: "no speeds at all",
            bandwidth: json!({}),
            description: Value::Null,
            link_type: json!("dark-fiber"),
            provider: json!("AnyCompany"),
            tags: Value::Null,
        },
        LinkCase {
            name: "zero speeds",
            bandwidth: json!({ "UploadSpeed": 0, "DownloadSpeed": 0 }),
            description: json!(""),
            link_type: json!(""),
            provider: json!(""),
            tags: Value::Null,
        },
        LinkCase {
            name: "the largest Integer speeds",
            bandwidth: json!({ "UploadSpeed": i32::MAX, "DownloadSpeed": i32::MAX }),
            description: json!("line-rate"),
            link_type: json!("ethernet"),
            provider: json!("AnyCompany"),
            tags: Value::Null,
        },
        LinkCase {
            name: "descriptors at their published maximum length",
            bandwidth: json!({ "UploadSpeed": 10, "DownloadSpeed": 10 }),
            description: json!("d".repeat(256)),
            link_type: json!("t".repeat(128)),
            provider: json!("p".repeat(128)),
            tags: Value::Null,
        },
        LinkCase {
            name: "multi-line and non-ascii descriptors",
            bandwidth: json!({ "UploadSpeed": 7, "DownloadSpeed": 9 }),
            description: json!("line one\nline two"),
            link_type: json!("fibre \u{1f310}"),
            provider: json!("Ünternet"),
            tags: json!([{ "Key": "ключ", "Value": "значение" }]),
        },
    ]
}

/// A `Link` without its `State`, so a create response and a later read can be compared
/// member for member without racing the `PENDING` -> `AVAILABLE` transition the clock
/// owes (which its own test below covers).
fn without_state(link: &Value) -> Value {
    let mut copy = link.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("State");
    }
    copy
}

/// `CreateLink` against `site` of `network`.
async fn create_link(emulator: &Emulator, network: &str, site: &str, body: &Value) -> (u16, Value) {
    let mut request = body.clone();
    if let Some(object) = request.as_object_mut() {
        object.insert("SiteId".to_string(), json!(site));
    }
    emulator
        .post(&format!("/global-networks/{network}/links"), request)
        .await
}

/// `CreateLink` that must succeed, answering the minted `LinkId`.
pub(crate) async fn create_link_ok(
    emulator: &Emulator,
    network: &str,
    site: &str,
    body: &Value,
) -> String {
    let (status, created) = create_link(emulator, network, site, body).await;
    assert_eq!(
        status, 200,
        "CreateLink({body}): expected 200, got {status} with {created}"
    );
    created["Link"]["LinkId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateLink({body}): no LinkId in {created}"))
        .to_string()
}

/// A link body carrying just the required `Bandwidth`, for tests about something else.
pub(crate) fn plain_link() -> Value {
    json!({ "Bandwidth": { "UploadSpeed": 10, "DownloadSpeed": 20 } })
}

#[tokio::test]
async fn create_link_stores_every_member_and_get_links_answers_from_the_stored_record() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("links").await;
    let site = emulator.create_site(&network, "hq").await;

    let mut minted: Vec<String> = Vec::new();
    for case in link_cases() {
        let name = case.name;
        let (status, created) = create_link(&emulator, &network, &site, &case.body()).await;
        assert_eq!(
            status, 200,
            "case {name}: CreateLink expected 200, got {status} with {created}"
        );
        let link = created["Link"].clone();
        let link_id = link["LinkId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name}: no LinkId in {created}"))
            .to_string();
        assert!(
            is_minted_id(&link_id, "link"),
            "case {name}: expected a link-<17 hex> id, got {link_id:?}"
        );
        assert!(
            !minted.contains(&link_id),
            "case {name}: id {link_id:?} was already minted for another link"
        );
        assert_eq!(
            link["LinkArn"],
            json!(expected_scoped_arn("link", &network, &link_id)),
            "case {name}: the ARN must name the global network and the minted id, got {link}"
        );
        assert_eq!(
            link["GlobalNetworkId"],
            json!(network.clone()),
            "case {name}: the link must back-reference its global network, got {link}"
        );
        assert_eq!(
            link["SiteId"],
            json!(site.clone()),
            "case {name}: the link must back-reference its site, got {link}"
        );
        assert_eq!(
            link["Bandwidth"], case.bandwidth,
            "case {name}: Bandwidth must be stored as sent, got {link}"
        );
        assert_eq!(
            link["Description"], case.description,
            "case {name}: Description must be stored as sent, got {link}"
        );
        assert_eq!(
            link["Type"], case.link_type,
            "case {name}: Type must be stored as sent, got {link}"
        );
        assert_eq!(
            link["Provider"], case.provider,
            "case {name}: Provider must be stored as sent, got {link}"
        );
        assert_eq!(
            link["Tags"],
            case.expected_tags(),
            "case {name}: Tags must be stored as sent, got {link}"
        );
        assert_eq!(
            link["State"],
            json!("PENDING"),
            "case {name}: a caller acting immediately must still see PENDING, got {link}"
        );
        assert!(
            link["CreatedAt"].as_f64().is_some_and(|at| at > 0.0),
            "case {name}: CreatedAt must be a timestamp, got {link}"
        );

        // ROUND TRIP: the stored record is what the reader answers, member for member.
        let (status, listed) = emulator
            .get(&format!(
                "/global-networks/{network}/links?linkIds={link_id}"
            ))
            .await;
        assert_eq!(
            status, 200,
            "case {name}: GetLinks expected 200, got {status} with {listed}"
        );
        assert_eq!(
            ids_of(&listed, "Links", "LinkId"),
            vec![link_id.clone()],
            "case {name}: GetLinks must answer exactly the created link, got {listed}"
        );
        assert_eq!(
            without_state(&listed["Links"][0]),
            without_state(&link),
            "case {name}: GetLinks must answer the stored record, got {listed}"
        );

        minted.push(link_id);
    }

    // Every link of the site is readable at once, in key order, each exactly once.
    let (_, listed) = emulator
        .get(&format!("/global-networks/{network}/links?maxResults=500"))
        .await;
    let mut expected = minted.clone();
    expected.sort();
    assert_eq!(
        ids_of(&listed, "Links", "LinkId"),
        expected,
        "GetLinks must answer every stored link of the global network, got {listed}"
    );
}

#[tokio::test]
async fn get_links_filters_by_every_member_the_request_names() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("filters").await;
    let other_network = emulator.create_global_network("other").await;
    let site_a = emulator.create_site(&network, "a").await;
    let site_b = emulator.create_site(&network, "b").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    let bandwidth = json!({ "UploadSpeed": 10, "DownloadSpeed": 20 });
    let broadband_p1 = create_link_ok(
        &emulator,
        &network,
        &site_a,
        &json!({ "Bandwidth": bandwidth, "Type": "broadband", "Provider": "P1" }),
    )
    .await;
    let fiber_p2 = create_link_ok(
        &emulator,
        &network,
        &site_a,
        &json!({ "Bandwidth": bandwidth, "Type": "dark-fiber", "Provider": "P2" }),
    )
    .await;
    let broadband_p2 = create_link_ok(
        &emulator,
        &network,
        &site_b,
        &json!({ "Bandwidth": bandwidth, "Type": "broadband", "Provider": "P2" }),
    )
    .await;
    let undescribed = create_link_ok(&emulator, &network, &site_b, &plain_link()).await;
    let foreign = create_link_ok(&emulator, &other_network, &other_site, &plain_link()).await;

    let all = {
        let mut ids = vec![
            broadband_p1.clone(),
            fiber_p2.clone(),
            broadband_p2.clone(),
            undescribed.clone(),
        ];
        ids.sort();
        ids
    };

    // Rows differ along which filter is used, how many links it selects (every one, a
    // subset, exactly one, none) and whether the filtered member is even set on a link.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        ("no filter", String::new(), all.clone()),
        (
            "siteId",
            format!("siteId={site_a}"),
            sorted(vec![broadband_p1.clone(), fiber_p2.clone()]),
        ),
        (
            "siteId of the other site",
            format!("siteId={site_b}"),
            sorted(vec![broadband_p2.clone(), undescribed.clone()]),
        ),
        (
            "type",
            "type=broadband".to_string(),
            sorted(vec![broadband_p1.clone(), broadband_p2.clone()]),
        ),
        (
            "provider",
            "provider=P2".to_string(),
            sorted(vec![fiber_p2.clone(), broadband_p2.clone()]),
        ),
        (
            "type and provider together",
            "type=broadband&provider=P2".to_string(),
            vec![broadband_p2.clone()],
        ),
        (
            "type and provider that no link combines",
            "type=dark-fiber&provider=P1".to_string(),
            vec![],
        ),
        ("a type no link carries", "type=satellite".to_string(), vec![]),
        (
            "linkIds naming one link",
            format!("linkIds={fiber_p2}"),
            vec![fiber_p2.clone()],
        ),
        (
            "linkIds naming several links",
            format!("linkIds={broadband_p1}&linkIds={undescribed}"),
            sorted(vec![broadband_p1.clone(), undescribed.clone()]),
        ),
        (
            "linkIds naming a link of another global network",
            format!("linkIds={foreign}"),
            vec![],
        ),
        (
            "linkIds naming nothing stored",
            "linkIds=link-00000000000000000".to_string(),
            vec![],
        ),
        (
            "linkIds and a type the named link does not carry",
            format!("linkIds={fiber_p2}&type=broadband"),
            vec![],
        ),
    ];

    for (case, query, want) in cases {
        let uri = if query.is_empty() {
            format!("/global-networks/{network}/links")
        } else {
            format!("/global-networks/{network}/links?{query}")
        };
        let (status, body) = emulator.get(&uri).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            ids_of(&body, "Links", "LinkId"),
            want,
            "case {case}: GetLinks returned the wrong set, got {body}"
        );
    }

    // A listing never leaks across global networks.
    let (_, body) = emulator
        .get(&format!("/global-networks/{other_network}/links"))
        .await;
    assert_eq!(
        ids_of(&body, "Links", "LinkId"),
        vec![foreign],
        "GetLinks must answer only the links of the global network it addresses, got {body}"
    );
}

/// `ids` sorted, which is the order the reader answers in (its store is keyed by id).
fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[tokio::test]
async fn get_links_refuses_site_id_combined_with_type_or_provider() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("exclusive").await;
    let site = emulator.create_site(&network, "hq").await;
    create_link_ok(
        &emulator,
        &network,
        &site,
        &json!({ "Bandwidth": { "UploadSpeed": 1 }, "Type": "broadband", "Provider": "P1" }),
    )
    .await;

    // "If you specify the site ID, you cannot specify the type or provider in the same
    // request. You can specify the type and provider in the same request."
    // (`smithy_query query="doc" shape="GetLinks"`). The rows span every combination of
    // the three filters.
    let cases: Vec<(&str, String, u16)> = vec![
        ("siteId alone", format!("siteId={site}"), 200),
        ("type alone", "type=broadband".to_string(), 200),
        ("provider alone", "provider=P1".to_string(), 200),
        (
            "type and provider",
            "type=broadband&provider=P1".to_string(),
            200,
        ),
        (
            "siteId and type",
            format!("siteId={site}&type=broadband"),
            400,
        ),
        (
            "siteId and provider",
            format!("siteId={site}&provider=P1"),
            400,
        ),
        (
            "siteId, type and provider",
            format!("siteId={site}&type=broadband&provider=P1"),
            400,
        ),
    ];

    for (case, query, want) in cases {
        let (status, body) = emulator
            .get(&format!("/global-networks/{network}/links?{query}"))
            .await;
        assert_eq!(
            status, want, "case {case}: expected {want}, got {status} with {body}"
        );
        if want == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: wrong error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn create_link_refuses_a_type_or_provider_outside_the_documented_constraints() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("descriptors").await;
    let site = emulator.create_site(&network, "hq").await;

    // "Maximum length of 128 characters. Cannot include the following characters: | \ ^"
    // (`smithy_query query="doc" shape="CreateLinkRequest" member="Provider"`, the same
    // sentence for `Type`). The rows carry each forbidden character, the bound itself and
    // one past it — for both members the rule covers.
    let values: Vec<(&str, String, u16)> = vec![
        ("plain", "broadband".to_string(), 200),
        ("empty", String::new(), 200),
        ("at the maximum length", "x".repeat(128), 200),
        ("one past the maximum length", "x".repeat(129), 400),
        ("far past the maximum length", "x".repeat(200), 400),
        ("a pipe", "a|b".to_string(), 400),
        ("a backslash", "a\\b".to_string(), 400),
        ("a caret", "a^b".to_string(), 400),
        ("a forbidden character at the end", "ab^".to_string(), 400),
        ("every forbidden character", "|\\^".to_string(), 400),
    ];

    let mut accepted: Vec<String> = Vec::new();
    for member in ["Type", "Provider"] {
        for (case, value, want) in &values {
            let mut body = serde_json::Map::new();
            body.insert(
                "Bandwidth".to_string(),
                json!({ "UploadSpeed": 1, "DownloadSpeed": 2 }),
            );
            body.insert(member.to_string(), json!(value));
            let body = Value::Object(body);
            let (status, answer) = create_link(&emulator, &network, &site, &body).await;
            assert_eq!(
                status, *want,
                "case {member}/{case}: expected {want}, got {status} with {answer}"
            );
            if *want == 400 {
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {member}/{case}: wrong error code in {answer}"
                );
            } else {
                accepted.push(
                    answer["Link"]["LinkId"]
                        .as_str()
                        .unwrap_or_else(|| panic!("case {member}/{case}: no LinkId in {answer}"))
                        .to_string(),
                );
            }
        }
    }

    // A refused create stores nothing: only the accepted rows are readable.
    let (_, listed) = emulator
        .get(&format!("/global-networks/{network}/links?maxResults=500"))
        .await;
    accepted.sort();
    assert_eq!(
        ids_of(&listed, "Links", "LinkId"),
        accepted,
        "a refused CreateLink must not have stored anything, got {listed}"
    );
}

#[tokio::test]
async fn create_link_refuses_a_parent_that_does_not_exist_and_get_links_is_not_found() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("parents").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_network = emulator.create_global_network("other").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    // Rows differ along which parent is missing: the global network in the path, the site
    // in the body, and a site that exists but under a different global network. `CreateLink`
    // validates the references its input carries before anything is minted, so each of them
    // is a field-validation failure on the member that carried it (the full rule, with the
    // member and prose each refusal reports, is pinned by `unknown_parent_reference`);
    // `GetLinks` addresses the global network itself, so an absent one stays a not-found.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "an unknown global network",
            "global-network-00000000000000000".to_string(),
            site.clone(),
        ),
        (
            "an id that is not a Network Manager id at all",
            "nope".to_string(),
            site.clone(),
        ),
        (
            "an unknown site",
            network.clone(),
            "site-00000000000000000".to_string(),
        ),
        (
            "a site of another global network",
            network.clone(),
            other_site.clone(),
        ),
    ];

    for (case, path_network, body_site) in cases {
        let (status, answer) = create_link(&emulator, &path_network, &body_site, &plain_link()).await;
        assert_eq!(
            status, 400,
            "case {case}: CreateLink expected 400, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: wrong error code in {answer}"
        );
    }

    for unknown in ["global-network-00000000000000000", "nope"] {
        let (status, answer) = emulator
            .get(&format!("/global-networks/{unknown}/links"))
            .await;
        assert_eq!(
            status, 404,
            "case GetLinks on {unknown:?}: expected 404, got {status} with {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case GetLinks on {unknown:?}: wrong error code in {answer}"
        );
    }

    // None of the refusals stored anything.
    let (_, listed) = emulator
        .get(&format!("/global-networks/{network}/links"))
        .await;
    assert_eq!(
        ids_of(&listed, "Links", "LinkId"),
        Vec::<String>::new(),
        "a refused CreateLink must not have stored anything, got {listed}"
    );
}

#[tokio::test]
async fn get_links_pages_every_link_of_its_own_global_network() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("paged").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_network = emulator.create_global_network("other").await;
    let other_site = emulator.create_site(&other_network, "elsewhere").await;

    let mut expected = Vec::new();
    for index in 0..4 {
        expected.push(
            create_link_ok(
                &emulator,
                &network,
                &site,
                &json!({ "Bandwidth": { "UploadSpeed": index + 1 } }),
            )
            .await,
        );
    }
    expected.sort();
    // A link of another global network must never show up on these pages.
    create_link_ok(&emulator, &other_network, &other_site, &plain_link()).await;

    // The rule generalises over the page size, so the rows span it: no `MaxResults`, each
    // size below the stored count, the count itself, one past it, and the modelled
    // maximum (`MaxResults` range 1..500).
    for page_size in [None, Some(1), Some(2), Some(3), Some(4), Some(5), Some(500)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref value) = token {
                query.push(format!("nextToken={value}"));
            }
            let uri = if query.is_empty() {
                format!("/global-networks/{network}/links")
            } else {
                format!("/global-networks/{network}/links?{}", query.join("&"))
            };
            let (status, body) = emulator.get(&uri).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size:?} page {pages}: expected 200, got {status} with {body}"
            );
            let page = ids_of(&body, "Links", "LinkId");
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case MaxResults={page_size:?} page {pages}: a page must not exceed MaxResults, got {} items",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case MaxResults={page_size:?}: pagination did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case MaxResults={page_size:?}: GetLinks must page exactly its own links, each once"
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_mint_is_refused_by_get_links() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("tokens").await;
    let site = emulator.create_site(&network, "hq").await;
    create_link_ok(&emulator, &network, &site, &plain_link()).await;

    // Every row is a NON-empty value: the empty string is no token at all — it satisfies
    // the modelled `length 0..2048` and names no position, so it asks for the first page
    // (`crate::empty_page_token`).
    let cases: Vec<(&str, String)> = vec![
        ("not hex", "zzzz".to_string()),
        ("an odd number of hex digits", "abc".to_string()),
        (
            "hex of a token another service would mint",
            hex::encode("someone-else:link-0"),
        ),
        ("hex of non-utf8 bytes", hex::encode([0xff, 0xfe, 0xfd])),
    ];

    for (case, token) in cases {
        let (status, body) = emulator
            .get(&format!(
                "/global-networks/{network}/links?nextToken={token}"
            ))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for an unparseable NextToken, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case}: wrong error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_created_link_reports_pending_until_the_settle_span_the_clock_owes_elapses() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("settling").await;
    let site = emulator.create_site(&network, "hq").await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;

    let (_, immediate) = emulator
        .get(&format!("/global-networks/{network}/links?linkIds={link}"))
        .await;
    assert_eq!(
        immediate["Links"][0]["State"],
        json!("PENDING"),
        "GetLinks straight after the create: expected PENDING, got {immediate}"
    );
    let created_at = immediate["Links"][0]["CreatedAt"].clone();

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    let (_, settled) = emulator
        .get(&format!("/global-networks/{network}/links?linkIds={link}"))
        .await;
    assert_eq!(
        settled["Links"][0]["State"],
        json!("AVAILABLE"),
        "GetLinks after the settle span: expected AVAILABLE, got {settled}"
    );
    assert_eq!(
        settled["Links"][0]["CreatedAt"], created_at,
        "the settle transition must not restamp CreatedAt, got {settled}"
    );
}

#[tokio::test]
async fn deleting_a_site_that_still_holds_links_is_refused_as_a_validation_failure() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("conflicts").await;
    let held = emulator.create_site(&network, "held").await;
    let empty = emulator.create_site(&network, "empty").await;
    let link = create_link_ok(&emulator, &network, &held, &plain_link()).await;

    // "Deletes an existing site. The site cannot be associated with any device or link."
    // (`smithy_query query="doc" shape="DeleteSite"`).
    let (status, refused) = emulator
        .delete(&format!("/global-networks/{network}/sites/{held}"))
        .await;
    assert_eq!(
        status, 400,
        "DeleteSite with a link: expected 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "DeleteSite with a link: wrong error code in {refused}"
    );
    let (_, listed) = emulator
        .get(&format!("/global-networks/{network}/links"))
        .await;
    assert_eq!(
        ids_of(&listed, "Links", "LinkId"),
        vec![link],
        "a refused DeleteSite must leave the links alone, got {listed}"
    );

    // A site that holds no link still deletes, so the refusal is about the links only.
    let (status, deleted) = emulator
        .delete(&format!("/global-networks/{network}/sites/{empty}"))
        .await;
    assert_eq!(
        status, 200,
        "DeleteSite without links: expected 200, got {status} with {deleted}"
    );
}

#[tokio::test]
async fn creating_past_the_link_quota_is_refused_with_the_modelled_error_and_stores_nothing() {
    use service_com_amazonaws_networkmanager_networkmanager::state::MAX_LINKS_PER_SITE;

    let emulator = Emulator::new();
    let network = emulator.create_global_network("quota").await;
    let full = emulator.create_site(&network, "full").await;
    let sibling = emulator.create_site(&network, "sibling").await;

    let mut links: Vec<String> = Vec::new();
    for _ in 0..MAX_LINKS_PER_SITE {
        links.push(create_link_ok(&emulator, &network, &full, &plain_link()).await);
    }
    let (status, refused) = create_link(&emulator, &network, &full, &plain_link()).await;
    assert_eq!(
        status, 402,
        "CreateLink at the per-site quota: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "CreateLink at the per-site quota: wrong error code in {refused}"
    );

    let (_, listed) = emulator
        .get(&format!(
            "/global-networks/{network}/links?siteId={full}&maxResults=500"
        ))
        .await;
    links.sort();
    assert_eq!(
        ids_of(&listed, "Links", "LinkId"),
        links,
        "a refused CreateLink must not have stored anything, got {listed}"
    );

    // The quota is per site, so a sibling of a full site still has room.
    let (status, accepted) = create_link(&emulator, &network, &sibling, &plain_link()).await;
    assert_eq!(
        status, 200,
        "CreateLink on a sibling of a full site: expected 200, got {status} with {accepted}"
    );
}
