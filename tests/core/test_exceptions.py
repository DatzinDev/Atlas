"""Contract tests for the Atlas exception hierarchy and its wire format."""

from __future__ import annotations

import pytest

from atlas.core.exceptions import (
    AtlasError,
    DatabaseError,
    InvalidFilter,
    InvalidGeometry,
    InvalidParameter,
    LayerNotFound,
    PermissionDenied,
    QueryTimeout,
    QueryTooLarge,
    ServiceDisabled,
    UnsupportedCRS,
    UnsupportedFormat,
    error_response,
)

# The public contract: every entry is (class, status_code, code). Changing a `code` here is a
# breaking change for every client that branches on it, so this table is the canonical reference.
CONTRACT: list[tuple[type[AtlasError], int, str]] = [
    (AtlasError, 500, "atlas_error"),
    (LayerNotFound, 404, "layer_not_found"),
    (ServiceDisabled, 404, "service_disabled"),
    (UnsupportedCRS, 400, "unsupported_crs"),
    (UnsupportedFormat, 400, "unsupported_format"),
    (InvalidParameter, 400, "invalid_parameter"),
    (InvalidFilter, 400, "invalid_filter"),
    (InvalidGeometry, 400, "invalid_geometry"),
    (QueryTooLarge, 413, "query_too_large"),
    (QueryTimeout, 504, "query_timeout"),
    (DatabaseError, 500, "database_error"),
    (PermissionDenied, 403, "permission_denied"),
]

SUBCLASSES = [entry[0] for entry in CONTRACT if entry[0] is not AtlasError]


def every_error_class(root: type[AtlasError] = AtlasError) -> list[type[AtlasError]]:
    """Walk the whole hierarchy, however deep it grows."""
    found = [root]
    for subclass in root.__subclasses__():
        found.extend(every_error_class(subclass))
    return found


class TestHierarchy:
    @pytest.mark.parametrize(("error", "status_code", "code"), CONTRACT)
    def test_the_declared_status_code_and_code(
        self, error: type[AtlasError], status_code: int, code: str
    ) -> None:
        assert error.status_code == status_code
        assert error.code == code

    @pytest.mark.parametrize("error", SUBCLASSES)
    def test_every_error_descends_from_atlas_error(self, error: type[AtlasError]) -> None:
        """One `except AtlasError` in the host app must catch everything Atlas raises."""
        assert issubclass(error, AtlasError)

    def test_no_two_errors_share_a_code(self) -> None:
        """`code` is what a client branches on: a duplicate makes two failures indistinguishable."""
        codes = [error.code for error in every_error_class()]
        assert len(codes) == len(set(codes)), f"duplicated codes: {sorted(codes)}"

    def test_the_contract_table_covers_the_whole_hierarchy(self) -> None:
        """A new exception must be added to this test's table, not slipped in unnoticed."""
        assert set(every_error_class()) == {entry[0] for entry in CONTRACT}

    def test_an_atlas_error_is_a_plain_exception(self) -> None:
        with pytest.raises(Exception, match="boom"):
            raise AtlasError("boom")


class TestMessageAndDetails:
    def test_the_message_is_kept_and_is_the_string_form(self) -> None:
        error = LayerNotFound("unknown layer 'roads'")
        assert error.message == "unknown layer 'roads'"
        assert str(error) == "unknown layer 'roads'"

    def test_details_default_to_an_empty_dict(self) -> None:
        """Never `None`: callers can always iterate the details without a guard."""
        assert LayerNotFound("nope").details == {}

    def test_keyword_arguments_become_details(self) -> None:
        error = UnsupportedCRS("EPSG:9999 is not supported", requested=9999, supported=[4326, 3857])
        assert error.details == {"requested": 9999, "supported": [4326, 3857]}

    def test_a_request_parameter_error_names_the_offending_parameter(self) -> None:
        """WMS and WFS both report which parameter was wrong, never just "bad request"."""
        error = InvalidParameter("WIDTH must be a positive integer", parameter="WIDTH", value="abc")
        assert error.details == {"parameter": "WIDTH", "value": "abc"}


class TestErrorResponse:
    def test_the_shape_is_always_the_same(self) -> None:
        assert error_response(LayerNotFound("unknown layer 'roads'", layer="roads")) == {
            "error": {
                "code": "layer_not_found",
                "message": "unknown layer 'roads'",
                "details": {"layer": "roads"},
            }
        }

    def test_details_are_an_empty_dict_when_there_are_none(self) -> None:
        """The key is always present, so a client never has to test for its existence."""
        assert error_response(QueryTimeout("the query took too long"))["error"]["details"] == {}

    def test_the_response_does_not_alias_the_exception_details(self) -> None:
        """Mutating the payload must not rewrite the exception a caller is still logging."""
        error = InvalidFilter("bad filter", field="name")
        error_response(error)["error"]["details"]["field"] = "tampered"
        assert error.details == {"field": "name"}


class TestDatabaseErrorDoesNotLeak:
    # A psycopg message names schemas, tables, columns and sometimes literal values from the query.
    DRIVER_MESSAGE = 'relation "internal_billing.secrets" does not exist\nLINE 1: SELECT ...'

    def test_the_driver_message_is_not_the_public_message(self) -> None:
        error = DatabaseError(self.DRIVER_MESSAGE)
        assert self.DRIVER_MESSAGE not in error.message
        assert "internal_billing" not in error.message

    def test_the_driver_message_never_reaches_the_response(self) -> None:
        payload = error_response(DatabaseError(self.DRIVER_MESSAGE, layer="roads"))
        assert "internal_billing" not in repr(payload)

    def test_the_driver_message_is_kept_for_the_log(self) -> None:
        """Hidden from the client, not thrown away: the operator still needs it."""
        assert DatabaseError(self.DRIVER_MESSAGE).technical_message == self.DRIVER_MESSAGE

    def test_public_details_still_reach_the_response(self) -> None:
        payload = error_response(DatabaseError(self.DRIVER_MESSAGE, layer="roads"))
        assert payload["error"]["details"] == {"layer": "roads"}
