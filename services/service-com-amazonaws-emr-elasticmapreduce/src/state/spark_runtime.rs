// SPDX-License-Identifier: CC0-1.0

//! The SPARK runtime a submitted application finds on its import path.
//!
//! A `spark-submit` of a Python application runs that application in the contained Python
//! interpreter this workspace embeds ([`super::spark_step_run`]), and an application built on
//! Spark resolves its engine through the `pyspark` package. These are the modules this
//! emulator supplies for it — ordinary Python source, handed to the interpreter as the
//! modules on the application's path, exactly as the Lambda runtime's AWS SDK modules are
//! handed to a handler.
//!
//! # What they are, and what they are not
//!
//! They are an ENGINE, not a stub and not a transcript: every transformation and every action
//! is COMPUTED, in this process, by running the application's own functions over the
//! application's own data ([`RDD_SOURCE`]). `parallelize` slices a collection into partitions,
//! `map` calls the application's function on each element, `reduceByKey` folds the
//! application's operator over the values of each key, and `collect` answers what those
//! computations produced. Nothing is read off the application's source text and no result is
//! synthesised: an application that computes a sum gets the sum of its own data, and one whose
//! function raises gets that exception.
//!
//! What they are not is a distributed Spark. The partitions are lists in this process, there
//! is no cluster to schedule them on, and the surface is the one this emulator can compute —
//! the resilient distributed dataset, the session and context that hand one out, and the
//! Hadoop filesystem a Spark application reads and writes through. An application that
//! reaches for anything else (the SQL planner, a streaming source, the machine-learning
//! library) raises the way a missing attribute raises, so the step it was submitted by FAILS
//! carrying that error rather than reporting a success nothing computed.
//!
//! # Where its filesystem reads and writes land
//!
//! A path an application names in Amazon S3 is a real object of this emulator's own object
//! store: [`FILESYSTEM_SOURCE`] performs each read and write with the AWS SDK the runtime
//! supplies the guest (`boto3`), whose calls travel over the emulator's in-process callback
//! and are signed as the role the step runs as. So an application that writes its output to
//! `s3://…` leaves an object a later `GetObject` really reads, and one that reads from
//! `s3://…` sees what was really put there. A path in any other filesystem — a node's own
//! disk, HDFS — is one this emulator holds nothing for, and the filesystem says so the way
//! Hadoop says it, rather than pretending to have written it.
//!
//! # Bounds and containment
//!
//! Neither is here: both are the interpreter's, in host code the application cannot reach
//! (the deadline, the step budget, the memory ceiling, the recursion limit and the output cap
//! of [`super::spark_step_run`], and the native-module allow-list of the engine itself).
//! These modules add no host surface of their own: they reach the object store through the
//! same SDK seam any guest call uses, and nothing else.

use ::service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;

/// The modules this emulator puts on a submitted application's import path, as the
/// interpreter takes them: the `pyspark` package, and the entry module that RUNS the
/// application ([`APPLICATION_ENTRY_SOURCE`]).
///
/// `spark_version` is the version of Spark the cluster's own release ships, which is what the
/// application is told it is running on ([`RELEASE_SOURCE`]) — `None` for a cluster whose
/// release bundles no Spark, where nothing is known to tell it.
#[must_use]
pub fn spark_runtime_modules(spark_version: Option<&str>) -> Vec<PackageModule> {
    let release = release_source(spark_version);
    [
        (format!("{APPLICATION_ENTRY_MODULE}.py"), APPLICATION_ENTRY_SOURCE.to_string()),
        ("pyspark/__init__.py".to_string(), PYSPARK_SOURCE.to_string()),
        ("pyspark/release.py".to_string(), release),
        ("pyspark/conf.py".to_string(), CONF_SOURCE.to_string()),
        ("pyspark/filesystem.py".to_string(), FILESYSTEM_SOURCE.to_string()),
        ("pyspark/rdd.py".to_string(), RDD_SOURCE.to_string()),
        ("pyspark/context.py".to_string(), CONTEXT_SOURCE.to_string()),
        ("pyspark/jvm.py".to_string(), JVM_SOURCE.to_string()),
        ("pyspark/sql/__init__.py".to_string(), SQL_SOURCE.to_string()),
    ]
    .into_iter()
    .map(|(path, source)| PackageModule { path, source })
    .collect()
}

/// `pyspark.release`: the ONE value of this runtime that is not the same for every run — the
/// version of Spark the cluster's release ships, which an application reads as
/// `spark.version`.
///
/// It is the version the cluster's own release catalog publishes for Spark
/// ([`super::release_application_version`]), so an application on an older release is told
/// that release's version rather than one spelled here. A cluster whose release bundles no
/// Spark tells the application nothing about a version it has none of.
fn release_source(spark_version: Option<&str>) -> String {
    let version = match spark_version {
        // Rendered through the JSON encoder, so the catalog's value reaches the module as a
        // string literal whatever it holds.
        Some(version) => ::serde_json::Value::String(version.to_string()).to_string(),
        None => "None".to_string(),
    };
    format!(
        "{RELEASE_SOURCE}\n# The version of Spark the cluster's own release ships.\nVERSION = \
         {version}\n"
    )
}

/// The doc comment [`release_source`] builds `pyspark/release.py` from.
const RELEASE_SOURCE: &str = r#""""The Spark this cluster's release ships, as its application is told it."""
"#;

/// The module this emulator runs the submitted application FROM, and the function in it the
/// engine calls.
pub const APPLICATION_ENTRY_MODULE: &str = "__emr_spark_application";
/// See [`APPLICATION_ENTRY_MODULE`].
pub const APPLICATION_ENTRY_FUNCTION: &str = "run";
/// The handler the interpreter resolves: `<module>.<function>`.
#[must_use]
pub fn application_entry_handler() -> String {
    format!("{APPLICATION_ENTRY_MODULE}.{APPLICATION_ENTRY_FUNCTION}")
}

/// The entry module: it runs the application the way `spark-submit` runs one — as the
/// `__main__` module, with the application's path and its own arguments on `sys.argv` — and
/// lets whatever the application raises travel out to the engine, which is what reports it.
const APPLICATION_ENTRY_SOURCE: &str = r#"
"""Runs a submitted Spark application, as the Python program it is."""

import sys


def run(event, context):
    # The command line a Spark application resolves its arguments off: its own path
    # followed by the arguments the submit passed it, which is what spark-submit hands it.
    sys.argv = [event["path"]] + list(event["arguments"])
    globals_of_main = {"__name__": "__main__", "__file__": event["path"]}
    exec(compile(event["source"], event["path"], "exec"), globals_of_main)
    return None
"#;

/// `pyspark`: the names an application imports from the package itself.
const PYSPARK_SOURCE: &str = r#"
"""The `pyspark` package, as this emulator supplies it to a submitted application."""

from pyspark.conf import SparkConf
from pyspark.context import SparkContext
from pyspark.rdd import RDD

__all__ = ["SparkConf", "SparkContext", "RDD"]
"#;

/// `pyspark.conf`: the configuration an application is submitted with, which it sets and
/// reads back.
const CONF_SOURCE: &str = r#"
"""The configuration of one Spark application."""


class SparkConf(object):
    """The key/value configuration `SparkConf` holds, as set and read back."""

    def __init__(self, loadDefaults=True, _jvm=None, _jconf=None):
        self._settings = {}

    def set(self, key, value):
        self._settings[str(key)] = str(value)
        return self

    def setAll(self, pairs):
        for key, value in pairs:
            self.set(key, value)
        return self

    def setIfMissing(self, key, value):
        if str(key) not in self._settings:
            self.set(key, value)
        return self

    def setAppName(self, value):
        return self.set("spark.app.name", value)

    def setMaster(self, value):
        return self.set("spark.master", value)

    def setSparkHome(self, value):
        return self.set("spark.home", value)

    def get(self, key, defaultValue=None):
        return self._settings.get(str(key), defaultValue)

    def getAll(self):
        return list(self._settings.items())

    def contains(self, key):
        return str(key) in self._settings

    def toDebugString(self):
        return "\n".join("%s=%s" % (key, value) for key, value in self.getAll())
"#;

/// `pyspark.filesystem`: the Hadoop-compatible filesystem a path resolves in.
///
/// This is where a Spark application's reads and writes LAND. A path in Amazon S3 is an
/// object of this emulator's own object store, reached with the AWS SDK the runtime supplies
/// the guest; every other filesystem is one this emulator holds nothing for, and a path in
/// it raises the way Hadoop raises for a scheme it has no filesystem for.
const FILESYSTEM_SOURCE: &str = r#"
"""The filesystems a Spark application's paths resolve in."""

# The schemes that name an object of Amazon S3: the EMR filesystem's own and the Hadoop
# S3A/S3N ones, which address the same store.
S3_SCHEMES = ("s3", "s3a", "s3n")

# How many bytes one write of this filesystem holds before it is flushed to the store. The
# bounds that matter are the interpreter's own, in host code; this one keeps a single stream
# from holding more than one object's worth of bytes.
MAX_OBJECT_BYTES = 64 * 1024 * 1024


class UnsupportedFileSystem(IOError):
    """A path whose filesystem this emulator does not have, as Hadoop words it."""


def _text(value):
    """A path argument as its text, whether it arrived as a string or as a Path."""
    if hasattr(value, "toString"):
        return value.toString()
    return str(value)


def split_uri(path):
    """The (scheme, authority, path) a URI names; the scheme is None for a bare path."""
    text = _text(path)
    if "://" not in text:
        return (None, None, text)
    scheme, rest = text.split("://", 1)
    if "/" not in rest:
        return (scheme, rest, "")
    authority, within = rest.split("/", 1)
    return (scheme, authority, within)


def object_of(path):
    """The (bucket, key) an Amazon S3 URI names, or None for any other path."""
    scheme, authority, within = split_uri(path)
    if scheme is None or scheme.lower() not in S3_SCHEMES:
        return None
    if not authority or not within:
        return None
    return (authority, within)


def _client():
    """The object store client, built through the SDK this runtime supplies the guest."""
    import boto3

    return boto3.client("s3")


class FileStatus(object):
    """One entry of a listing: the path, its length and whether it is a directory."""

    def __init__(self, path, length=0, directory=False):
        self._path = path
        self._length = length
        self._directory = directory

    def getPath(self):
        return self._path

    def getLen(self):
        return self._length

    def isDirectory(self):
        return self._directory

    def isDir(self):
        return self._directory

    def isFile(self):
        return not self._directory


class FSDataOutputStream(object):
    """An open write: the bytes are held until the stream is closed, then stored."""

    def __init__(self, bucket, key):
        self._bucket = bucket
        self._key = key
        self._buffer = bytearray()
        self._closed = False

    def write(self, data, offset=None, length=None):
        if self._closed:
            raise IOError("the stream for s3://%s/%s is closed" % (self._bucket, self._key))
        if isinstance(data, int):
            chunk = bytes(bytearray([data & 0xFF]))
        elif isinstance(data, str):
            chunk = data.encode("utf-8")
        else:
            chunk = bytes(bytearray(data))
        if offset is not None:
            end = len(chunk) if length is None else offset + length
            chunk = chunk[offset:end]
        if len(self._buffer) + len(chunk) > MAX_OBJECT_BYTES:
            raise IOError(
                "an object of at most %d bytes may be written" % (MAX_OBJECT_BYTES,)
            )
        self._buffer.extend(chunk)

    def writeBytes(self, text):
        self.write(text)

    def writeUTF(self, text):
        self.write(text)

    # Hadoop's flush and sync make written bytes visible to a reader. An object store has
    # one visible state, the stored object, so these hold the bytes until the close that
    # stores them.
    def flush(self):
        return None

    def hflush(self):
        return None

    def hsync(self):
        return None

    def sync(self):
        return None

    def close(self):
        if self._closed:
            return None
        self._closed = True
        _client().put_object(
            Bucket=self._bucket, Key=self._key, Body=bytes(self._buffer)
        )
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close()
        return False


class FSDataInputStream(object):
    """An open read of a stored object."""

    def __init__(self, body):
        self._body = body
        self._at = 0

    def read(self, size=None):
        if size is None or size < 0:
            chunk = self._body[self._at:]
            self._at = len(self._body)
            return chunk
        chunk = self._body[self._at:self._at + size]
        self._at += len(chunk)
        return chunk

    def readFully(self, *rest):
        return self.read()

    def available(self):
        return len(self._body) - self._at

    def close(self):
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.close()
        return False


class FileSystem(object):
    """The filesystem of one Amazon S3 bucket, as `Path.getFileSystem` hands one out."""

    def __init__(self, scheme, bucket):
        self._scheme = scheme
        self._bucket = bucket

    @staticmethod
    def get(uri, conf=None):
        """The filesystem a URI resolves in. `FileSystem.get(uri, conf)` in Hadoop."""
        named = object_of(uri)
        if named is None:
            scheme, authority, within = split_uri(uri)
            if scheme is not None and scheme.lower() in S3_SCHEMES and authority:
                return FileSystem(scheme.lower(), authority)
            raise UnsupportedFileSystem("No FileSystem for scheme: %s" % (scheme,))
        bucket, _ = named
        scheme, _, _ = split_uri(uri)
        return FileSystem(scheme.lower(), bucket)

    def getUri(self):
        return "%s://%s" % (self._scheme, self._bucket)

    def _key(self, path):
        named = object_of(path)
        if named is None:
            raise UnsupportedFileSystem("No FileSystem for path: %s" % (_text(path),))
        bucket, key = named
        if bucket != self._bucket:
            raise UnsupportedFileSystem(
                "Wrong FS: %s, expected %s" % (_text(path), self.getUri())
            )
        return key

    def create(self, path, overwrite=True, *rest, **named):
        key = self._key(path)
        if not overwrite and self.exists(path):
            raise IOError("%s already exists" % (_text(path),))
        return FSDataOutputStream(self._bucket, key)

    def append(self, path, *rest, **named):
        raise UnsupportedFileSystem(
            "Append is not supported by %s" % (self.getUri(),)
        )

    def open(self, path, bufferSize=None):
        return FSDataInputStream(read_bytes(path))

    def exists(self, path):
        key = self._key(path)
        client = _client()
        try:
            client.head_object(Bucket=self._bucket, Key=key)
            return True
        except Exception:
            listed = client.list_objects_v2(
                Bucket=self._bucket, Prefix=key.rstrip("/") + "/", MaxKeys=1
            )
            return int(listed.get("KeyCount") or 0) > 0

    def delete(self, path, recursive=False):
        key = self._key(path)
        client = _client()
        if not recursive:
            client.delete_object(Bucket=self._bucket, Key=key)
            return True
        for status in self.listStatus(path):
            client.delete_object(Bucket=self._bucket, Key=self._key(status.getPath()))
        return True

    def mkdirs(self, path, *rest):
        # An object store holds objects, not directories: the prefix a path names exists
        # once an object is written under it, so there is nothing to create here.
        self._key(path)
        return True

    def listStatus(self, path):
        prefix = self._key(path).rstrip("/")
        listed = _client().list_objects_v2(Bucket=self._bucket, Prefix=prefix)
        statuses = []
        for entry in listed.get("Contents") or []:
            statuses.append(
                FileStatus(
                    "%s://%s/%s" % (self._scheme, self._bucket, entry["Key"]),
                    int(entry.get("Size") or 0),
                )
            )
        return statuses

    def getFileStatus(self, path):
        key = self._key(path)
        read = _client().head_object(Bucket=self._bucket, Key=key)
        return FileStatus(
            "%s://%s/%s" % (self._scheme, self._bucket, key),
            int(read.get("ContentLength") or 0),
        )

    def close(self):
        return None


def read_bytes(path):
    """The bytes of the object `path` names."""
    named = object_of(path)
    if named is None:
        scheme, _, _ = split_uri(path)
        raise UnsupportedFileSystem("No FileSystem for scheme: %s" % (scheme,))
    bucket, key = named
    read = _client().get_object(Bucket=bucket, Key=key)
    body = read["Body"].read()
    if isinstance(body, str):
        return body.encode("utf-8")
    return bytes(bytearray(body))


def read_text(path):
    """The object `path` names, decoded as the text a line-based read sees."""
    return read_bytes(path).decode("utf-8")


def write_text(path, text):
    """Store `text` as the object `path` names, through one open stream."""
    stream = FileSystem.get(path).create(path, True)
    stream.write(text)
    stream.close()
    return None
"#;

/// `pyspark.rdd`: the dataset itself — where every transformation and action is COMPUTED.
const RDD_SOURCE: &str = r#"
"""The resilient distributed dataset of an application, computed in this process."""

from pyspark import filesystem

# The name Spark writes a successful output's marker under, beside the part files of the
# output it wrote.
SUCCESS_MARKER = "_SUCCESS"


def slices(items, count):
    """`items` split into `count` partitions, the way a collection is sliced into them."""
    values = list(items)
    count = max(int(count), 1)
    size = len(values) // count
    extra = len(values) % count
    partitions = []
    start = 0
    for index in range(count):
        end = start + size + (1 if index < extra else 0)
        partitions.append(values[start:end])
        start = end
    return partitions


def _folded(operator, values):
    """`operator` folded over `values`, left to right, as reduce does."""
    total = values[0]
    for value in values[1:]:
        total = operator(total, value)
    return total


class RDD(object):
    """One dataset: its partitions, and the computations over them."""

    def __init__(self, partitions):
        self._partitions = [list(partition) for partition in partitions]

    # --- the partitions, and the elements across them -----------------------------------

    def _elements(self):
        return [value for partition in self._partitions for value in partition]

    def getNumPartitions(self):
        return len(self._partitions)

    def glom(self):
        return RDD([[list(partition)] for partition in self._partitions])

    def repartition(self, numPartitions):
        return RDD(slices(self._elements(), numPartitions))

    def coalesce(self, numPartitions, shuffle=False):
        return self.repartition(numPartitions)

    # --- transformations: the application's own functions, called on its own data --------

    def map(self, f, preservesPartitioning=False):
        return RDD([[f(value) for value in part] for part in self._partitions])

    def flatMap(self, f, preservesPartitioning=False):
        return RDD(
            [[out for value in part for out in f(value)] for part in self._partitions]
        )

    def filter(self, f):
        return RDD([[value for value in part if f(value)] for part in self._partitions])

    def mapPartitions(self, f, preservesPartitioning=False):
        return RDD([list(f(iter(part))) for part in self._partitions])

    def mapPartitionsWithIndex(self, f, preservesPartitioning=False):
        return RDD(
            [list(f(index, iter(part))) for index, part in enumerate(self._partitions)]
        )

    def keyBy(self, f):
        return self.map(lambda value: (f(value), value))

    def mapValues(self, f):
        return self.map(lambda pair: (pair[0], f(pair[1])))

    def flatMapValues(self, f):
        return self.flatMap(lambda pair: [(pair[0], out) for out in f(pair[1])])

    def keys(self):
        return self.map(lambda pair: pair[0])

    def values(self):
        return self.map(lambda pair: pair[1])

    def distinct(self, numPartitions=None):
        kept = []
        for value in self._elements():
            if value not in kept:
                kept.append(value)
        return RDD(slices(kept, numPartitions or self.getNumPartitions()))

    def union(self, other):
        return RDD(self._partitions + other._partitions)

    def intersection(self, other):
        held = other._elements()
        return RDD(
            slices(
                [value for value in self.distinct()._elements() if value in held],
                self.getNumPartitions(),
            )
        )

    def subtract(self, other, numPartitions=None):
        held = other._elements()
        return RDD(
            slices(
                [value for value in self._elements() if value not in held],
                numPartitions or self.getNumPartitions(),
            )
        )

    def zip(self, other):
        return RDD(
            slices(
                list(zip(self._elements(), other._elements())), self.getNumPartitions()
            )
        )

    def zipWithIndex(self):
        return RDD(
            slices(
                [(value, index) for index, value in enumerate(self._elements())],
                self.getNumPartitions(),
            )
        )

    def groupByKey(self, numPartitions=None):
        order = []
        grouped = {}
        for key, value in self._elements():
            if key not in grouped:
                grouped[key] = []
                order.append(key)
            grouped[key].append(value)
        return RDD(
            slices(
                [(key, grouped[key]) for key in order],
                numPartitions or self.getNumPartitions(),
            )
        )

    def reduceByKey(self, func, numPartitions=None):
        return self.groupByKey(numPartitions).mapValues(
            lambda values: _folded(func, values)
        )

    def _partitioned_by_key(self):
        """The values of each key, per partition: [{key: [value, ...]}, ...]."""
        per_partition = []
        for partition in self._partitions:
            grouped = {}
            for key, value in partition:
                grouped.setdefault(key, []).append(value)
            per_partition.append(grouped)
        return per_partition

    def _keys_in_order(self, per_partition):
        """Every key the dataset holds, in the order the partitions first carry it."""
        order = []
        for grouped in per_partition:
            for key in grouped:
                if key not in order:
                    order.append(key)
        return order

    def foldByKey(self, zeroValue, func, numPartitions=None):
        def folded(values):
            total = zeroValue
            for value in values:
                total = func(total, value)
            return total

        return self.groupByKey(numPartitions).mapValues(folded)

    def aggregateByKey(self, zeroValue, seqFunc, combFunc, numPartitions=None):
        # The values of a key WITHIN one partition are folded with seqFunc from the zero
        # value, and those per-partition results are combined with combFunc — so an
        # application whose two functions differ gets what the real action computes.
        per_partition = self._partitioned_by_key()
        aggregated = []
        for key in self._keys_in_order(per_partition):
            combined = None
            for grouped in per_partition:
                if key not in grouped:
                    continue
                folded = zeroValue
                for value in grouped[key]:
                    folded = seqFunc(folded, value)
                combined = folded if combined is None else combFunc(combined, folded)
            aggregated.append((key, combined))
        return RDD(slices(aggregated, numPartitions or self.getNumPartitions()))

    def combineByKey(self, createCombiner, mergeValue, mergeCombiners, numPartitions=None):
        # The same two stages: a combiner per key per partition, then the combiners merged
        # across partitions.
        per_partition = self._partitioned_by_key()
        combined_pairs = []
        for key in self._keys_in_order(per_partition):
            combined = None
            for grouped in per_partition:
                if key not in grouped:
                    continue
                values = grouped[key]
                partial = createCombiner(values[0])
                for value in values[1:]:
                    partial = mergeValue(partial, value)
                combined = partial if combined is None else mergeCombiners(combined, partial)
            combined_pairs.append((key, combined))
        return RDD(slices(combined_pairs, numPartitions or self.getNumPartitions()))

    def sortByKey(self, ascending=True, numPartitions=None, keyfunc=None):
        chosen = keyfunc or (lambda key: key)
        ordered = sorted(
            self._elements(), key=lambda pair: chosen(pair[0]), reverse=not ascending
        )
        return RDD(slices(ordered, numPartitions or self.getNumPartitions()))

    def sortBy(self, keyfunc, ascending=True, numPartitions=None):
        ordered = sorted(self._elements(), key=keyfunc, reverse=not ascending)
        return RDD(slices(ordered, numPartitions or self.getNumPartitions()))

    def join(self, other, numPartitions=None):
        right = other._elements()
        joined = []
        for key, value in self._elements():
            for other_key, other_value in right:
                if other_key == key:
                    joined.append((key, (value, other_value)))
        return RDD(slices(joined, numPartitions or self.getNumPartitions()))

    # Caching holds a dataset in memory for the next computation. Every dataset here is
    # already the computed partitions, so these answer the dataset itself.
    def cache(self):
        return self

    def persist(self, storageLevel=None):
        return self

    def unpersist(self, blocking=False):
        return self

    def setName(self, name):
        self._name = str(name)
        return self

    # --- actions ------------------------------------------------------------------------

    def collect(self):
        return list(self._elements())

    def toLocalIterator(self):
        return iter(self._elements())

    def count(self):
        return len(self._elements())

    def isEmpty(self):
        return not self._elements()

    def first(self):
        values = self._elements()
        if not values:
            raise ValueError("RDD is empty")
        return values[0]

    def take(self, num):
        return self._elements()[: max(int(num), 0)]

    def takeOrdered(self, num, key=None):
        return sorted(self._elements(), key=key)[: max(int(num), 0)]

    def top(self, num, key=None):
        return sorted(self._elements(), key=key, reverse=True)[: max(int(num), 0)]

    def reduce(self, f):
        values = self._elements()
        if not values:
            raise ValueError("Can not reduce() empty RDD")
        return _folded(f, values)

    def fold(self, zeroValue, op):
        # "Aggregate the elements of each partition, and then the results for all the
        # partitions", from the zero value in both halves.
        total = zeroValue
        for partition in self._partitions:
            folded = zeroValue
            for value in partition:
                folded = op(folded, value)
            total = op(total, folded)
        return total

    def aggregate(self, zeroValue, seqOp, combOp):
        # Each partition is folded with seqOp from the zero value, and the partition
        # results are then combined with combOp from it, which is what the action does.
        total = zeroValue
        for partition in self._partitions:
            folded = zeroValue
            for value in partition:
                folded = seqOp(folded, value)
            total = combOp(total, folded)
        return total

    def sum(self):
        return sum(self._elements())

    def max(self, key=None):
        return max(self._elements(), key=key) if key else max(self._elements())

    def min(self, key=None):
        return min(self._elements(), key=key) if key else min(self._elements())

    def mean(self):
        values = self._elements()
        if not values:
            raise ValueError("RDD is empty")
        return float(sum(values)) / len(values)

    def stats(self):
        values = self._elements()
        return {"count": len(values), "sum": sum(values)}

    def countByKey(self):
        counts = {}
        for key, _ in self._elements():
            counts[key] = counts.get(key, 0) + 1
        return counts

    def countByValue(self):
        counts = {}
        for value in self._elements():
            counts[value] = counts.get(value, 0) + 1
        return counts

    def collectAsMap(self):
        return dict(self._elements())

    def lookup(self, key):
        return [value for held, value in self._elements() if held == key]

    def foreach(self, f):
        for value in self._elements():
            f(value)
        return None

    def foreachPartition(self, f):
        for partition in self._partitions:
            f(iter(partition))
        return None

    def saveAsTextFile(self, path, compressionCodecClass=None):
        """Write one part file per partition under `path`, beside the success marker.

        That is the output layout Spark writes: the path names a directory and each
        partition is a file in it, so a reader of the output reads the parts. Every write
        goes through the filesystem the path resolves in, so the objects are real.
        """
        prefix = filesystem._text(path).rstrip("/")
        for index, partition in enumerate(self._partitions):
            lines = "".join("%s\n" % (value,) for value in partition)
            filesystem.write_text("%s/part-%05d" % (prefix, index), lines)
        filesystem.write_text("%s/%s" % (prefix, SUCCESS_MARKER), "")
        return None
"#;

/// `pyspark.context`: the context an application hands its work to.
const CONTEXT_SOURCE: &str = r#"
"""The Spark context of one application: where its datasets come from."""

from pyspark import filesystem
from pyspark.conf import SparkConf
from pyspark.rdd import RDD, slices

# How many partitions a collection is sliced into when the application names no number.
# CHOSEN: the work runs in this one process, so the default parallelism is one.
DEFAULT_PARALLELISM = 1


class Broadcast(object):
    """A value broadcast to the work: the value itself, read back through `value`."""

    def __init__(self, value):
        self._value = value

    @property
    def value(self):
        return self._value

    def unpersist(self, blocking=False):
        return None

    def destroy(self):
        return None


class Accumulator(object):
    """A value the work adds to, as `Accumulator` holds one."""

    def __init__(self, value):
        self._value = value

    def add(self, term):
        self._value = self._value + term
        return None

    @property
    def value(self):
        return self._value

    def __iadd__(self, term):
        self.add(term)
        return self


class SparkContext(object):
    """The application's context. One per application, as `getOrCreate` hands it out."""

    _active = None

    def __init__(self, master=None, appName=None, conf=None, **named):
        self._conf = conf or SparkConf()
        if master:
            self._conf.setMaster(master)
        if appName:
            self._conf.setAppName(appName)
        self._stopped = False
        SparkContext._active = self

    @classmethod
    def getOrCreate(cls, conf=None):
        if cls._active is None or cls._active._stopped:
            cls._active = SparkContext(conf=conf)
        return cls._active

    @property
    def appName(self):
        return self._conf.get("spark.app.name", "")

    @property
    def master(self):
        return self._conf.get("spark.master", "local")

    @property
    def applicationId(self):
        return self._conf.get("spark.app.id", "local-application")

    @property
    def defaultParallelism(self):
        return DEFAULT_PARALLELISM

    def getConf(self):
        return self._conf

    def setLogLevel(self, logLevel):
        return None

    def setJobGroup(self, groupId, description, interruptOnCancel=False):
        return None

    def setLocalProperty(self, key, value):
        return None

    def parallelize(self, c, numSlices=None):
        return RDD(slices(c, numSlices or DEFAULT_PARALLELISM))

    def emptyRDD(self):
        return RDD([[]])

    def range(self, start, end=None, step=1, numSlices=None):
        if end is None:
            start, end = 0, start
        return self.parallelize(list(range(start, end, step)), numSlices)

    def textFile(self, name, minPartitions=None, use_unicode=True):
        """The lines of the object `name` names, as a dataset of them."""
        text = filesystem.read_text(name)
        lines = text.split("\n")
        if lines and lines[-1] == "":
            lines = lines[:-1]
        return RDD(slices(lines, minPartitions or DEFAULT_PARALLELISM))

    def wholeTextFiles(self, path, minPartitions=None, use_unicode=True):
        return RDD(
            slices(
                [(filesystem._text(path), filesystem.read_text(path))],
                minPartitions or DEFAULT_PARALLELISM,
            )
        )

    def broadcast(self, value):
        return Broadcast(value)

    def accumulator(self, value, accum_param=None):
        return Accumulator(value)

    def stop(self):
        self._stopped = True
        if SparkContext._active is self:
            SparkContext._active = None
        return None

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.stop()
        return False
"#;

/// `pyspark.jvm`: the gateway a Spark application reaches Java classes through.
///
/// A PySpark application reaches the Hadoop filesystem the way it does on a cluster —
/// `spark._jvm.org.apache.hadoop.fs.Path`, `FileSystem.get`, the configuration off
/// `spark._jsc.hadoopConfiguration()` — so the gateway hands out exactly the classes this
/// emulator can perform, and a class it does not have raises the way an absent attribute
/// does.
const JVM_SOURCE: &str = r#"
"""The Java classes a Spark application reaches through its gateway."""

from pyspark import filesystem
from pyspark.filesystem import FileSystem, FileStatus

# The Spark the application is running on: the version the cluster's own release ships, as
# the per-run module `pyspark.release` carries it.
from pyspark.release import VERSION as SPARK_VERSION


class URI(object):
    """`java.net.URI`: the address a filesystem is resolved from."""

    def __init__(self, text):
        self._text = str(text)

    def toString(self):
        return self._text

    def getScheme(self):
        scheme, _, _ = filesystem.split_uri(self._text)
        return scheme

    def getHost(self):
        _, authority, _ = filesystem.split_uri(self._text)
        return authority

    def getPath(self):
        _, _, within = filesystem.split_uri(self._text)
        return "/" + within

    def __str__(self):
        return self._text


class Configuration(object):
    """`org.apache.hadoop.conf.Configuration`: the settings a filesystem is opened with."""

    def __init__(self, loadDefaults=True):
        self._settings = {}

    def get(self, name, defaultValue=None):
        return self._settings.get(str(name), defaultValue)

    def set(self, name, value):
        self._settings[str(name)] = str(value)
        return None

    def setBoolean(self, name, value):
        self.set(name, "true" if value else "false")
        return None

    def getBoolean(self, name, defaultValue=False):
        held = self.get(name)
        if held is None:
            return defaultValue
        return held == "true"

    def unset(self, name):
        self._settings.pop(str(name), None)
        return None


class Path(object):
    """`org.apache.hadoop.fs.Path`: a path in a filesystem."""

    def __init__(self, first, child=None):
        base = filesystem._text(first)
        if child is None:
            self._text = base
        else:
            self._text = "%s/%s" % (base.rstrip("/"), filesystem._text(child).lstrip("/"))

    def toString(self):
        return self._text

    def toUri(self):
        return URI(self._text)

    def getName(self):
        return self._text.rstrip("/").rsplit("/", 1)[-1]

    def getParent(self):
        trimmed = self._text.rstrip("/")
        if "/" not in trimmed:
            return None
        return Path(trimmed.rsplit("/", 1)[0])

    def getFileSystem(self, conf=None):
        return FileSystem.get(self._text, conf)

    def isAbsolute(self):
        return "://" in self._text or self._text.startswith("/")

    def __str__(self):
        return self._text


class JavaPackage(object):
    """One package of the gateway: the classes under it, by name."""

    def __init__(self, name, members):
        self._name = name
        self._members = members

    def __getattr__(self, name):
        members = object.__getattribute__(self, "_members")
        if name in members:
            return members[name]
        held = object.__getattribute__(self, "_name")
        raise AttributeError(
            "%s.%s is not a class this emulator supplies" % (held, name)
        )


def gateway():
    """The root of the gateway: what `spark._jvm` answers."""
    hadoop_fs = JavaPackage(
        "org.apache.hadoop.fs",
        {"Path": Path, "FileSystem": FileSystem, "FileStatus": FileStatus},
    )
    hadoop_conf = JavaPackage(
        "org.apache.hadoop.conf", {"Configuration": Configuration}
    )
    hadoop = JavaPackage(
        "org.apache.hadoop", {"fs": hadoop_fs, "conf": hadoop_conf}
    )
    apache = JavaPackage("org.apache", {"hadoop": hadoop})
    org = JavaPackage("org", {"apache": apache})
    net = JavaPackage("java.net", {"URI": URI})
    java = JavaPackage("java", {"net": net})
    return JavaPackage("jvm", {"org": org, "java": java})


class JavaSparkContext(object):
    """`spark._jsc`: the Java context an application reads its Hadoop settings off."""

    def __init__(self, conf=None):
        self._configuration = Configuration()

    def hadoopConfiguration(self):
        return self._configuration

    def version(self):
        return SPARK_VERSION

    def stop(self):
        return None
"#;

/// `pyspark.sql`: the session an application starts and the dataset of rows it hands out.
const SQL_SOURCE: &str = r#"
"""The Spark session of one application, and the rows a dataset of them holds."""

from pyspark import filesystem
from pyspark.conf import SparkConf
from pyspark.context import SparkContext
from pyspark.jvm import JavaSparkContext, SPARK_VERSION, gateway
from pyspark.rdd import RDD, slices


class Row(object):
    """One row: its fields by name and by position, as `Row` holds them."""

    def __init__(self, *values, **named):
        if values and named:
            raise ValueError("a Row is built from positions or from names, not both")
        if named:
            self._fields = list(named.keys())
            self._values = [named[field] for field in self._fields]
        else:
            self._fields = ["_%d" % (index + 1,) for index in range(len(values))]
            self._values = list(values)

    def __getitem__(self, item):
        if isinstance(item, str):
            return self._values[self._fields.index(item)]
        return self._values[item]

    def __getattr__(self, name):
        fields = object.__getattribute__(self, "_fields")
        if name in fields:
            return object.__getattribute__(self, "_values")[fields.index(name)]
        raise AttributeError("no such field: %s" % (name,))

    def __len__(self):
        return len(self._values)

    def __iter__(self):
        return iter(self._values)

    def __eq__(self, other):
        if isinstance(other, Row):
            return self._fields == other._fields and self._values == other._values
        return tuple(self._values) == other

    def __repr__(self):
        pairs = ", ".join(
            "%s=%r" % (field, value) for field, value in zip(self._fields, self._values)
        )
        return "Row(%s)" % (pairs,)

    def asDict(self, recursive=False):
        return dict(zip(self._fields, self._values))


class DataFrameWriter(object):
    """Where a dataset of rows is WRITTEN: through the filesystem its path resolves in."""

    def __init__(self, frame):
        self._frame = frame
        self._mode = "errorifexists"
        self._format = None

    def mode(self, saveMode):
        self._mode = str(saveMode).lower()
        return self

    def format(self, source):
        self._format = str(source).lower()
        return self

    def option(self, key, value):
        return self

    def options(self, **named):
        return self

    def _rendered(self, kind):
        if kind == "json":
            import json

            return [
                json.dumps(row.asDict()) if isinstance(row, Row) else json.dumps(row)
                for row in self._frame.collect()
            ]
        return [
            ",".join(str(value) for value in row) for row in self._frame.collect()
        ]

    def _write(self, kind, path):
        lines = self._rendered(kind)
        prefix = filesystem._text(path).rstrip("/")
        filesystem.write_text(
            "%s/part-00000" % (prefix,), "".join("%s\n" % (line,) for line in lines)
        )
        filesystem.write_text("%s/_SUCCESS" % (prefix,), "")
        return None

    def csv(self, path, **named):
        return self._write("csv", path)

    def text(self, path, **named):
        return self._write("text", path)

    def json(self, path, **named):
        return self._write("json", path)

    def save(self, path=None, **named):
        return self._write(self._format or "csv", path)


class DataFrame(object):
    """A dataset of rows: the rows themselves, and the computations over them."""

    def __init__(self, rows, columns):
        self._rows = list(rows)
        self._columns = list(columns)

    @property
    def columns(self):
        return list(self._columns)

    @property
    def rdd(self):
        return RDD(slices(self._rows, 1))

    @property
    def write(self):
        return DataFrameWriter(self)

    def collect(self):
        return list(self._rows)

    def count(self):
        return len(self._rows)

    def take(self, num):
        return self._rows[: max(int(num), 0)]

    def first(self):
        if not self._rows:
            return None
        return self._rows[0]

    def toLocalIterator(self):
        return iter(self._rows)

    def select(self, *columns):
        names = [str(column) for column in columns]
        for name in names:
            if name not in self._columns:
                raise ValueError("no such column: %s" % (name,))
        rows = [
            Row(**dict((name, row[name]) for name in names)) for row in self._rows
        ]
        return DataFrame(rows, names)

    def show(self, n=20, truncate=True, vertical=False):
        for row in self._rows[: max(int(n), 0)]:
            print(row)
        return None


class SparkSession(object):
    """The session an application runs in, as `SparkSession.builder` hands one out."""

    class Builder(object):
        """The builder of one session: what it is named, where it runs, how configured."""

        def __init__(self):
            self._conf = SparkConf()

        def appName(self, name):
            self._conf.setAppName(name)
            return self

        def master(self, master):
            self._conf.setMaster(master)
            return self

        def config(self, key=None, value=None, conf=None, **named):
            if conf is not None:
                self._conf.setAll(conf.getAll())
            if key is not None:
                self._conf.set(key, value)
            for held, setting in named.items():
                self._conf.set(held, setting)
            return self

        def enableHiveSupport(self):
            return self.config("spark.sql.catalogImplementation", "hive")

        def getOrCreate(self):
            if SparkSession._active is None or SparkSession._active._stopped:
                SparkSession._active = SparkSession(
                    SparkContext.getOrCreate(self._conf)
                )
            return SparkSession._active

    _active = None

    def __init__(self, sparkContext):
        self._context = sparkContext
        self._jvm_gateway = gateway()
        self._java_context = JavaSparkContext(sparkContext.getConf())
        self._stopped = False
        SparkSession._active = self

    @property
    def sparkContext(self):
        return self._context

    @property
    def version(self):
        return SPARK_VERSION

    @property
    def conf(self):
        return self._context.getConf()

    # The gateway and the Java context an application reaches the Hadoop filesystem
    # through, under the names PySpark gives them.
    @property
    def _jvm(self):
        return self._jvm_gateway

    @property
    def _jsc(self):
        return self._java_context

    @property
    def _sc(self):
        return self._context

    def createDataFrame(self, data, schema=None, **named):
        rows = list(data)
        columns = []
        if isinstance(schema, (list, tuple)):
            columns = [str(column) for column in schema]
        elif rows:
            first = rows[0]
            if isinstance(first, Row):
                columns = list(first._fields)
            elif isinstance(first, dict):
                columns = list(first.keys())
            else:
                columns = ["_%d" % (index + 1,) for index in range(len(first))]
        built = []
        for row in rows:
            if isinstance(row, Row):
                built.append(row)
            elif isinstance(row, dict):
                built.append(Row(**row))
            else:
                built.append(Row(**dict(zip(columns, list(row)))))
        return DataFrame(built, columns)

    def range(self, start, end=None, step=1, numPartitions=None):
        if end is None:
            start, end = 0, start
        rows = [Row(**{"id": value}) for value in range(start, end, step)]
        return DataFrame(rows, ["id"])

    def stop(self):
        self._stopped = True
        self._context.stop()
        if SparkSession._active is self:
            SparkSession._active = None
        return None

    def newSession(self):
        return SparkSession(self._context)

    def __enter__(self):
        return self

    def __exit__(self, kind, value, traceback):
        self.stop()
        return False


# `SparkSession.builder`, the attribute an application starts from.
SparkSession.builder = SparkSession.Builder()

__all__ = ["DataFrame", "Row", "SparkSession"]
"#;
