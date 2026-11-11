// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE POLICY slice of this service's state: what makes a document a resource
//! policy SageMaker will attach to a model package group, and the sentence each
//! condition is refused with.
//!
//! `PolicyString` is modelled `@length 1..20480, pattern .*`
//! (`smithy_query(query="operation",operation="PutModelPackageGroupPolicy")`), so the
//! boundary validator passes any text of that size: whether the text is a POLICY is the
//! service's own question, answered here once rather than in the handler, and answered
//! BEFORE anything is stored.
//!
//! Three properties are read, in the order a reader of the document would reach them:
//!
//! 1. **The document is ASCII.** A policy document is attached to an IAM-evaluated
//!    resource and the policy language is ASCII text, so a document carrying anything else
//!    is refused rather than stored for a caller to discover later.
//! 2. **The document is a JSON OBJECT.** IAM's policy grammar makes a document an object
//!    whose `Version`/`Id`/`Statement` elements are its members; text that does not parse,
//!    or parses as anything but an object, is not a policy document.
//! 3. **Every element is one the policy language DEFINES.** A document naming an element
//!    the grammar does not have states something IAM cannot evaluate, so it is refused and
//!    the element is named — at the document's top level and inside every statement.
//!
//! Nothing beyond that is checked: which actions, principals or conditions a statement
//! names is IAM's business, and a document this accepts is stored and handed back
//! BYTE-FOR-BYTE rather than reformatted, so a caller reads back exactly the text it sent.
//!
//! Every condition is a property of the REQUEST — a document the caller has to change, not
//! one to retry — so each is refused as a 400 `ValidationException` and none is the
//! group-state `ConflictException` the operation models. Each sentence opens with the
//! `Invalid Policy: ` subject the service states a policy failure under.

/// The elements an IAM policy DOCUMENT may carry, from the policy grammar in the IAM user
/// guide: the policy-wide `Version` and `Id`, and the `Statement` block itself.
pub const POLICY_ELEMENTS: [&str; 3] = ["Version", "Id", "Statement"];

/// The elements one policy STATEMENT may carry, from the same grammar.
pub const STATEMENT_ELEMENTS: [&str; 9] = [
    "Sid",
    "Effect",
    "Principal",
    "NotPrincipal",
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "Condition",
];

/// The subject every policy refusal of this service opens with.
const INVALID_POLICY: &str = "Invalid Policy: ";

/// Why SageMaker refuses `policy` as the resource policy of a model package group, or
/// `None` when it accepts it.
///
/// The returned text is the refusal's message; the caller renders it as the 400
/// `ValidationException` a request it cannot take is answered with.
#[must_use]
pub fn resource_policy_violation(policy: &str) -> Option<String> {
    if !policy.is_ascii() {
        return Some(format!(
            "{INVALID_POLICY}Policy string can only contain ASCII characters"
        ));
    }
    // A document that does not parse, or parses as anything but an object, is not policy
    // JSON at all — one sentence for the whole class, because what the caller is told is
    // that the document is not a policy document.
    let Some(document) = ::serde_json::from_str::<::serde_json::Value>(policy)
        .ok()
        .and_then(|parsed| match parsed {
            ::serde_json::Value::Object(document) => Some(document),
            _ => None,
        })
    else {
        return Some(format!(
            "{INVALID_POLICY}This policy contains invalid Json"
        ));
    };

    // The document's own elements, then each statement's: an element the policy language
    // does not define is named, so the caller is told WHICH element it has to drop.
    if let Some(unknown) = first_unknown_element(&document, &POLICY_ELEMENTS) {
        return Some(unknown_field(unknown));
    }
    // "Statement" is where a policy states its permissions, so an object carrying none
    // states no policy: it parses as JSON but not as a policy document, which is the class
    // above.
    let Some(statement) = document.get("Statement") else {
        return Some(format!(
            "{INVALID_POLICY}This policy contains invalid Json"
        ));
    };
    // One statement, or a list of them: the grammar admits both forms. An EMPTY list is a
    // policy that states nothing, which the service takes.
    let statements: Vec<&::serde_json::Value> = match statement {
        ::serde_json::Value::Array(entries) => entries.iter().collect(),
        ::serde_json::Value::Object(_) => vec![statement],
        _ => {
            return Some(format!(
                "{INVALID_POLICY}This policy contains invalid Json"
            ))
        }
    };
    for entry in statements {
        let Some(fields) = entry.as_object() else {
            return Some(format!(
                "{INVALID_POLICY}This policy contains invalid Json"
            ));
        };
        if let Some(unknown) = first_unknown_element(fields, &STATEMENT_ELEMENTS) {
            return Some(unknown_field(unknown));
        }
    }
    None
}

/// The first element of `object` that `defined` does not hold, in the order the document
/// carries them, or `None` when every element is one the grammar defines.
fn first_unknown_element<'fields>(
    object: &'fields ::serde_json::Map<String, ::serde_json::Value>,
    defined: &[&str],
) -> Option<&'fields str> {
    object
        .keys()
        .map(String::as_str)
        .find(|element| !defined.contains(element))
}

/// How this service states an element the policy language does not define: the element AS
/// THE DOCUMENT SPELLED IT, so the caller can find it.
fn unknown_field(element: &str) -> String {
    format!("{INVALID_POLICY}Unknown field {element}")
}

#[cfg(test)]
mod tests {
    use super::resource_policy_violation;

    /// A policy statement the rows below vary one element of at a time.
    fn statement() -> String {
        r#"{"Sid":"Share","Effect":"Allow","Principal":{"AWS":"*"},"Action":"sagemaker:DescribeModelPackage","Resource":"*"}"#
            .to_string()
    }

    /// RULE: a document is taken exactly when it is ASCII, a JSON object, and carries only
    /// elements the policy language defines.
    #[test]
    fn a_document_is_taken_only_when_it_is_an_ascii_policy() {
        let one = statement();
        // (case, document, taken)
        let rows: Vec<(&str, String, bool)> = vec![
            (
                "a one-statement policy",
                format!(r#"{{"Version":"2012-10-17","Statement":[{one}]}}"#),
                true,
            ),
            (
                "a policy carrying an Id",
                format!(r#"{{"Version":"2012-10-17","Id":"share","Statement":[{one}]}}"#),
                true,
            ),
            (
                "no version element at all",
                format!(r#"{{"Statement":[{one}]}}"#),
                true,
            ),
            (
                "a bare statement rather than a list",
                format!(r#"{{"Version":"2012-10-17","Statement":{one}}}"#),
                true,
            ),
            (
                "an empty statement list",
                r#"{"Version":"2012-10-17","Statement":[]}"#.to_string(),
                true,
            ),
            (
                "a statement using the negated elements",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","NotPrincipal":"*","NotAction":"sagemaker:*","NotResource":"*"}]}"#
                    .to_string(),
                true,
            ),
            (
                "a non-ASCII statement id",
                format!(
                    r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
                    one.replace("Share", "Modèle")
                ),
                false,
            ),
            (
                "a non-ASCII condition value",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"sagemaker:*","Condition":{"StringEquals":{"aws:ResourceTag/team":"données"}}}]}"#
                    .to_string(),
                false,
            ),
            ("text that is not JSON", "this is not a policy".to_string(), false),
            (
                "a truncated object",
                r#"{"Version":"2012-10-17""#.to_string(),
                false,
            ),
            ("a JSON array", format!("[{one}]"), false),
            ("a JSON string", r#""just a string""#.to_string(), false),
            ("a JSON number", "42".to_string(), false),
            ("a JSON null", "null".to_string(), false),
            ("an empty object", "{}".to_string(), false),
            (
                "an element the document level does not define",
                format!(r#"{{"Version":"2012-10-17","NotAValidPolicyKey":1,"Statement":[{one}]}}"#),
                false,
            ),
            (
                "a statement element the grammar does not define",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sagemaker:*","Principal":"*","Effects":"Allow"}]}"#
                    .to_string(),
                false,
            ),
            (
                "a statement list of strings",
                r#"{"Version":"2012-10-17","Statement":["allow everything"]}"#.to_string(),
                false,
            ),
            (
                "a statement element that is not a statement",
                r#"{"Version":"2012-10-17","Statement":42}"#.to_string(),
                false,
            ),
            (
                "one good statement and one carrying an unknown element",
                format!(
                    r#"{{"Version":"2012-10-17","Statement":[{one},{{"Effect":"Allow","Grant":"*"}}]}}"#
                ),
                false,
            ),
        ];

        for (case, document, taken) in rows {
            let violation = resource_policy_violation(&document);
            assert!(
                violation.is_none() == taken,
                "case {case:?}: expected taken={taken}, got {violation:?}"
            );
        }
    }

    /// RULE: each refusal states WHY the document is not a policy, opening with the
    /// service's `Invalid Policy: ` subject and naming an unknown element.
    #[test]
    fn each_refusal_states_why_the_document_is_not_a_policy() {
        // (case, document, the whole message)
        let rows: Vec<(&str, String, &str)> = vec![
            (
                "non-ASCII content",
                r#"{"Version":"2012-10-17","Statement":[{"Sid":"Modèle","Effect":"Allow","Action":"sagemaker:*","Principal":"*"}]}"#
                    .to_string(),
                "Invalid Policy: Policy string can only contain ASCII characters",
            ),
            (
                "JSON that does not parse",
                r#"{"Version":"2012-10-17","Statement":["#.to_string(),
                "Invalid Policy: This policy contains invalid Json",
            ),
            (
                "JSON that is not an object",
                r#"["Statement"]"#.to_string(),
                "Invalid Policy: This policy contains invalid Json",
            ),
            (
                "an unknown document element",
                r#"{"Version":"2012-10-17","NotAValidPolicyKey":1}"#.to_string(),
                "Invalid Policy: Unknown field NotAValidPolicyKey",
            ),
            (
                "an unknown statement element",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sagemaker:*","Grant":"*"}]}"#
                    .to_string(),
                "Invalid Policy: Unknown field Grant",
            ),
            (
                "no statement at all",
                r#"{"Version":"2012-10-17"}"#.to_string(),
                "Invalid Policy: This policy contains invalid Json",
            ),
        ];

        for (case, document, expected) in rows {
            let violation = resource_policy_violation(&document);
            assert_eq!(
                violation.as_deref(),
                Some(expected),
                "case {case:?}: the refusal's message, got {violation:?}"
            );
        }
    }
}
