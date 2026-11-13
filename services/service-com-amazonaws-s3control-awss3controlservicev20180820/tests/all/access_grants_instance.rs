// SPDX-License-Identifier: CC0-1.0

//! The S3 Access Grants instance lifecycle, driven end to end over HTTP.
//!
//! The rule under test: `CreateAccessGrantsInstance` records ONE S3 Access
//! Grants instance per (account, Region) — the `default` id every instance
//! carries, an ARN in that Region, the instant of the create, and, only when the
//! request associated an IAM Identity Center instance, that instance's ARN plus
//! the application ARN S3 Access Grants assigns for it — so every later
//! `GetAccessGrantsInstance` by that account in that Region reports exactly that
//! record, a create for a pair that already has one is refused rather than
//! replacing it, and an account with no instance in the Region the request is
//! signed for is refused as not existing.
//!
//! Every row below differs along something that rule generalises over: whether
//! an Identity Center instance is associated and which one, whether that ARN
//! carries an account segment, how many tags the create submits (both modelled
//! bounds of `TagList`, `len 0..50`), the Region the request is signed for, and
//! the account that owns the instance.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes it
//! per the Smithy model, runs the handler and serializes the restXml response.
//! The router, the request builder and the XML readers are the sibling suites',
//! reused rather than copied.

use std::collections::BTreeSet;

use crate::access_point_lifecycle::{
    assert_foreign_account_refused, call_in_region, xml_escape, Emulator, Reply, ACCOUNT,
    OTHER_ACCOUNT, REGION, XMLNS,
};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_INSTANCE_ID, ACCESS_GRANTS_INSTANCE_QUOTA,
};

/// A second signing Region, used to pin that an instance is Region-scoped: the
/// same account creating one here creates another resource rather than
/// conflicting with the one in [`REGION`].
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, so a table can give each row a Region of its own and
/// still have one left over to read an absent instance in.
const THIRD_REGION: &str = "ap-southeast-2";

/// A fourth and fifth signing Region, for the same reason: an instance's
/// identity is (account, Region), and a caller is authorized for ONE account, so
/// a row that needs an identity of its own takes another Region.
const FOURTH_REGION: &str = "sa-east-1";
const FIFTH_REGION: &str = "eu-west-3";

/// The largest tag set `CreateAccessGrantsInstance` accepts: `Tags` targets
/// `TagList`, which the model constrains to `len 0..50`.
const MAX_TAGS: usize = 50;

/// One `CreateAccessGrantsInstance` request: the members the model lets a caller
/// vary, plus the account and Region that are the resource's identity.
struct CreateCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    /// `Some` associates an IAM Identity Center instance, which is what makes
    /// the service assign an Identity Center application ARN.
    identity_center_arn: Option<&'static str>,
    /// How many tags the create submits.
    tags: usize,
}

impl CreateCase {
    const fn new(case: &'static str, account_id: &'static str, region: &'static str) -> Self {
        Self {
            case,
            account_id,
            region,
            identity_center_arn: None,
            tags: 0,
        }
    }

    fn request_body(&self) -> String {
        let mut xml = format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\">");
        if let Some(identity_center_arn) = self.identity_center_arn {
            xml.push_str(&format!(
                "<IdentityCenterArn>{}</IdentityCenterArn>",
                xml_escape(identity_center_arn)
            ));
        }
        if self.tags > 0 {
            xml.push_str("<Tags>");
            for index in 0..self.tags {
                xml.push_str(&format!(
                    "<Tag><Key>key-{index}</Key><Value>value-{index}</Value></Tag>"
                ));
            }
            xml.push_str("</Tags>");
        }
        xml.push_str("</CreateAccessGrantsInstanceRequest>");
        xml
    }
}

/// Create the instance `case` describes, signed for that case's Region.
async fn create(emulator: &Emulator, case: &CreateCase) -> Reply {
    call_in_region(
        &emulator.router,
        case.region,
        "POST",
        "/v20180820/accessgrantsinstance",
        case.account_id,
        &case.request_body(),
    )
    .await
}

/// Read the instance `account_id` owns in `region`.
///
/// `pub(crate)` because the sibling refusal suite reads an instance back the
/// same way rather than writing this again.
pub(crate) async fn get(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance",
        account_id,
        "",
    )
    .await
}

/// The ARN an S3 Access Grants instance in `region` must report: the
/// `access-grants/default` resource in the `s3` ARN namespace, in that Region,
/// under the emulator's canonical account.
fn expected_arn(region: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:access-grants/{ACCESS_GRANTS_INSTANCE_ID}",
        ::emulator_server::emulator_account_id()
    )
}

/// The create cases every lifecycle assertion is run over. Each row owns a
/// distinct (account, Region) pair, because that pair is the resource's
/// identity, and differs from the others in something the behaviour under test
/// generalises over.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase::new("no optional members", ACCOUNT, REGION),
        CreateCase {
            identity_center_arn: Some("arn:aws:sso:::instance/ssoins-1234567890abcdef"),
            ..CreateCase::new("identity center, no account segment", ACCOUNT, OTHER_REGION)
        },
        CreateCase {
            // The modelled pattern admits an optional 12-digit account segment,
            // so this is the other form of the same member.
            identity_center_arn: Some("arn:aws:sso::123456789012:instance/ssoins-fedcba0987654321"),
            tags: 1,
            ..CreateCase::new("identity center with an account segment", ACCOUNT, THIRD_REGION)
        },
        CreateCase {
            identity_center_arn: Some("arn:aws:sso:::instance/ssoins-1234567890abcdef"),
            ..CreateCase::new(
                "another Region, same identity center instance",
                ACCOUNT,
                FOURTH_REGION,
            )
        },
        CreateCase {
            tags: MAX_TAGS,
            ..CreateCase::new("the largest tag set TagList admits", ACCOUNT, FIFTH_REGION)
        },
    ]
}

/// Assert `reply` reports the instance `case` describes, and answer its
/// `CreatedAt`.
fn assert_reports_instance(case: &CreateCase, reply: &Reply, call: &str) -> String {
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected {call} to answer 200, got {} with body {}",
        case.case, reply.status, reply.body
    );

    let id = reply.text("AccessGrantsInstanceId").unwrap_or_default();
    assert_eq!(
        id, ACCESS_GRANTS_INSTANCE_ID,
        "case {:?}: expected {call} to report the documented instance id, got {id:?}",
        case.case
    );

    let arn = reply.text("AccessGrantsInstanceArn").unwrap_or_default();
    assert_eq!(
        arn,
        expected_arn(case.region),
        "case {:?}: expected {call} to report the instance ARN in the signing Region, got {arn:?}",
        case.case
    );

    let instance_arn = reply.text("IdentityCenterInstanceArn");
    assert_eq!(
        instance_arn.as_deref(),
        case.identity_center_arn,
        "case {:?}: expected {call} to report the associated Identity Center instance, got {instance_arn:?}",
        case.case
    );
    // `IdentityCenterArn` is the deprecated spelling of the same member, so it
    // carries the same value — and is absent for the same instances.
    assert_eq!(
        reply.text("IdentityCenterArn").as_deref(),
        case.identity_center_arn,
        "case {:?}: expected {call} to report the deprecated IdentityCenterArn as the Identity \
         Center instance ARN, got body {}",
        case.case,
        reply.body
    );

    let application_arn = reply.text("IdentityCenterApplicationArn");
    match case.identity_center_arn {
        None => assert_eq!(
            application_arn, None,
            "case {:?}: expected {call} to report no Identity Center application for an \
             instance created without an association, got {application_arn:?}",
            case.case
        ),
        Some(identity_center_arn) => {
            let application_arn = application_arn.unwrap_or_default();
            // `IdentityCenterApplicationArn`'s modelled pattern:
            // `^arn:[^:]+:sso::\d{12}:application/.*$` — an `sso` ARN carries no
            // Region, and the account segment is the emulator's canonical one.
            let prefix = format!(
                "arn:aws:sso::{}:application/",
                ::emulator_server::emulator_account_id()
            );
            assert!(
                application_arn.starts_with(&prefix),
                "case {:?}: expected {call} to report an Identity Center application ARN \
                 starting {prefix:?}, got {application_arn:?}",
                case.case
            );
            // The application is a subresource of the Identity Center instance
            // it was created under, so its ARN names that instance's id.
            let instance_id = identity_center_arn
                .rsplit("instance/")
                .next()
                .unwrap_or_default();
            assert!(
                application_arn.contains(instance_id),
                "case {:?}: expected the application ARN to name the Identity Center instance \
                 {instance_id:?}, got {application_arn:?}",
                case.case
            );
        }
    }

    let created_at = reply.text("CreatedAt").unwrap_or_default();
    assert!(
        created_at.contains('T') && created_at.ends_with('Z'),
        "case {:?}: expected {call} to report an ISO-8601 CreatedAt, got {created_at:?}",
        case.case
    );
    created_at
}

#[tokio::test]
async fn create_then_get_reports_the_stored_instance_for_every_shape_of_create() {
    let emulator = Emulator::start();
    let mut application_arns = BTreeSet::new();

    for case in create_cases() {
        let created = create(&emulator, &case).await;
        let created_at = assert_reports_instance(&case, &created, "CreateAccessGrantsInstance");

        let read = get(&emulator, case.account_id, case.region).await;
        let read_created_at = assert_reports_instance(&case, &read, "GetAccessGrantsInstance");
        assert_eq!(
            read_created_at, created_at,
            "case {:?}: expected the read to report the instant of the create, got body {}",
            case.case, read.body
        );

        // A read may not change what a later reader sees: every member the
        // first read reported is the one the next read reports.
        let read_again = get(&emulator, case.account_id, case.region).await;
        assert_eq!(
            read_again.body, read.body,
            "case {:?}: expected a second read to report the same instance, got body {}",
            case.case, read_again.body
        );

        if let Some(application_arn) = read.text("IdentityCenterApplicationArn") {
            assert!(
                application_arns.insert(application_arn.clone()),
                "case {:?}: expected a distinct Identity Center application per instance, got a \
                 repeat of {application_arn:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn an_account_with_no_instance_in_the_signing_region_is_refused() {
    let emulator = Emulator::start();

    // Before any create, no Region has an instance.
    for (account_id, region) in [
        (ACCOUNT, REGION),
        (ACCOUNT, OTHER_REGION),
        (ACCOUNT, THIRD_REGION),
        (ACCOUNT, FOURTH_REGION),
    ] {
        let missing = get(&emulator, account_id, region).await;
        assert_eq!(
            missing.status, 404,
            "case {account_id:?}/{region:?}: expected a read with no instance to answer 404, \
             got {} with body {}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.code(),
            "AccessGrantsInstanceNotExistsError",
            "case {account_id:?}/{region:?}: expected the not-exists error code, got body {}",
            missing.body
        );
    }

    // An instance is scoped to the account and Region it was created for, so
    // creating one leaves every OTHER pair without one.
    let case = CreateCase::new("the one created instance", ACCOUNT, REGION);
    assert_eq!(
        create(&emulator, &case).await.status,
        200,
        "expected the create to succeed"
    );
    for (account_id, region) in [
        (ACCOUNT, OTHER_REGION),
        (ACCOUNT, THIRD_REGION),
        (ACCOUNT, FIFTH_REGION),
    ] {
        let missing = get(&emulator, account_id, region).await;
        assert_eq!(
            missing.status, 404,
            "case {account_id:?}/{region:?}: expected another pair's read to answer 404 after \
             the create, got {} with body {}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.code(),
            "AccessGrantsInstanceNotExistsError",
            "case {account_id:?}/{region:?}: expected the not-exists error code, got body {}",
            missing.body
        );
    }

    // A read addressed in another account is not a not-found: the account is
    // authorized before any instance is looked up, so the answer says nothing
    // about whether that account holds one.
    for region in [REGION, OTHER_REGION] {
        assert_foreign_account_refused(
            &format!("GetAccessGrantsInstance for another account in {region}"),
            OTHER_ACCOUNT,
            &get(&emulator, OTHER_ACCOUNT, region).await,
        );
    }
}

#[tokio::test]
async fn a_repeat_create_in_the_same_region_conflicts_and_leaves_the_instance_alone() {
    let emulator = Emulator::start();

    for case in create_cases() {
        assert_eq!(
            create(&emulator, &case).await.status,
            200,
            "case {:?}: expected the first create to succeed",
            case.case
        );
        let first = get(&emulator, case.account_id, case.region).await;

        // The repeat differs from the first create in its other members, so a
        // conflict that silently overwrote the record would be visible below.
        let repeat = CreateCase {
            identity_center_arn: Some("arn:aws:sso::210987654321:instance/ssoins-0000000000000001"),
            tags: 2,
            ..CreateCase::new(case.case, case.account_id, case.region)
        };
        let conflict = create(&emulator, &repeat).await;
        assert_eq!(
            conflict.status, 409,
            "case {:?}: expected a repeat create to answer 409, got {} with body {}",
            case.case, conflict.status, conflict.body
        );
        assert_eq!(
            conflict.code(),
            "AccessGrantsInstanceAlreadyExists",
            "case {:?}: expected the conflict error code, got body {}",
            case.case,
            conflict.body
        );
        assert_eq!(
            conflict.text("Message").as_deref(),
            Some(
                ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::
                    ACCESS_GRANTS_INSTANCE_ALREADY_EXISTS_MESSAGE
            ),
            "case {:?}: expected the service's fixed conflict message, got body {}",
            case.case,
            conflict.body
        );

        let after = get(&emulator, case.account_id, case.region).await;
        assert_eq!(
            after.body, first.body,
            "case {:?}: expected the refused create to leave the stored instance alone, got \
             body {}",
            case.case, after.body
        );
    }
}

#[tokio::test]
async fn one_account_holds_a_separate_instance_in_every_region() {
    let emulator = Emulator::start();
    let mut arns = BTreeSet::new();

    for region in [REGION, OTHER_REGION, THIRD_REGION] {
        let case = CreateCase::new("one account, one Region each", ACCOUNT, region);
        let created = create(&emulator, &case).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: expected a create in a Region the account has no instance in to \
             succeed, got {} with body {}",
            created.status, created.body
        );
        let arn = created.text("AccessGrantsInstanceArn").unwrap_or_default();
        assert_eq!(
            arn,
            expected_arn(region),
            "case {region:?}: expected the ARN to name the signing Region, got {arn:?}",
        );
        assert!(
            arns.insert(arn.clone()),
            "case {region:?}: expected one instance ARN per Region, got a repeat of {arn:?}"
        );
    }

    // Each of them is still readable from its own Region afterwards: a later
    // create in another Region neither replaced nor moved an earlier instance.
    for region in [REGION, OTHER_REGION, THIRD_REGION] {
        let read = get(&emulator, ACCOUNT, region).await;
        assert_eq!(
            read.text("AccessGrantsInstanceArn").unwrap_or_default(),
            expected_arn(region),
            "case {region:?}: expected the instance created in this Region to still be read \
             here, got body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn the_store_is_bounded_and_the_instances_it_holds_stay_readable() {
    let emulator = Emulator::start();
    // An instance's key is (account, Region). The ACCOUNT half is not
    // caller-chosen — a request for any account but the caller's own is refused
    // before anything is stored (asserted below) — so the bound is reached by
    // varying the REGION half, which the caller chooses in its signing scope.
    let region = |index: usize| format!("us-bounded-{index}");

    for index in 0..ACCESS_GRANTS_INSTANCE_QUOTA {
        let signed_for = region(index);
        let created = call_in_region(
            &emulator.router,
            &signed_for,
            "POST",
            "/v20180820/accessgrantsinstance",
            ACCOUNT,
            &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {signed_for:?}: expected a create within the store's bound to succeed, got {} \
             with body {}",
            created.status, created.body
        );
    }

    let past_the_bound = region(ACCESS_GRANTS_INSTANCE_QUOTA);
    let refused = call_in_region(
        &emulator.router,
        &past_the_bound,
        "POST",
        "/v20180820/accessgrantsinstance",
        ACCOUNT,
        &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
    )
    .await;
    assert_eq!(
        refused.status, 403,
        "case {past_the_bound:?}: expected the create past the store's bound to answer 403, got \
         {} with body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.code(),
        "AccessDenied",
        "case {past_the_bound:?}: expected the store-full refusal's error code, got body {}",
        refused.body
    );
    let missing = get(&emulator, ACCOUNT, &past_the_bound).await;
    assert_eq!(
        missing.code(),
        "AccessGrantsInstanceNotExistsError",
        "case {past_the_bound:?}: expected the refused create to have recorded nothing, got \
         body {}",
        missing.body
    );

    // The other half of the key cannot be varied at all: a create addressed in
    // another account never reaches the store, so no caller can grow it by
    // minting account ids.
    assert_foreign_account_refused(
        "CreateAccessGrantsInstance for another account",
        OTHER_ACCOUNT,
        &call_in_region(
            &emulator.router,
            REGION,
            "POST",
            "/v20180820/accessgrantsinstance",
            OTHER_ACCOUNT,
            &format!("<CreateAccessGrantsInstanceRequest xmlns=\"{XMLNS}\"/>"),
        )
        .await,
    );

    // The refusal is about the store's size, not about the instances in it:
    // every one the bound admitted is still readable.
    for index in [0, ACCESS_GRANTS_INSTANCE_QUOTA / 2, ACCESS_GRANTS_INSTANCE_QUOTA - 1] {
        let signed_for = region(index);
        let read = get(&emulator, ACCOUNT, &signed_for).await;
        assert_eq!(
            read.text("AccessGrantsInstanceArn").unwrap_or_default(),
            expected_arn(&signed_for),
            "case {signed_for:?}: expected the stored instance to still be readable, got body {}",
            read.body
        );
    }
}
