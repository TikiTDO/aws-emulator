// SPDX-License-Identifier: CC0-1.0

//! The standing recording the tax-inheritance election is reserved to, end to end
//! over the wire — with the AWS Organizations control plane mounted beside this
//! service on the one shared engine.
//!
//! RULE: which account manages the organization an account belongs to is state the
//! Organizations control plane owns — "the account whose user is calling the
//! `CreateOrganization` operation automatically becomes the management account of
//! the new organization", and no TaxSettings request puts an account anywhere — so
//! the gate `PutTaxInheritance` reserves the organization-level election to READS
//! that fact from that control plane's store through the shared engine. A client
//! that establishes the organization there is therefore served the write and reads
//! the election back, and a caller that manages no organization is refused with the
//! modelled `ValidationException` (HTTP 400, `errorCode` `FieldValidationFailed`,
//! no `fieldList`) having recorded nothing.
//!
//! The rows span the worlds the rule generalises over: no Organizations control
//! plane on the engine at all, one whose store holds no organization, an
//! organization established by mounting that control plane, one created through its
//! own `CreateOrganization` over the wire, and an organization some OTHER account
//! manages (two ids, at both ends of the account-id range) — each against both
//! `HeritageStatus` values.
//!
//! The one world with no wire path is the organization another account manages:
//! Organizations makes the account that creates an organization its management
//! account, so that world is arranged through the crate's one shared helper
//! (`record_management_account`), which writes the organization's own stored
//! record — the very state its handlers answer from.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_taxsettings_taxsettings::state::{
    DEFAULT_HERITAGE_STATUS, ERROR_CODE_FIELD_VALIDATION_FAILED, HERITAGE_STATUS_OPT_IN,
    HERITAGE_STATUS_OPT_OUT, TAX_INHERITANCE_MANAGEMENT_ACCOUNT_ONLY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::tax_registration_lifecycle::{call, field_list, mount_on, record_management_account};

/// Every value of the model's `HeritageStatus` enum
/// (`smithy_query(query="shape", shape="HeritageStatus")`: `OptIn | OptOut`).
const HERITAGE_STATUSES: &[&str] = &[HERITAGE_STATUS_OPT_IN, HERITAGE_STATUS_OPT_OUT];

/// The region every request is signed for; neither service's state is regional, so
/// no assertion depends on it.
const REGION: &str = "us-east-1";

/// The `X-Amz-Target` prefix the Organizations emulator routes on (that crate's
/// `OPERATIONS.md`, "Wire protocol").
const ORGANIZATIONS_TARGET_PREFIX: &str = "AWSOrganizationsV20161128";

/// How the organization the emulated account belongs to is established, if at all.
#[derive(Debug, Clone, Copy)]
enum Organization {
    /// No organization at all: the account is a standalone one.
    None,
    /// The one mounting the Organizations control plane establishes.
    EstablishedByMount,
    /// One created through that control plane's own `CreateOrganization`, over the
    /// wire.
    CreatedOverTheWire,
    /// One this account id manages, rather than the emulated account.
    ManagedBy(&'static str),
}

/// One world a `PutTaxInheritance` call is made in.
#[derive(Debug)]
struct World {
    case: &'static str,
    /// Whether the Organizations control plane is mounted on the same engine.
    control_plane: bool,
    organization: Organization,
    /// Whether a write of the election by the emulated account is served in it.
    served: bool,
}

/// This service, and optionally the Organizations control plane, on one engine.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// Build the world `world` describes.
    async fn new(world: &World) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = mount_on(&engine);
        if world.control_plane {
            router.register_service(
                service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(
                    Arc::clone(&engine),
                ),
            );
        }
        let emulator = Self { engine, router };

        match world.organization {
            // Mounting the control plane establishes the organization the emulated
            // account belongs to, so a world that holds none takes it away again —
            // and so does one that establishes its own organization below. A world
            // with no control plane mounted has nothing to take away: nothing has
            // established an organization on that engine.
            Organization::None => {
                if world.control_plane {
                    emulator.forget_the_organization();
                }
            }
            Organization::EstablishedByMount => {}
            Organization::CreatedOverTheWire => {
                emulator.forget_the_organization();
                emulator.create_organization(world.case).await;
            }
            Organization::ManagedBy(account_id) => {
                record_management_account(&emulator.engine, Some(account_id));
            }
        }
        emulator
    }

    /// Put the emulated account in no organization at all, through the crate's one
    /// shared helper for the standing.
    fn forget_the_organization(&self) {
        record_management_account(&self.engine, None);
    }

    /// Create the organization through the Organizations emulator's own
    /// `CreateOrganization`, over that service's wire.
    async fn create_organization(&self, case: &str) {
        let (status, created) = self.organizations("CreateOrganization", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateOrganization expected 200, got {status} with {created:?}"
        );
    }

    /// The `MasterAccountId` the organization's own `DescribeOrganization` reports,
    /// read over the wire.
    async fn described_management_account(&self, case: &str) -> String {
        let (status, described) = self.organizations("DescribeOrganization", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeOrganization expected 200, got {status} with {described:?}"
        );
        described
            .get("Organization")
            .and_then(|organization| organization.get("MasterAccountId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// Call one operation of the Organizations control plane mounted beside this
    /// service, over THAT service's wire protocol (`awsJson1_1`, routed by
    /// `X-Amz-Target`).
    async fn organizations(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("organizations.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header(
                "x-amz-target",
                format!("{ORGANIZATIONS_TARGET_PREFIX}.{operation}"),
            )
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential=AKIDEXAMPLE/20260101/{REGION}/organizations/aws4_request, \
                     SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the test body serializes"),
            ))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }
}

/// The election `GetTaxInheritance` reports.
async fn read_election(router: &MultiServiceRouter, case: &str) -> Value {
    let got = call(router, "GetTaxInheritance", &json!({})).await;
    assert_eq!(
        got.status, 200,
        "case {case:?}: GetTaxInheritance expected 200, got {got:?}"
    );
    got.get("heritageStatus").clone()
}

/// The worlds the rule is asserted over.
const WORLDS: &[World] = &[
    World {
        case: "the Organizations control plane is not mounted on the engine",
        control_plane: false,
        organization: Organization::None,
        served: false,
    },
    World {
        case: "the control plane is mounted but holds no organization",
        control_plane: true,
        organization: Organization::None,
        served: false,
    },
    World {
        case: "mounting the control plane established the organization",
        control_plane: true,
        organization: Organization::EstablishedByMount,
        served: true,
    },
    World {
        case: "the organization was created over the wire",
        control_plane: true,
        organization: Organization::CreatedOverTheWire,
        served: true,
    },
    World {
        case: "another account manages the organization",
        control_plane: true,
        organization: Organization::ManagedBy("000000000001"),
        served: false,
    },
    World {
        case: "the manager is at the other end of the account id range",
        control_plane: true,
        organization: Organization::ManagedBy("999999999999"),
        served: false,
    },
];

/// RULE: the gate reads the organization's management account from the control
/// plane that owns it, so a client that establishes the organization THERE — by
/// nothing but HTTP requests — is served the election it records and reads that
/// election back, whichever `HeritageStatus` it elects.
#[tokio::test]
async fn an_organization_established_in_organizations_admits_the_election() {
    for world in WORLDS.iter().filter(|world| world.served) {
        for election in HERITAGE_STATUSES {
            let emulator = Emulator::new(world).await;
            let case = format!("{} / {election}", world.case);

            // The standing the gate judges by is the organization's own record of
            // it, as that control plane reports it over the wire.
            let management_account = emulator.described_management_account(&case).await;
            assert_eq!(
                management_account,
                engine::emulator_account_id(),
                "case {case:?}: expected the emulated account to manage the organization, got \
                 {management_account:?}"
            );

            let before = read_election(&emulator.router, &case).await;
            assert_eq!(
                before,
                json!(DEFAULT_HERITAGE_STATUS),
                "case {case:?}: expected the default election before any write, got {before:?}"
            );

            let put = call(
                &emulator.router,
                "PutTaxInheritance",
                &json!({ "heritageStatus": election }),
            )
            .await;
            assert_eq!(
                (put.status, put.error_code.as_str()),
                (200, ""),
                "case {case:?}: expected the election to be served with 200, got {put:?}"
            );
            assert_eq!(
                put.body,
                json!({}),
                "case {case:?}: expected the modelled empty body, got {:?}",
                put.body
            );

            let reported = read_election(&emulator.router, &case).await;
            assert_eq!(
                reported,
                json!(election),
                "case {case:?}: expected the recorded election to be reported, got {reported:?}"
            );
        }
    }
}

/// RULE: a caller that manages no organization — whatever the control plane beside
/// this service does or does not record — is refused with the modelled
/// `ValidationException` (400, `FieldValidationFailed`, no `fieldList`) and the
/// election the account already had is the one the read still reports.
#[tokio::test]
async fn a_caller_that_manages_no_organization_records_no_election() {
    for world in WORLDS.iter().filter(|world| !world.served) {
        for election in HERITAGE_STATUSES {
            let emulator = Emulator::new(world).await;
            let case = format!("{} / {election}", world.case);

            // Where another account manages the organization, that arrangement is
            // confirmed over the Organizations wire rather than assumed: the gate
            // judges by the id that control plane reports.
            if let Organization::ManagedBy(manager) = world.organization {
                let described = emulator.described_management_account(&case).await;
                assert_eq!(
                    described, manager,
                    "case {case:?}: expected the organization to be managed by {manager:?}, got \
                     {described:?}"
                );
            }

            let answer = call(
                &emulator.router,
                "PutTaxInheritance",
                &json!({ "heritageStatus": election }),
            )
            .await;
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {answer:?}"
            );
            assert_eq!(
                answer.get("errorCode").as_str(),
                Some(ERROR_CODE_FIELD_VALIDATION_FAILED),
                "case {case:?}: expected the FieldValidationFailed code, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.get("message").as_str(),
                Some(TAX_INHERITANCE_MANAGEMENT_ACCOUNT_ONLY),
                "case {case:?}: expected the refusal to state the rule, got {:?}",
                answer.body
            );
            assert!(
                field_list(&answer).is_empty(),
                "case {case:?}: expected no fieldList, got {:?}",
                answer.body
            );

            let reported = read_election(&emulator.router, &case).await;
            assert_eq!(
                reported,
                json!(DEFAULT_HERITAGE_STATUS),
                "case {case:?}: expected the refusal to record nothing, got {reported:?}"
            );
        }
    }
}

/// RULE: the gate reads the standing per call, so an account that is put in an
/// organization after being refused is served the very same request, and one the
/// organization is taken from is refused again — the election the gate admitted
/// stays recorded, because a refusal records nothing and reading changes nothing.
#[tokio::test]
async fn the_standing_is_read_from_the_control_plane_on_every_call() {
    for election in HERITAGE_STATUSES {
        let emulator = Emulator::new(&World {
            case: "an account put in an organization between two writes",
            control_plane: true,
            organization: Organization::None,
            served: false,
        })
        .await;
        let case = format!("put in an organization between two writes / {election}");
        let body = json!({ "heritageStatus": election });

        let refused = call(&emulator.router, "PutTaxInheritance", &body).await;
        assert_eq!(
            (refused.status, refused.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the first write to be refused, got {refused:?}"
        );

        // The organization is created where it belongs, over that service's wire.
        emulator.create_organization(&case).await;

        let served = call(&emulator.router, "PutTaxInheritance", &body).await;
        assert_eq!(
            (served.status, served.error_code.as_str()),
            (200, ""),
            "case {case:?}: expected the same write to be served once the organization exists, \
             got {served:?}"
        );
        let reported = read_election(&emulator.router, &case).await;
        assert_eq!(
            reported,
            json!(election),
            "case {case:?}: expected the served write to be reported, got {reported:?}"
        );

        // Taken out of the organization again, the caller manages none: the next
        // write is refused and the election it recorded before still answers the
        // read.
        emulator.forget_the_organization();
        let refused_again = call(&emulator.router, "PutTaxInheritance", &body).await;
        assert_eq!(
            (refused_again.status, refused_again.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a write from a standalone account to be refused, got \
             {refused_again:?}"
        );
        let still = read_election(&emulator.router, &case).await;
        assert_eq!(
            still,
            json!(election),
            "case {case:?}: expected the recorded election to survive the refusal, got {still:?}"
        );
    }
}
