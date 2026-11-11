// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a Python handler imports: the `boto3` and `botocore` modules this runtime
//! supplies, in Python, over the one host function that makes a call
//! ([`super::aws_api`]).
//!
//! AWS Lambda's managed `python3.x` runtimes ship the AWS SDK for Python on the function's
//! import path, so `import boto3` at module scope succeeds and the handler can call another
//! service without shipping a dependency of its own. These modules are that import path's
//! contents here. They are the SDK's CLIENT interface — `boto3.client(...)`, an operation
//! per method, `botocore.exceptions.ClientError` for a refusal — because that is the
//! interface a handler calls a service through.
//!
//! What they do and do not do:
//!
//! * A method name is turned into the operation it names (`send_message` ->
//!   `SendMessage`) and its keyword arguments ARE the operation's input document, which is
//!   what an AWS JSON protocol puts on the wire; the host builds, signs and dispatches the
//!   request ([`super::aws_api`]) and the answer's document is what the method returns,
//!   with the `ResponseMetadata` the answer itself carried.
//! * A refusal is raised as `botocore.exceptions.ClientError`, carrying the service's own
//!   error code and message under the `response` member and rendering the sentence botocore
//!   renders, so a handler that catches one and reports `response["Error"]["Code"]` reads
//!   the emulated service's own answer.
//! * `client.exceptions.<ShapeName>` is the modelled error class for that name, a subclass
//!   of `ClientError`, so `except client.exceptions.QueueDoesNotExist` catches exactly that
//!   refusal and nothing else.
//! * DynamoDB's RESOURCE interface is supplied too ([`dynamodb`]), because it is the
//!   interface an application stores from inside a function through:
//!   `boto3.resource("dynamodb").Table(name)` carries the table name, its items are plain
//!   Python values rather than the wire's attribute values, and a `Key`/`Attr` condition is
//!   built into the expression the service reads. It is the same client underneath, so the
//!   request, the signature and the refusal are the ones above. A service this runtime
//!   supplies no resource classes for is REFUSED rather than served an interface that does
//!   nothing.
//! * There is no paginator: `client.get_paginator(...)` raises rather than pretending to
//!   serve one. A blob member travels as the base64 text an AWS JSON protocol carries it
//!   as, in both directions, and `bytes` a handler passes are encoded to it. A `datetime`
//!   a handler passes for a timestamp member travels as the seconds since the epoch that
//!   same protocol carries a timestamp as, whether or not it carries a zone of its own.
//! * A client is built with the CONFIGURATION the SDK's own `botocore.config.Config`
//!   carries, whose options and defaults are the SDK's: the Region it names is the one the
//!   client's calls are made in, and an option name the SDK does not have is refused the way
//!   the SDK refuses it.
//! * A member of an ANSWER that carries an object's own bytes — `GetObject`'s `Body` — is
//!   handed over as `botocore.response.StreamingBody`, the stream the SDK's own client
//!   answers with, so a handler reads it with `read()`; which members those are is named by
//!   the answer itself ([`super::aws_api::Answer::streams`]).
//! * Nothing here is the containment. The services that can be reached, the bounds on a
//!   call and the signature it carries are all host code's ([`super::aws_api`],
//!   [`super::python`]); a handler that rebinds or deletes a name in these modules changes
//!   none of them.

use super::package::PackageModule;


/// DynamoDB's resource interface: the service resource, its tables, and the item
/// marshalling between a plain Python value and an attribute value.
mod dynamodb;

/// The modules this runtime puts on a Python handler's import path, as the SDK it ships.
///
/// Resolved AFTER the deployment package's own entries, so a package that ships its own
/// `boto3` wins — which is the order a real runtime resolves them in, where the package
/// directory precedes the runtime's own site-packages.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    [
        ("botocore/__init__.py", BOTOCORE_SOURCE),
        ("botocore/exceptions.py", BOTOCORE_EXCEPTIONS_SOURCE),
        ("botocore/config.py", BOTOCORE_CONFIG_SOURCE),
        ("botocore/response.py", BOTOCORE_RESPONSE_SOURCE),
        ("botocore/credentials.py", BOTOCORE_CREDENTIALS_SOURCE),
        ("botocore/awsrequest.py", BOTOCORE_AWSREQUEST_SOURCE),
        ("botocore/auth.py", BOTOCORE_AUTH_SOURCE),
        ("botocore/session.py", BOTOCORE_SESSION_SOURCE),
        ("botocore/client.py", BOTOCORE_CLIENT_SOURCE),
        ("boto3/__init__.py", BOTO3_SOURCE),
        ("boto3/session.py", BOTO3_SESSION_SOURCE),
    ]
    .into_iter()
    .chain(dynamodb::modules())
    .map(|(path, source)| PackageModule {
        path: path.to_string(),
        source: source.to_string(),
    })
    .collect()
}

/// `botocore`: the package the client interface and the exceptions live under.
///
/// `xform_name` lives here because that is where the SDK keeps it: it is the one rule that
/// turns a model's own member or operation name into the Python one an SDK reports it under
/// (`TableName` -> `table_name`, `LatestStreamARN` -> `latest_stream_arn`).
const BOTOCORE_SOURCE: &str = r#"
"""The AWS SDK for Python's lower half, as this runtime supplies it."""

import re

_first_cap_regex = re.compile("(.)([A-Z][a-z]+)")
_number_cap_regex = re.compile("([a-z])([0-9]+)")
_end_cap_regex = re.compile("([a-z0-9])([A-Z])")
# A pluralised acronym (`ARNs`, `ACLs`, `CNAMEs`) is one word, not one per letter.
_special_case_transform = re.compile("[A-Z]{2,}s$")


def xform_name(name, sep="_"):
    """A model's name as the Python one the SDK reports it under."""
    if sep in name:
        # Already transformed.
        return name
    special = _special_case_transform.search(name)
    if special is not None:
        matched = special.group()
        name = "%s%s%ss" % (name[: -len(matched)], sep, matched[:-1].lower())
    first = _first_cap_regex.sub(r"\1" + sep + r"\2", name)
    numbered = _number_cap_regex.sub(r"\1" + sep + r"\2", first)
    return _end_cap_regex.sub(r"\1" + sep + r"\2", numbered).lower()
"#;

/// `botocore.exceptions`: the exception types a handler catches.
///
/// `ClientError`'s sentence is the one botocore renders — "An error occurred (<Code>) when
/// calling the <Operation> operation: <Message>" — because that sentence is what a handler
/// logs and what a caller reading the handler's report sees.
const BOTOCORE_EXCEPTIONS_SOURCE: &str = r#"
"""The exceptions an AWS SDK call raises."""


class BotoCoreError(Exception):
    """A failure of the SDK itself rather than an answer from a service."""

    fmt = "An unspecified error occurred"

    def __init__(self, **kwargs):
        Exception.__init__(self, self.fmt.format(**kwargs))
        self.kwargs = kwargs


class DataNotFoundError(BotoCoreError):
    fmt = "Unable to load data for: {data_path}"


class UnknownServiceError(DataNotFoundError):
    fmt = "Unknown service: '{service_name}'. Valid service names are: {known_service_names}"


class ParamValidationError(BotoCoreError):
    fmt = "Parameter validation failed:\n{report}"


class ReadTimeoutError(BotoCoreError):
    fmt = 'Read timeout on endpoint URL: "{endpoint_url}"'


class SdkCallNotMade(BotoCoreError):
    """The call could not be made at all, and why. Carries no answer from any service."""

    fmt = "{detail}"


class ClientError(Exception):
    """A service answered the call with a refusal."""

    MSG_TEMPLATE = (
        "An error occurred ({error_code}) when calling the {operation_name} "
        "operation: {error_message}"
    )

    def __init__(self, error_response, operation_name):
        error = error_response.get("Error", {})
        Exception.__init__(
            self,
            self.MSG_TEMPLATE.format(
                error_code=error.get("Code", "Unknown"),
                error_message=error.get("Message", "Unknown"),
                operation_name=operation_name,
            ),
        )
        self.response = error_response
        self.operation_name = operation_name


class InvalidRetryConfigurationError(BotoCoreError):
    fmt = (
        'Cannot provide retry configuration for "{retry_config_option}". '
        "Valid retry configuration options are: {valid_options}"
    )


class InvalidMaxRetryAttemptsError(InvalidRetryConfigurationError):
    fmt = (
        'Value provided to "max_attempts": {provided_max_attempts} must be an integer '
        "greater than or equal to {min_value}."
    )


class InvalidRetryModeError(InvalidRetryConfigurationError):
    fmt = (
        'Invalid value provided to "mode": "{provided_retry_mode}" must be one of: '
        "{valid_modes}"
    )
"#;

/// `botocore.config`: the client configuration a handler builds a client with.
///
/// The managed runtimes' SDK carries this module, and a handler that wants a call made
/// differently from the default — a region of its own, no retry at all, a timeout — passes
/// `botocore.config.Config(...)` to the client it builds. What it holds is the SDK's own
/// option set, in the SDK's own order, so a value passed positionally lands on the option
/// botocore binds it to and an option name the SDK does not have is refused the way the SDK
/// refuses it. Each option's default is the SDK's own.
///
/// The CALL itself is the host's ([`super::aws_api`]), which makes one attempt into this
/// process over an in-memory seam: there is no connection to time out and no attempt to
/// repeat, so the options bearing on transport are carried and reported rather than acted
/// on. The one option that decides where a call goes — the region — IS acted on by the
/// client built with it.
const BOTOCORE_CONFIG_SOURCE: &str = r#"
"""The configuration one client is built with."""

from botocore.exceptions import (
    InvalidMaxRetryAttemptsError,
    InvalidRetryConfigurationError,
    InvalidRetryModeError,
)

# The retry members a `retries` configuration may carry, and the modes it may name
# (`Config(retries={"max_attempts": 3, "mode": "standard"})`).
_RETRY_OPTIONS = ("total_max_attempts", "max_attempts", "mode")
_RETRY_MODES = ("legacy", "standard", "adaptive")


class Config:
    """A client's configuration, as `botocore.config.Config` holds one.

    The options are the SDK's own, in the SDK's own order: a positional argument lands on
    the option that order binds it to, and a name this set does not hold is a `TypeError`.
    """

    OPTION_DEFAULTS = {
        "region_name": None,
        "signature_version": None,
        "user_agent": None,
        "user_agent_extra": None,
        "user_agent_appid": None,
        "connect_timeout": 60,
        "read_timeout": 60,
        "parameter_validation": True,
        "max_pool_connections": 10,
        "proxies": None,
        "proxies_config": None,
        "s3": None,
        "retries": None,
        "client_cert": None,
        "inject_host_prefix": True,
        "endpoint_discovery_enabled": None,
        "use_dualstack_endpoint": None,
        "use_fips_endpoint": None,
        "ignore_configured_endpoint_urls": None,
        "defaults_mode": None,
        "tcp_keepalive": None,
        "request_min_compression_size_bytes": None,
        "disable_request_compression": None,
        "client_context_params": None,
        "sigv4a_signing_region_set": None,
        "request_checksum_calculation": None,
        "response_checksum_validation": None,
        "account_id_endpoint_mode": None,
    }

    def __init__(self, *args, **kwargs):
        self._user_provided_options = self._record_user_provided_options(args, kwargs)
        options = dict(self.OPTION_DEFAULTS)
        options.update(self._user_provided_options)
        for option, value in options.items():
            setattr(self, option, value)
        self._validate_retry_configuration(self.retries)

    def _record_user_provided_options(self, args, kwargs):
        option_order = list(self.OPTION_DEFAULTS)
        provided = {}
        for key, value in kwargs.items():
            if key not in self.OPTION_DEFAULTS:
                raise TypeError("Got unexpected keyword argument %r" % (key,))
            provided[key] = value
        if len(args) > len(option_order):
            raise TypeError(
                "Takes at most %s arguments (%s given)" % (len(option_order), len(args))
            )
        for index, argument in enumerate(args):
            option = option_order[index]
            if option in provided:
                raise TypeError("Got multiple values for keyword argument %r" % (option,))
            provided[option] = argument
        return provided

    def _validate_retry_configuration(self, retries):
        if not retries:
            return
        for key, value in retries.items():
            if key not in _RETRY_OPTIONS:
                raise InvalidRetryConfigurationError(
                    retry_config_option=key,
                    valid_options=", ".join(_RETRY_OPTIONS),
                )
            if key == "max_attempts" and value < 0:
                raise InvalidMaxRetryAttemptsError(
                    provided_max_attempts=value, min_value=0
                )
            if key == "total_max_attempts" and value < 1:
                raise InvalidMaxRetryAttemptsError(
                    provided_max_attempts=value, min_value=1
                )
            if key == "mode" and value not in _RETRY_MODES:
                raise InvalidRetryModeError(
                    provided_retry_mode=value, valid_modes=", ".join(_RETRY_MODES)
                )

    def merge(self, other_config):
        """This configuration with `other_config`'s own options over it.

        "Any options not provided by the user in the other config object will be the
        options this object holds", so what the caller of either one set wins in the order
        they are merged, and an option neither set keeps its default.
        """
        options = dict(self._user_provided_options)
        options.update(other_config._user_provided_options)
        return Config(**options)
"#;

/// `botocore.response`: the body of an answer that carries BYTES rather than text.
///
/// An object read answers with the object's own payload, which the SDK's client hands over
/// as a stream (`GetObject`'s `Body`), so a handler reads it with `read()`, iterates its
/// lines or uses it as a context manager. The bytes are already here — the whole answer
/// crossed the runtime's own call seam before the client returned — so no read of one waits
/// on anything.
const BOTOCORE_RESPONSE_SOURCE: &str = r#"
"""The body of an answer carrying bytes, as this runtime's SDK hands one over."""


class StreamingBody:
    """The bytes one answer carried, read as the stream a client answers with."""

    def __init__(self, data, content_length=None):
        self._data = bytes(data)
        self._position = 0
        self.content_length = (
            len(self._data) if content_length is None else content_length
        )

    def read(self, amt=None):
        """The next `amt` bytes, or everything that is left."""
        if amt is None:
            chunk = self._data[self._position:]
        else:
            chunk = self._data[self._position:self._position + max(int(amt), 0)]
        self._position += len(chunk)
        return chunk

    def close(self):
        self._position = len(self._data)

    def tell(self):
        return self._position

    def iter_chunks(self, chunk_size=1024):
        while True:
            chunk = self.read(chunk_size)
            if not chunk:
                return
            yield chunk

    def iter_lines(self, chunk_size=1024, keepends=False):
        for line in self.read().splitlines(keepends):
            yield line

    def __iter__(self):
        return self.iter_lines()

    def __enter__(self):
        return self

    def __exit__(self, *_exception):
        self.close()
        return False
"#;

/// `botocore.credentials`: the handle on the invocation's credentials.
///
/// A handler that signs a request of its own asks the session for its credentials and hands
/// them to [`BOTOCORE_AUTH_SOURCE`]'s signer. What it gets is a HANDLE and not a credential:
/// the signature is put on by host code with a credential this runtime never hands the
/// guest, so there is nothing secret here to read — and nothing is invented either, so a
/// handler that reads `access_key` finds no key rather than one that signs nothing.
const BOTOCORE_CREDENTIALS_SOURCE: &str = r#"
"""The invocation's credentials, which the handler holds no copy of."""


class Credentials:
    """A handle on the credentials this invocation's calls are signed with.

    The host signs those calls (see `botocore.auth`), so no access key, secret or session
    token of the invocation is readable here.
    """

    method = "lambda-runtime"

    def __init__(self, access_key=None, secret_key=None, token=None):
        self.access_key = access_key
        self.secret_key = secret_key
        self.token = token

    def get_frozen_credentials(self):
        return self
"#;

/// `botocore.awsrequest`: the request a handler builds to sign and send itself.
const BOTOCORE_AWSREQUEST_SOURCE: &str = r#"
"""One HTTP request a handler builds for itself."""


class AWSRequest:
    """A request a handler signs and then sends with an HTTP client of its own.

    The members are botocore's: `method`, `url`, `headers`, `body` (which `data` is the
    other name of), and `params` for the query. Nothing is sent by building one.
    """

    def __init__(self, method=None, url=None, headers=None, data=None, params=None):
        self.method = method
        self.url = url
        self.headers = dict(headers or {})
        self.data = data
        self.params = dict(params or {})
        self.context = {}

    @property
    def body(self):
        return self.data

    @body.setter
    def body(self, value):
        self.data = value

    def prepare(self):
        return self
"#;

/// `botocore.auth`: the signer a handler puts on a request of its own.
///
/// WHAT IT DOES: it records on the request which service and Region the signature is for,
/// and nothing else. The signature itself is put on by HOST code when the request is sent
/// ([`super::guest_http`]), with a credential the guest never holds — so this writes no
/// `Authorization` header, because it has nothing to write one from. A handler that signs a
/// request and sends it with an HTTP client reaches the service it addressed, signed; a
/// handler that reads the header back finds the request as it left it.
const BOTOCORE_AUTH_SOURCE: &str = r#"
"""Signing one request a handler built itself."""

_SIGNING_CONTEXT = "emulator_signing"


class BaseSigner:
    def add_auth(self, request):
        raise NotImplementedError("add_auth")


class SigV4Auth(BaseSigner):
    """The SigV4 signer, as a handler builds one: a credential handle, a service and a
    Region.

    `add_auth` states on the request WHICH service and Region its signature is for. The
    signature is put on by the runtime when the request is sent, out of a credential the
    handler never holds, so nothing secret passes through here and no signature is
    invented.
    """

    def __init__(self, credentials, service_name, region_name):
        self.credentials = credentials
        self.service_name = service_name
        self.region_name = region_name

    def add_auth(self, request):
        context = getattr(request, "context", None)
        if context is None:
            context = {}
            request.context = context
        context[_SIGNING_CONTEXT] = {
            "service": self.service_name,
            "region": self.region_name,
        }
        return request


class SigV4QueryAuth(SigV4Auth):
    """The same signer for a signature carried in the query, which is what a presigned URL
    carries. A URL is presigned by the client itself
    (`client.generate_presigned_url(...)`), where the runtime signs it; this exists so a
    handler that names the class finds it."""

    def __init__(self, credentials, service_name, region_name, expires=3600):
        SigV4Auth.__init__(self, credentials, service_name, region_name)
        self.expires = expires
"#;

/// `botocore.session`: the session the lower half of the SDK is reached through.
const BOTOCORE_SESSION_SOURCE: &str = r#"
"""The botocore session a handler asks for its credentials or a client."""

from botocore.credentials import Credentials


class Session:
    def __init__(self, **_ignored):
        self._credentials = Credentials()

    def get_credentials(self):
        return self._credentials

    def get_scoped_config(self):
        return {}

    def create_client(self, service_name, region_name=None, **kwargs):
        from botocore.client import Client

        return Client(service_name, region_name=region_name, **kwargs)


def get_session(**kwargs):
    return Session(**kwargs)
"#;

/// `botocore.client`: the client itself, and the one host call behind every method.
const BOTOCORE_CLIENT_SOURCE: &str = r##"
"""One AWS SDK client: a method per operation, over the runtime's own call seam."""

import base64
import json

from botocore.config import Config
from botocore.exceptions import (
    ClientError,
    ParamValidationError,
    ReadTimeoutError,
    SdkCallNotMade,
    UnknownServiceError,
)
from botocore.response import StreamingBody

_host = __import__("_lambda_host")

# The client that carries `generate_db_auth_token`: the SDK adds it to the RDS client
# rather than to every client, because the token is a database login's, not an API call's.
_DB_AUTH_TOKEN_SERVICES = ("rds",)

# The input members a call carries BYTES in, by service. Every protocol this SDK speaks
# carries a blob as base64 text, so the bytes a handler passes in one of these are encoded
# before the call is made; a `str` is encoded as UTF-8 first, which is what a client does
# with a text body. The members of the ANSWER that carry bytes are named by the answer
# itself, so nothing about them is listed here.
_BLOB_PARAMETERS = {"s3": ("Body",), "appconfig": ("Content",)}


def _operation_name(method):
    """The operation a client method names: `send_message` -> `SendMessage`."""
    return "".join(part[:1].upper() + part[1:] for part in method.split("_") if part)


def _epoch_seconds(value):
    """A `datetime` as the seconds since the epoch a JSON protocol carries a timestamp as.

    An SDK takes a native date-time for an input member the model types as a timestamp and
    sends it in the form the called service's protocol carries one in, which for the AWS
    JSON protocols this SDK speaks is the seconds since the epoch. A value carrying its own
    zone is read in that zone; one carrying none is read as UTC, the zone a function runs
    in, so the two forms of the same instant travel as the same number.
    """
    import datetime

    if value.tzinfo is not None:
        value = value.astimezone(datetime.timezone.utc).replace(tzinfo=None)
    return int((value - datetime.datetime(1970, 1, 1)).total_seconds())


def _encoded(value):
    """A value `json` does not write by itself.

    Every protocol this SDK speaks carries a blob as base64 text, so the `bytes` a handler
    passes are encoded to it, and a timestamp as the seconds since the epoch, so a
    `datetime` a handler passes for a timestamp member is converted to them
    ([`_epoch_seconds`]) rather than refused: a native date-time is what an SDK takes for
    such a member. Anything else is a value this SDK cannot send, and says so.
    """
    import datetime

    if isinstance(value, (bytes, bytearray)):
        return base64.b64encode(bytes(value)).decode("ascii")
    if isinstance(value, datetime.datetime):
        return _epoch_seconds(value)
    raise TypeError("%r is not a value an AWS SDK call can carry" % (value,))


def _copy_source(value):
    """The `CopySource` of a copy, as the SDK renders one.

    `CopySource` travels as the text of a header, and the SDK lets a caller give it either
    as that text or as `{"Bucket": …, "Key": …, "VersionId": …}` — in which case the SDK is
    what renders it: the bucket, a slash, the key with the characters a header cannot carry
    percent-encoded, and the version the copy reads, when one is named.
    """
    if not isinstance(value, dict):
        return value
    from urllib.parse import quote

    rendered = "%s/%s" % (
        value.get("Bucket", ""),
        quote(str(value.get("Key", "")).encode("utf-8"), safe="/~"),
    )
    version = value.get("VersionId")
    if version:
        rendered += "?versionId=%s" % (quote(str(version).encode("utf-8"), safe="/~"),)
    return rendered


def _materialise_bytes(document, path):
    """Replace the value `path` reaches with the bytes its base64 text carries.

    `path` is the object keys and list indices that reach one value from the root of the
    answer's document, as the runtime read them out of the called operation's own model: a
    JSON protocol carries a blob as base64 text, and what the SDK gives a caller is the
    bytes. A path the document does not hold, or a value that is not that text, is left
    exactly as it arrived.
    """
    held = document
    for step in path[:-1]:
        try:
            held = held[step]
        except (KeyError, IndexError, TypeError):
            return
    if not path:
        return
    last = path[-1]
    try:
        carried = held[last]
    except (KeyError, IndexError, TypeError):
        return
    if isinstance(carried, str):
        held[last] = base64.b64decode(carried)


def _error_code(error_type, document):
    """The error code a refusal names, from the header or the document's own member."""
    for named in (
        error_type,
        document.get("__type"),
        document.get("Code"),
        document.get("code"),
    ):
        if not named:
            continue
        shape = named.split("#")[-1].split(":")[0].strip()
        if shape:
            return shape
    return "Unknown"


def _error_message(document):
    for member in ("message", "Message", "errorMessage"):
        text = document.get(member)
        if isinstance(text, str):
            return text
    return ""


class _ClientExceptions:
    """`client.exceptions`: the modelled error class for each error code."""

    def __init__(self, service_name):
        self._service_name = service_name
        self._classes = {"ClientError": ClientError}

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        return self.from_code(name)

    def from_code(self, code):
        held = self._classes.get(code)
        if held is None:
            held = type(str(code), (ClientError,), {})
            self._classes[code] = held
        return held


class _ClientMeta:
    """`client.meta`: what the client was built for."""

    def __init__(self, service_name, region_name, endpoint_url, config=None):
        self.service_model = _ServiceModel(service_name)
        self.region_name = region_name
        self.endpoint_url = endpoint_url
        # `client.meta.config`: the configuration this client was built with, which is where
        # a handler reads back what it asked for.
        self.config = config if config is not None else Config(region_name=region_name)


class _ServiceModel:
    def __init__(self, service_name):
        self.service_name = service_name
        self.service_id = service_name


class Client:
    """A client for one service in one Region."""

    def __init__(self, service_name, region_name=None, config=None, **_ignored):
        import os

        self._service_name = service_name
        # Which Region the calls of this client are made in: the one the caller named, else
        # the one its `Config` carries ("The region to use in instantiating the client"),
        # else the Region the invocation itself runs in.
        self._config = config
        self._region_name = (
            region_name
            or getattr(config, "region_name", None)
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or ""
        )
        self._endpoint_url = "https://%s.%s.amazonaws.com" % (
            service_name,
            self._region_name,
        )
        self.exceptions = _ClientExceptions(service_name)
        self.meta = _ClientMeta(
            service_name, self._region_name, self._endpoint_url, self._config
        )

    def __getattr__(self, name):
        if name.startswith("_"):
            raise AttributeError(name)
        # `generate_db_auth_token` is not an operation: the SDK adds it to the RDS client
        # alone, and on every other client the attribute does not exist at all.
        if name == "generate_db_auth_token":
            if self._service_name not in _DB_AUTH_TOKEN_SERVICES:
                raise AttributeError(
                    "'%s' object has no attribute '%s'" % (self._service_name, name)
                )
            return self._db_auth_token
        operation_name = _operation_name(name)

        def method(**parameters):
            return self._call(operation_name, parameters)

        method.__name__ = name
        return method

    def can_paginate(self, operation_name):
        return False

    def _db_auth_token(self, DBHostname, Port, DBUsername, Region=None):
        """The authentication token a database login presents in place of a password.

        Not an API call: the SDK builds the token in the client, by presigning a request
        to connect as one database user on one endpoint. It is minted by the runtime,
        which holds the credential it is signed with.
        """
        answered = json.loads(
            _host.db_auth_token(
                str(DBHostname),
                int(Port),
                str(DBUsername),
                Region or self._region_name,
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            raise SdkCallNotMade(detail=answered.get("detail") or "")
        return answered.get("token") or ""

    def get_paginator(self, operation_name):
        raise SdkCallNotMade(
            detail="the AWS SDK this runtime supplies has no paginator; call %s in a "
            "loop over its own continuation member instead" % (operation_name,)
        )

    def close(self):
        return None

    def generate_presigned_url(
        self, ClientMethod, Params=None, ExpiresIn=3600, HttpMethod=None
    ):
        """A URL that carries its own signature, as botocore presigns one.

        The URL is built and SIGNED by the runtime, out of the operation's own model and a
        credential the handler never holds, so what comes back addresses the service this
        client was built for and carries a signature over exactly that request.
        """
        operation_name = _operation_name(ClientMethod)
        try:
            expires = int(ExpiresIn)
        except (TypeError, ValueError):
            raise ParamValidationError(
                report="ExpiresIn must be a number of seconds, got %r" % (ExpiresIn,)
            )
        answered = json.loads(
            _host.presign(
                self._service_name,
                operation_name,
                self._region_name,
                json.dumps(Params or {}, default=_encoded),
                expires,
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            self._raise_refusal(refused, answered, operation_name)
        return answered.get("url")

    def _input(self, parameters):
        """The input as the operation's own model takes it.

        The SDK renders a few members a caller may give in a friendlier form than the wire
        carries: `CopySource` is the one this SDK renders ([`_copy_source`]), because a copy
        is routinely written with the bucket and the key apart.
        """
        source = parameters.get("CopySource")
        if isinstance(source, dict):
            parameters = dict(parameters, CopySource=_copy_source(source))
        return parameters

    def _call(self, operation_name, parameters):
        parameters = self._input(parameters)
        answered = json.loads(
            _host.aws(
                self._service_name,
                operation_name,
                self._region_name,
                json.dumps(self._sent(parameters), default=_encoded),
            )
        )
        refused = answered.get("refused")
        if refused is not None:
            self._raise_refusal(refused, answered, operation_name)
        status = answered.get("status", 0)
        text = answered.get("body") or ""
        try:
            document = json.loads(text) if text else {}
        except ValueError:
            document = {}
        if not isinstance(document, dict):
            document = {}
        metadata = {
            "RequestId": answered.get("requestId"),
            "HTTPStatusCode": status,
            "RetryAttempts": 0,
        }
        if 200 <= status < 300:
            # The members the ANSWER says carry bytes are handed over as the stream a
            # client answers one with, decoded from the base64 they travelled as.
            for member in answered.get("streams") or ():
                carried = document.get(member)
                if isinstance(carried, str):
                    document[member] = StreamingBody(base64.b64decode(carried))
            # Every other value the answer says carries bytes is handed over as those
            # bytes, which is what the SDK gives a caller for a blob member: a JSON
            # protocol travels one as base64 text, so the text is decoded wherever the
            # answer places it — at whatever depth, inside a list or a map as well.
            for path in answered.get("blobs") or ():
                _materialise_bytes(document, path)
            document["ResponseMetadata"] = metadata
            return document
        code = _error_code(answered.get("errorType"), document)
        response = {
            "Error": {"Code": code, "Message": _error_message(document)},
            "ResponseMetadata": metadata,
        }
        raise self.exceptions.from_code(code)(response, operation_name)

    def _sent(self, parameters):
        """The call's parameters, with every byte-carrying member encoded as base64."""
        members = _BLOB_PARAMETERS.get(self._service_name, ())
        if not members:
            return parameters
        sent = dict(parameters)
        for member in members:
            if member not in sent:
                continue
            value = sent[member]
            if isinstance(value, str):
                value = value.encode("utf-8")
            if not isinstance(value, (bytes, bytearray)):
                raise ParamValidationError(
                    report="Invalid type for parameter %s: this SDK carries it as bytes "
                    "or as a string" % (member,)
                )
            sent[member] = base64.b64encode(bytes(value)).decode("ascii")
        return sent

    def _raise_refusal(self, refused, answered, operation_name):
        detail = answered.get("detail") or ""
        if refused == "unknown_service":
            raise UnknownServiceError(
                service_name=self._service_name, known_service_names=detail
            )
        if refused == "unknown_operation":
            raise AttributeError(
                "'%s' object has no attribute '%s'"
                % (self._service_name, operation_name)
            )
        if refused == "timed_out":
            raise ReadTimeoutError(endpoint_url=self._endpoint_url)
        raise SdkCallNotMade(detail=detail)
"##;

/// `boto3`: the names a handler imports.
const BOTO3_SOURCE: &str = r#"
"""The AWS SDK for Python, as this runtime supplies it."""

from boto3.session import Session


def client(*args, **kwargs):
    """A client for one service, as `boto3.client(...)` builds one."""
    return Session().client(*args, **kwargs)


def resource(*args, **kwargs):
    """A service resource, as `boto3.resource(...)` builds one."""
    return Session().resource(*args, **kwargs)


def setup_default_session(**kwargs):
    return None
"#;

/// `boto3.session`: the session a client is built from.
const BOTO3_SESSION_SOURCE: &str = r#"
"""The session a client is built from."""

from botocore.client import Client
from botocore.credentials import Credentials
from botocore.exceptions import SdkCallNotMade

# The services this runtime's SDK supplies resource classes for.
_RESOURCE_SERVICES = ("dynamodb",)


class Session:
    def __init__(self, region_name=None, **_ignored):
        import os

        self.region_name = (
            region_name
            or os.environ.get("AWS_REGION")
            or os.environ.get("AWS_DEFAULT_REGION")
            or None
        )

    def client(self, service_name, region_name=None, **kwargs):
        # The Region a client calls in, in the order the SDK resolves it: the one this call
        # named, else the one the client's own `Config` carries, else the session's.
        configured = getattr(kwargs.get("config"), "region_name", None)
        return Client(
            service_name,
            region_name=region_name or configured or self.region_name,
            **kwargs,
        )

    def get_credentials(self):
        """The handle on this invocation's credentials.

        A handler asks for these to sign a request of its own
        (`botocore.auth.SigV4Auth`); the signature is the runtime's, so the handle carries
        no key, secret or token of the invocation.
        """
        return Credentials()

    def resource(self, service_name, region_name=None, **kwargs):
        if service_name in _RESOURCE_SERVICES:
            # Imported here rather than at module scope: a handler that only builds
            # clients pays nothing for the resource interface's own modules.
            from boto3.dynamodb.table import service_resource

            return service_resource(
                self.client(service_name, region_name=region_name, **kwargs)
            )
        raise SdkCallNotMade(
            detail="the AWS SDK this runtime supplies has resource classes for %s only; "
            "build a client with boto3.client(%r) instead"
            % (", ".join(sorted(_RESOURCE_SERVICES)), service_name)
        )
"#;
