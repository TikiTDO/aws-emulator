// SPDX-License-Identifier: CC0-1.0

//! Region scoping of every resource this service holds, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness [`crate::virtual_cluster_job_run_loop::Emr`] and
//! driven with restJson1 HTTP requests whose SigV4 credential scope — the thing a
//! regional endpoint is addressed through — names the region under test, so routing,
//! deserialization, constraint validation, the handler and serialization all run exactly
//! as they do for an AWS SDK client.
//!
//! The RULE under test: EMR on EKS is a regional service, so a resource exists only in
//! the region whose endpoint created it. Every read, every listing, every mutate and
//! every reference resolves an id or an ARN among the resources of the region the request
//! is signed for: a resource of another region is not found there, is not listed there,
//! and cannot be changed from there. The rule holds for every resource class this service
//! stores (virtual cluster, job run, job template, security configuration, managed
//! endpoint), and the `clientToken` an `@idempotencyToken` carries names a resource in
//! one region, so the same token spent at two endpoints mints one resource at each.
//!
//! The tables span what the rule generalises over: the region PAIR (four pairs, including
//! one pair driven both ways round), the resource class, the operation kind (describe,
//! list, delete, update, cancel, start, tag, untag, list-tags) and the member a request
//! uses to REFERENCE another resource.

use serde_json::{json, Value};

use crate::resource_tag_removal::encoded;
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr};

/// The region pairs every table below is driven over: `(home, other)`, two DIFFERENT
/// regions of the same partition, so an id minted at `home` is offered to `other`.
///
/// The rows differ in the geography and the form of the region code, and the first pair
/// appears both ways round, so neither direction is privileged and nothing depends on one
/// region being the emulator's own default (`state::DEFAULT_REGION` is `us-east-1`).
const REGION_PAIRS: [(&str, &str); 4] = [
    ("us-east-1", "eu-west-1"),
    ("eu-west-1", "us-east-1"),
    ("ap-southeast-2", "us-west-2"),
    ("ca-central-1", "eu-central-1"),
];

/// `POST /virtualclusters` — `CreateVirtualCluster`.
async fn create_cluster(emr: &Emr, region: &str, body: Value) -> Answer {
    emr.send_in(region, "POST", "/virtualclusters", Some(body))
        .await
}

/// One `CreateVirtualCluster` body naming a container provider's namespace.
fn cluster_body(name: &str, token: &str, namespace: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "containerProvider": {
            "id": "eks-region-fixture",
            "type": "EKS",
            "info": {"eksInfo": {"namespace": namespace}},
        },
        "tags": {"team": "data"},
    })
}

/// One acceptable `StartJobRun` body: a submission naming no job template carries
/// `executionRoleArn`, `releaseLabel` and `jobDriver` itself.
fn job_body(token: &str) -> Value {
    json!({
        "clientToken": token,
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
        "releaseLabel": "emr-6.10.0-latest",
        "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
    })
}

/// One acceptable `CreateJobTemplate` body.
fn template_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "jobTemplateData": {
            "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
            "releaseLabel": "emr-6.15.0-latest",
            "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
        },
    })
}

/// One acceptable `CreateSecurityConfiguration` body.
fn configuration_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "securityConfigurationData": {},
    })
}

/// One acceptable `CreateManagedEndpoint` body: the members the model marks `@required`
/// beside the `@httpLabel` cluster id.
fn endpoint_body(name: &str, token: &str) -> Value {
    json!({
        "name": name,
        "clientToken": token,
        "type": "JUPYTER_ENTERPRISE_GATEWAY",
        "releaseLabel": "emr-6.10.0-latest",
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
    })
}

/// The id a create answered with.
fn created_id(answer: &Answer, case: &str) -> String {
    text(answer.ok(case), "/id", case)
}

/// One resource of each class, created against ONE region's endpoint.
struct Fixture {
    cluster: String,
    job_run: String,
    template: String,
    configuration: String,
    endpoint: String,
}

impl Fixture {
    /// Create a cluster with a job run, a managed endpoint, a job template and a security
    /// configuration in `region`. `tag` distinguishes one region's fixture from the
    /// other's, so no two creates share a `clientToken` by accident.
    async fn build(emr: &Emr, region: &str, tag: &str) -> Self {
        let case = format!("the {region} fixture");
        let cluster = created_id(
            &create_cluster(
                emr,
                region,
                cluster_body(
                    &format!("cluster-{tag}"),
                    &format!("vc-tok-{tag}"),
                    &format!("ns-{tag}"),
                ),
            )
            .await,
            &case,
        );
        let job_run = created_id(
            &emr.send_in(
                region,
                "POST",
                &format!("/virtualclusters/{cluster}/jobruns"),
                Some(job_body(&format!("job-tok-{tag}"))),
            )
            .await,
            &case,
        );
        let endpoint = created_id(
            &emr.send_in(
                region,
                "POST",
                &format!("/virtualclusters/{cluster}/endpoints"),
                Some(endpoint_body(
                    &format!("endpoint-{tag}"),
                    &format!("ep-tok-{tag}"),
                )),
            )
            .await,
            &case,
        );
        let template = created_id(
            &emr.send_in(
                region,
                "POST",
                "/jobtemplates",
                Some(template_body(
                    &format!("template-{tag}"),
                    &format!("jt-tok-{tag}"),
                )),
            )
            .await,
            &case,
        );
        let configuration = created_id(
            &emr.send_in(
                region,
                "POST",
                "/securityconfigurations",
                Some(configuration_body(
                    &format!("config-{tag}"),
                    &format!("sc-tok-{tag}"),
                )),
            )
            .await,
            &case,
        );
        Self {
            cluster,
            job_run,
            template,
            configuration,
            endpoint,
        }
    }

    /// The ARN of this fixture's virtual cluster in `region`, as the tagging operations
    /// name it.
    fn cluster_arn(&self, region: &str) -> String {
        format!(
            "arn:aws:emr-containers:{region}:{}:/virtualclusters/{}",
            engine::emulator_account_id(),
            self.cluster
        )
    }
}

/// One read of one stored resource: the path it is read at and the member the answer
/// carries it in.
struct ReadCase {
    label: &'static str,
    path: String,
    /// The JSON pointer of the resource in a successful answer.
    pointer: &'static str,
    /// Whether the answer carries the resource's own `arn`, which has to name the region
    /// the resource was created in.
    carries_arn: bool,
    /// The error class the OTHER region's endpoint refuses this read with.
    code: &'static str,
}

/// RULE, reading: a resource is described only at the endpoint of the region that created
/// it, the ARN it reports names that region, and offering its id to another region's
/// endpoint is the modelled `ResourceNotFoundException` (HTTP 400) — the answer for a
/// resource that is not there. A read that names the resource by its ARN is refused there
/// as an invalid resource reference instead, because an ARN of another region is not one
/// that endpoint mints at all ([`crate::tagging_resource_arn_form`] pins that class).
///
/// The rows span every resource class that has a reader, including `ListTagsForResource`,
/// which resolves an ARN rather than an id.
#[tokio::test]
async fn a_resource_is_read_only_at_the_endpoint_of_the_region_that_created_it() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        let fixture = Fixture::build(&emr, home, "read").await;
        let cases = [
            ReadCase {
                label: "DescribeVirtualCluster",
                path: format!("/virtualclusters/{}", fixture.cluster),
                pointer: "/virtualCluster",
                carries_arn: true,
                code: "ResourceNotFoundException",
            },
            ReadCase {
                label: "DescribeJobRun",
                path: format!(
                    "/virtualclusters/{}/jobruns/{}",
                    fixture.cluster, fixture.job_run
                ),
                pointer: "/jobRun",
                carries_arn: true,
                code: "ResourceNotFoundException",
            },
            ReadCase {
                label: "DescribeManagedEndpoint",
                path: format!(
                    "/virtualclusters/{}/endpoints/{}",
                    fixture.cluster, fixture.endpoint
                ),
                pointer: "/endpoint",
                carries_arn: true,
                code: "ResourceNotFoundException",
            },
            ReadCase {
                label: "DescribeJobTemplate",
                path: format!("/jobtemplates/{}", fixture.template),
                pointer: "/jobTemplate",
                carries_arn: true,
                code: "ResourceNotFoundException",
            },
            ReadCase {
                label: "DescribeSecurityConfiguration",
                path: format!("/securityconfigurations/{}", fixture.configuration),
                pointer: "/securityConfiguration",
                carries_arn: true,
                code: "ResourceNotFoundException",
            },
            ReadCase {
                label: "ListTagsForResource",
                path: format!("/tags/{}", encoded(&fixture.cluster_arn(home))),
                pointer: "/tags",
                carries_arn: false,
                code: "ValidationException",
            },
        ];

        for case in &cases {
            let label = format!("{} from {home} to {other}", case.label);
            let at_home = emr.send_in(home, "GET", &case.path, None).await;
            let found = field(at_home.ok(&label), case.pointer, &label).clone();
            if case.carries_arn {
                let arn = text(&found, "/arn", &label);
                assert!(
                    arn.starts_with(&format!("arn:aws:emr-containers:{home}:")),
                    "case {label:?}: expected an arn in {home}, got {arn:?}"
                );
            }

            emr.send_in(other, "GET", &case.path, None)
                .await
                .refused(&label, 400, case.code);

            // The refused cross-region read changed nothing the owning region sees.
            let again = emr.send_in(home, "GET", &case.path, None).await;
            assert_eq!(
                field(again.ok(&label), case.pointer, &label),
                &found,
                "case {label:?}: a cross-region read changed what the owning region reports"
            );
        }
    }
}

/// The `id`s a listing reported, under `pointer`.
fn listed_ids(body: &Value, pointer: &str, case: &str) -> Vec<String> {
    field(body, pointer, case)
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: {pointer} is not a list: {body}"))
        .iter()
        .map(|record| text(record, "/id", case))
        .collect()
}

/// RULE, listing: a listing reports the resources of the region it is signed for and no
/// others, so two regions holding resources of the same class answer disjoint sets; and a
/// listing scoped to a virtual cluster of another region selects none of that cluster's
/// records here, so it answers an empty page — the cluster id scopes the collection and
/// resolves no resource ([`crate::cluster_scoped_listing_scope`]).
#[tokio::test]
async fn a_listing_reports_only_the_resources_of_the_region_it_is_signed_for() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        let here = Fixture::build(&emr, home, "list-here").await;
        let there = Fixture::build(&emr, other, "list-there").await;

        // The account-wide listings: each endpoint reports its own region's records.
        let flat = [
            ("ListJobTemplates", "/jobtemplates", "/templates"),
            (
                "ListSecurityConfigurations",
                "/securityconfigurations",
                "/securityConfigurations",
            ),
        ];
        for (operation, path, pointer) in flat {
            let label = format!("{operation} in {home} beside {other}");
            let own = match operation {
                "ListJobTemplates" => (here.template.clone(), there.template.clone()),
                _ => (here.configuration.clone(), there.configuration.clone()),
            };
            let at_home = emr.send_in(home, "GET", path, None).await;
            let home_ids = listed_ids(at_home.ok(&label), pointer, &label);
            assert_eq!(
                home_ids,
                vec![own.0.clone()],
                "case {label:?}: {home} must list exactly its own record"
            );

            let at_other = emr.send_in(other, "GET", path, None).await;
            let other_ids = listed_ids(at_other.ok(&label), pointer, &label);
            assert_eq!(
                other_ids,
                vec![own.1.clone()],
                "case {label:?}: {other} must list exactly its own record"
            );
            assert!(
                !home_ids.iter().any(|id| other_ids.contains(id)),
                "case {label:?}: the two regions' listings overlap: {home_ids:?} and {other_ids:?}"
            );
        }

        // The listings scoped to a virtual cluster: the records a cluster holds are the
        // ones of the region the request is signed for, and no others.
        let nested = [
            ("ListJobRuns", "jobruns", "/jobRuns", here.job_run.clone()),
            (
                "ListManagedEndpoints",
                "endpoints",
                "/endpoints",
                here.endpoint.clone(),
            ),
        ];
        for (operation, segment, pointer, expected) in nested {
            let label = format!("{operation} of a {home} cluster from {other}");
            let path = format!("/virtualclusters/{}/{segment}", here.cluster);
            let at_home = emr.send_in(home, "GET", &path, None).await;
            assert_eq!(
                listed_ids(at_home.ok(&label), pointer, &label),
                vec![expected.clone()],
                "case {label:?}: {home} must list the cluster's own record"
            );

            let at_other = emr.send_in(other, "GET", &path, None).await;
            let crossed = listed_ids(at_other.ok(&label), pointer, &label);
            assert!(
                crossed.is_empty(),
                "case {label:?}: {other} must report no record of a {home} cluster, got \
                 {crossed:?}"
            );
        }
    }
}

/// One mutate offered to the wrong region: how it is sent and the error code the
/// operation models for a resource that is not there.
struct MutateCase {
    label: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    code: &'static str,
}

/// RULE, mutating: a mutate names a resource of the region it is signed for, so a request
/// offering another region's id is refused with the error that operation models for a
/// resource that is not there — and one offering another region's ARN with the error that
/// operation models for a resource reference it cannot resolve at all — and the resource it
/// named is left exactly as it was.
///
/// The rows span every mutate this service implements: the teardown, the update, the
/// cancellation, the two deletes, the two creates that reference a parent cluster, and the
/// two tagging writes.
#[tokio::test]
async fn a_cross_region_mutate_is_refused_and_leaves_the_resource_as_it_was() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        let fixture = Fixture::build(&emr, home, "mutate").await;
        let cluster = &fixture.cluster;
        let arn = fixture.cluster_arn(home);
        let cases = [
            MutateCase {
                label: "DeleteVirtualCluster",
                method: "DELETE",
                path: format!("/virtualclusters/{cluster}"),
                body: None,
                code: "ValidationException",
            },
            MutateCase {
                label: "UpdateVirtualCluster",
                method: "PATCH",
                path: format!("/virtualclusters/{cluster}"),
                body: Some(json!({
                    "clientToken": "cross-region-update",
                    "schedulerConfiguration": {"maxConcurrentJobRuns": 3},
                })),
                code: "ResourceNotFoundException",
            },
            MutateCase {
                label: "CancelJobRun",
                method: "DELETE",
                path: format!("/virtualclusters/{cluster}/jobruns/{}", fixture.job_run),
                body: None,
                code: "ValidationException",
            },
            MutateCase {
                label: "StartJobRun",
                method: "POST",
                path: format!("/virtualclusters/{cluster}/jobruns"),
                body: Some(job_body("cross-region-job")),
                code: "ResourceNotFoundException",
            },
            MutateCase {
                label: "CreateManagedEndpoint",
                method: "POST",
                path: format!("/virtualclusters/{cluster}/endpoints"),
                body: Some(endpoint_body("cross-region-endpoint", "cross-region-ep")),
                code: "ResourceNotFoundException",
            },
            MutateCase {
                label: "DeleteJobTemplate",
                method: "DELETE",
                path: format!("/jobtemplates/{}", fixture.template),
                body: None,
                code: "ValidationException",
            },
            MutateCase {
                label: "DeleteSecurityConfiguration",
                method: "DELETE",
                path: format!("/securityconfigurations/{}", fixture.configuration),
                body: None,
                code: "ValidationException",
            },
            MutateCase {
                label: "TagResource",
                method: "POST",
                path: format!("/tags/{}", encoded(&arn)),
                body: Some(json!({"tags": {"team": "stolen"}})),
                // An ARN of another region is not one this endpoint mints, which is an
                // invalid resource reference rather than a resource that is missing here
                // ([`crate::tagging_resource_arn_form`] pins that class).
                code: "ValidationException",
            },
            MutateCase {
                label: "UntagResource",
                method: "DELETE",
                path: format!("/tags/{}?tagKeys=team", encoded(&arn)),
                body: None,
                code: "ValidationException",
            },
        ];

        for case in &cases {
            let label = format!("{} from {other} against {home}", case.label);
            emr.send_in(other, case.method, &case.path, case.body.clone())
                .await
                .refused(&label, 400, case.code);
        }

        // Nothing the refused calls named moved: the cluster is still RUNNING and still
        // carries the tag it was created with, the run was never cancelled, and both the
        // template and the configuration are still there.
        let label = format!("the {home} fixture after the {other} mutates");
        let described = emr
            .send_in(home, "GET", &format!("/virtualclusters/{cluster}"), None)
            .await;
        let described = field(described.ok(&label), "/virtualCluster", &label).clone();
        assert_eq!(
            text(&described, "/state", &label),
            "RUNNING",
            "case {label:?}: a cross-region delete tore the cluster down: {described}"
        );
        assert_eq!(
            field(&described, "/tags", &label),
            &json!({"team": "data"}),
            "case {label:?}: a cross-region tagging call rewrote the cluster's tags"
        );
        assert_eq!(
            described.get("schedulerConfiguration"),
            None,
            "case {label:?}: a cross-region update applied a scheduler configuration"
        );

        let run = emr
            .send_in(
                home,
                "GET",
                &format!("/virtualclusters/{cluster}/jobruns/{}", fixture.job_run),
                None,
            )
            .await;
        let run_state = text(field(run.ok(&label), "/jobRun", &label), "/state", &label);
        assert!(
            run_state != "CANCEL_PENDING" && run_state != "CANCELLED",
            "case {label:?}: a cross-region cancel moved the run to {run_state}"
        );

        for (what, path) in [
            ("the job template", format!("/jobtemplates/{}", fixture.template)),
            (
                "the security configuration",
                format!("/securityconfigurations/{}", fixture.configuration),
            ),
        ] {
            let label = format!("{what} after the {other} deletes");
            emr.send_in(home, "GET", &path, None).await.ok(&label);
        }
    }
}

/// RULE, idempotency: the `clientToken` an `@idempotencyToken` carries names a resource in
/// ONE region, so the same token spent at two endpoints mints one resource at each, each
/// described only in its own region and each carrying its own region's ARN, and a replay
/// at either endpoint still answers that endpoint's resource.
#[tokio::test]
async fn one_client_token_mints_one_resource_in_each_region() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        // One create per account-wide resource class, each driven in both regions under
        // the SAME token.
        let creates: [(&str, &str, Value); 3] = [
            (
                "CreateVirtualCluster",
                "/virtualclusters",
                cluster_body("shared-token", "one-token", "ns-shared"),
            ),
            (
                "CreateJobTemplate",
                "/jobtemplates",
                template_body("shared-token", "one-token"),
            ),
            (
                "CreateSecurityConfiguration",
                "/securityconfigurations",
                configuration_body("shared-token", "one-token"),
            ),
        ];

        for (operation, path, body) in creates {
            let label = format!("{operation} under one token in {home} and {other}");
            let in_home = created_id(
                &emr.send_in(home, "POST", path, Some(body.clone())).await,
                &label,
            );
            let in_other = created_id(
                &emr.send_in(other, "POST", path, Some(body.clone())).await,
                &label,
            );
            assert_ne!(
                in_home, in_other,
                "case {label:?}: one token must mint a resource in each region"
            );

            // Each endpoint replays its OWN resource, not the other region's.
            for (region, expected) in [(home, &in_home), (other, &in_other)] {
                let replay = created_id(
                    &emr.send_in(region, "POST", path, Some(body.clone())).await,
                    &label,
                );
                assert_eq!(
                    &replay, expected,
                    "case {label:?}: the replay in {region} answered another region's resource"
                );
            }

            // And each resource is readable only where it was minted.
            let reader = match operation {
                "CreateVirtualCluster" => "/virtualclusters",
                "CreateJobTemplate" => "/jobtemplates",
                _ => "/securityconfigurations",
            };
            for (region, id, foreign) in [
                (home, &in_home, &in_other),
                (other, &in_other, &in_home),
            ] {
                let at_region = emr
                    .send_in(region, "GET", &format!("{reader}/{id}"), None)
                    .await;
                at_region.ok(&label);
                emr.send_in(region, "GET", &format!("{reader}/{foreign}"), None)
                    .await
                    .refused(&label, 400, "ResourceNotFoundException");
            }
        }
    }
}

/// RULE, references: a member that names ANOTHER resource is resolved in the region the
/// request is signed for, so a reference to a resource of another region is the modelled
/// `ResourceNotFoundException` (HTTP 400) — while the same reference inside its own region
/// resolves.
///
/// The rows are the two references this service's creates carry:
/// `CreateVirtualCluster$securityConfigurationId` and `StartJobRun$jobTemplateId`. A job
/// template that DOES resolve starts the run it describes, which is what makes the
/// refusal of the other region's template about the region and not about the reference.
#[tokio::test]
async fn a_reference_to_another_regions_resource_is_not_found() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        let here = Fixture::build(&emr, home, "ref-here").await;
        let there = Fixture::build(&emr, other, "ref-there").await;

        // `securityConfigurationId`: the configuration of the other region does not exist
        // at this endpoint, while this region's own does.
        let label = format!("securityConfigurationId of {other} used in {home}");
        let mut foreign = cluster_body("foreign-config", "vc-foreign-config", "ns-foreign");
        foreign["securityConfigurationId"] = json!(there.configuration);
        create_cluster(&emr, home, foreign)
            .await
            .refused(&label, 400, "ResourceNotFoundException");

        let label = format!("securityConfigurationId of {home} used in {home}");
        let mut own = cluster_body("own-config", "vc-own-config", "ns-own");
        own["securityConfigurationId"] = json!(here.configuration);
        create_cluster(&emr, home, own).await.ok(&label);

        // `jobTemplateId`: the template of the other region does not exist at this
        // endpoint; this region's own one does, and supplies the run's values.
        let label = format!("jobTemplateId of {other} used in {home}");
        let mut foreign = job_body("job-foreign-template");
        foreign["jobTemplateId"] = json!(there.template);
        emr.send_in(
            home,
            "POST",
            &format!("/virtualclusters/{}/jobruns", here.cluster),
            Some(foreign),
        )
        .await
        .refused(&label, 400, "ResourceNotFoundException");

        let label = format!("jobTemplateId of {home} used in {home}");
        let mut own = job_body("job-own-template");
        own["jobTemplateId"] = json!(here.template);
        emr.send_in(
            home,
            "POST",
            &format!("/virtualclusters/{}/jobruns", here.cluster),
            Some(own),
        )
        .await
        .ok(&label);
    }
}

/// RULE, the namespace mapping: "a single virtual cluster maps to a single Kubernetes
/// namespace", and the container cluster that namespace belongs to is itself regional, so
/// the collision is resolved among the clusters of the region the create is signed for:
/// the same provider and namespace may be registered once in each region, and a second
/// registration inside ONE region is refused with the modelled `ValidationException`.
#[tokio::test]
async fn a_namespace_mapping_collides_only_inside_one_region() {
    for (home, other) in REGION_PAIRS {
        let emr = Emr::new();
        let label = format!("one namespace registered in {home} and {other}");
        let in_home = created_id(
            &create_cluster(
                &emr,
                home,
                cluster_body("ns-home", "vc-ns-home", "shared-namespace"),
            )
            .await,
            &label,
        );
        let in_other = created_id(
            &create_cluster(
                &emr,
                other,
                cluster_body("ns-other", "vc-ns-other", "shared-namespace"),
            )
            .await,
            &label,
        );
        assert_ne!(
            in_home, in_other,
            "case {label:?}: the same namespace must be registrable once per region"
        );

        // A second registration of that namespace in one region is the collision.
        create_cluster(
            &emr,
            home,
            cluster_body("ns-home-again", "vc-ns-home-again", "shared-namespace"),
        )
        .await
        .refused(&label, 400, "ValidationException");
    }
}
