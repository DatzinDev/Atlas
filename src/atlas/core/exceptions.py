"""The complete taxonomy of failures Atlas can report, and their wire format.

Every error a caller can observe lives here, so a host application needs a single
`except AtlasError` to cover the library. Each class carries the HTTP status it maps to and a
stable machine-readable `code`; the `code` is public contract and is never renamed without a
version bump.
"""

from __future__ import annotations

from typing import Any


class AtlasError(Exception):
    """Base class for everything Atlas raises on purpose.

    Subclasses only override `status_code` and `code`. The message is written for a human reading
    the response, and `details` carries structured, client-safe context for it — the layer that was
    requested, the CRS that was rejected, the limit that was exceeded.
    """

    status_code: int = 500
    code: str = "atlas_error"

    def __init__(self, message: str, **details: Any) -> None:
        super().__init__(message)
        self.message = message
        self.details: dict[str, Any] = details


class LayerNotFound(AtlasError):
    """No layer is registered under the requested name."""

    status_code = 404
    code = "layer_not_found"


class ServiceDisabled(AtlasError):
    """The layer exists, but it does not publish the requested service (WMS or WFS)."""

    status_code = 404
    code = "service_disabled"


class UnsupportedCRS(AtlasError):
    """The requested CRS is not in `SUPPORTED_CRS`."""

    status_code = 400
    code = "unsupported_crs"


class UnsupportedFormat(AtlasError):
    """The requested output format is not one the service produces."""

    status_code = 400
    code = "unsupported_format"


class InvalidParameter(AtlasError):
    """A request parameter is missing, malformed or outside its allowed range.

    Covers what OGC reports as `MissingParameterValue` and `InvalidParameterValue`: a `GetMap`
    without `BBOX`, a non-numeric `WIDTH`, an unknown `SERVICE`. `details` should name the
    parameter so the client can point at it.
    """

    status_code = 400
    code = "invalid_parameter"


class InvalidFilter(AtlasError):
    """The filter could not be parsed, or names a column outside the layer's whitelist."""

    status_code = 400
    code = "invalid_filter"


class InvalidGeometry(AtlasError):
    """The supplied geometry is malformed or not usable for the operation."""

    status_code = 400
    code = "invalid_geometry"


class QueryTooLarge(AtlasError):
    """Serving the request would exceed a limit set in `AtlasConfig`."""

    status_code = 413
    code = "query_too_large"


class QueryTimeout(AtlasError):
    """The query exceeded `AtlasConfig.query_timeout`."""

    status_code = 504
    code = "query_timeout"


class PermissionDenied(AtlasError):
    """The host application's `PermissionChecker` refused the operation."""

    status_code = 403
    code = "permission_denied"


class DatabaseError(AtlasError):
    """PostGIS could not serve the request.

    The driver's message is deliberately withheld from the client: psycopg reports schema, table
    and column names, and sometimes literal values from the query, which is internal topology no
    caller is entitled to. It is kept on `technical_message` for the operator's log instead.
    """

    status_code = 500
    code = "database_error"

    #: What the client is told, whatever the driver said.
    PUBLIC_MESSAGE = "the database could not complete the request"

    def __init__(self, message: str, **details: Any) -> None:
        super().__init__(self.PUBLIC_MESSAGE, **details)
        self.technical_message = message


def error_response(exc: AtlasError) -> dict[str, Any]:
    """Render an error as the JSON body every Atlas endpoint returns.

    The shape never varies, so a client can read `error.code` without first checking what failed.
    `details` is always a dict, empty when there is no extra context.
    """
    return {
        "error": {
            "code": exc.code,
            "message": exc.message,
            "details": dict(exc.details),
        }
    }
