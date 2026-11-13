// SPDX-License-Identifier: CC0-1.0

//! @op EnableMFADevice
//! Enables one of the account's virtual MFA devices for an IAM user, once the two pass codes
//! the request presents prove the sender holds that device.
//!
//! "Enables the specified MFA device and associates it with the specified IAM user" (this
//! operation's documentation): the device is the one the request's `SerialNumber` names — for a
//! virtual device that serial IS its ARN (`VirtualMFADevice$SerialNumber`) — and the
//! association is what makes the device the one `sts:GetSessionToken` and `sts:AssumeRole`
//! validate that user's MFA challenge against.
//!
//! The two authentication codes are AUTHENTICATED rather than taken on trust: they have to be
//! the pass codes a device provisioned with this device's own seed emits in two consecutive
//! time steps ([`crate::state::virtual_mfa_device_answers_in_sequence`]), so a request that did
//! not come from the holder of the seed enables nothing and is refused
//! `InvalidAuthenticationCode` (HTTP 403).
//!
//! ---
//!
//! # Modeled contract for `EnableMFADevice`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json")
//! ```
//!
//! Enables the specified MFA device and associates it with the specified IAM user. When
//! enabled, the MFA device is required for every subsequent login by the IAM user
//! associated with the device.
//!
//! ## Input
//!
//! * `AuthenticationCode1` (authenticationCodeType, len 6..6, pattern ^[\d]+$) **required** —
//!   An authentication code emitted by the device. The format for this parameter is a string
//!   of six digits. Submit your request immediately after generating the authentication
//!   codes. If you generate the codes and then wait too long to submit the request, the MFA
//!   device successfully associates with the user but the MFA device becomes out of sync.
//!   This happens because time-based one-time passwords (TOTP) expire after a short period of
//!   time. If this happens, you can resync the device.
//! * `AuthenticationCode2` (authenticationCodeType, len 6..6, pattern ^[\d]+$) **required** —
//!   A subsequent authentication code emitted by the device. The format for this parameter is
//!   a string of six digits. Submit your request immediately after generating the
//!   authentication codes. If you generate the codes and then wait too long to submit the
//!   request, the MFA device successfully associates with the user but the MFA device becomes
//!   out of sync. This happens because time-based one-time passwords (TOTP) expire after a
//!   short period of time. If this happens, you can resync the device.
//! * `SerialNumber` (serialNumberType, len 9..256, pattern ^[\w+=/:,.@-]+$) **required** —
//!   The serial number that uniquely identifies the MFA device. For virtual MFA devices, the
//!   serial number is the device ARN. This parameter allows (through its regex pattern) a
//!   string of characters consisting of upper and lowercase alphanumeric characters with no
//!   spaces. You can also include any of the following characters: =,.@:/-
//! * `UserName` (existingUserNameType, len 1..128, pattern ^[\w+=,.@-]+$) **required** — The
//!   name of the IAM user for whom you want to enable the MFA device. This parameter allows
//!   (through its regex pattern) a string of characters consisting of upper and lowercase
//!   alphanumeric characters with no spaces. You can also include any of the following
//!   characters: _+=,.@-
//!
//! ## Errors
//!
//! * `ConcurrentModificationException` (HTTP 409) — The request was rejected because multiple
//!   requests to change this object were submitted simultaneously. Wait a few minutes and
//!   submit your request again.
//! * `EntityAlreadyExistsException` (HTTP 409) — The request was rejected because it
//!   attempted to create a resource that already exists.
//! * `EntityTemporarilyUnmodifiableException` (HTTP 409) — The request was rejected because
//!   it referenced an entity that is temporarily unmodifiable, such as a user name that was
//!   deleted and then recreated. The error indicates that the request is likely to succeed if
//!   you try again after waiting several minutes. The error message describes the entity.
//! * `InvalidAuthenticationCodeException` (HTTP 403) — The request was rejected because the
//!   authentication code was not recognized. The error message describes the specific error.
//! * `LimitExceededException` (HTTP 409) — The request was rejected because it attempted to
//!   create resources beyond the current Amazon Web Services account limits. The error
//!   message describes the limit exceeded.
//! * `NoSuchEntityException` (HTTP 404) — The request was rejected because it referenced a
//!   resource entity that does not exist. The error message describes the resource.
//! * `ServiceFailureException` (HTTP 500) — The request processing has failed because of an
//!   unknown error, exception or failure.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `EnableMFADeviceError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json");

#[derive(Debug, Clone, Default)]
pub struct EnableMFADeviceOutput;

impl EnableMFADeviceOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static ENABLE_MFA_DEVICE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &EnableMFADeviceOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for EnableMFADeviceOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `EnableMFADeviceError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iam#EnableMFADevice")]
#[derive(Debug)]
pub enum EnableMFADeviceErrorModeled {
    ConcurrentModificationException(ConcurrentModificationException),
    EntityAlreadyExistsException(EntityAlreadyExistsException),
    EntityTemporarilyUnmodifiableException(EntityTemporarilyUnmodifiableException),
    InvalidAuthenticationCodeException(InvalidAuthenticationCodeException),
    LimitExceededException(LimitExceededException),
    NoSuchEntityException(NoSuchEntityException),
    ServiceFailureException(ServiceFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::enable_mfa_device::EnableMFADeviceError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type EnableMFADeviceError = crate::scaffolded::Scaffolded<EnableMFADeviceErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<EnableMFADeviceErrorModeled> for EnableMFADeviceError {
    fn from(modeled: EnableMFADeviceErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `EnableMFADevice` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct EnableMFADevice<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> EnableMFADevice<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for EnableMFADevice<Eng>
{
    type Input = EnableMFADeviceInput;
    type Output = EnableMFADeviceOutput;
    type Error = EnableMFADeviceError;

    const NAME: &'static str = "EnableMFADevice";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::ENABLE_MFA_DEVICE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = ENABLE_MFA_DEVICE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(EnableMFADeviceErrorModeled::ServiceFailureException(
                ServiceFailureException {
                    message: Some("the IAM store is unavailable".to_string()),
                },
            )
            .into());
        };
        let mut store = state.lock();

        // The LOOKUP comes first but the answer to it is withheld until the call has been
        // authorized, because IAM decides a call against the caller's policies before it
        // reads the resource.
        let user_key = crate::state::name_key(&input.user_name);
        let held = store.users.get(&user_key);

        // IAM authorizes its own API: the signing principal's identity-based policies are
        // evaluated before anything is enabled. The decision is made on the ARN of the USER
        // the device is enabled for — the held user's own, or, for a name the account holds no
        // user under, the ARN that name would carry at the default path — and a refusal names
        // that user as the request named it, leaving the device unassigned.
        let resource = held.map_or_else(
            || crate::state::user_arn(crate::state::DEFAULT_PATH, &input.user_name),
            |user| user.arn.clone(),
        );
        crate::state::authorization::authorize_named(
            &store,
            ctx,
            crate::state::authorization::ENABLE_MFA_DEVICE,
            &resource,
            &format!("user {}", input.user_name),
        )?;

        if held.is_none() {
            return Err(EnableMFADeviceErrorModeled::NoSuchEntityException(
                NoSuchEntityException {
                    message: Some(format!(
                        "The user with name {} cannot be found.",
                        input.user_name
                    )),
                },
            )
            .into());
        }

        // The device the serial number names. "For virtual MFA devices, the serial number is
        // the device ARN" (`$SerialNumber`), so the account's virtual devices are where a
        // serial is resolved; a serial naming none of them references an entity that does not
        // exist, which is what the refusal reports — naming the serial the request supplied
        // and the user it named.
        let Some(device) = store.virtual_mfa_device(&input.serial_number).cloned() else {
            return Err(EnableMFADeviceErrorModeled::NoSuchEntityException(
                NoSuchEntityException {
                    message: Some(format!(
                        "MFA Device {} cannot be found for user {}.",
                        input.serial_number, input.user_name
                    )),
                },
            )
            .into());
        };

        // A device is ONE user's: "enables the specified MFA device and associates it with
        // the specified IAM user", so a device another user already holds cannot be enabled
        // for a second one, and re-enabling the device its holder already has is the same
        // conflict — a resource that already exists.
        if let Some(holder) = &device.user_key {
            return Err(
                EnableMFADeviceErrorModeled::EntityAlreadyExistsException(
                    EntityAlreadyExistsException {
                        message: Some(format!(
                            "Device {} is already enabled for user {}.",
                            input.serial_number,
                            store
                                .users
                                .get(holder)
                                .map_or_else(|| holder.clone(), |user| user.user_name.clone())
                        )),
                    },
                )
                .into(),
            );
        }

        // The codes are AUTHENTICATED against the device's own seed (see the module docs):
        // two codes in a row, so what the request proves is that its sender holds a device
        // whose clock runs with this one. Nothing is enabled when they do not.
        if !crate::state::virtual_mfa_device_answers_in_sequence(
            &device.base32_string_seed,
            &input.authentication_code1,
            &input.authentication_code2,
            ::std::time::SystemTime::now(),
        ) {
            return Err(
                EnableMFADeviceErrorModeled::InvalidAuthenticationCodeException(
                    InvalidAuthenticationCodeException {
                        message: Some(INVALID_AUTHENTICATION_CODE.to_string()),
                    },
                )
                .into(),
            );
        }

        // What the account keeps: the device assigned to that user, from this instant — "the
        // date and time on which the virtual MFA device was enabled"
        // (`VirtualMFADevice$EnableDate`), which every later read of the device reports.
        let enabled_at = crate::state::now();
        if let Some(record) = store.virtual_mfa_devices.get_mut(&input.serial_number) {
            record.user_key = Some(user_key);
            record.enable_date = Some(enabled_at);
        }
        Ok(EnableMFADeviceOutput)
    }
}

/// The sentence IAM refuses a pair of codes the device's seed does not answer with.
///
/// Fixed text: it quotes neither code back, because what it reports is that the codes were not
/// recognized rather than what either of them was.
const INVALID_AUTHENTICATION_CODE: &str =
    "The authentication code was not recognized or has expired. Please try again with a new \
     code.";
