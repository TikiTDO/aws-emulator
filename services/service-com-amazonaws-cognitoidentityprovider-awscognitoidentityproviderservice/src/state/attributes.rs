// SPDX-License-Identifier: CC0-1.0

//! A user's attributes: the plans an update or a deletion of them is decided by,
//! and the pool schema those plans are judged against.

use super::*;

/// Decide what `requested` does to `user` of `pool`, or the refusal it earns.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_update`]), so a request refused halfway
/// through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * An attribute the pool's schema does not declare cannot hold a value, and
///   one it declares immutable cannot change (`SchemaAttributeType.Mutable`),
///   so neither is an update to make — nor is blanking an attribute the schema
///   marks required, which has to keep a value.
/// * One value per attribute: `AttributeType` is "the name and value of a user
///   attribute", so a list naming the same attribute twice asks for two values
///   of one attribute, and a list naming more attributes than the schema
///   declares asks for more than any user of the pool can hold.
/// * "To delete an attribute from your user, submit the attribute in your API
///   request with a blank value", so a blank value REMOVES the stored
///   attribute rather than storing an empty one.
/// * A value whose attribute has a FORM of its own has to have that form
///   ([`attribute_value_defect`]), so a `phone_number` that is not a phone
///   number in E.164 form is refused rather than stored.
/// * A new `email` or `phone_number` is no longer the value its verified flag
///   vouched for ([`verified_flag`]), so the flag goes unconfirmed unless the
///   request sets it itself, and a deleted one takes its flag with it.
/// * A pool that verifies the attribute itself (`AutoVerifiedAttributes`) or
///   requires verification before the update
///   (`AttributesRequireVerificationBeforeUpdate`) sends a verification
///   message, and the second holds the new value pending until the user
///   confirms it — unless the caller's own flag asserts it is already verified
///   ([`VerifiedFlagAuthority`]).
/// * An alias value names ONE user of the pool, so a value another user holds
///   as an active alias moves to this user and the other holder's verified flag
///   is demoted with it; an alias attribute that carries no such flag cannot be
///   moved that way, so taking its value is refused.
///
/// Every step below is bounded by the POOL's schema rather than by the length
/// of the submitted list: the count is capped and repeated names are refused
/// before any per-attribute work, so one request cannot drive work the caller
/// chose the size of.
pub fn plan_attribute_update(
    pool: &UserPool,
    user: &User,
    requested: &[AttributeType],
    authority: VerifiedFlagAuthority,
) -> Result<AttributeUpdatePlan, AttributeUpdateRefusal> {
    // A request cannot update more attributes than the pool declares: past
    // that it either names one the schema does not declare or names one twice,
    // both refused below. Checked FIRST, so nothing further is proportional to
    // a list length the caller chose.
    if requested.len() > pool.schema.len() {
        return Err(AttributeUpdateRefusal::Invalid(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be updated in one request",
            pool.schema.len()
        )));
    }
    // Every attribute written has to be one the pool's schema declares. Asked
    // of the submitted list as it arrived, so an undeclared attribute is
    // reported as undeclared whatever else the list carries, and EVERY one of
    // them is reported: the schema validation states a line per attribute it
    // will not write.
    let names: Vec<String> = requested
        .iter()
        .map(|attribute| attribute.name.clone())
        .collect();
    let undeclared: Vec<&str> = names
        .iter()
        .filter(|name| !pool.declares_attribute(name))
        .map(String::as_str)
        .collect();
    if !undeclared.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            undeclared
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotInSchema)),
        )));
    }
    // A user holds ONE value per attribute, so a list naming the same
    // attribute twice names no single update to make. Keyed by name, so every
    // lookup below is a lookup rather than a scan of the request.
    let mut submitted: BTreeMap<&str, &str> = BTreeMap::new();
    for attribute in requested {
        let value = attribute.value.as_deref().unwrap_or_default();
        if submitted.insert(attribute.name.as_str(), value).is_some() {
            return Err(AttributeUpdateRefusal::Invalid(format!(
                "Invalid attributes given, {} was submitted more than once",
                attribute.name
            )));
        }
    }

    let mut updates = Vec::new();
    let mut verifications = Vec::new();
    let mut demotions: Vec<(String, &'static str)> = Vec::new();
    // `SchemaAttributeType.Mutable` is what says a stored value may change, so an
    // attribute the schema freezes cannot be updated at all — and EVERY one the
    // request names is reported, a line each, exactly as the undeclared ones above
    // are: the schema validation states a line per attribute it will not write.
    // Judged in request order, over the attributes this request actually writes.
    let frozen: Vec<&str> = requested
        .iter()
        .map(|attribute| attribute.name.as_str())
        .filter(|name| !settled_by_a_deletion(name, &submitted))
        .filter(|name| !pool.attribute_mutable(name))
        .collect();
    if !frozen.is_empty() {
        return Err(AttributeUpdateRefusal::Invalid(schema_validation_report(
            frozen
                .into_iter()
                .map(|name| (name, SchemaValidationFinding::NotUpdatable)),
        )));
    }
    for (name, value) in &submitted {
        let name = (*name).to_string();
        // A verified flag whose attribute this same request DELETES has
        // nothing left to record, so the deletion settles the flag (below)
        // rather than the value submitted for it.
        if settled_by_a_deletion(&name, &submitted) {
            continue;
        }
        let value = (*value).to_string();
        // A value whose attribute has a FORM of its own has to have that form
        // ([`attribute_value_defect`]), whatever else the request asks for.
        if let Some(defect) = attribute_value_defect(&name, &value) {
            return Err(AttributeUpdateRefusal::Invalid(defect.to_string()));
        }
        if value.is_empty() {
            // An attribute the schema marks required must hold a value, so it
            // is not one a blank value may delete.
            if pool.attribute_required(&name) {
                return Err(AttributeUpdateRefusal::Invalid(format!(
                    "Attribute {name} cannot be deleted: this user pool's schema declares it required"
                )));
            }
            // A deleted email address or phone number leaves its verified flag
            // vouching for a value the user no longer holds, so the flag goes
            // with it.
            let stale_verified_flag = verified_flag(&name);
            updates.push(AttributeUpdate {
                name,
                value: None,
                deferred: false,
                stale_verified_flag,
            });
            continue;
        }

        let unchanged = user
            .attributes
            .get(&name)
            .is_some_and(|stored| stored == &value);
        let verified_flag = verified_flag(&name);
        // A caller who may assert the value verified, and does, has nothing
        // left to verify: the value applies at once and no message goes out.
        let asserted_verified = authority == VerifiedFlagAuthority::Asserts
            && verified_flag.is_some_and(|flag| {
                submitted
                    .get(flag)
                    .is_some_and(|value| *value == FLAG_CONFIRMED)
            });
        // A pool that requires verification before the update holds the new
        // value pending instead of storing it over the confirmed one.
        let deferred = !asserted_verified
            && verified_flag.is_some()
            && pool.requires_verification_before_update(&name);
        if deferred && unchanged {
            // The user already holds this value, confirmed: there is nothing
            // to hold pending and nothing to confirm.
            continue;
        }
        // A value that changed is no longer the one the verified flag vouched
        // for — unless the request sets that flag itself, which is then the
        // value stored for it.
        let stale_verified_flag = verified_flag
            .filter(|flag| !deferred && !unchanged && !submitted.contains_key(*flag));

        // An alias value names ONE user of the pool, so this user taking
        // `value` is the other holders losing it. An `email` or a
        // `phone_number` is an alias only while its verified flag vouches for
        // it ([`verified_flag`]), so the holder's flag is what the write takes
        // away: the update is accepted and the previous holder's copy is
        // demoted out of verified status. An alias attribute that carries no
        // such flag cannot be demoted, so there the value stays the holder's
        // and this update is refused. Only an alias attribute of the pool is
        // looked up, and each is looked up once, so the pool's users are walked
        // at most once per alias attribute the pool configures.
        let becomes_active_alias = !deferred
            && verified_flag
                .is_none_or(|flag| flag_after_update(&submitted, user, flag, &stale_verified_flag));
        if becomes_active_alias && pool.is_alias_attribute(&name) {
            match verified_flag {
                Some(flag) => demotions.extend(
                    pool.alias_holder_keys(&name, &value, &user.username)
                        .into_iter()
                        .map(|key| (key, flag)),
                ),
                None if pool.alias_taken(&name, &value, &user.username) => {
                    return Err(AttributeUpdateRefusal::AliasTaken(name));
                }
                None => {}
            }
        }

        // A verification message goes to a NEW email address or phone number
        // the pool verifies itself, and to one it holds pending until the user
        // confirms it. One entry per attribute at most, since the attributes
        // are the distinct ones submitted.
        if !unchanged
            && !asserted_verified
            && (deferred || pool.auto_verifies(&name))
            && delivery_medium(&name).is_some()
        {
            verifications.push((name.clone(), value.clone()));
        }

        updates.push(AttributeUpdate {
            name,
            value: Some(value),
            deferred,
            stale_verified_flag,
        });
    }

    Ok(AttributeUpdatePlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        updates,
        verifications,
        demotions,
    })
}

/// The whole deletion: which stored user it applies to and which of that
/// user's attributes it removes ([`plan_attribute_deletion`]).
#[derive(Debug)]
pub struct AttributeDeletionPlan {
    /// The pool holding the user this plan applies to.
    pub user_pool_id: String,
    /// The STORED username of that user, however the request named it.
    pub username: String,
    /// Every attribute name to remove, each once: the names the request asked
    /// for, plus the verified flag of each, which has nothing left to vouch for
    /// once the value it describes is gone.
    pub deletions: BTreeSet<String>,
}

/// Decide what deleting `requested` does to `user` of `pool`, or the message of
/// the `InvalidParameterException` the request earns — which the asking
/// operation carries in the modelled error IT declares.
///
/// A pure read of the stored pool and user: it returns the plan rather than
/// applying it ([`User::apply_attribute_deletion`]), so a request refused
/// halfway through leaves nothing written.
///
/// The rules, each from this service's model:
///
/// * `AttributeNameListType` is a list of attribute NAMES and a user holds one
///   value per attribute, so naming an attribute twice asks for the same single
///   removal, and a list of more distinct names than the schema declares names
///   more attributes than any user of the pool can hold.
/// * An attribute the pool's schema does not declare cannot hold a value, so it
///   is not one a user can be asked to give up.
/// * An attribute the schema declares immutable (`SchemaAttributeType.Mutable`)
///   cannot change, and taking its value away changes it; one the schema marks
///   required has to keep a value, so neither is one this operation may remove.
/// * A deleted `email` or `phone_number` leaves its verified flag
///   ([`verified_flag`]) vouching for a value the user no longer holds, so the
///   flag goes with it.
///
/// Every per-attribute step below is bounded by the POOL's schema rather than
/// by the length of the submitted list: the names are reduced to the distinct
/// ones and their count is capped before any per-attribute work, so one request
/// cannot drive work the caller chose the size of.
pub fn plan_attribute_deletion(
    pool: &UserPool,
    user: &User,
    requested: &[String],
) -> Result<AttributeDeletionPlan, String> {
    let names: Vec<String> = requested
        .iter()
        .collect::<BTreeSet<&String>>()
        .into_iter()
        .cloned()
        .collect();
    // Checked FIRST, so nothing further is proportional to a list length the
    // caller chose.
    if names.len() > pool.schema.len() {
        return Err(format!(
            "Invalid attributes given, at most {} attributes of this user pool can be deleted in one request",
            pool.schema.len()
        ));
    }
    if let Some(unknown) = pool.unknown_attribute(&names) {
        return Err(undetermined_attribute_type_message(&unknown));
    }

    let mut deletions = BTreeSet::new();
    for name in names {
        if !pool.attribute_mutable(&name) {
            return Err(non_mutable_attribute_message(&name));
        }
        if pool.attribute_required(&name) {
            return Err(format!(
                "Attribute {name} cannot be deleted: this user pool's schema declares it required"
            ));
        }
        // Exactly as a blank value submitted to an attribute update takes it.
        if let Some(flag) = verified_flag(&name) {
            deletions.insert(flag.to_string());
        }
        deletions.insert(name);
    }

    Ok(AttributeDeletionPlan {
        user_pool_id: pool.id.clone(),
        username: user.username.clone(),
        deletions,
    })
}

/// The message `InvalidParameterException` carries for a `phone_number` value
/// that is not a phone number: the fixed sentence Amazon Cognito answers, which
/// names neither the attribute nor the value the request submitted.
pub const INVALID_PHONE_NUMBER_MESSAGE: &str = "Invalid phone number format.";

/// The ATTRIBUTE whose value is a phone number, and so has a form of its own to
/// satisfy ([`attribute_value_defect`]).
const PHONE_NUMBER_ATTRIBUTE: &str = "phone_number";

/// The most digits a phone number in E.164 form carries, country calling code
/// included (the form `AdminCreateUser.UserAttributes` requires: "a phone number
/// must be in E.164 format", this service's model).
const E164_MAX_DIGITS: usize = 15;

/// Why `value` is not a value the attribute `name` can hold, or `None` when the
/// value has the form that attribute's values take.
///
/// A user attribute whose values have a FORM is checked against that form before
/// it is stored, whichever operation submitted it: "a phone number must be in
/// E.164 format" (`AdminCreateUser.UserAttributes` in this service's model),
/// which is a `+` followed by the number's digits, so a `phone_number` that is
/// not one is a bad parameter rather than a value a user may hold. Every other
/// attribute's values are judged by the pool's schema alone, so none is refused
/// here.
///
/// A BLANK value is no value at all — "to delete an attribute from your user,
/// submit the attribute in your API request with a blank value" (this service's
/// model) — so it is not refused for the form it does not have.
pub fn attribute_value_defect(name: &str, value: &str) -> Option<&'static str> {
    if name != PHONE_NUMBER_ATTRIBUTE || value.is_empty() || is_e164(value) {
        return None;
    }
    Some(INVALID_PHONE_NUMBER_MESSAGE)
}

/// Whether `value` is a phone number in E.164 form: a `+`, then between one and
/// [`E164_MAX_DIGITS`] decimal digits and nothing else.
///
/// The digit count is read off the bytes before they are examined, so a long
/// value is rejected on its length rather than scanned.
fn is_e164(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('+') else {
        return false;
    };
    !digits.is_empty()
        && digits.len() <= E164_MAX_DIGITS
        && digits.bytes().all(|digit| digit.is_ascii_digit())
}

/// The attribute whose confirmation `flag` records, or `None` when `flag` is
/// not one of the verified flags ([`verified_flag`]).
pub(super) fn verified_attribute_of(flag: &str) -> Option<&'static str> {
    ["email", "phone_number"]
        .into_iter()
        .find(|attribute| verified_flag(attribute) == Some(flag))
}

/// Whether `name` is a verified flag whose attribute the same request DELETES, so
/// the deletion settles the flag and the value submitted for it writes nothing
/// ([`plan_attribute_update`]).
pub(super) fn settled_by_a_deletion(name: &str, submitted: &BTreeMap<&str, &str>) -> bool {
    verified_attribute_of(name)
        .is_some_and(|described| submitted.get(described).is_some_and(|value| value.is_empty()))
}

/// Whether `flag` says "confirmed" once this request has been applied: the
/// value the request sets for it if it names it, else `false` where the change
/// makes the flag stale, else the value the user already holds.
pub(super) fn flag_after_update(
    submitted: &BTreeMap<&str, &str>,
    user: &User,
    flag: &str,
    stale_verified_flag: &Option<&'static str>,
) -> bool {
    if let Some(value) = submitted.get(flag) {
        return *value == FLAG_CONFIRMED;
    }
    if stale_verified_flag.is_some() {
        return false;
    }
    user.attributes
        .get(flag)
        .is_some_and(|value| value == FLAG_CONFIRMED)
}

/// The `UserMFASettingList` value for SMS message MFA: that member is "the MFA
/// options that are activated for the user. The possible values in this list are
/// `SMS_MFA`, `EMAIL_OTP`, and `SOFTWARE_TOKEN_MFA`" (this service's model), and
/// SMS is the one factor an `MFAOptionType` configures.
pub const SMS_MFA_SETTING: &str = "SMS_MFA";

/// The `UserMFASettingList` value for email message MFA, the second of the three
/// values that member documents (see [`SMS_MFA_SETTING`]).
pub const EMAIL_OTP_MFA_SETTING: &str = "EMAIL_OTP";

/// The `UserMFASettingList` value for time-based one-time password MFA, the third
/// of the three values that member documents (see [`SMS_MFA_SETTING`]).
pub const SOFTWARE_TOKEN_MFA_SETTING: &str = "SOFTWARE_TOKEN_MFA";

/// `AuthFactorType`'s password factor, which "can only be used as a first
/// authentication factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const PASSWORD_AUTH_FACTOR: &str = "PASSWORD";

/// `AuthFactorType`'s email one-time-password factor.
pub const EMAIL_OTP_AUTH_FACTOR: &str = "EMAIL_OTP";

/// `AuthFactorType`'s SMS one-time-password factor.
pub const SMS_OTP_AUTH_FACTOR: &str = "SMS_OTP";

/// `AuthFactorType`'s passkey factor.
pub const WEB_AUTHN_AUTH_FACTOR: &str = "WEB_AUTHN";

/// `AuthFactorType`'s time-based one-time-password factor, which "can only be
/// used as an MFA factor" (`ConfiguredUserAuthFactors` in this service's model).
pub const SOFTWARE_TOKEN_AUTH_FACTOR: &str = "SOFTWARE_TOKEN";

/// Every `AuthFactorType` value, in the order the model DECLARES them in
/// (`smithy_query(query="shape",shape="AuthFactorType")`) rather than the
/// alphabetical order the generated `crate::ops::types::AUTH_FACTOR_TYPE_VALUES`
/// catalog carries them in. `ConfiguredUserAuthFactors` reports a user's factors
/// in that declared order, as its own example `["PASSWORD", "WEB_AUTHN"]` shows
/// (this service's model).
pub const AUTH_FACTOR_ORDER: [&str; 5] = [
    PASSWORD_AUTH_FACTOR,
    EMAIL_OTP_AUTH_FACTOR,
    SMS_OTP_AUTH_FACTOR,
    WEB_AUTHN_AUTH_FACTOR,
    SOFTWARE_TOKEN_AUTH_FACTOR,
];

/// The `WebAuthnFactorConfigurationType` value under which "passkey
/// authentication with user verification satisfies MFA requirements"
/// (`WebAuthnConfigurationType.FactorConfiguration` in this service's model); its
/// only other value makes passkeys a single authentication factor.
pub const WEB_AUTHN_MULTI_FACTOR: &str = "MULTI_FACTOR_WITH_USER_VERIFICATION";

/// The `AuthFactorType` value a `UserMFASettingList` setting activates as an MFA
/// factor, or `None` for a setting that names no `AuthFactorType` value. The one
/// place the two value sets are mapped to each other.
pub fn mfa_auth_factor_of(setting: &str) -> Option<&'static str> {
    match setting {
        SMS_MFA_SETTING => Some(SMS_OTP_AUTH_FACTOR),
        EMAIL_OTP_MFA_SETTING => Some(EMAIL_OTP_AUTH_FACTOR),
        SOFTWARE_TOKEN_MFA_SETTING => Some(SOFTWARE_TOKEN_AUTH_FACTOR),
        _ => None,
    }
}

/// The only `DeliveryMediumType` an `MFAOptionType` may set:
/// `MFAOptionType.DeliveryMedium` is "the delivery medium to send the MFA code.
/// You can use this parameter to set only the `SMS` delivery medium value" (this
/// service's model).
pub const MFA_OPTION_DELIVERY_MEDIUM: &str = "SMS";

/// The only attribute an `MFAOptionType` may name:
/// `MFAOptionType.AttributeName` is "the attribute name of the MFA option type.
/// The only valid value is `phone_number`" (this service's model).
pub const MFA_OPTION_ATTRIBUTE: &str = "phone_number";

/// The one SMS MFA option a user holds while SMS message MFA is activated for
/// it: an option sets the only delivery medium ([`MFA_OPTION_DELIVERY_MEDIUM`])
/// and names the only attribute ([`MFA_OPTION_ATTRIBUTE`]) an `MFAOptionType`
/// may carry, so a user holds either this option or none.
pub fn sms_mfa_option() -> MFAOptionType {
    MFAOptionType {
        delivery_medium: Some(MFA_OPTION_DELIVERY_MEDIUM.to_string()),
        attribute_name: Some(MFA_OPTION_ATTRIBUTE.to_string()),
    }
}

/// What one submitted MFA settings structure asks for one factor
/// ([`User::set_mfa_preference`]): the `UserMFASettingList` setting the factor is
/// reported under, whether the request activates it, and whether the request
/// makes it the user's preferred factor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfaSettingRequest<'a> {
    pub setting: &'a str,
    pub enabled: bool,
    pub preferred: bool,
}

impl MfaSettingRequest<'_> {
    /// Whether this setting asks to ACTIVATE the factor, which is what needs the
    /// delivery configuration the factor reaches the user through. A setting that
    /// does not turns the factor off, which needs nothing.
    pub fn activates(&self) -> bool {
        self.enabled
    }
}

/// Why a submitted MFA preference is refused ([`User::set_mfa_preference`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaPreferenceRejection {
    /// The `UserMFASettingList` setting the request asks to ACTIVATE while the
    /// user holds no delivery configuration for it.
    NoDeliveryConfig(String),
    /// The request asks for a factor to be the user's PREFERRED one while not
    /// activating it: a factor that is off is not one a user prefers.
    PreferredWhileDisabled,
}

impl MfaPreferenceRejection {
    /// What the refusal reports.
    ///
    /// An activation the user cannot receive reports the factor it asked for and
    /// that the user holds no configuration to receive it with. A preference over a
    /// factor the request leaves off reports that alone and names NO factor: "only
    /// one factor can be set as preferred" (`SetUserMFAPreference` in this
    /// service's model) and a factor that is off is not one a user prefers, so the
    /// refusal is about the preference rather than about any one factor. Nothing
    /// stored is disclosed either way.
    pub fn message(&self) -> String {
        match self {
            Self::NoDeliveryConfig(setting) => {
                format!("User does not have delivery config set to turn on {setting}")
            }
            Self::PreferredWhileDisabled => "Can't mark a disabled mfa as preferred".to_string(),
        }
    }
}

/// Why a submitted `MFAOptionListType` configures no SMS MFA option
/// ([`sms_mfa_options`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MfaOptionRejection {
    /// The submitted delivery medium, which is not the one SMS MFA travels by,
    /// or `None` where the option set no medium at all.
    DeliveryMedium(Option<String>),
    /// The submitted attribute name, which is not the one an MFA option may
    /// name, or `None` where the option named no attribute at all.
    AttributeName(Option<String>),
}

impl MfaOptionRejection {
    /// What the refusal reports: the member of the submitted option that an SMS
    /// MFA configuration cannot carry, and the value submitted for it. Only
    /// submitted values are named, so nothing stored is disclosed.
    pub fn message(&self) -> String {
        match self {
            Self::DeliveryMedium(Some(medium)) => format!(
                "Invalid DeliveryMedium: {medium}, the only delivery medium an MFA option can \
                 set is {MFA_OPTION_DELIVERY_MEDIUM}"
            ),
            Self::DeliveryMedium(None) => format!(
                "Invalid MFAOptions: an MFA option must set {MFA_OPTION_DELIVERY_MEDIUM} as its \
                 DeliveryMedium"
            ),
            Self::AttributeName(Some(name)) => format!(
                "Invalid AttributeName: {name}, the only attribute name an MFA option can name \
                 is {MFA_OPTION_ATTRIBUTE}"
            ),
            Self::AttributeName(None) => format!(
                "Invalid MFAOptions: an MFA option must name {MFA_OPTION_ATTRIBUTE} as its \
                 AttributeName"
            ),
        }
    }
}

/// The SMS MFA configuration a submitted `MFAOptionListType` asks one user to
/// hold, or the refusal it earns.
///
/// `MFAOptions` is a parameter "you can use only to set an SMS configuration
/// that uses SMS for delivery" (`SetUserSettings` and `AdminSetUserSettings` in
/// this service's model), so every submitted option has to set SMS delivery of
/// the one attribute an option may name. A user holds ONE value per attribute,
/// so the same option submitted twice is still one configuration and the
/// DISTINCT options are what is stored.
///
/// A PURE function of the request: it decides the configuration without reading
/// or writing stored state, so a request refused here writes nothing. Each
/// submitted option costs one constant-time check and the configuration it
/// yields holds at most one option per attribute an option may name, so the
/// length of the submitted list bounds nothing that is stored.
pub fn sms_mfa_options(
    requested: &[MFAOptionType],
) -> Result<Vec<MFAOptionType>, MfaOptionRejection> {
    let mut stored: Vec<MFAOptionType> = Vec::new();
    for option in requested {
        match option.delivery_medium.as_deref() {
            Some(MFA_OPTION_DELIVERY_MEDIUM) => {}
            other => {
                return Err(MfaOptionRejection::DeliveryMedium(
                    other.map(str::to_string),
                ))
            }
        }
        match option.attribute_name.as_deref() {
            Some(MFA_OPTION_ATTRIBUTE) => {}
            other => return Err(MfaOptionRejection::AttributeName(other.map(str::to_string))),
        }
        if !stored
            .iter()
            .any(|kept| kept.attribute_name == option.attribute_name)
        {
            stored.push(option.clone());
        }
    }
    Ok(stored)
}

/// Where a password-reset message for one user goes
/// ([`UserPool::recovery_destination`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryDestination {
    /// The attribute the message is addressed to, and the stored value it goes
    /// to.
    Delivered {
        attribute: String,
        destination: String,
    },
    /// This pool reserves password resets to administrators (`admin_only`), so
    /// no self-service message is sent at all.
    AdminOnly,
    /// The user holds no verified address or number for any mechanism this pool
    /// allows, so there is nowhere to send a reset code.
    None,
}

/// The `RecoveryOptionNameType` value that "prevents self-service account
/// recovery" (`RecoveryOptionType` in this service's model).
pub const RECOVERY_ADMIN_ONLY: &str = "admin_only";

/// The message a password reset is refused with where the pool reserves account
/// recovery to administrators ([`RecoveryDestination::AdminOnly`]): no
/// password-reset message is sent at all, so no reset can be begun through the
/// API that sends one.
pub const ADMIN_ONLY_RECOVERY_MESSAGE: &str = "Contact administrator to reset password.";

/// The message a password reset is refused with where the user holds no verified
/// address a reset message could go to ([`RecoveryDestination::None`]): "if
/// neither a verified phone number nor a verified email exists, Amazon Cognito
/// responds with an InvalidParameterException error" (`ForgotPassword` in this
/// service's model).
pub const NO_RECOVERY_DESTINATION_MESSAGE: &str =
    "Cannot reset password for the user as there is no registered/verified email or phone_number";

/// The recovery mechanisms a pool that configures none gets, in priority order:
/// Amazon Cognito's default account recovery is the verified email address,
/// falling back to the verified phone number (the two attribute-bearing
/// `RecoveryOptionNameType` values).
pub(super) const DEFAULT_RECOVERY_MECHANISMS: [&str; 2] = ["verified_email", "verified_phone_number"];

/// The `AccountRecoverySetting` a pool is STORED with, which is the one a
/// request carried or Amazon Cognito's own default.
///
/// A pool always has an effective account-recovery configuration — it is what
/// decides where a self-service reset message goes
/// ([`UserPool::recovery_destination`]) — so a request that configures none is
/// stored with the service default rather than with nothing, and
/// `DescribeUserPool` reports the configuration the pool actually applies.
/// The default is [`DEFAULT_RECOVERY_MECHANISMS`] in that order, each
/// mechanism's `Priority` being its rank from 1, since "groups ... take
/// precedence" by ascending priority ("if `verified_email` has a priority of 1
/// and `verified_phone_number` has a priority of 2, your user pool sends
/// account-recovery messages to a verified email address but falls back to an
/// SMS message", `RecoveryOptionType` in this service's model).
pub fn effective_account_recovery_setting(
    requested: Option<AccountRecoverySettingType>,
) -> AccountRecoverySettingType {
    requested.unwrap_or_else(|| AccountRecoverySettingType {
        recovery_mechanisms: Some(
            DEFAULT_RECOVERY_MECHANISMS
                .iter()
                .enumerate()
                .map(|(index, name)| RecoveryOptionType {
                    priority: i32::try_from(index).unwrap_or_default() + 1,
                    name: (*name).to_string(),
                })
                .collect(),
        ),
    })
}

/// The user attribute a `RecoveryOptionNameType` recovers through, or `None` for
/// a mechanism that names no attribute (`admin_only`).
pub(super) fn recovered_attribute(mechanism: &str) -> Option<&'static str> {
    match mechanism {
        "verified_email" => Some("email"),
        "verified_phone_number" => Some("phone_number"),
        _ => None,
    }
}

/// The recovered attribute whose reset message is delivered over SMS \u2014 the one
/// `RecoveryOptionNameType` mechanism that does not qualify for a user with SMS
/// MFA activated ([`UserPool::recovery_destination`]).
pub const SMS_RECOVERED_ATTRIBUTE: &str = "phone_number";

/// How many trailing characters of a masked phone number stay readable, which
/// is what identifies the number to the user who gave it.
pub(super) const UNMASKED_DESTINATION_SUFFIX: usize = 4;

/// `destination` as a `CodeDeliveryDetailsType.Destination`.
///
/// Amazon Cognito reports the address it sent the code to MASKED, so the reply
/// tells the caller which of the user's addresses was used without disclosing
/// an address the caller does not already hold. An email address keeps only the
/// FIRST character of its local part and the first character of its domain,
/// each followed by a fixed `***` run — nothing of the domain past that first
/// character is disclosed, not even its last label; anything else keeps a
/// leading `+` and its last [`UNMASKED_DESTINATION_SUFFIX`] characters.
pub fn masked_destination(destination: &str) -> String {
    if let Some((local, domain)) = destination.split_once('@') {
        let local_initial = local.chars().next().unwrap_or('*');
        let domain_initial = domain.chars().next().unwrap_or('*');
        return format!("{local_initial}***@{domain_initial}***");
    }
    let characters: Vec<char> = destination.chars().collect();
    let readable_from = characters
        .len()
        .saturating_sub(UNMASKED_DESTINATION_SUFFIX);
    characters
        .iter()
        .enumerate()
        .map(|(index, character)| {
            if index >= readable_from || (index == 0 && *character == '+') {
                *character
            } else {
                '*'
            }
        })
        .collect()
}

/// The delivery details a message to a SIMULATED destination is reported with.
///
/// `PreventUserExistenceErrors` set to `ENABLED` "suppresses messages that might
/// indicate a valid user exists" (this service's model): the account-confirmation
/// and password-recovery operations therefore answer a request naming a user
/// nobody registered exactly as they answer one naming a user that exists —
/// reporting a delivery — rather than with a refusal that tells the caller the
/// account does not exist. Nothing was sent and nothing is stored, so the
/// destination reported is a SIMULATED one: a fixed masked address, which names
/// nothing of the request and nothing of this emulator's state, so no two
/// usernames can be told apart by the answer they get.
pub fn simulated_code_delivery_details() -> Option<CodeDeliveryDetailsType> {
    code_delivery_details(SIMULATED_DESTINATION_ATTRIBUTE, SIMULATED_DESTINATION)
}

/// The attribute a simulated delivery is reported against: an email address is
/// the destination every pool can address a message to, and the medium it
/// travels by is the one [`delivery_medium`] maps it to.
pub(super) const SIMULATED_DESTINATION_ATTRIBUTE: &str = "email";

/// The address [`simulated_code_delivery_details`] reports, which identifies
/// nobody: it is reported through the same masking every real destination is
/// reported through ([`masked_destination`]), so the answer for a user nobody
/// registered has the shape of the answer for one that exists.
pub(super) const SIMULATED_DESTINATION: &str = "a@b";

/// The delivery details for a verification message sent to `value` for
/// `attribute`, or `None` for an attribute no message is sent for.
///
/// Rendered from the value the message goes to, masked as
/// [`masked_destination`] reports it.
pub fn code_delivery_details(attribute: &str, value: &str) -> Option<CodeDeliveryDetailsType> {
    Some(CodeDeliveryDetailsType {
        destination: Some(masked_destination(value)),
        delivery_medium: Some(delivery_medium(attribute)?.to_string()),
        attribute_name: Some(attribute.to_string()),
    })
}

/// The schema a new pool is created with: the standard attributes, with a
/// requested attribute of the same name overriding its properties, and every
/// other requested attribute added under the `custom:` prefix Amazon Cognito
/// gives custom attributes.
///
/// The stored ORDER is the pool's own, as created: "an array of attributes for
/// the new user pool. You can add custom attributes and modify the properties of
/// default attributes" (`CreateUserPool.Schema` in this service's model), so the
/// attributes the request names are stored in the order IT names them — a custom
/// one among the standard ones where the request put it — followed by the
/// standard attributes it does not name, in Amazon Cognito's standard claim
/// order. [`SUB_ATTRIBUTE`] leads the schema whatever the request says: the pool
/// mints it for every user rather than taking it from a caller. Every read of
/// the schema — `DescribeUserPool`, `GetCSVHeader` — reports that one order, and
/// a later `AddCustomAttributes` appends behind it.
///
/// The work is bounded and linear in the request: each requested attribute is
/// resolved through a name-keyed index rather than by scanning what the schema
/// already holds, the ordering reads each attribute's rank out of a map, and no
/// more attributes are taken than a pool may hold
/// ([`MAX_CUSTOM_ATTRIBUTES_PER_POOL`] beside the standard ones) — a bound
/// `CreateUserPool` refuses past before it calls this, so nothing a request can
/// reach is dropped here.
pub fn effective_schema(requested: Option<Vec<SchemaAttributeType>>) -> Vec<SchemaAttributeType> {
    let requested = requested.unwrap_or_default();
    let mut schema: Vec<SchemaAttributeType> =
        STANDARD_SCHEMA.iter().map(standard_schema_attribute).collect();

    // Where each stored attribute sits, so a requested one is resolved by name
    // instead of by scanning the schema.
    let mut position: BTreeMap<String, usize> = schema
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| entry.name.clone().map(|name| (name, at)))
        .collect();
    // The rank of each attribute the request names, in the order it names them.
    let mut requested_rank: BTreeMap<String, usize> = BTreeMap::new();

    let bound = STANDARD_SCHEMA.len().saturating_add(MAX_CUSTOM_ATTRIBUTES_PER_POOL);
    for attribute in requested.iter().take(bound) {
        let Some(name) = attribute.name.clone() else {
            continue;
        };
        if let Some(standard) = position.get(&name).and_then(|at| schema.get_mut(*at)) {
            standard.required = attribute.required.or(standard.required);
            standard.mutable = attribute.mutable.or(standard.mutable);
            if attribute.string_attribute_constraints.is_some() {
                standard.string_attribute_constraints =
                    attribute.string_attribute_constraints.clone();
            }
            if attribute.number_attribute_constraints.is_some() {
                standard.number_attribute_constraints =
                    attribute.number_attribute_constraints.clone();
            }
            let next = requested_rank.len();
            requested_rank.entry(name).or_insert(next);
            continue;
        }
        if let Some((stored_name, entry)) = custom_schema_attribute(attribute) {
            if position.contains_key(&stored_name) {
                continue;
            }
            position.insert(stored_name.clone(), schema.len());
            schema.push(entry);
            let next = requested_rank.len();
            requested_rank.insert(stored_name, next);
        }
    }

    // One attribute is stored under exactly one name, so ranking each entry and
    // sorting by that rank keeps the minted attribute first, then the requested
    // order, then the standard order a stable sort leaves the rest in.
    schema.sort_by_key(|entry| {
        let name = entry.name.as_deref().unwrap_or_default();
        if name == SUB_ATTRIBUTE {
            return (0, 0);
        }
        match requested_rank.get(name) {
            Some(rank) => (1, *rank),
            None => (2, 0),
        }
    });
    schema
}

/// The message a `CreateUserPool` whose `Schema` asks for more custom
/// attributes than a pool may hold is refused with: the per-pool quota is
/// [`MAX_CUSTOM_ATTRIBUTES_PER_POOL`], the same one `AddCustomAttributes`
/// enforces for a schema that grows after the pool exists, and the request
/// names no pool yet.
pub fn custom_attribute_quota_message() -> String {
    format!("A user pool holds at most {MAX_CUSTOM_ATTRIBUTES_PER_POOL} custom attributes.")
}

/// How many of `requested`'s attributes a pool would hold as CUSTOM ones: every
/// requested attribute that is not one of the standard attributes a pool already
/// carries, which are not counted against the quota.
pub fn requested_custom_attribute_count(requested: &[SchemaAttributeType]) -> usize {
    requested
        .iter()
        .filter(|attribute| {
            attribute
                .name
                .as_deref()
                .is_some_and(|name| !is_standard_attribute(name))
        })
        .count()
}

/// The `AttributeDataType` a requested custom attribute that names none is
/// stored with: `SchemaAttributeType.AttributeDataType` is "the data type of the
/// attribute" and `String` is the one every unqualified custom attribute reads
/// back as (this service's model). One of
/// [`crate::ops::types::ATTRIBUTE_DATA_TYPE_VALUES`].
pub const DEFAULT_ATTRIBUTE_DATA_TYPE: &str = "String";

/// The STORED NAME and the schema entry a requested CUSTOM attribute becomes,
/// with the defaults a custom attribute carries filled in.
///
/// Its stored name is the requested name under the prefix Amazon Cognito gives
/// it: "When you add an attribute with a `Name` value of `MyAttribute`, Amazon
/// Cognito creates the custom attribute `custom:MyAttribute`. When
/// `DeveloperOnlyAttribute` is `true`, Amazon Cognito creates your attribute as
/// `dev:MyAttribute`" (`SchemaAttributeType.Name`, this service's model). A name
/// that already carries one of those prefixes is stored as it is, so a schema
/// read and a re-add agree on the stored name.
///
/// `None` when the request names no attribute: an entry with no name could never
/// be found again. `CustomAttributeNameType` has a minimum length of 1, so only
/// a request the model does not admit reaches that answer.
pub fn custom_schema_attribute(
    requested: &SchemaAttributeType,
) -> Option<(String, SchemaAttributeType)> {
    let name = requested.name.clone()?;
    if name.is_empty() {
        return None;
    }
    let developer_only = requested.developer_only_attribute.unwrap_or(false);
    let prefix = if developer_only { "dev:" } else { "custom:" };
    let stored_name = if name.starts_with("custom:") || name.starts_with("dev:") {
        name
    } else {
        format!("{prefix}{name}")
    };
    let data_type = requested
        .attribute_data_type
        .clone()
        .unwrap_or_else(|| DEFAULT_ATTRIBUTE_DATA_TYPE.to_string());
    // The attribute reports the constraints its own data type has, holding the
    // bounds the request asked for and EMPTY where it asked for none — the same
    // rule the default schema's attributes report theirs by
    // ([`schema_attribute_constraints`]).
    let (min, max) = match data_type.as_str() {
        NUMBER_ATTRIBUTE_DATA_TYPE => (
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_value.as_deref()),
            requested
                .number_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_value.as_deref()),
        ),
        _ => (
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.min_length.as_deref()),
            requested
                .string_attribute_constraints
                .as_ref()
                .and_then(|constraints| constraints.max_length.as_deref()),
        ),
    };
    let (number_attribute_constraints, string_attribute_constraints) =
        schema_attribute_constraints(&data_type, min, max);
    let entry = SchemaAttributeType {
        name: Some(stored_name.clone()),
        attribute_data_type: Some(data_type.clone()),
        developer_only_attribute: Some(developer_only),
        // A custom attribute is mutable unless the request says otherwise;
        // a custom attribute can never be required.
        mutable: Some(requested.mutable.unwrap_or(true)),
        required: Some(false),
        number_attribute_constraints,
        string_attribute_constraints,
    };
    Some((stored_name, entry))
}
