// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeCacheParameters`.
//!
//! The rule under test: the operation reports the named cache parameter group's
//! effective parameter list — its FAMILY's published engine defaults with the
//! values set on that group laid over them, each reported under
//! `Parameter$Source` `user` when the group holds a value for it and `system`
//! when it reports the engine's own; the `Source` member selects the parameters
//! of one source; the answer is paged at the `MaxRecords` window the member
//! documents with a resumable `Marker` — over the plain `Parameters`, which are
//! this operation's paginated items, while the cache-node-type-specific list
//! comes whole with every page; and a group name the account does not
//! hold is refused with the modelled 404 `CacheParameterGroupNotFound`.
//!
//! Every assertion goes over the wire through the same router the production
//! emulator mounts, reusing the harness in [`crate::harness`]; only the HTTP
//! status and the XML body are inspected, so an answer that was not computed
//! from the persisted group and the persisted catalog cannot pass. The tables
//! below range over the axes the rule generalises over — the GROUP (a family
//! default and a custom group), the FAMILY behind it (and with it the engine
//! catalog and the version line), the VALUE set on a parameter, the `Source`
//! spelling and the page size — so the one group a recorded call happens to
//! name is one row of many.
//!
//! The values set on a group are seeded straight into the store through
//! [`Emulator::with_state`] rather than through `ModifyCacheParameterGroup`:
//! what this suite is about is the READER, so a row fixes the stored value
//! directly, including values no write would accept, and the suite that covers
//! the writer asserts the two agree. The scanners below are `pub(crate)`
//! because that suite reads a parameter out of an answer exactly the same way.

use std::collections::BTreeSet;

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use crate::harness::{params, Emulator, Reply};

/// The custom group the single-group suites create, and the prefix every other
/// created group's name starts with. A custom group is the one a customer may
/// set parameter values on.
const CUSTOM_GROUP: &str = "fixer-params";

/// One parameter as the response reports it: the list it was reported in, its
/// name, and the fields that followed it.
///
/// `pub(crate)` for the reason the shared harness is: the `catalog_publication`
/// suite reads a parameter out of a `DescribeCacheParameters` answer too, and a
/// second copy of this reader would be a second thing to keep correct.
#[derive(Debug)]
pub(crate) struct Reported {
    pub(crate) name: String,
    pub(crate) node_type_specific: bool,
    pub(crate) fields: Vec<(String, String)>,
}

impl Reported {
    /// The text of `field` on this parameter, or `None` when the reply reports
    /// no such element for it (an absent element reads as `None` too: a value
    /// that is the empty string carries no text).
    pub(crate) fn field(&self, field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, text)| text.as_str())
    }
}

/// Every parameter a reply reports, in document order, each flagged with which
/// of the two lists `CacheParameterGroupDetails` carries it is in.
///
/// A parameter's fields are the elements BETWEEN its `ParameterName` and the
/// next one's, so a parameter that reports no value cannot borrow the next
/// parameter's.
pub(crate) fn reported(reply: &Reply) -> Vec<Reported> {
    const SPECIFIC: &str = "/CacheNodeTypeSpecificParameters/CacheNodeTypeSpecificParameter/";
    const PLAIN: &str = "/Parameters/Parameter/";
    let mut out: Vec<Reported> = Vec::new();
    for (path, text) in &reply.nodes {
        let node_type_specific = path.contains(SPECIFIC);
        if !node_type_specific && !path.contains(PLAIN) {
            continue;
        }
        let field = path.rsplit('/').next().unwrap_or_default();
        if field == "ParameterName" {
            out.push(Reported {
                name: text.clone(),
                node_type_specific,
                fields: Vec::new(),
            });
            continue;
        }
        if let Some(last) = out.last_mut() {
            last.fields.push((field.to_string(), text.clone()));
        }
    }
    out
}

/// Every reported parameter name, in document order.
pub(crate) fn names(reply: &Reply) -> Vec<String> {
    reported(reply)
        .into_iter()
        .map(|parameter| parameter.name)
        .collect()
}

/// The names in the PAGED list — the plain `Parameters`, which are this
/// operation's paginated items (`smithy_query(query="operation",
/// operation="DescribeCacheParameters")`: `items Parameters`).
pub(crate) fn paged_names(reply: &Reply) -> Vec<String> {
    reported(reply)
        .into_iter()
        .filter(|parameter| !parameter.node_type_specific)
        .map(|parameter| parameter.name)
        .collect()
}

/// The names in the list the `MaxRecords` window does NOT page: the
/// cache-node-type-specific parameters, which every page carries in full.
pub(crate) fn unpaged_names(reply: &Reply) -> Vec<String> {
    reported(reply)
        .into_iter()
        .filter(|parameter| parameter.node_type_specific)
        .map(|parameter| parameter.name)
        .collect()
}

/// The parameter `name` as `reply` reports it.
pub(crate) fn one(reply: &Reply, case: &str, name: &str) -> Reported {
    reported(reply)
        .into_iter()
        .find(|parameter| parameter.name == name)
        .unwrap_or_else(|| {
            panic!(
                "case {case}: {name} must be reported, got {:?}",
                names(reply)
            )
        })
}

/// Ask for the parameters of `group`, with whatever else `extra` names.
pub(crate) async fn describe(emulator: &Emulator, group: &str, extra: &[(&str, &str)]) -> Reply {
    let mut pairs = params(&[("CacheParameterGroupName", group)]);
    pairs.extend(params(extra));
    emulator.call("DescribeCacheParameters", &pairs).await
}

/// [`describe`], asserting the call succeeded.
pub(crate) async fn describe_ok(emulator: &Emulator, group: &str, extra: &[(&str, &str)]) -> Reply {
    let reply = describe(emulator, group, extra).await;
    assert_eq!(
        reply.status, 200,
        "case {group} {extra:?}: must succeed, got {} {}",
        reply.status, reply.body
    );
    reply
}

/// Create a custom parameter group of `family` named `name`, asserting the
/// create was accepted — every custom-group row reads the created group back
/// through `DescribeCacheParameters`.
pub(crate) async fn create_group(emulator: &Emulator, name: &str, family: &str) {
    let reply = emulator
        .call(
            "CreateCacheParameterGroup",
            &params(&[
                ("CacheParameterGroupName", name),
                ("CacheParameterGroupFamily", family),
                ("Description", "created by this crate's parameter-group suites"),
            ]),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {name}/{family}: CreateCacheParameterGroup must answer 200, got {} {}",
        reply.status, reply.body
    );
}

/// Set `value` on `group` for the parameter `name` by writing the store
/// directly, which is how this suite fixes what the READER has to answer with —
/// including a value `ModifyCacheParameterGroup` would refuse.
fn set_value(emulator: &Emulator, group: &str, name: &str, value: &str) {
    emulator.with_state(|store| {
        let record = store
            .cache_parameter_groups
            .get_mut(group)
            .unwrap_or_else(|| panic!("case {group}: the suite created this group"));
        record
            .parameter_values
            .insert(name.to_string(), value.to_string());
    });
}

/// The name of every parameter the store publishes for `family`.
fn catalog_names(family: &str) -> BTreeSet<String> {
    state::family_default_parameters(family)
        .unwrap_or_else(|| panic!("case {family}: a published family has a catalog"))
        .values()
        .map(|parameter| parameter.name.clone())
        .collect()
}

/// The PAGED parameters of `group`, walked page by page at `page_size`, with
/// how many pages it took.
///
/// Every page is also held to the rule that the cache-node-type-specific list is
/// not paged: it comes whole beside each window of plain parameters.
async fn walk(emulator: &Emulator, group: &str, page_size: &str) -> (Vec<String>, usize) {
    let window: usize = page_size.parse().expect("a page size");
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    let mut unpaged: Option<Vec<String>> = None;
    loop {
        let mut extra: Vec<(&str, &str)> = vec![("MaxRecords", page_size)];
        if let Some(token) = marker.as_deref() {
            extra.push(("Marker", token));
        }
        let reply = describe_ok(emulator, group, &extra).await;
        let page = paged_names(&reply);
        pages += 1;
        assert!(
            pages <= 50,
            "case {group} at {page_size} per page: the walk must terminate, got {pages} pages"
        );
        let on_this_page = unpaged_names(&reply);
        match unpaged.as_ref() {
            Some(expected) => assert_eq!(
                &on_this_page, expected,
                "case {group} page {pages}: every page reports the whole \
                 cache-node-type-specific list, got {on_this_page:?}"
            ),
            None => {
                assert!(
                    !on_this_page.is_empty(),
                    "case {group} page {pages}: the family publishes node-type-specific \
                     parameters, so a page reports them, got {}",
                    reply.body
                );
                unpaged = Some(on_this_page);
            }
        }
        let token = reply.marker().map(str::to_string);
        if token.is_some() {
            assert_eq!(
                page.len(),
                window,
                "case {group} page {pages}: a page that carries a marker is full, got {page:?}"
            );
        }
        seen.extend(page);
        match token {
            Some(token) => marker = Some(token),
            None => break,
        }
    }
    (seen, pages)
}

#[tokio::test]
async fn every_group_reports_the_published_catalog_of_its_family() {
    let emulator = Emulator::new();

    // The axis: the group, and with it the family behind it. Every family the
    // service publishes is a row through its own default group, and three of
    // them are a row again through a custom group created for that family — so
    // both kinds of group, both engine catalogs and every version line are
    // exercised.
    let mut cases: Vec<(String, String)> = state::PARAMETER_GROUP_FAMILIES
        .iter()
        .map(|family| (format!("default.{family}"), (*family).to_string()))
        .collect();
    for family in ["redis7", "memcached1.4", "valkey8"] {
        let name = format!("{CUSTOM_GROUP}-{}", family.replace('.', "-"));
        create_group(&emulator, &name, family).await;
        cases.push((name, family.to_string()));
    }

    for (group, family) in cases {
        let expected = state::family_default_parameters(&family)
            .unwrap_or_else(|| panic!("case {group}: a published family has a catalog"));
        let reply = describe_ok(&emulator, &group, &[]).await;
        let parameters = reported(&reply);

        let reported_names: BTreeSet<String> = parameters
            .iter()
            .map(|parameter| parameter.name.clone())
            .collect();
        assert_eq!(
            reported_names,
            catalog_names(&family),
            "case {group}: expected the stored catalog of {family}, got {reported_names:?}"
        );
        assert_eq!(
            reported_names.len(),
            parameters.len(),
            "case {group}: no parameter may be reported twice, got {:?}",
            names(&reply)
        );
        // Every published family's catalog fits the default page, so a complete
        // answer pages nothing.
        assert_eq!(
            reply.marker(),
            None,
            "case {group}: a complete answer carries no marker, got {}",
            reply.body
        );

        for record in expected.values() {
            let case = format!("{group}/{}", record.name);
            let parameter = one(&reply, &case, &record.name);
            // A parameter whose value depends on the cache node type belongs to
            // the OTHER list, with one value per node type the catalog carries.
            assert_eq!(
                parameter.node_type_specific,
                record.is_node_type_specific(),
                "case {case}: expected node-type-specific {}, got {}",
                record.is_node_type_specific(),
                parameter.node_type_specific
            );
            if record.is_node_type_specific() {
                let values = parameter
                    .fields
                    .iter()
                    .filter(|(field, _)| field == "CacheNodeType")
                    .count();
                assert_eq!(
                    values,
                    record.node_type_values.len(),
                    "case {case}: expected one value per published node type, got {values}"
                );
            } else {
                // The value is the catalog's own, reported verbatim; a catalog
                // default that is the empty string carries no text.
                let expected_value = record.value.as_deref().unwrap_or_default();
                let reported_value = parameter.field("ParameterValue").unwrap_or_default();
                assert_eq!(
                    reported_value, expected_value,
                    "case {case}: expected the catalog's default value {expected_value:?}, got \
                     {reported_value:?}"
                );
            }
            // Nothing has set a value on these groups, so every parameter is
            // reported as the service's own.
            assert_eq!(
                parameter.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: an unset parameter's source is {}, got {:?}",
                state::PARAMETER_SOURCE_SYSTEM,
                parameter.field("Source")
            );
            assert_eq!(
                parameter.field("DataType"),
                Some(record.data_type.as_str()),
                "case {case}: expected the catalog's data type {}, got {:?}",
                record.data_type,
                parameter.field("DataType")
            );
            assert_eq!(
                parameter.field("IsModifiable"),
                Some(if record.is_modifiable { "true" } else { "false" }),
                "case {case}: expected the catalog's IsModifiable {}, got {:?}",
                record.is_modifiable,
                parameter.field("IsModifiable")
            );
            assert_eq!(
                parameter.field("ChangeType"),
                Some(record.change_type.as_str()),
                "case {case}: expected the catalog's change type {}, got {:?}",
                record.change_type,
                parameter.field("ChangeType")
            );
            assert_eq!(
                parameter.field("MinimumEngineVersion"),
                Some(record.minimum_engine_version.as_str()),
                "case {case}: expected the catalog's minimum engine version {}, got {:?}",
                record.minimum_engine_version,
                parameter.field("MinimumEngineVersion")
            );
        }

        // A describe is a read: asking again reports the same list.
        let again = describe_ok(&emulator, &group, &[]).await;
        assert_eq!(
            names(&again),
            names(&reply),
            "case {group}: a read must not change what the next caller sees"
        );
    }
}

/// One term of a numeric `AllowedValues`, in the forms the published catalog
/// writes them in.
#[derive(Debug)]
enum Term {
    /// A single permitted value — the `0` of `0,20-`.
    Literal(f64),
    /// A range bounded only at the bottom: `20-`.
    From(f64),
    /// A range bounded at BOTH ends: `1048576-536870912`.
    Closed(f64, f64),
}

/// `allowed` split into its comma-separated terms, or `None` when it is not a
/// numeric value set at all (an enumeration of words, say).
fn numeric_terms(allowed: &str) -> Option<Vec<Term>> {
    allowed
        .split(',')
        .map(|term| match term.split_once('-') {
            Some((low, "")) => Some(Term::From(low.parse().ok()?)),
            Some((low, high)) => Some(Term::Closed(low.parse().ok()?, high.parse().ok()?)),
            None => Some(Term::Literal(term.parse().ok()?)),
        })
        .collect()
}

/// The widest closed integer range `reply` publishes for a MODIFIABLE parameter,
/// as (the parameter, the span it covers) — what a client scanning the catalog
/// for a parameter it may tune, and for the bounds to validate against, finds.
fn widest_modifiable_integer_range(reply: &Reply) -> Option<(String, f64)> {
    let mut widest: Option<(String, f64)> = None;
    for parameter in reported(reply) {
        if parameter.field("IsModifiable") != Some("true")
            || parameter.field("DataType") != Some("integer")
        {
            continue;
        }
        let Some(terms) = parameter.field("AllowedValues").and_then(numeric_terms) else {
            continue;
        };
        let [Term::Closed(low, high)] = terms[..] else {
            continue;
        };
        if widest.as_ref().is_none_or(|(_, span)| high - low > *span) {
            widest = Some((parameter.name.clone(), high - low));
        }
    }
    widest
}

/// A parameter a customer may SET is published with the value set it may be set
/// to: `Parameter$AllowedValues` carries the engine's own constraint, so a client
/// that reads the catalog to decide what to tune — and to what — has the bounds
/// to validate against instead of an empty element.
///
/// The axes: the FAMILY (both engine catalogs, every published version line) and
/// the kind of group it is reached through (a family default and a custom group),
/// and within each answer every numeric parameter the catalog publishes rather
/// than the one a recorded request happened to look at.
#[tokio::test]
async fn every_modifiable_numeric_parameter_publishes_the_values_it_may_take() {
    let emulator = Emulator::new();

    let mut cases: Vec<(String, String)> = state::PARAMETER_GROUP_FAMILIES
        .iter()
        .map(|family| (format!("default.{family}"), (*family).to_string()))
        .collect();
    for family in ["redis7", "memcached1.6", "valkey8"] {
        let name = format!("{CUSTOM_GROUP}-ranges-{}", family.replace('.', "-"));
        create_group(&emulator, &name, family).await;
        cases.push((name, family.to_string()));
    }

    // The engine lines whose catalog bounds a modifiable integer parameter at
    // both ends widely enough to tune against: Redis OSS 2.8 brought the sparse
    // HyperLogLog's byte bound, 4.0 active defragmentation's scan bound and 5.0
    // the protocol element's. A client discovering a parameter to tune on one of
    // these families must find such a range.
    let wide_ranges = [
        "redis2.8",
        "redis3.2",
        "redis4.0",
        "redis5.0",
        "redis6.x",
        "redis7",
        "valkey7",
        "valkey8",
    ];
    // The span the discovery case needs: a range of at least a thousand values,
    // which is what distinguishes a tunable range from a flag's `0,1`.
    const WIDE_SPAN: f64 = 1000.0;

    for (group, family) in cases {
        let reply = describe_ok(&emulator, &group, &[]).await;
        for parameter in reported(&reply) {
            let case = format!("{group}/{}", parameter.name);
            let numeric = matches!(parameter.field("DataType"), Some("integer" | "float"));
            // A parameter the engine or the node type fixes cannot be set at all,
            // so it has no set for a caller to discover.
            if !numeric || parameter.field("IsModifiable") != Some("true") {
                continue;
            }
            let allowed = parameter.field("AllowedValues").unwrap_or_default();
            assert!(
                !allowed.is_empty(),
                "case {case}: a modifiable numeric parameter must publish the values it may \
                 take, got {allowed:?}"
            );
            let terms = numeric_terms(allowed).unwrap_or_else(|| {
                panic!("case {case}: expected a numeric value set, got {allowed:?}")
            });
            for term in &terms {
                match term {
                    // Every bound the published catalog states is a finite,
                    // non-negative number: a parameter's floor is a count, a size
                    // or a percentage.
                    Term::Literal(value) | Term::From(value) => assert!(
                        value.is_finite() && *value >= 0.0,
                        "case {case}: expected a finite, non-negative bound, got {allowed:?}"
                    ),
                    Term::Closed(low, high) => assert!(
                        *low >= 0.0 && low <= high && high.is_finite(),
                        "case {case}: a closed range must run upwards from a non-negative \
                         bound, got {allowed:?}"
                    ),
                }
            }
        }

        let widest = widest_modifiable_integer_range(&reply);
        if wide_ranges.contains(&family.as_str()) {
            let (parameter, span) = widest.unwrap_or_else(|| {
                panic!(
                    "case {group}: expected a modifiable integer parameter with a closed range, \
                     got {:?}",
                    names(&reply)
                )
            });
            assert!(
                span >= WIDE_SPAN,
                "case {group}: expected a closed range spanning at least {WIDE_SPAN} values, got \
                 {span} on {parameter}"
            );
        }

        // Reading the catalog twice reports the same value sets: a describe is a
        // read, so nothing about it is minted per call.
        let again = describe_ok(&emulator, &group, &[]).await;
        let (before, after) = (reported(&reply), reported(&again));
        let allowed_values = |parameters: &[Reported]| -> Vec<(String, String)> {
            parameters
                .iter()
                .map(|parameter| {
                    (
                        parameter.name.clone(),
                        parameter
                            .field("AllowedValues")
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        };
        assert_eq!(
            allowed_values(&after),
            allowed_values(&before),
            "case {group}: a read must not change the value sets the next caller sees"
        );
    }
}

#[tokio::test]
async fn a_group_is_resolved_by_name_however_it_is_cased() {
    let emulator = Emulator::new();
    create_group(&emulator, CUSTOM_GROUP, "redis7").await;

    // A `CacheParameterGroupName` "is stored as a lowercase string", so the
    // same group answers however the request spells it.
    let lower = describe_ok(&emulator, CUSTOM_GROUP, &[]).await;
    for spelling in [
        CUSTOM_GROUP.to_ascii_uppercase(),
        "Fixer-Params".to_string(),
    ] {
        let reply = describe_ok(&emulator, &spelling, &[]).await;
        assert_eq!(
            names(&reply),
            names(&lower),
            "case {spelling}: must report the same parameters as {CUSTOM_GROUP}"
        );
    }
    let default_lower = describe_ok(&emulator, "default.redis7", &[]).await;
    let default_upper = describe_ok(&emulator, "DEFAULT.REDIS7", &[]).await;
    assert_eq!(
        names(&default_upper),
        names(&default_lower),
        "case DEFAULT.REDIS7: a family default is resolved the same way"
    );
}

#[tokio::test]
async fn a_value_set_on_a_group_is_reported_as_the_customers() {
    let emulator = Emulator::new();

    // The axis: the value set on a parameter — its family and engine catalog,
    // its data type, and the FORM of the value, including the numeric forms a
    // value must keep every digit of (a fraction, a trailing zero, a leading
    // zero, an integer wider than a double's mantissa) and a value set on a
    // parameter whose engine default is the empty string. Every parameter named
    // here is one its family publishes (asserted below), and every value
    // differs from that family's engine default (asserted below), so a row that
    // echoed the catalog back could not pass.
    let cases: [(&str, &str, &str); 9] = [
        ("redis7", "timeout", "300"),
        ("redis7", "maxmemory-policy", "allkeys-lru"),
        ("redis7", "reserved-memory-percent", "0"),
        ("redis6.x", "maxmemory-samples", "0100"),
        ("redis2.8", "timeout", "1.50"),
        ("redis5.0", "notify-keyspace-events", "gxE"),
        ("valkey8", "repl-backlog-size", "9007199254740993"),
        ("memcached1.6", "chunk_size_growth_factor", "1.0625"),
        ("memcached1.4", "max_item_size", "2097152"),
    ];
    for (index, (family, parameter, value)) in cases.into_iter().enumerate() {
        let case = format!("{family}/{parameter}={value}");
        let group = format!("{CUSTOM_GROUP}-set-{index}");
        let untouched = format!("{CUSTOM_GROUP}-untouched-{index}");
        create_group(&emulator, &group, family).await;
        create_group(&emulator, &untouched, family).await;

        // Only a parameter the family publishes can be set on one of its
        // groups; a row naming anything else would be testing nothing.
        assert!(
            catalog_names(family).contains(parameter),
            "case {case}: the family must publish this parameter, got {:?}",
            catalog_names(family)
        );
        set_value(&emulator, &group, parameter, value);

        let reply = describe_ok(&emulator, &group, &[]).await;
        let reported_one = one(&reply, &case, parameter);
        // The value is reported EXACTLY as it was stored: no rounding, no
        // reformatting, no dropped digit.
        assert_eq!(
            reported_one.field("ParameterValue").unwrap_or_default(),
            value,
            "case {case}: expected the value set on the group verbatim, got {:?}",
            reported_one.field("ParameterValue")
        );
        assert_eq!(
            reported_one.field("Source"),
            Some(state::PARAMETER_SOURCE_USER),
            "case {case}: a value set on the group is reported as {}, got {:?}",
            state::PARAMETER_SOURCE_USER,
            reported_one.field("Source")
        );
        // Only that parameter changed: every other one of the group still
        // reports the service's own value.
        for other in reported(&reply)
            .iter()
            .filter(|other| other.name != parameter)
        {
            assert_eq!(
                other.field("Source"),
                Some(state::PARAMETER_SOURCE_SYSTEM),
                "case {case}: {} was not set, so its source is {}, got {:?}",
                other.name,
                state::PARAMETER_SOURCE_SYSTEM,
                other.field("Source")
            );
        }

        // The set value lives on the GROUP, not on the family catalog: another
        // group of the same family, and the family's engine defaults, still
        // report the engine's own value.
        let neighbour_reply = describe_ok(&emulator, &untouched, &[]).await;
        let neighbour = one(&neighbour_reply, &case, parameter);
        assert_eq!(
            neighbour.field("Source"),
            Some(state::PARAMETER_SOURCE_SYSTEM),
            "case {case}: another group of the family is untouched, got {:?}",
            neighbour.field("Source")
        );
        assert_ne!(
            neighbour.field("ParameterValue").unwrap_or_default(),
            value,
            "case {case}: the row's value must differ from the engine default, or it proves \
             nothing"
        );
        let defaults = emulator
            .call(
                "DescribeEngineDefaultParameters",
                &params(&[("CacheParameterGroupFamily", family)]),
            )
            .await;
        let published = one(&defaults, &case, parameter);
        assert_eq!(
            published.field("ParameterValue").unwrap_or_default(),
            neighbour.field("ParameterValue").unwrap_or_default(),
            "case {case}: the family's engine default must be unchanged by a value set on one \
             group"
        );
    }
}

#[tokio::test]
async fn the_source_member_selects_the_parameters_of_that_source() {
    let emulator = Emulator::new();
    create_group(&emulator, CUSTOM_GROUP, "redis7").await;
    // Two set values, so the "user" selection is a proper subset of the list.
    let set: [(&str, &str); 2] = [("timeout", "120"), ("maxmemory-policy", "noeviction")];
    for (parameter, value) in set {
        set_value(&emulator, CUSTOM_GROUP, parameter, value);
    }
    let all: Vec<String> = names(&describe_ok(&emulator, CUSTOM_GROUP, &[]).await);
    let set_names: BTreeSet<String> = set
        .iter()
        .map(|(parameter, _)| (*parameter).to_string())
        .collect();
    let unset_names: BTreeSet<String> = all
        .iter()
        .filter(|name| !set_names.contains(*name))
        .cloned()
        .collect();
    assert!(
        !set_names.is_empty() && !unset_names.is_empty(),
        "both selections must be non-empty, or the partition proves nothing: {set_names:?} / \
         {unset_names:?}"
    );

    // The axis: the `Source` spelling, over the vocabulary the member documents
    // ("The parameter types to return. Valid values: user | system |
    // engine-default" — `smithy_query(query="doc",
    // shape="DescribeCacheParametersMessage", member="Source")`), in both
    // cases. `engine-default` selects the same parameters as `system`, because
    // an engine default IS the service's own value and is reported under that
    // spelling.
    let cases: [(&str, &BTreeSet<String>); 6] = [
        ("user", &set_names),
        ("USER", &set_names),
        ("system", &unset_names),
        ("System", &unset_names),
        ("engine-default", &unset_names),
        ("ENGINE-DEFAULT", &unset_names),
    ];
    for (source, expected) in cases {
        let reply = describe_ok(&emulator, CUSTOM_GROUP, &[("Source", source)]).await;
        let selected: BTreeSet<String> = names(&reply).into_iter().collect();
        assert_eq!(
            &selected, expected,
            "case Source={source}: expected {expected:?}, got {selected:?}"
        );
        for parameter in reported(&reply) {
            let reported_source = parameter.field("Source").unwrap_or_default();
            let selects_system = source.eq_ignore_ascii_case(state::PARAMETER_SOURCE_SYSTEM)
                || source.eq_ignore_ascii_case(state::PARAMETER_SOURCE_FILTER_ENGINE_DEFAULT);
            let expected_source = if selects_system {
                state::PARAMETER_SOURCE_SYSTEM
            } else {
                state::PARAMETER_SOURCE_USER
            };
            assert_eq!(
                reported_source, expected_source,
                "case Source={source}: {} is reported as {reported_source}, so it must not be \
                 selected",
                parameter.name
            );
        }
    }
    // The two selections partition the whole list, and asking for neither is
    // asking for both.
    let mut partitioned: BTreeSet<String> = set_names.clone();
    partitioned.extend(unset_names.clone());
    assert_eq!(
        partitioned,
        all.iter().cloned().collect::<BTreeSet<String>>(),
        "the user and system selections must partition the group's parameter list"
    );

    // A spelling outside that vocabulary is a value the member may not take.
    for source in ["bogus", "engine_default", "users", "SYSTEM-DEFAULT", "user "] {
        let reply = describe(&emulator, CUSTOM_GROUP, &[("Source", source)]).await;
        assert_eq!(
            reply.status, 400,
            "case Source={source:?}: must be refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case Source={source:?}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
}

#[tokio::test]
async fn the_answer_is_paged_at_the_window_max_records_documents() {
    let emulator = Emulator::new();
    create_group(&emulator, CUSTOM_GROUP, "redis7").await;
    let full = paged_names(&describe_ok(&emulator, CUSTOM_GROUP, &[]).await);
    // The smallest page the member allows has to be smaller than the list, or
    // nothing below pages at all.
    assert!(
        full.len() > 20,
        "this suite's group must out-grow the smallest page, got {} parameters",
        full.len()
    );

    // The axis: the page size, over the window `MaxRecords` documents
    // ("Default: 100. Constraints: minimum 20; maximum 100") — each published
    // bound and two sizes between them.
    for page_size in ["20", "21", "37", "100"] {
        let window: usize = page_size.parse().expect("a page size");
        let (seen, pages) = walk(&emulator, CUSTOM_GROUP, page_size).await;
        assert_eq!(
            seen, full,
            "case MaxRecords={page_size}: a walk must report the whole list, in order"
        );
        let expected_pages = full.len().div_ceil(window);
        assert_eq!(
            pages, expected_pages,
            "case MaxRecords={page_size}: expected {expected_pages} pages, got {pages}"
        );
    }

    // One past each published bound is refused rather than clamped.
    for page_size in ["19", "101", "0", "-1"] {
        let reply = describe(&emulator, CUSTOM_GROUP, &[("MaxRecords", page_size)]).await;
        assert_eq!(
            reply.status, 400,
            "case MaxRecords={page_size}: must be refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case MaxRecords={page_size}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }

    // A marker is only meaningful inside the group it was minted for: one from
    // another group, one from another paginated describe, and one nothing
    // minted all resume nothing.
    let first = describe_ok(&emulator, CUSTOM_GROUP, &[("MaxRecords", "20")]).await;
    let token = first
        .marker()
        .unwrap_or_else(|| panic!("a partial page carries a marker, got {}", first.body))
        .to_string();
    let other = "fixer-params-other";
    create_group(&emulator, other, "redis7").await;
    let defaults = emulator
        .call(
            "DescribeEngineDefaultParameters",
            &params(&[
                ("CacheParameterGroupFamily", "redis7"),
                ("MaxRecords", "20"),
            ]),
        )
        .await;
    let foreign = defaults
        .marker()
        .unwrap_or_else(|| panic!("the family's defaults page too, got {}", defaults.body))
        .to_string();
    for (case, group, marker) in [
        ("another group", other, token.as_str()),
        ("another describe", CUSTOM_GROUP, foreign.as_str()),
        ("not a token", CUSTOM_GROUP, "zzz"),
        ("a token of nothing", CUSTOM_GROUP, "6e6f7065"),
    ] {
        let reply = describe(&emulator, group, &[("Marker", marker)]).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: a marker not minted for {group} must be refused, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterValue",
            "case {case}: expected InvalidParameterValue, got {}",
            reply.body
        );
    }
    // The group's OWN marker resumes the walk where its page ended.
    let second = describe_ok(&emulator, CUSTOM_GROUP, &[("Marker", &token)]).await;
    assert_eq!(
        paged_names(&second),
        full[20..].to_vec(),
        "the group's own marker must resume at the record after its page"
    );
}

#[tokio::test]
async fn a_group_the_account_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    create_group(&emulator, CUSTOM_GROUP, "redis7").await;
    // A group that existed and was deleted is a name the account no longer
    // holds, so it is refused exactly like one that never existed.
    let deleted = "fixer-params-deleted";
    create_group(&emulator, deleted, "memcached1.6").await;
    let removed = emulator
        .call(
            "DeleteCacheParameterGroup",
            &params(&[("CacheParameterGroupName", deleted)]),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "DeleteCacheParameterGroup must answer 200, got {} {}",
        removed.status, removed.body
    );

    // The axis: the way the name misses — never created, created and deleted, a
    // `default.` name of a family the service does not publish, the bare
    // `default.` prefix, and a name that differs from a stored one by more than
    // case.
    for group in [
        "no-such-group",
        deleted,
        "default.redis99",
        "default.",
        "fixer-params-2",
    ] {
        let reply = describe(&emulator, group, &[]).await;
        assert_eq!(
            reply.status, 404,
            "case {group}: must be refused 404, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "CacheParameterGroupNotFound",
            "case {group}: expected CacheParameterGroupNotFound, got {}",
            reply.body
        );
        assert!(
            reply.error_message().contains(group),
            "case {group}: the refusal must name the group asked for, got {}",
            reply.error_message()
        );
    }
}
