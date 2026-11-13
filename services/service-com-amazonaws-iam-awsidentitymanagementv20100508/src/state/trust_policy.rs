// SPDX-License-Identifier: CC0-1.0

//! What IAM STORES for a role's trust policy, and so what every read of that role reports.
//!
//! A trust policy — the `AssumeRolePolicyDocument` a role carries — is the one policy document
//! IAM holds that names PRINCIPALS, and the policy language writes an AWS ACCOUNT principal two
//! ways: by the account's id alone, or by the ARN of that account's root user. IAM saves the
//! second, so a principal submitted as the bare id is stored and reported as
//! `arn:<partition>:iam::<account-id>:root` — which is why a client that compares a role's trust
//! principal with the account root (a `PassRole` gate, a policy diff) sees the root ARN whichever
//! of the two spellings the document was written in.
//!
//! Nothing else about the document moves. `policyDocumentType` admits the tab, line feed and
//! carriage return a caller indents a document with, so the text a role reports is the text that
//! was submitted apart from that one spelling: the document is walked once and only an account
//! principal's value is rewritten
//! ([`::service_common::iam_policy_rendering::rewritten_values`]).
//!
//! The rule is about SAVING a policy, so it belongs to the writers of the member — `CreateRole`,
//! `UpdateAssumeRolePolicy` and `AcquireRole`, which renders one from a role template — and the
//! readers (`GetRole`, `ListRoles`, `GetAccountAuthorizationDetails`) go on reporting the stored
//! text verbatim.

/// The key a policy's `Principal`/`NotPrincipal` element names an AWS ACCOUNT principal under
/// (`::service_common::iam_policy_rendering::PRINCIPAL_ELEMENTS` lists the four keys such an
/// element takes; this is the one that names an account, a user or a role).
const ACCOUNT_PRINCIPAL: &str = "AWS";

/// The digits an AWS account id carries: every account id is exactly twelve of them, which is
/// the one form the account's own identifier takes.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The trust policy IAM stores for `document`: every ACCOUNT principal named by the account's id
/// alone written as that account's root-user ARN, and every other byte of the submitted text
/// kept — the whitespace between its tokens, its member order and its list forms included.
///
/// Applying this to what it produced returns it unchanged, so a client that puts back the
/// document it read stores the same text again.
#[must_use]
pub fn stored_trust_policy(document: &str) -> String {
    ::service_common::iam_policy_rendering::rewritten_values(
        document,
        &::service_common::iam_policy_rendering::ElementValues {
            element: ACCOUNT_PRINCIPAL,
            written_as: &account_root_arn_of,
        },
    )
}

/// The root-user ARN of the account `value` names by its id alone, or `None` for a value that is
/// not an account id — an ARN (a root ARN included, which is what makes the rewrite a fixed
/// point), the `*` wildcard, or anything else, each of which is stored as it was written.
///
/// The PARTITION is the one every ARN this emulator mints carries, read off the account root ARN
/// the crate already builds ([`super::account_root_arn`]) rather than written here, so an account
/// is named in the same partition as the roles that trust it.
fn account_root_arn_of(value: &str) -> Option<String> {
    if value.len() != ACCOUNT_ID_DIGITS || !value.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let root = super::account_root_arn();
    let partition = ::service_common::arn::parse(&root).map(|arn| arn.partition.to_string())?;
    Some(format!("arn:{partition}:iam::{value}:root"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: an account principal named by the account's id alone is stored as that account's
    /// root-user ARN, wherever the document names one, and nothing else about the text moves.
    #[test]
    fn an_account_id_principal_is_stored_as_the_account_root_arn() {
        // The rows differ along what the rule generalises over: which account is named (this
        // emulator's own and another), where the principal sits (alone, in a list, under the
        // complement element), the spacing of the submitted text, and the values the rule does
        // not touch (an ARN, a wildcard, a service principal, an id under another element).
        let rows: [(&str, &str, &str); 9] = [
            (
                "this emulator's own account, named by its id",
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"000000000000"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#,
            ),
            (
                "another account, named by its id",
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::210987654321:root"},"Action":"sts:AssumeRole"}]}"#,
            ),
            (
                "a root ARN already, which is what the rule writes",
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "a list of account ids keeps its list form",
                r#"{"Statement":[{"Principal":{"AWS":["210987654321","123456789012"]}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:root","arn:aws:iam::123456789012:root"]}}]}"#,
            ),
            (
                "a one-element list keeps its list form",
                r#"{"Statement":[{"Principal":{"AWS":["210987654321"]}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:root"]}}]}"#,
            ),
            (
                "the complement element names one too",
                r#"{"Statement":[{"NotPrincipal":{"AWS":"210987654321"}}]}"#,
                r#"{"Statement":[{"NotPrincipal":{"AWS":"arn:aws:iam::210987654321:root"}}]}"#,
            ),
            (
                "an indented document keeps its indentation",
                "{\r\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Principal\": {\"AWS\": \"210987654321\"}\n\t\t}\n\t]\n}",
                "{\r\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Principal\": {\"AWS\": \"arn:aws:iam::210987654321:root\"}\n\t\t}\n\t]\n}",
            ),
            (
                "a role ARN, a wildcard and a service principal are stored as written",
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:role/r","*"],"Service":"ec2.amazonaws.com"}}]}"#,
                r#"{"Statement":[{"Principal":{"AWS":["arn:aws:iam::210987654321:role/r","*"],"Service":"ec2.amazonaws.com"}}]}"#,
            ),
            (
                "an account id under another element is not a principal",
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
                r#"{"Statement":[{"Sid":"210987654321","Principal":{"Service":"210987654321"},"Action":"sts:AssumeRole"}]}"#,
            ),
        ];

        for (case, submitted, stored) in rows {
            let actual = stored_trust_policy(submitted);
            assert_eq!(
                actual, stored,
                "case {case:?}: expected {stored:?}, got {actual:?}"
            );
            let again = stored_trust_policy(&actual);
            assert_eq!(
                again, actual,
                "case {case:?}: the stored form must be a fixed point, got {again:?}"
            );
        }
    }

    /// RULE: only a value that is an account id — twelve decimal digits and nothing else — is
    /// written as a root ARN.
    #[test]
    fn only_a_twelve_digit_id_names_an_account() {
        // Rows: the published length, one digit either side of it, the empty value, a
        // non-decimal character inside one, and a padded one.
        let rows: [(&str, &str, bool); 7] = [
            ("twelve digits", "210987654321", true),
            ("eleven digits", "21098765432", false),
            ("thirteen digits", "2109876543210", false),
            ("no digits at all", "", false),
            ("a letter among the digits", "21098765432a", false),
            ("a space among the digits", "21098765432 ", false),
            ("twelve zeroes", "000000000000", true),
        ];

        for (case, value, names_an_account) in rows {
            let actual = account_root_arn_of(value);
            assert_eq!(
                actual.is_some(),
                names_an_account,
                "case {case:?}: expected an account ARN? {names_an_account}, got {actual:?}"
            );
            if names_an_account {
                let root = crate::state::account_root_arn();
                let partition = ::service_common::arn::parse(&root)
                    .expect("the account root ARN parses")
                    .partition
                    .to_string();
                let expected = format!("arn:{partition}:iam::{value}:root");
                assert_eq!(
                    actual.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
