// SPDX-License-Identifier: CC0-1.0

//! The PAGINATOR of the AWS SDK a Python handler imports: `botocore.paginate`, the module
//! `client.get_paginator(...)` answers out of.
//!
//! The managed `python3.x` runtimes ship the whole AWS SDK for Python, so a handler that has
//! to read a listing longer than one page asks the client for a paginator and iterates it
//! rather than writing the continuation loop itself. This module is that interface: a
//! `Paginator` for one operation, and the `PageIterator` its `paginate(...)` answers with.
//!
//! What it does and does not do:
//!
//! * One page is ONE call of the operation, made through the client's own method — so the
//!   request, the signature, the destination and every bound on it are the ones every other
//!   call of that client is made under ([`super::super::aws_api`]), and a page's contents are
//!   the service's own answer. Nothing here reads or writes state of its own.
//! * WHICH members are driven is the called operation's own model's
//!   ([`super::super::node_sdk_exports`], reached through [`super::pagination_config`]): the
//!   input member the continuation value is sent back in, the output member it arrives in, the
//!   input member a page's size is asked for in, and the output members a page's results
//!   arrive in. An operation the model does not mark as paginated has no paginator at all,
//!   which is what `OperationNotPageableError` says.
//! * The iteration stops when the answer carries no continuation value, which is what the
//!   service itself says about there being a further page. A `PaginationConfig` the caller
//!   passes is honoured the way the SDK honours it: `PageSize` asks the operation for a page
//!   of that size, `MaxItems` bounds how many items the iterator serves — truncating the last
//!   page and answering a `resume_token` that resumes exactly there — and `StartingToken`
//!   resumes from one.
//! * `build_full_result` joins the pages' own result members into one document, and names the
//!   resume token under `NextToken` when the iteration stopped early, which is the document a
//!   caller passes back as the next `StartingToken`.

/// The modules of the paginator interface, in the order they are put on the import path.
#[must_use]
pub(super) fn modules() -> Vec<(&'static str, &'static str)> {
    vec![("botocore/paginate.py", PAGINATE_SOURCE)]
}

/// `botocore.paginate`: the paginator of one operation, and the pages it serves.
const PAGINATE_SOURCE: &str = r#"
"""Paging one operation through, as the SDK's own paginator does."""

import base64
import json

from botocore.exceptions import PaginationError


class TokenEncoder:
    """The text a resume token travels as: its document, as base64 of the JSON of it."""

    def encode(self, token):
        return base64.b64encode(
            json.dumps(token, separators=(",", ":"), sort_keys=True).encode("utf-8")
        ).decode("utf-8")


class TokenDecoder:
    """The document a resume token carries, read back from that text."""

    def decode(self, token):
        return json.loads(base64.b64decode(token.encode("utf-8")).decode("utf-8"))


# The member a resume token carries the number of items already served out of the page it
# resumes at, which is what makes a token taken mid-page resume mid-page.
_TRUNCATE_AMOUNT = "boto_truncate_amount"


def _as_list(value):
    """A setting that may name one member or several, as the list of them."""
    if value is None:
        return []
    if isinstance(value, (list, tuple)):
        return list(value)
    return [value]


def _search(document, path):
    """The value `path` names in `document`, or `None` when it holds none.

    A path is the member names from the root of the answer, so a member of a nested
    structure is reached the way the model states it.
    """
    held = document
    for step in str(path).split("."):
        if not isinstance(held, dict):
            return None
        held = held.get(step)
        if held is None:
            return None
    return held


class Paginator:
    """The paginator of ONE operation: the method one page is a call of, and the members
    the model says that operation is paged through."""

    def __init__(self, method, operation_name, pagination_config):
        self._method = method
        self._operation_name = operation_name
        self._pagination_cfg = pagination_config
        self._input_token = _as_list(pagination_config.get("input_token"))
        self._output_token = _as_list(pagination_config.get("output_token"))
        self._more_results = pagination_config.get("more_results")
        self._result_keys = _as_list(pagination_config.get("result_key"))
        self._limit_key = pagination_config.get("limit_key")

    @property
    def result_keys(self):
        return list(self._result_keys)

    def paginate(self, **kwargs):
        """The pages of this operation for one input, served as they are asked for."""
        paging = self._extract_paging_params(kwargs)
        return PageIterator(
            self._method,
            self._input_token,
            self._output_token,
            self._more_results,
            self._result_keys,
            self._limit_key,
            paging["MaxItems"],
            paging["StartingToken"],
            paging["PageSize"],
            kwargs,
        )

    def _extract_paging_params(self, kwargs):
        config = kwargs.pop("PaginationConfig", {})
        max_items = config.get("MaxItems")
        if max_items is not None:
            max_items = int(max_items)
        page_size = config.get("PageSize")
        if page_size is not None:
            if self._limit_key is None:
                raise PaginationError(
                    message="PageSize parameter is not supported for the pagination "
                    "interface for this operation."
                )
            page_size = int(page_size)
        return {
            "MaxItems": max_items,
            "StartingToken": config.get("StartingToken"),
            "PageSize": page_size,
        }


class PageIterator:
    """The pages of one paginated call, each the operation's own answer."""

    def __init__(
        self,
        method,
        input_token,
        output_token,
        more_results,
        result_keys,
        limit_key,
        max_items,
        starting_token,
        page_size,
        op_kwargs,
    ):
        self._method = method
        self._input_token = input_token
        self._output_token = output_token
        self._more_results = more_results
        self._result_keys = result_keys
        self._limit_key = limit_key
        self._max_items = max_items
        self._starting_token = starting_token
        self._page_size = page_size
        self._op_kwargs = op_kwargs
        self._resume_token = None
        self._token_encoder = TokenEncoder()
        self._token_decoder = TokenDecoder()

    @property
    def result_keys(self):
        return list(self._result_keys)

    @property
    def resume_token(self):
        """The token the iteration stopped at, or `None` when it reached the end."""
        return self._resume_token

    @resume_token.setter
    def resume_token(self, value):
        if isinstance(value, dict):
            self._resume_token = self._token_encoder.encode(value)
            return
        self._resume_token = value

    def __iter__(self):
        current = dict(self._op_kwargs)
        if self._page_size is not None:
            current[self._limit_key] = self._page_size
        token, skipping = self._starting_point()
        if self._starting_token is not None:
            # Only a resumption states the token of the FIRST call; without one the input
            # is the caller's own, down to a continuation value it passed itself.
            self._inject_token(current, token)
        served = 0
        while True:
            page = self._method(**current)
            # How many of THIS page's items a resumption had already served, which a token
            # taken inside it has to carry as well.
            skipped = 0
            if skipping:
                self._skip(page, skipping)
                skipped = skipping
                skipping = 0
            carried = self._items(page)
            if self._max_items is not None and served + carried > self._max_items:
                keep = self._max_items - served
                self._truncate(page, keep)
                self.resume_token = self._resumed_at(token, keep + skipped)
                yield page
                return
            yield page
            served += carried
            next_token = self._next_token(page)
            if all(value is None for value in next_token.values()):
                return
            if self._max_items is not None and served >= self._max_items:
                self.resume_token = next_token
                return
            token = next_token
            self._inject_token(current, token)

    def build_full_result(self):
        """Every page's own results, joined into the one document the SDK joins them into.

        The members joined are the operation's own result members; a token the iteration
        stopped at is named under `NextToken`, which is what a caller passes back as the
        next `StartingToken`.
        """
        joined = {}
        for page in self:
            for key in self._result_keys:
                value = page.get(key)
                if value is None:
                    continue
                held = joined.get(key)
                if held is None:
                    joined[key] = value
                elif isinstance(held, list) and isinstance(value, list):
                    held.extend(value)
                elif isinstance(held, dict) and isinstance(value, dict):
                    held.update(value)
                else:
                    joined[key] = held + value
        if self.resume_token is not None:
            joined["NextToken"] = self.resume_token
        return joined

    def _starting_point(self):
        """The token the first call carries, and how many of its items were already served.

        A token taken mid-page names the page it was taken in and how far into it the
        caller had read, so resuming asks for that page again and drops what it already saw.
        """
        empty = dict((name, None) for name in self._input_token)
        if self._starting_token is None:
            return empty, 0
        try:
            carried = self._token_decoder.decode(self._starting_token)
        except (ValueError, TypeError):
            raise PaginationError(
                message="Invalid token: %s" % (self._starting_token,)
            )
        if not isinstance(carried, dict):
            raise PaginationError(message="Invalid token: %s" % (self._starting_token,))
        skipping = int(carried.pop(_TRUNCATE_AMOUNT, 0) or 0)
        token = dict(empty)
        token.update(carried)
        return token, skipping

    def _resumed_at(self, token, served_from_page):
        """The token resuming the page `token` fetched, after its first items."""
        resumed = dict(token)
        resumed[_TRUNCATE_AMOUNT] = served_from_page
        return resumed

    def _inject_token(self, kwargs, token):
        for name, value in token.items():
            if value is None:
                kwargs.pop(name, None)
            else:
                kwargs[name] = value

    def _next_token(self, page):
        """The input members the NEXT call carries, read off this page's own answer.

        A page carrying no value in the member the continuation value arrives in is the last
        one, which is what stops the iteration.
        """
        if self._more_results is not None and not _search(page, self._more_results):
            return dict((name, None) for name in self._input_token)
        tokens = {}
        for at, name in enumerate(self._input_token):
            carried = None
            if at < len(self._output_token):
                carried = _search(page, self._output_token[at])
            tokens[name] = carried if carried else None
        return tokens

    def _items(self, page):
        """How many items this page carries in the operation's own result members."""
        held = 0
        for key in self._result_keys:
            value = page.get(key)
            if isinstance(value, list):
                held += len(value)
        return held

    def _truncate(self, page, keep):
        """Keep the first `keep` items of this page's results, dropping the rest."""
        remaining = max(int(keep), 0)
        for key in self._result_keys:
            value = page.get(key)
            if not isinstance(value, list):
                continue
            page[key] = value[:remaining]
            remaining -= len(page[key])

    def _skip(self, page, skipping):
        """Drop the first `skipping` items of this page, which a caller already read."""
        remaining = max(int(skipping), 0)
        for key in self._result_keys:
            value = page.get(key)
            if not isinstance(value, list):
                continue
            dropped = min(remaining, len(value))
            page[key] = value[dropped:]
            remaining -= dropped
"#;
