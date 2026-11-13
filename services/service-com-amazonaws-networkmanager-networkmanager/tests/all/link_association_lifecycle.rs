// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the link-association slice (`AssociateLink`,
//! `GetLinkAssociations`, `DisassociateLink`), driven over HTTP.
//!
//! Every call goes through the real `MultiServiceRouter`, so the restJson1 routes, the
//! label/query bindings and the error envelopes are exercised as a client hits them.
//! Nothing reaches into `crate::state`: an association is only ever read back through
//! `GetLinkAssociations`.
//!
//! The harness is the one `global_network_lifecycle` defines — one shared engine, this
//! service, the production dispatch.

use serde_json::{json, Value};

use crate::device_lifecycle::create_device_ok;
use crate::global_network_lifecycle::{error_code, Emulator, PAST_SETTLE};
use crate::link_lifecycle::{create_link_ok, plain_link};

/// `AssociateLink` for the `device`/`link` pair of `network`.
///
/// Shared with the suite that tables this operation's refusals
/// (`crate::associate_link_refusals`), so a request is built in exactly one place.
pub(crate) async fn associate(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: &str,
) -> (u16, Value) {
    emulator
        .post(
            &format!("/global-networks/{network}/link-associations"),
            json!({ "DeviceId": device, "LinkId": link }),
        )
        .await
}

/// `AssociateLink` that must succeed, answering the `LinkAssociation` it reported.
///
/// Shared with [`associate`]'s other caller.
pub(crate) async fn associate_ok(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: &str,
) -> Value {
    let (status, body) = associate(emulator, network, device, link).await;
    assert_eq!(
        status, 200,
        "AssociateLink({device}, {link}): expected 200, got {status} with {body}"
    );
    body["LinkAssociation"].clone()
}

/// `DisassociateLink` for the `device`/`link` pair of `network`.
///
/// Shared with the suite that unties a pair to reach the state behind it
/// (`crate::delete_live_reference_refusal`), so the request is built in exactly one place.
pub(crate) async fn disassociate(
    emulator: &Emulator,
    network: &str,
    device: &str,
    link: &str,
) -> (u16, Value) {
    emulator
        .delete(&format!(
            "/global-networks/{network}/link-associations?deviceId={device}&linkId={link}"
        ))
        .await
}

/// `GetLinkAssociations` on `network` with `query` appended to the URI.
///
/// Shared with [`associate`]'s other caller, which reads an association back through it.
pub(crate) async fn get_associations(
    emulator: &Emulator,
    network: &str,
    query: &str,
) -> (u16, Value) {
    let uri = if query.is_empty() {
        format!("/global-networks/{network}/link-associations")
    } else {
        format!("/global-networks/{network}/link-associations?{query}")
    };
    emulator.get(&uri).await
}

/// The device/link pairs a `GetLinkAssociations` page lists, in the order returned.
///
/// Shared with [`associate`]'s other caller.
pub(crate) fn pairs_of(body: &Value) -> Vec<(String, String)> {
    body["LinkAssociations"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a LinkAssociations array in {body}"))
        .iter()
        .map(|association| {
            (
                association["DeviceId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected DeviceId in {association}"))
                    .to_string(),
                association["LinkId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected LinkId in {association}"))
                    .to_string(),
            )
        })
        .collect()
}

/// One row of a `GetLinkAssociations` table: the query string the request carries and the
/// device/link pairs the read must answer for it, in key order.
struct SelectorCase {
    name: &'static str,
    query: String,
    expected: Vec<(String, String)>,
}

impl SelectorCase {
    fn new(name: &'static str, query: String, expected: &[(&String, &String)]) -> Self {
        Self {
            name,
            query,
            expected: expected
                .iter()
                .map(|(device, link)| ((*device).clone(), (*link).clone()))
                .collect(),
        }
    }
}

/// A device and a link, both at a fresh site of `network`, ready to be associated.
async fn sited_device_and_link(emulator: &Emulator, network: &str, label: &str) -> (String, String) {
    let site = emulator.create_site(network, label).await;
    let device = create_device_ok(emulator, network, json!({ "SiteId": site })).await;
    let link = create_link_ok(emulator, network, &site, &plain_link()).await;
    (device, link)
}

#[tokio::test]
async fn get_link_associations_answers_the_stored_pairs_for_every_selector_the_request_carries() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("associations").await;
    let site = emulator.create_site(&network, "hq").await;

    // Two devices and two links at one site: "A device can be associated to multiple
    // links and a link can be associated to multiple devices" (`smithy_query query="doc"
    // shape="AssociateLink"`), so the table below spans both fan-outs — and the fourth
    // pair is left unassociated, so a selector naming it must report nothing.
    let device_one = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let device_two = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let link_one = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let link_two = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    for (device, link) in [
        (&device_one, &link_one),
        (&device_one, &link_two),
        (&device_two, &link_one),
    ] {
        let association = associate_ok(&emulator, &network, device, link).await;
        assert_eq!(
            association["GlobalNetworkId"],
            json!(network),
            "AssociateLink({device}, {link}): the association must name its global network, got {association}"
        );
        assert_eq!(
            (
                association["DeviceId"].clone(),
                association["LinkId"].clone()
            ),
            (json!(device), json!(link)),
            "AssociateLink({device}, {link}): the association must name the pair it ties, got {association}"
        );
    }

    // Each row is a selector and the stored pairs it must select, in key order (device
    // first, then link). An id that names nothing stored selects nothing; a pair that is
    // not associated selects nothing either.
    let unknown = "device-0000000000000beef";
    let cases = vec![
        SelectorCase::new(
            "deviceId with two associations",
            format!("deviceId={device_one}"),
            &[(&device_one, &link_one), (&device_one, &link_two)],
        ),
        SelectorCase::new(
            "deviceId with one association",
            format!("deviceId={device_two}"),
            &[(&device_two, &link_one)],
        ),
        SelectorCase::new(
            "linkId with two associations",
            format!("linkId={link_one}"),
            &[(&device_one, &link_one), (&device_two, &link_one)],
        ),
        SelectorCase::new(
            "linkId with one association",
            format!("linkId={link_two}"),
            &[(&device_one, &link_two)],
        ),
        SelectorCase::new(
            "both selectors naming an associated pair",
            format!("deviceId={device_one}&linkId={link_one}"),
            &[(&device_one, &link_one)],
        ),
        SelectorCase::new(
            "both selectors naming a pair that is not associated",
            format!("deviceId={device_two}&linkId={link_two}"),
            &[],
        ),
        SelectorCase::new(
            "deviceId naming nothing stored",
            format!("deviceId={unknown}"),
            &[],
        ),
        SelectorCase::new(
            "linkId naming nothing stored",
            "linkId=link-0000000000000beef".to_string(),
            &[],
        ),
        SelectorCase::new(
            "an associated device with a link that names nothing stored",
            format!("deviceId={device_one}&linkId=link-0000000000000beef"),
            &[],
        ),
    ];

    for case in cases {
        let SelectorCase {
            name,
            query,
            expected,
        } = case;
        let (status, listed) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, 200,
            "case {name}: GetLinkAssociations({query}) expected 200, got {status} with {listed}"
        );
        let actual = pairs_of(&listed);
        assert_eq!(
            actual, expected,
            "case {name}: GetLinkAssociations({query}) expected {expected:?}, got {actual:?}"
        );
        assert!(
            listed["NextToken"].is_null(),
            "case {name}: a single page must carry no NextToken, got {listed}"
        );
    }

    // A second global network's association is invisible here, and this one's is
    // invisible there: the listing is scoped to the global network in the path.
    let other = emulator.create_global_network("elsewhere").await;
    let (other_device, other_link) = sited_device_and_link(&emulator, &other, "branch").await;
    associate_ok(&emulator, &other, &other_device, &other_link).await;
    let (_, listed) = get_associations(&emulator, &other, &format!("deviceId={device_one}")).await;
    assert_eq!(
        pairs_of(&listed),
        Vec::<(String, String)>::new(),
        "a device of another global network must select nothing here, got {listed}"
    );
    let (_, listed) = get_associations(&emulator, &other, &format!("deviceId={other_device}")).await;
    assert_eq!(
        pairs_of(&listed),
        vec![(other_device.clone(), other_link.clone())],
        "the other global network must answer its own association, got {listed}"
    );
}

#[tokio::test]
async fn get_link_associations_requires_either_a_device_or_a_link() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("selectors").await;
    let (device, link) = sited_device_and_link(&emulator, &network, "hq").await;
    associate_ok(&emulator, &network, &device, &link).await;

    // "Either the device ID or the link ID must be specified" (`smithy_query query="doc"
    // shape="GetLinkAssociations"`): the rows differ in which selectors the query string
    // carries, including a request that carries only the paging members.
    let cases: Vec<(&str, String, u16)> = vec![
        ("no query string at all", String::new(), 400),
        ("only paging members", "maxResults=10".to_string(), 400),
        (
            "only a page token slot",
            "maxResults=1&nextToken=".to_string(),
            400,
        ),
        ("the device only", format!("deviceId={device}"), 200),
        ("the link only", format!("linkId={link}"), 200),
        (
            "both the device and the link",
            format!("deviceId={device}&linkId={link}"),
            200,
        ),
    ];

    for (case, query, expected) in cases {
        let (status, body) = get_associations(&emulator, &network, &query).await;
        assert_eq!(
            status, expected,
            "case {case}: GetLinkAssociations({query}) expected {expected}, got {status} with {body}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case}: expected a ValidationException, got {body}"
            );
            let named: Vec<&str> = body["Fields"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected Fields in {body}"))
                .iter()
                .map(|field| field["Name"].as_str().unwrap_or("<no Name>"))
                .collect();
            assert_eq!(
                named,
                vec!["DeviceId", "LinkId"],
                "case {case}: the refusal must name both selectors, got {body}"
            );
        }
    }

    // The ORDER of the two refusals: the request's own members are checked before any
    // stored resource is looked up, so a request with no selector is a 400 even against a
    // global network that names nothing, while one that carries a selector reaches the
    // lookup and is a 404 naming the global network.
    let unknown_network = "global-network-0000000000000beef";
    let (status, body) = get_associations(&emulator, unknown_network, "").await;
    assert_eq!(
        status, 400,
        "no selector against an unknown global network: expected 400, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "no selector against an unknown global network: expected a ValidationException, got {body}"
    );
    let (status, body) =
        get_associations(&emulator, unknown_network, &format!("deviceId={device}")).await;
    assert_eq!(
        status, 404,
        "a selector against an unknown global network: expected 404, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a selector against an unknown global network: expected a ResourceNotFoundException, got {body}"
    );
    assert_eq!(
        (body["ResourceType"].clone(), body["ResourceId"].clone()),
        (json!("global-network"), json!(unknown_network)),
        "the refusal must name the global network it could not find, got {body}"
    );
}

#[tokio::test]
async fn associate_link_refuses_a_pair_that_is_not_two_resources_of_one_site() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("pairing").await;
    let site = emulator.create_site(&network, "hq").await;
    let other_site = emulator.create_site(&network, "branch").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
    let device_elsewhere =
        create_device_ok(&emulator, &network, json!({ "SiteId": other_site })).await;
    let link_elsewhere = create_link_ok(&emulator, &network, &other_site, &plain_link()).await;
    // `SiteId` is optional on a device, so a device with no site at all exists too.
    let unsited_device = create_device_ok(&emulator, &network, json!({})).await;
    // A second global network, to prove a resource is only addressable through its own.
    let other_network = emulator.create_global_network("foreign").await;
    let (foreign_device, foreign_link) =
        sited_device_and_link(&emulator, &other_network, "foreign-site").await;

    // Each row is a request and the answer the rule demands. The rule has two halves:
    // every resource the call names must exist within the global network in the path
    // (404 on the one that does not), and "the device and link must be in the same global
    // network and the same site" (`smithy_query query="doc" shape="AssociateLink"`) — a
    // state-level refusal, so a 400 `ValidationException`.
    let cases: Vec<(&str, &str, &str, &str, u16, &str)> = vec![
        (
            "a device and a link of one site",
            network.as_str(),
            device.as_str(),
            link.as_str(),
            200,
            "",
        ),
        (
            "a device and a link at different sites",
            network.as_str(),
            device.as_str(),
            link_elsewhere.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a device with no site",
            network.as_str(),
            unsited_device.as_str(),
            link.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a link at the device's other site",
            network.as_str(),
            device_elsewhere.as_str(),
            link.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a device id that names nothing stored",
            network.as_str(),
            "device-0000000000000beef",
            link.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a link id that names nothing stored",
            network.as_str(),
            device.as_str(),
            "link-0000000000000beef",
            404,
            "ResourceNotFoundException",
        ),
        (
            // The association is a new record stored under the global network the path
            // addresses, so that parent reference is a field failure rather than the
            // missing-resource answer the resources inside it get.
            "an unknown global network",
            "global-network-0000000000000beef",
            device.as_str(),
            link.as_str(),
            400,
            "ValidationException",
        ),
        (
            "a device stored under another global network",
            network.as_str(),
            foreign_device.as_str(),
            link.as_str(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a link stored under another global network",
            network.as_str(),
            device.as_str(),
            foreign_link.as_str(),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, in_network, with_device, with_link, expected, code) in cases {
        let (status, body) = associate(&emulator, in_network, with_device, with_link).await;
        assert_eq!(
            status, expected,
            "case {case}: AssociateLink expected {expected}, got {status} with {body}"
        );
        if expected != 200 {
            assert_eq!(
                error_code(&body),
                code,
                "case {case}: expected {code}, got {body}"
            );
        }
    }

    // Only the one accepted row stored anything: a refused association is stored nowhere,
    // whichever half of the rule refused it. Each row is a selector and the associations
    // the store must hold for it.
    let stored = vec![
        SelectorCase::new(
            "the accepted pair",
            format!("deviceId={device}"),
            &[(&device, &link)],
        ),
        SelectorCase::new(
            "the device with no site",
            format!("deviceId={unsited_device}"),
            &[],
        ),
        SelectorCase::new(
            "the device at the other site",
            format!("deviceId={device_elsewhere}"),
            &[],
        ),
        SelectorCase::new(
            "the link at the other site",
            format!("linkId={link_elsewhere}"),
            &[],
        ),
        SelectorCase::new(
            "the link of the accepted pair",
            format!("linkId={link}"),
            &[(&device, &link)],
        ),
    ];
    for case in stored {
        let SelectorCase {
            name,
            query,
            expected,
        } = case;
        let (_, listed) = get_associations(&emulator, &network, &query).await;
        let actual = pairs_of(&listed);
        assert_eq!(
            actual, expected,
            "case {name}: {query} expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_association_reports_pending_until_the_settle_span_the_clock_owes_elapses() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("settling").await;
    let (device, link) = sited_device_and_link(&emulator, &network, "hq").await;

    let created = associate_ok(&emulator, &network, &device, &link).await;
    assert_eq!(
        created["LinkAssociationState"],
        json!("PENDING"),
        "AssociateLink: expected PENDING on the creating call, got {created}"
    );
    let (_, immediate) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        immediate["LinkAssociations"][0]["LinkAssociationState"],
        json!("PENDING"),
        "a read straight after the association: expected PENDING, got {immediate}"
    );

    // The transition is owed by the clock, not by the read: nothing polls for it.
    std::thread::sleep(PAST_SETTLE);

    let (_, settled) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        settled["LinkAssociations"][0]["LinkAssociationState"],
        json!("AVAILABLE"),
        "a read after the settle span: expected AVAILABLE, got {settled}"
    );

    // The pair IS the association's identity, so associating it again creates nothing: it
    // reports the stored association, which has already settled, rather than restarting
    // the span.
    let again = associate_ok(&emulator, &network, &device, &link).await;
    assert_eq!(
        again["LinkAssociationState"],
        json!("AVAILABLE"),
        "AssociateLink repeated after the settle span: expected AVAILABLE, got {again}"
    );
    let (_, listed) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        pairs_of(&listed),
        vec![(device.clone(), link.clone())],
        "a repeated AssociateLink must not store a second association, got {listed}"
    );
}

#[tokio::test]
async fn disassociate_link_removes_the_association_a_later_read_would_list() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("removal").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let links: Vec<String> = {
        let mut minted = Vec::new();
        for _ in 0..3 {
            minted.push(create_link_ok(&emulator, &network, &site, &plain_link()).await);
        }
        minted
    };
    for link in &links {
        associate_ok(&emulator, &network, &device, link).await;
    }

    let (status, removed) = disassociate(&emulator, &network, &device, &links[1]).await;
    assert_eq!(
        status, 200,
        "DisassociateLink: expected 200, got {status} with {removed}"
    );
    assert_eq!(
        removed["LinkAssociation"]["LinkAssociationState"],
        json!("DELETING"),
        "DisassociateLink must report the removed association as DELETING, got {removed}"
    );
    assert_eq!(
        (
            removed["LinkAssociation"]["DeviceId"].clone(),
            removed["LinkAssociation"]["LinkId"].clone(),
        ),
        (json!(device), json!(links[1])),
        "DisassociateLink must report the pair it removed, got {removed}"
    );

    let (_, listed) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        pairs_of(&listed),
        vec![
            (device.clone(), links[0].clone()),
            (device.clone(), links[2].clone()),
        ],
        "the removed association must be gone and the others untouched, got {listed}"
    );

    // Each row names a pair or a resource that holds no association to remove: the
    // request addresses something that is not there, which is the missing-resource answer.
    let cases: Vec<(&str, &str, &str, &str)> = vec![
        (
            "the pair that was just removed",
            network.as_str(),
            device.as_str(),
            links[1].as_str(),
        ),
        (
            "an unknown global network",
            "global-network-0000000000000beef",
            device.as_str(),
            links[0].as_str(),
        ),
        (
            "an unknown device",
            network.as_str(),
            "device-0000000000000beef",
            links[0].as_str(),
        ),
        (
            "an unknown link",
            network.as_str(),
            device.as_str(),
            "link-0000000000000beef",
        ),
    ];
    for (case, in_network, with_device, with_link) in cases {
        let (status, body) = disassociate(&emulator, in_network, with_device, with_link).await;
        assert_eq!(
            status, 404,
            "case {case}: DisassociateLink expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case}: expected a ResourceNotFoundException, got {body}"
        );
    }

    // Every surviving association is still listed after the refusals above.
    let (_, listed) = get_associations(&emulator, &network, &format!("deviceId={device}")).await;
    assert_eq!(
        pairs_of(&listed).len(),
        2,
        "a refused DisassociateLink must remove nothing, got {listed}"
    );
}

/// One row of the `DisassociateLink` table: how far the pair's device and link are each
/// fanned out to OTHER partners, and whether the association has settled out of `PENDING`
/// by the time it is removed.
struct RemovalCase {
    name: &'static str,
    extra_links: usize,
    extra_devices: usize,
    settled_first: bool,
}

#[tokio::test]
async fn disassociate_link_removes_only_the_addressed_pair_whatever_its_fan_out_or_state() {
    let emulator = Emulator::new();

    // "Disassociates an existing device from a link" (`smithy_query query="doc"
    // shape="DisassociateLink"`): the request addresses ONE device/link pair, so the rows
    // differ in how many other associations share that pair's device or its link, and in
    // the state the pair is in when the call lands — the removal is of the pair, not of
    // anything the fan-out or the clock reached.
    let cases = [
        RemovalCase {
            name: "a lone pair, still pending",
            extra_links: 0,
            extra_devices: 0,
            settled_first: false,
        },
        RemovalCase {
            name: "a lone pair, settled",
            extra_links: 0,
            extra_devices: 0,
            settled_first: true,
        },
        RemovalCase {
            name: "a device fanned out to more links",
            extra_links: 3,
            extra_devices: 0,
            settled_first: false,
        },
        RemovalCase {
            name: "a link fanned out to more devices",
            extra_links: 0,
            extra_devices: 3,
            settled_first: false,
        },
        RemovalCase {
            name: "both fan-outs, settled",
            extra_links: 2,
            extra_devices: 1,
            settled_first: true,
        },
    ];

    for case in &cases {
        let RemovalCase {
            name,
            extra_links,
            extra_devices,
            settled_first,
        } = case;
        let network = emulator.create_global_network(name).await;
        let site = emulator.create_site(&network, "hq").await;
        let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
        let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
        associate_ok(&emulator, &network, &device, &link).await;

        // The associations that must survive the removal: the same device tied to other
        // links, and the same link tied to other devices.
        let mut survives_on_device: Vec<(String, String)> = Vec::new();
        for _ in 0..*extra_links {
            let other_link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
            associate_ok(&emulator, &network, &device, &other_link).await;
            survives_on_device.push((device.clone(), other_link));
        }
        let mut survives_on_link: Vec<(String, String)> = Vec::new();
        for _ in 0..*extra_devices {
            let other_device =
                create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
            associate_ok(&emulator, &network, &other_device, &link).await;
            survives_on_link.push((other_device, link.clone()));
        }
        survives_on_device.sort();
        survives_on_link.sort();

        if *settled_first {
            // The transition is owed by the clock, not by a poll, so the test waits it out.
            tokio::time::sleep(PAST_SETTLE).await;
        }

        let (status, removed) = disassociate(&emulator, &network, &device, &link).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DisassociateLink expected 200, got {status} with {removed}"
        );
        let reported = removed["LinkAssociation"].clone();
        assert_eq!(
            (
                reported["GlobalNetworkId"].clone(),
                reported["DeviceId"].clone(),
                reported["LinkId"].clone(),
                reported["LinkAssociationState"].clone(),
            ),
            (
                json!(network),
                json!(device),
                json!(link),
                json!("DELETING"),
            ),
            "case {name:?}: DisassociateLink must report the removed pair as DELETING, got {reported}"
        );

        // The removal is readable: neither selector lists the pair any more, and every
        // other association of that device and of that link is untouched.
        let (_, by_device) =
            get_associations(&emulator, &network, &format!("deviceId={device}")).await;
        let mut actual = pairs_of(&by_device);
        actual.sort();
        assert_eq!(
            actual, survives_on_device,
            "case {name:?}: deviceId={device} expected {survives_on_device:?}, got {actual:?}"
        );
        let (_, by_link) = get_associations(&emulator, &network, &format!("linkId={link}")).await;
        let mut actual = pairs_of(&by_link);
        actual.sort();
        assert_eq!(
            actual, survives_on_link,
            "case {name:?}: linkId={link} expected {survives_on_link:?}, got {actual:?}"
        );

        // The pair holds no association now, so a second removal has nothing to address.
        let (status, body) = disassociate(&emulator, &network, &device, &link).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a repeated DisassociateLink expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: a repeated DisassociateLink expected a ResourceNotFoundException, got {body}"
        );

        // The record is gone rather than hidden: associating the pair again makes a NEW
        // association, which starts the settle span over at `PENDING`.
        let remade = associate_ok(&emulator, &network, &device, &link).await;
        assert_eq!(
            remade["LinkAssociationState"],
            json!("PENDING"),
            "case {name:?}: a re-made association expected PENDING, got {remade}"
        );
        let (_, relisted) = get_associations(&emulator, &network, &format!("linkId={link}")).await;
        let mut actual = pairs_of(&relisted);
        actual.sort();
        let mut expected = survives_on_link.clone();
        expected.push((device.clone(), link.clone()));
        expected.sort();
        assert_eq!(
            actual, expected,
            "case {name:?}: after re-association linkId={link} expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn get_link_associations_pages_the_selected_associations_exactly_once() {
    let emulator = Emulator::new();
    let network = emulator.create_global_network("paging").await;
    let site = emulator.create_site(&network, "hq").await;
    let device = create_device_ok(&emulator, &network, json!({ "SiteId": site })).await;
    let mut expected: Vec<(String, String)> = Vec::new();
    for _ in 0..5 {
        let link = create_link_ok(&emulator, &network, &site, &plain_link()).await;
        associate_ok(&emulator, &network, &device, &link).await;
        expected.push((device.clone(), link.clone()));
    }
    expected.sort();

    // The rows differ in the page size the request asks for: every size from one item to
    // more than the store holds, plus the absent member. `MaxResults` is modelled
    // `range 1..500` (`smithy_query operation="GetLinkAssociations"`), so 1 and 500 are
    // the published bounds and each is a row.
    for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(6), Some(500)] {
        let mut seen: Vec<(String, String)> = Vec::new();
        let mut token: Option<String> = None;
        let mut requests = 0;
        loop {
            let mut query = vec![format!("deviceId={device}")];
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(ref resume) = token {
                query.push(format!("nextToken={resume}"));
            }
            let (status, page) = get_associations(&emulator, &network, &query.join("&")).await;
            assert_eq!(
                status, 200,
                "page size {page_size:?}: expected 200, got {status} with {page}"
            );
            let listed = pairs_of(&page);
            assert!(
                !listed.is_empty(),
                "page size {page_size:?}: a page must not be empty while items remain, got {page}"
            );
            if let Some(size) = page_size {
                assert!(
                    listed.len() <= usize::try_from(size).expect("the page size fits a usize"),
                    "page size {page_size:?}: a page must hold at most {size} items, got {listed:?}"
                );
            }
            seen.extend(listed);
            token = page["NextToken"].as_str().map(str::to_string);
            requests += 1;
            assert!(
                requests <= 8,
                "page size {page_size:?}: the token walk must terminate, saw {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "page size {page_size:?}: the walk must visit every association exactly once, got {seen:?}"
        );
    }

    // A NON-empty token this service never minted is a field failure, not an empty page.
    // The empty string is no token at all — it satisfies the modelled `length 0..2048` and
    // names no position, so it asks for the first page (`crate::empty_page_token`).
    for token in ["not-hex", "6e6f74206f7572732d746f6b656e"] {
        let (status, body) =
            get_associations(&emulator, &network, &format!("deviceId={device}&nextToken={token}"))
                .await;
        assert_eq!(
            status, 400,
            "token {token:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "token {token:?}: expected a ValidationException, got {body}"
        );
    }
}
