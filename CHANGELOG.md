# Changelog

All notable changes to this project are documented in this file.

The format follows [Conventional Changelog](https://github.com/conventional-changelog/conventional-changelog),
derived from [Conventional Commits](https://www.conventionalcommits.org/), and this project adheres
to [Semantic Versioning](https://semver.org/). Only `feat`, `fix`, `perf` and breaking changes are
listed here; see [docs/changelog.md](docs/changelog.md) for when and how to update this file.

## Unreleased

> Atlas has not been released yet. Everything before this section is pre-release scaffolding —
> packaging, CI, the development PostGIS environment, tests and documentation — which the
> convention keeps out of the changelog. The public API lands in `0.1.0`; see
> [docs/roadmap.md](docs/roadmap.md).

### Features

* **postgis:** add the async connection pool with statement timeout and streaming ([37ad744](https://github.com/zamax14/Atlas/commit/37ad744))
* **core:** add the Atlas exception hierarchy and the error response format ([87464f4](https://github.com/zamax14/Atlas/commit/87464f4))
* **core:** make AtlasConfig frozen and reject unknown fields ([da193f5](https://github.com/zamax14/Atlas/commit/da193f5))
* **core:** reject a pool minimum size above the maximum ([19082cf](https://github.com/zamax14/Atlas/commit/19082cf))
* **core:** add AtlasConfig with query limits and connection options ([67501e5](https://github.com/zamax14/Atlas/commit/67501e5))

### Bug Fixes

* **dev:** make the seeded layers overlap and look like a real network ([218a849](https://github.com/zamax14/Atlas/commit/218a849))
* **ci:** skip install, lint, types and tests while pyproject.toml does not exist ([7e64c12](https://github.com/zamax14/Atlas/commit/7e64c12))
