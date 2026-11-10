// SPDX-License-Identifier: CC0-1.0

//! The TLS LAYER of this runtime's standard library: the `ssl` module a Python handler
//! imports.
//!
//! A `python3.x` runtime is a promise about an interpreter AND about the standard library
//! that comes with it: a handler whose module body says `import socket, ssl` imports
//! cleanly on the runtime it was written for, and so it has to here — a module of the
//! standard library that this emulator registers no extension for is SUPPLIED on the
//! import path instead of being absent from it, exactly as the socket interface is
//! ([`super::guest_http::runtime_stdlib_modules`], which this module's source is resolved
//! beside). An import that fails is a runtime without that module; what this runtime
//! refuses is the CALL that would need a capability it does not have, not the import.
//!
//! What is this module's own, and what it refuses:
//!
//! * the NAMES a program reads off `ssl` — the protocol and version constants, the
//!   verification modes and flags, the option bits, the error classes and the purposes a
//!   context is built for — are all here, so a module that imports `ssl` to name one of
//!   them, or to catch one of its errors, imports and runs;
//! * a CONTEXT is real settings held in memory: what `create_default_context` builds,
//!   what a caller then changes on it and what it reads back are this module's own, and
//!   the language's own refusals of an inconsistent pair (verification off while the host
//!   name is checked) are kept;
//! * the ENCODINGS of a certificate (`PEM`/`DER`) and the certificate TIME format are
//!   computation, so they are performed here rather than refused;
//! * a SESSION is not: a Lambda function in this emulator opens no connection
//!   ([`super::guest_http`], whose `socket` reaches only the in-process X-Ray daemon), so
//!   there is no byte stream for a handshake to run on. Every call that would need one —
//!   the handshake itself, reading or writing through it, the peer's certificate, the
//!   channel binding — raises this module's own `SSLError`, and a certificate file of the
//!   HOST raises the one file refusal this runtime has ([`super::python`]).
//!
//! Nothing here is the containment, and nothing here invents a session: a call that cannot
//! be served says so, and a value a caller reads back is one this module computed or was
//! given.

use super::package::PackageModule;

/// The modules of the standard library this half of the runtime supplies: the TLS layer.
///
/// Resolved with the socket interface it is built on, before the frozen standard library
/// and after the deployment package's own entries — so this IS the `ssl` a handler
/// imports, and a package shipping its own still wins.
#[must_use]
pub fn runtime_stdlib_modules() -> Vec<PackageModule> {
    vec![PackageModule {
        path: "ssl.py".to_string(),
        source: SSL_SOURCE.to_string(),
    }]
}

/// `ssl`: the TLS layer of this runtime.
///
/// The surface is the standard library's, so a handler and the standard library's own
/// modules that are built on TLS (`http.client`, `urllib.request`, `smtplib`, `ftplib`,
/// `poplib`) read the names they expect; what no call synthesises is a SESSION, because
/// this runtime opens no connection for one.
const SSL_SOURCE: &str = r#"
"""The TLS layer of this runtime: a session needs a connection, which a function has none of."""

import base64
import collections
import enum

import socket as _socket

# What a call that would need a negotiated session answers. There is no byte stream to hand
# shake over, because this runtime opens no connection.
_NO_SESSION = (
    "a Lambda function in this emulator negotiates no TLS session: it opens no connection "
    "for one to run on"
)

# The PEM framing of a certificate, which is the form a certificate is carried in.
PEM_HEADER = "-----BEGIN CERTIFICATE-----"
PEM_FOOTER = "-----END CERTIFICATE-----"


class _SSLMethod(enum.IntEnum):
    """The protocol a context is built for, by the numbers `ssl` names them with."""

    PROTOCOL_SSLv23 = 2
    PROTOCOL_TLS = 2
    PROTOCOL_TLSv1 = 3
    PROTOCOL_TLSv1_1 = 4
    PROTOCOL_TLSv1_2 = 5
    PROTOCOL_TLS_CLIENT = 16
    PROTOCOL_TLS_SERVER = 17


PROTOCOL_SSLv23 = _SSLMethod.PROTOCOL_SSLv23
PROTOCOL_TLS = _SSLMethod.PROTOCOL_TLS
PROTOCOL_TLSv1 = _SSLMethod.PROTOCOL_TLSv1
PROTOCOL_TLSv1_1 = _SSLMethod.PROTOCOL_TLSv1_1
PROTOCOL_TLSv1_2 = _SSLMethod.PROTOCOL_TLSv1_2
PROTOCOL_TLS_CLIENT = _SSLMethod.PROTOCOL_TLS_CLIENT
PROTOCOL_TLS_SERVER = _SSLMethod.PROTOCOL_TLS_SERVER


class TLSVersion(enum.IntEnum):
    """A protocol version, as a context's own bounds are stated in."""

    MINIMUM_SUPPORTED = -2
    SSLv3 = 0x0300
    TLSv1 = 0x0301
    TLSv1_1 = 0x0302
    TLSv1_2 = 0x0303
    TLSv1_3 = 0x0304
    MAXIMUM_SUPPORTED = -1


class VerifyMode(enum.IntEnum):
    """Whether the peer's certificate is required, optional or not asked for."""

    CERT_NONE = 0
    CERT_OPTIONAL = 1
    CERT_REQUIRED = 2


CERT_NONE = VerifyMode.CERT_NONE
CERT_OPTIONAL = VerifyMode.CERT_OPTIONAL
CERT_REQUIRED = VerifyMode.CERT_REQUIRED


class SSLErrorNumber(enum.IntEnum):
    """What an error of the TLS layer reports as its first argument."""

    SSL_ERROR_NONE = 0
    SSL_ERROR_SSL = 1
    SSL_ERROR_WANT_READ = 2
    SSL_ERROR_WANT_WRITE = 3
    SSL_ERROR_WANT_X509_LOOKUP = 4
    SSL_ERROR_SYSCALL = 5
    SSL_ERROR_ZERO_RETURN = 6
    SSL_ERROR_WANT_CONNECT = 7
    SSL_ERROR_EOF = 8
    SSL_ERROR_INVALID_ERROR_CODE = 10


SSL_ERROR_NONE = SSLErrorNumber.SSL_ERROR_NONE
SSL_ERROR_SSL = SSLErrorNumber.SSL_ERROR_SSL
SSL_ERROR_WANT_READ = SSLErrorNumber.SSL_ERROR_WANT_READ
SSL_ERROR_WANT_WRITE = SSLErrorNumber.SSL_ERROR_WANT_WRITE
SSL_ERROR_WANT_X509_LOOKUP = SSLErrorNumber.SSL_ERROR_WANT_X509_LOOKUP
SSL_ERROR_SYSCALL = SSLErrorNumber.SSL_ERROR_SYSCALL
SSL_ERROR_ZERO_RETURN = SSLErrorNumber.SSL_ERROR_ZERO_RETURN
SSL_ERROR_WANT_CONNECT = SSLErrorNumber.SSL_ERROR_WANT_CONNECT
SSL_ERROR_EOF = SSLErrorNumber.SSL_ERROR_EOF
SSL_ERROR_INVALID_ERROR_CODE = SSLErrorNumber.SSL_ERROR_INVALID_ERROR_CODE

# The option bits a context's `options` is made of, by the values the TLS library names
# them with.
OP_ALL = 0x80000054
OP_NO_SSLv2 = 0x00000000
OP_NO_SSLv3 = 0x02000000
OP_NO_TLSv1 = 0x04000000
OP_NO_TLSv1_1 = 0x10000000
OP_NO_TLSv1_2 = 0x08000000
OP_NO_TLSv1_3 = 0x20000000
OP_NO_COMPRESSION = 0x00020000
OP_NO_TICKET = 0x00004000
OP_NO_RENEGOTIATION = 0x40000000
OP_CIPHER_SERVER_PREFERENCE = 0x00400000
OP_SINGLE_DH_USE = 0x00000000
OP_SINGLE_ECDH_USE = 0x00000000
OP_ENABLE_MIDDLEBOX_COMPAT = 0x00100000
OP_IGNORE_UNEXPECTED_EOF = 0x00000080
OP_LEGACY_SERVER_CONNECT = 0x00000004

# The verification flags a context's `verify_flags` is made of.
VERIFY_DEFAULT = 0x00000
VERIFY_CRL_CHECK_LEAF = 0x00004
VERIFY_CRL_CHECK_CHAIN = 0x0000C
VERIFY_X509_STRICT = 0x00020
VERIFY_ALLOW_PROXY_CERTS = 0x00040
VERIFY_X509_TRUSTED_FIRST = 0x08000
VERIFY_X509_PARTIAL_CHAIN = 0x80000

# WHICH of the names above a context admits being configured with: the versions and
# extensions this module's own settings accept. None of them is a session that was
# negotiated — there is none — so what they describe is the configuration surface.
HAS_SNI = True
HAS_ECDH = True
HAS_ALPN = True
HAS_NPN = False
HAS_PSK = False
HAS_SSLv2 = False
HAS_SSLv3 = False
HAS_TLSv1 = True
HAS_TLSv1_1 = True
HAS_TLSv1_2 = True
HAS_TLSv1_3 = True
HAS_NEVER_CHECK_COMMON_NAME = True

# No session is negotiated, so no channel of one can be bound to.
CHANNEL_BINDING_TYPES = []


class SSLError(OSError):
    """An error of the TLS layer."""

    library = None
    reason = None


class SSLZeroReturnError(SSLError):
    """The peer closed the session."""


class SSLWantReadError(SSLError):
    """The session needs more bytes from the peer before it can go on."""


class SSLWantWriteError(SSLError):
    """The session needs to write bytes to the peer before it can go on."""


class SSLSyscallError(SSLError):
    """The system call under the session failed."""


class SSLEOFError(SSLError):
    """The session ended without a close."""


class SSLCertVerificationError(SSLError, ValueError):
    """The peer's certificate was not verified."""

    verify_code = None
    verify_message = None


CertificateError = SSLCertVerificationError


class Purpose(enum.Enum):
    """What a context is built to verify, by the extended-key-usage it names.

    The values are the object identifiers X.509 gives those usages
    (`id-kp-serverAuth`, `id-kp-clientAuth`).
    """

    SERVER_AUTH = "1.3.6.1.5.5.7.3.1"
    CLIENT_AUTH = "1.3.6.1.5.5.7.3.2"

    @property
    def oid(self):
        return self.value

    @property
    def shortname(self):
        return "serverAuth" if self is Purpose.SERVER_AUTH else "clientAuth"


DefaultVerifyPaths = collections.namedtuple(
    "DefaultVerifyPaths",
    "cafile capath openssl_cafile_env openssl_cafile openssl_capath_env openssl_capath",
)


def get_default_verify_paths():
    """The host certificate store a context would load by default: there is none.

    This runtime reads no file of the host, so no path is named — the variables a TLS
    library reads such a path out of are, because they are what a caller sets.
    """
    return DefaultVerifyPaths(None, None, "SSL_CERT_FILE", None, "SSL_CERT_DIR", None)


def DER_cert_to_PEM_cert(der_cert_bytes):
    """The PEM text that carries `der_cert_bytes`."""
    encoded = base64.encodebytes(bytes(der_cert_bytes)).decode("ascii")
    return "%s\n%s%s\n" % (PEM_HEADER, encoded, PEM_FOOTER)


def PEM_cert_to_DER_cert(pem_cert_string):
    """The bytes a PEM certificate carries, the other way round."""
    pem_cert_string = pem_cert_string.strip()
    if not pem_cert_string.startswith(PEM_HEADER):
        raise ValueError("Invalid PEM encoding; must start with %s" % (PEM_HEADER,))
    if not pem_cert_string.endswith(PEM_FOOTER):
        raise ValueError("Invalid PEM encoding; must end with %s" % (PEM_FOOTER,))
    body = pem_cert_string[len(PEM_HEADER) : -len(PEM_FOOTER)]
    return base64.decodebytes(body.encode("ascii", "strict"))


def cert_time_to_seconds(cert_time):
    """The seconds since the epoch a certificate's own time format states.

    The format is the one a certificate carries, `"%b %d %H:%M:%S %Y GMT"`, whose month is
    the English abbreviation and whose zone is always GMT.
    """
    from calendar import timegm
    from time import strptime

    months = (
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    )
    time_format = " %d %H:%M:%S %Y GMT"
    try:
        month_number = months.index(cert_time[:3].title()) + 1
    except ValueError:
        raise ValueError(
            'time data %r does not match format "%%b%s"' % (cert_time, time_format)
        )
    parsed = strptime(cert_time[3:], time_format)
    return timegm((parsed[0], month_number) + tuple(parsed[2:6]) + (0, 0, 0))


def _certificates_in(cadata):
    """The certificates `cadata` carries, as the bytes of each.

    PEM text may carry several; a bytes-like value is one certificate's own bytes, which is
    what the standard library's own `cadata` accepts. The scan is the FRAMING only — it
    finds each certificate's begin and end line and hands the body to `base64` — so nothing
    here parses a certificate, recurses, or looks inside one.
    """
    if isinstance(cadata, (bytes, bytearray, memoryview)):
        return [bytes(cadata)]
    held = []
    rest = str(cadata)
    while PEM_HEADER in rest:
        start = rest.index(PEM_HEADER)
        end = rest.find(PEM_FOOTER, start)
        if end < 0:
            raise SSLError("Invalid PEM encoding; must end with %s" % (PEM_FOOTER,))
        end += len(PEM_FOOTER)
        held.append(PEM_cert_to_DER_cert(rest[start:end]))
        rest = rest[end:]
    if not held:
        raise SSLError("no start line: cadata does not hold a certificate")
    return held


class MemoryBIO(object):
    """A buffer the TLS layer would read from and write to, in memory.

    The buffer itself needs no session, so this one holds bytes and reports what it holds.
    """

    def __init__(self):
        self._held = bytearray()
        self._write_eof = False

    @property
    def pending(self):
        return len(self._held)

    @property
    def eof(self):
        return self._write_eof and not self._held

    def read(self, size=-1):
        if size is None or size < 0 or size > len(self._held):
            size = len(self._held)
        taken = bytes(self._held[:size])
        del self._held[:size]
        return taken

    def write(self, data):
        if self._write_eof:
            raise ValueError("cannot write() after write_eof()")
        held = bytes(data)
        self._held.extend(held)
        return len(held)

    def write_eof(self):
        self._write_eof = True


class SSLSession(object):
    """A session a later connection could resume. Nothing resumes one here."""

    def __init__(self):
        self._id = b""

    @property
    def id(self):
        return self._id

    @property
    def time(self):
        return 0

    @property
    def timeout(self):
        return 0

    @property
    def ticket_lifetime_hint(self):
        return 0

    @property
    def has_ticket(self):
        return False


class SSLContext(object):
    """The TLS settings a client or a server would negotiate with.

    Everything a caller sets here is held and read back; what the settings are never used
    for is a session, because this runtime opens no connection for one.
    """

    def __init__(self, protocol=PROTOCOL_TLS_CLIENT):
        try:
            protocol = _SSLMethod(protocol)
        except ValueError:
            raise ValueError("invalid or unsupported protocol version %r" % (protocol,))
        self._protocol = protocol
        self.options = OP_ALL | OP_NO_SSLv3 | OP_NO_COMPRESSION
        self.verify_flags = VERIFY_DEFAULT
        self.minimum_version = TLSVersion.MINIMUM_SUPPORTED
        self.maximum_version = TLSVersion.MAXIMUM_SUPPORTED
        self.sni_callback = None
        self.keylog_filename = None
        self.post_handshake_auth = False
        self.num_tickets = 2
        self._ciphers = "DEFAULT"
        self._alpn_protocols = []
        self._certificates = []
        self._chains = 0
        client = protocol == _SSLMethod.PROTOCOL_TLS_CLIENT
        self._check_hostname = client
        self._verify_mode = CERT_REQUIRED if client else CERT_NONE

    @property
    def protocol(self):
        return self._protocol

    @property
    def verify_mode(self):
        return self._verify_mode

    @verify_mode.setter
    def verify_mode(self, value):
        value = VerifyMode(value)
        if value == CERT_NONE and self._check_hostname:
            raise ValueError(
                "Cannot set verify_mode to CERT_NONE when check_hostname is enabled."
            )
        self._verify_mode = value

    @property
    def check_hostname(self):
        return self._check_hostname

    @check_hostname.setter
    def check_hostname(self, value):
        value = bool(value)
        # The host name cannot be checked against a certificate that is not asked for, so
        # asking for the check asks for the certificate — as the standard library's own
        # context does.
        if value and self._verify_mode == CERT_NONE:
            self._verify_mode = CERT_REQUIRED
        self._check_hostname = value

    def load_verify_locations(self, cafile=None, capath=None, cadata=None):
        if cafile is None and capath is None and cadata is None:
            raise TypeError("cafile, capath and cadata cannot be all omitted")
        for path in (cafile, capath):
            if path is not None:
                # The one file refusal this runtime has: a function opens no host file, so
                # a store named by a PATH is not readable while one passed as DATA is.
                open(path, "rb")
        if cadata is not None:
            self._certificates.extend(_certificates_in(cadata))

    def load_cert_chain(self, certfile, keyfile=None, password=None):
        for path in (certfile, keyfile):
            if path is not None:
                open(path, "rb")
        self._chains += 1

    def load_default_certs(self, purpose=Purpose.SERVER_AUTH):
        """The host's own certificate store, of which this runtime has none to load."""
        if not isinstance(purpose, Purpose):
            raise TypeError(purpose)

    def set_default_verify_paths(self):
        """The same store, named by the TLS library's own paths: there is none either."""

    def cert_store_stats(self):
        loaded = len(self._certificates)
        return {"x509": loaded, "x509_ca": loaded, "crl": 0}

    def get_ca_certs(self, binary_form=False):
        if binary_form:
            return list(self._certificates)
        if not self._certificates:
            return []
        raise SSLError(
            "a Lambda function in this emulator decodes no certificate: read a loaded one "
            "with binary_form=True"
        )

    def session_stats(self):
        return {
            "number": 0,
            "connect": 0,
            "connect_good": 0,
            "connect_renegotiate": 0,
            "accept": 0,
            "accept_good": 0,
            "accept_renegotiate": 0,
            "hits": 0,
            "misses": 0,
            "timeouts": 0,
            "cache_full": 0,
        }

    def set_ciphers(self, ciphers):
        if not str(ciphers):
            raise SSLError("No cipher can be selected.")
        self._ciphers = str(ciphers)

    def get_ciphers(self):
        """The suites a session would be negotiated with, which this runtime cannot state.

        The standard library answers the enabled suites out of the TLS library's own cipher
        database, which this runtime has none of: the cipher STRING a caller set is held
        (`set_ciphers`), and describing the suites it selects is refused here rather than
        answered with an empty list that would read as "no cipher is enabled".
        """
        raise SSLError(
            "a Lambda function in this emulator describes no cipher suite: it holds the "
            "cipher string that was set and negotiates no session to select one for"
        )

    def set_alpn_protocols(self, alpn_protocols):
        self._alpn_protocols = [str(one) for one in alpn_protocols]

    def set_npn_protocols(self, npn_protocols):
        raise NotImplementedError("The NPN extension requires OpenSSL 1.0.1 or later.")

    def set_servername_callback(self, server_name_callback):
        self.sni_callback = server_name_callback

    def wrap_socket(
        self,
        sock,
        server_side=False,
        do_handshake_on_connect=True,
        suppress_ragged_eofs=True,
        server_hostname=None,
        session=None,
    ):
        if server_side and server_hostname:
            raise ValueError("server_hostname can only be specified in client mode")
        if self._check_hostname and not server_side and not server_hostname:
            raise ValueError("check_hostname requires server_hostname")
        wrapped = SSLSocket(
            sock,
            self,
            server_side=server_side,
            server_hostname=server_hostname,
            suppress_ragged_eofs=suppress_ragged_eofs,
            session=session,
            do_handshake_on_connect=do_handshake_on_connect,
        )
        # A socket that is already connected would hand shake here, exactly as the standard
        # library's own wrap does; one that is not hands shake on its connect.
        if do_handshake_on_connect and wrapped._connected():
            wrapped.do_handshake()
        return wrapped

    def wrap_bio(
        self,
        incoming,
        outgoing,
        server_side=False,
        server_hostname=None,
        session=None,
    ):
        if server_side and server_hostname:
            raise ValueError("server_hostname can only be specified in client mode")
        if self._check_hostname and not server_side and not server_hostname:
            raise ValueError("check_hostname requires server_hostname")
        return SSLObject(
            incoming,
            outgoing,
            self,
            server_side=server_side,
            server_hostname=server_hostname,
            session=session,
        )


class SSLSocket(object):
    """A socket with the TLS layer put on it.

    The socket's own calls are the wrapped socket's; the calls that need a negotiated
    session — the handshake, reading and writing through it, the peer's certificate —
    raise, because this runtime opens no connection for a session to run on.
    """

    def __init__(
        self,
        sock,
        context,
        server_side=False,
        server_hostname=None,
        suppress_ragged_eofs=True,
        session=None,
        do_handshake_on_connect=True,
    ):
        self._sock = sock
        self.context = context
        self.server_side = bool(server_side)
        self.server_hostname = server_hostname
        self.suppress_ragged_eofs = suppress_ragged_eofs
        self.do_handshake_on_connect = do_handshake_on_connect
        self._held_session = session
        self._handshaken = False

    def __enter__(self):
        return self

    def __exit__(self, *exception):
        self.close()
        return False

    def __repr__(self):
        return "<SSLSocket server_hostname=%r>" % (self.server_hostname,)

    # The socket underneath, and the calls that are its own.

    @property
    def family(self):
        return self._sock.family

    @property
    def type(self):
        return self._sock.type

    @property
    def proto(self):
        return self._sock.proto

    def close(self):
        return self._sock.close()

    def detach(self):
        return self._sock.detach()

    def dup(self):
        raise NotImplementedError("Can't dup() %s instances" % (self.__class__.__name__,))

    def fileno(self):
        return self._sock.fileno()

    def settimeout(self, value):
        return self._sock.settimeout(value)

    def gettimeout(self):
        return self._sock.gettimeout()

    def setblocking(self, blocking):
        return self._sock.setblocking(blocking)

    def getblocking(self):
        return self._sock.getblocking()

    def setsockopt(self, *arguments):
        return self._sock.setsockopt(*arguments)

    def getsockopt(self, *arguments):
        return self._sock.getsockopt(*arguments)

    def bind(self, address):
        return self._sock.bind(address)

    def listen(self, *arguments):
        return self._sock.listen(*arguments)

    def accept(self):
        return self._sock.accept()

    def getsockname(self):
        return self._sock.getsockname()

    def getpeername(self):
        return self._sock.getpeername()

    def shutdown(self, how):
        return self._sock.shutdown(how)

    def connect(self, address):
        self._sock.connect(address)
        if self.do_handshake_on_connect:
            self.do_handshake()

    def connect_ex(self, address):
        answered = self._sock.connect_ex(address)
        if answered == 0 and self.do_handshake_on_connect:
            self.do_handshake()
        return answered

    # The calls that need a session.

    def do_handshake(self, block=False):
        raise SSLError(_NO_SESSION)

    def send(self, data, flags=0):
        self._negotiated()

    def sendall(self, data, flags=0):
        self._negotiated()

    def sendto(self, data, flags_or_address, address=None):
        raise ValueError("sendto not allowed on instances of %s" % (self.__class__,))

    def sendfile(self, file, offset=0, count=None):
        self._negotiated()

    def recv(self, buflen=1024, flags=0):
        self._negotiated()

    def recv_into(self, buffer, nbytes=None, flags=0):
        self._negotiated()

    def recvfrom(self, buflen=1024, flags=0):
        raise ValueError("recvfrom not allowed on instances of %s" % (self.__class__,))

    def read(self, len=1024, buffer=None):
        self._negotiated()

    def write(self, data):
        self._negotiated()

    def makefile(self, *arguments, **keywords):
        self._negotiated()

    def pending(self):
        return 0

    def unwrap(self):
        self._negotiated()

    def verify_client_post_handshake(self):
        raise SSLError(_NO_SESSION)

    def getpeercert(self, binary_form=False):
        raise ValueError("handshake not done yet")

    def get_channel_binding(self, cb_type="tls-unique"):
        raise ValueError(
            "'%s' channel binding type not implemented" % (cb_type,)
        )

    # What a session WOULD report, read off one that was never negotiated: nothing, which
    # is what the standard library answers before a handshake too.

    def cipher(self):
        return None

    def shared_ciphers(self):
        return None

    def compression(self):
        return None

    def version(self):
        return None

    def selected_alpn_protocol(self):
        return None

    def selected_npn_protocol(self):
        return None

    @property
    def session(self):
        return self._held_session

    @session.setter
    def session(self, value):
        self._held_session = value

    @property
    def session_reused(self):
        return None

    def _connected(self):
        """Whether the socket underneath already has a peer."""
        try:
            self._sock.getpeername()
        except OSError:
            return False
        return True

    def _negotiated(self):
        """The session this call needs, of which there is none: this always raises."""
        raise SSLError(_NO_SESSION)


class SSLObject(object):
    """The TLS layer over a pair of memory buffers rather than over a socket.

    The buffers are real (MemoryBIO); the session over them is not, for the same reason a
    socket's is not.
    """

    def __init__(
        self,
        incoming,
        outgoing,
        context,
        server_side=False,
        server_hostname=None,
        session=None,
    ):
        self.incoming = incoming
        self.outgoing = outgoing
        self.context = context
        self.server_side = bool(server_side)
        self.server_hostname = server_hostname
        self._held_session = session

    def do_handshake(self):
        raise SSLError(_NO_SESSION)

    def read(self, len=1024, buffer=None):
        raise SSLError(_NO_SESSION)

    def write(self, data):
        raise SSLError(_NO_SESSION)

    def unwrap(self):
        raise SSLError(_NO_SESSION)

    def verify_client_post_handshake(self):
        raise SSLError(_NO_SESSION)

    def getpeercert(self, binary_form=False):
        raise ValueError("handshake not done yet")

    def get_channel_binding(self, cb_type="tls-unique"):
        raise ValueError("'%s' channel binding type not implemented" % (cb_type,))

    def pending(self):
        return 0

    def cipher(self):
        return None

    def shared_ciphers(self):
        return None

    def compression(self):
        return None

    def version(self):
        return None

    def selected_alpn_protocol(self):
        return None

    def selected_npn_protocol(self):
        return None

    @property
    def session(self):
        return self._held_session

    @session.setter
    def session(self, value):
        self._held_session = value

    @property
    def session_reused(self):
        return None


def create_default_context(purpose=Purpose.SERVER_AUTH, *, cafile=None, capath=None, cadata=None):
    """The context the standard library builds for `purpose`: verification on, host name
    checked for a client."""
    if not isinstance(purpose, Purpose):
        raise TypeError(purpose)
    if purpose is Purpose.SERVER_AUTH:
        context = SSLContext(PROTOCOL_TLS_CLIENT)
        context.verify_mode = CERT_REQUIRED
        context.check_hostname = True
    else:
        context = SSLContext(PROTOCOL_TLS_SERVER)
    context.options |= OP_NO_COMPRESSION
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    elif context.verify_mode != CERT_NONE:
        context.load_default_certs(purpose)
    return context


def _create_unverified_context(
    protocol=None,
    *,
    cert_reqs=CERT_NONE,
    check_hostname=False,
    purpose=Purpose.SERVER_AUTH,
    certfile=None,
    keyfile=None,
    cafile=None,
    capath=None,
    cadata=None
):
    """The same context with the peer NOT verified, which is what a caller asking for one
    unverified gets."""
    if not isinstance(purpose, Purpose):
        raise TypeError(purpose)
    if purpose is Purpose.SERVER_AUTH:
        context = SSLContext(protocol or PROTOCOL_TLS_CLIENT)
    else:
        context = SSLContext(protocol or PROTOCOL_TLS_SERVER)
    context.check_hostname = False
    context.verify_mode = CERT_NONE
    if check_hostname:
        context.check_hostname = True
    if cert_reqs is not None:
        context.verify_mode = cert_reqs
    if certfile or keyfile:
        context.load_cert_chain(certfile, keyfile)
    if cafile or capath or cadata:
        context.load_verify_locations(cafile, capath, cadata)
    return context


_create_default_https_context = create_default_context
_create_stdlib_context = _create_unverified_context


def get_server_certificate(
    addr, ssl_version=PROTOCOL_TLS_CLIENT, ca_certs=None, timeout=_socket._GLOBAL_DEFAULT_TIMEOUT
):
    """The certificate the server at `addr` presents, read by connecting to it.

    The connection is the socket module's, which a function has no host network for.
    """
    host, port = addr
    if ca_certs is not None:
        context = _create_stdlib_context(ssl_version, cert_reqs=CERT_REQUIRED, cafile=ca_certs)
    else:
        context = _create_stdlib_context(ssl_version)
    with _socket.create_connection(addr, timeout=timeout) as sock:
        with context.wrap_socket(sock, server_hostname=host) as wrapped:
            return DER_cert_to_PEM_cert(wrapped.getpeercert(True))
"#;
