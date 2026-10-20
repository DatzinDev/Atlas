"""Fail closed unless a manual release identifies the current main commit."""

import os
import subprocess
import tomllib
from pathlib import Path


def validate_release() -> None:
    for key, expected in {
        "GITHUB_REPOSITORY": "DatzinDev/Atlas",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
    }.items():
        if os.environ.get(key) != expected:
            raise ValueError(f"invalid release context: {key}")
    version = tomllib.loads(Path("pyproject.toml").read_text())["project"]["version"]
    tag = os.environ.get("RELEASE_TAG", "")
    if tag != f"v{version}":
        raise ValueError("release tag must match the package version")
    tagged_commit = subprocess.check_output(
        ["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"], text=True
    ).strip()
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if tagged_commit != head or head != os.environ.get("GITHUB_SHA"):
        raise ValueError("release tag must point to the workflow main commit")


if __name__ == "__main__":
    validate_release()
