"""Configure the pinned GeoServer reference against a disposable benchmark database."""

import argparse
import base64
import hashlib
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

WORKSPACE = "atlas_bench"
STORE = "bench"
DOCKERFILE = Path(__file__).with_name("Dockerfile")
STORE_DEFAULTS = {
    "dbtype": "postgis",
    "Expose primary keys": "true",
    "max connections": "10",
    "Loose bbox": "false",
    "Estimated extends": "false",
}


class Rest:
    def __init__(self, url, user, password, timeout=30):
        self.url = url.rstrip("/")
        self.auth = "Basic " + base64.b64encode(f"{user}:{password}".encode()).decode()
        self.timeout = timeout

    def call(self, method, path, body=None, ok=(200, 201)):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.url + path, data=data, method=method)
        request.add_header("Authorization", self.auth)
        request.add_header("Accept", "application/json")
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                status, payload = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, payload = error.code, error.read()
        if status not in ok:
            raise RuntimeError(f"{method} {path} returned {status}")
        return json.loads(payload) if payload and payload[:1] in b"{[" else None


def check_database(name):
    if not name.startswith("atlas_bench_"):
        raise ValueError("refusing a database outside atlas_bench_*")


def store_parameters(args):
    return {
        **STORE_DEFAULTS,
        "host": args.db_host,
        "port": str(args.db_port),
        "database": args.database,
        "schema": args.schema,
        "user": args.db_user,
    }


def wait_ready(rest, timeout):
    deadline = time.monotonic() + timeout
    while True:
        try:
            return rest.call("GET", "/rest/about/version.json")
        except (OSError, RuntimeError):
            if time.monotonic() > deadline:
                raise
            time.sleep(2)


def configure(rest, parameters, password, layers):
    rest.call("DELETE", f"/rest/workspaces/{WORKSPACE}?recurse=true", ok=(200, 404))
    rest.call("POST", "/rest/workspaces", {"workspace": {"name": WORKSPACE}})
    entries = [{"@key": k, "$": v} for k, v in {**parameters, "passwd": password}.items()]
    store = {"dataStore": {"name": STORE, "connectionParameters": {"entry": entries}}}
    rest.call("POST", f"/rest/workspaces/{WORKSPACE}/datastores", store)
    for name, table in layers:
        feature = {
            "name": name,
            "nativeName": table,
            "srs": "EPSG:3857",
            "projectionPolicy": "FORCE_DECLARED",
        }
        path = f"/rest/workspaces/{WORKSPACE}/datastores/{STORE}/featuretypes"
        rest.call("POST", path, {"featureType": feature})
        rest.call("DELETE", f"/gwc/rest/layers/{WORKSPACE}:{name}", ok=(200, 404))
        rest.call(
            "GET",
            f"/{WORKSPACE}/ows?service=WFS&version=2.0.0"
            f"&request=DescribeFeatureType&typeNames={WORKSPACE}:{name}"
            "&outputFormat=application/json",
        )


def container(name):
    if not name:
        return None
    out = subprocess.run(["docker", "inspect", name], check=True, capture_output=True, text=True)
    info = json.loads(out.stdout)[0]
    env = dict(item.split("=", 1) for item in info["Config"]["Env"])
    return {
        "image": info["Config"]["Image"],
        "image_id": info["Image"],
        "java_version": env.get("JAVA_VERSION"),
        "java_opts": env.get("EXTRA_JAVA_OPTS"),
        "nano_cpus": info["HostConfig"]["NanoCpus"],
        "memory": info["HostConfig"]["Memory"],
    }


def manifest(versions, plugin, parameters, layers, runtime):
    config = {"datastore": parameters, "layers": [list(layer) for layer in layers]}
    canonical = json.dumps(config, sort_keys=True, separators=(",", ":")).encode()
    return {
        "engine": "geoserver",
        "versions": {r["@name"]: str(r["Version"]) for r in versions["about"]["resource"]},
        "vectortiles": plugin,
        "dockerfile_sha256": hashlib.sha256(DOCKERFILE.read_bytes()).hexdigest(),
        "container": runtime,
        "workspace": WORKSPACE,
        "config": config,
        "config_sha256": hashlib.sha256(canonical).hexdigest(),
    }


def layer(value):
    name, sep, table = value.partition("=")
    if not sep or not name or not table:
        raise argparse.ArgumentTypeError("use name=table")
    return name, table


def parse(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://127.0.0.1:8081/geoserver")
    parser.add_argument("--database", required=True)
    parser.add_argument("--schema", required=True)
    parser.add_argument("--layer", type=layer, action="append", required=True)
    parser.add_argument("--db-host", default="postgis")
    parser.add_argument("--db-port", type=int, default=5432)
    parser.add_argument("--db-user", default="atlas")
    parser.add_argument("--container", default="atlas-geoserver-1")
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args(argv)


def main(argv=None):
    args = parse(argv)
    check_database(args.database)
    if args.output.exists():
        raise SystemExit(f"{args.output} exists; refusing to overwrite")
    names = ("GEOSERVER_ADMIN_PASSWORD", "ATLAS_BENCH_DB_PASSWORD")
    secrets = {k: os.environ.get(k) for k in names}
    missing = [k for k, v in secrets.items() if not v]
    if missing:
        raise SystemExit(f"missing environment: {', '.join(missing)}")
    rest = Rest(
        args.url,
        os.environ.get("GEOSERVER_ADMIN_USER", "admin"),
        secrets["GEOSERVER_ADMIN_PASSWORD"],
    )
    versions = wait_ready(rest, 120)
    plugin = rest.call("GET", "/rest/about/manifest.json?manifest=gs-vectortiles.*")
    parameters = store_parameters(args)
    configure(rest, parameters, secrets["ATLAS_BENCH_DB_PASSWORD"], args.layer)
    data = manifest(
        versions,
        plugin["about"]["resource"]["Implementation-Version"],
        parameters,
        args.layer,
        container(args.container),
    )
    cached = rest.call("GET", "/gwc/rest/layers.json") or []
    data["gwc_layers"] = [name for name in cached if name.startswith(WORKSPACE + ":")]
    if data["gwc_layers"]:
        raise SystemExit("GWC still caches benchmark layers")
    args.output.write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
