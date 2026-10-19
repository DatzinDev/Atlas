"""Smoke test: the package is installed and importable."""

import atlas


def test_package_is_importable() -> None:
    assert atlas.__name__ == "atlas"
