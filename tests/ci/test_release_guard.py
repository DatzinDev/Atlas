"""A manual release must bind the version, protected tag and exact workflow commit."""

import importlib.util
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[2] / ".github/scripts/validate_release.py"
spec = importlib.util.spec_from_file_location("validate_release", SCRIPT)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


@pytest.fixture
def release_repo(tmp_path, monkeypatch):
    monkeypatch.chdir(tmp_path)
    subprocess.run(["git", "init", "-q"], check=True)
    (tmp_path / "pyproject.toml").write_text('[project]\nversion = "1.2.3"\n')
    subprocess.run(["git", "add", "pyproject.toml"], check=True)
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-qm",
            "test fixture",
        ],
        check=True,
    )
    subprocess.run(["git", "tag", "v1.2.3"], check=True)
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    for key, value in {
        "GITHUB_REPOSITORY": "DatzinDev/Atlas",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_SHA": sha,
        "RELEASE_TAG": "v1.2.3",
    }.items():
        monkeypatch.setenv(key, value)


def test_exact_main_release_is_allowed(release_repo):
    guard.validate_release()


@pytest.mark.parametrize(
    "key,value",
    [
        ("GITHUB_REPOSITORY", "attacker/Atlas"),
        ("GITHUB_REF", "refs/heads/develop"),
        ("GITHUB_EVENT_NAME", "pull_request"),
        ("GITHUB_SHA", "0" * 40),
        ("RELEASE_TAG", "v9.9.9"),
        ("RELEASE_TAG", "v1.2.3; echo injected"),
    ],
)
def test_invalid_release_is_rejected(release_repo, monkeypatch, key, value):
    monkeypatch.setenv(key, value)
    with pytest.raises(ValueError):
        guard.validate_release()


def test_tag_cannot_refer_to_an_older_commit(release_repo, monkeypatch):
    subprocess.run(
        [
            "git",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "later commit",
        ],
        check=True,
    )
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    monkeypatch.setenv("GITHUB_SHA", sha)
    with pytest.raises(ValueError):
        guard.validate_release()
