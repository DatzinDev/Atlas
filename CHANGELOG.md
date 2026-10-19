# Changelog

All notable changes to this project are documented in this file.

The format follows [Conventional Changelog](https://github.com/conventional-changelog/conventional-changelog),
derived from [Conventional Commits](https://www.conventionalcommits.org/), and this project adheres
to [Semantic Versioning](https://semver.org/). Only `feat`, `fix`, `perf` and breaking changes are
listed here; see [docs/changelog.md](docs/changelog.md) for when and how to update this file.

## Unreleased

## 0.1.0 - 2026-10-19

### Features

* **exports:** build Shapefile ZIP with explicit format limits ([2d8bc87](https://github.com/DatzinDev/Atlas/commit/2d8bc87))
* **exports:** build GeoPackage downloads with temporary file cleanup ([833f7f3](https://github.com/DatzinDev/Atlas/commit/833f7f3))
* **api:** stream filtered layer downloads with format selection ([d67f7c3](https://github.com/DatzinDev/Atlas/commit/d67f7c3))
* **exports:** stream UTF8 CSV with geometry modes ([49bab18](https://github.com/DatzinDev/Atlas/commit/49bab18))
* **exports:** stream GeoJSON feature collections ([f75f539](https://github.com/DatzinDev/Atlas/commit/f75f539))
* **exports:** define exporter protocol and format registry ([df13b1a](https://github.com/DatzinDev/Atlas/commit/df13b1a))
* **api:** export map images with request local styles ([b9381ba](https://github.com/DatzinDev/Atlas/commit/b9381ba))
* **wms:** mount logged WMS operations in Atlas router ([68d1fe7](https://github.com/DatzinDev/Atlas/commit/68d1fe7))
* **wms:** publish capabilities for supported map operations ([9d87b7b](https://github.com/DatzinDev/Atlas/commit/9d87b7b))
* **wms:** query feature attributes with pixel scaled tolerance ([4170088](https://github.com/DatzinDev/Atlas/commit/4170088))
* **wms:** add authorized GetMap with PNG and JPEG output ([fa62e14](https://github.com/DatzinDev/Atlas/commit/fa62e14))
* **rendering:** draw styled geometries with Pillow ([c76969c](https://github.com/DatzinDev/Atlas/commit/c76969c))
* **postgis:** fetch bounded WKB geometries for rendering ([7500f38](https://github.com/DatzinDev/Atlas/commit/7500f38))
* **rendering:** add renderer protocol and world to pixel mapping ([569b01d](https://github.com/DatzinDev/Atlas/commit/569b01d))

* **wfs:** dispatch read-only requests with permissions and operation logs ([7328c65](https://github.com/DatzinDev/Atlas/commit/7328c65))
* **wfs:** publish escaped read-only capabilities metadata ([c664b25](https://github.com/DatzinDev/Atlas/commit/c664b25))
* **wfs:** describe feature types with JSON Schema metadata ([aec0445](https://github.com/DatzinDev/Atlas/commit/aec0445))
* **wfs:** select validated properties while preserving feature ids ([65aeeca](https://github.com/DatzinDev/Atlas/commit/65aeeca))
* **wfs:** stream bounded GeoJSON feature collections ([33f8abc](https://github.com/DatzinDev/Atlas/commit/33f8abc))
* **wfs:** enforce bounded pagination and request aliases ([3dc5058](https://github.com/DatzinDev/Atlas/commit/3dc5058))
* **postgis:** filter bounding boxes without wrapping indexed geometry ([0611a89](https://github.com/DatzinDev/Atlas/commit/0611a89))
* **wfs:** validate typed filters and compile whitelisted predicates ([56114e0](https://github.com/DatzinDev/Atlas/commit/56114e0))
* **postgis:** compose bounded feature queries with column whitelists ([f9a6a32](https://github.com/DatzinDev/Atlas/commit/f9a6a32))
* **core:** report operation timing through an isolated logging hook ([102a664](https://github.com/DatzinDev/Atlas/commit/102a664))
* **core:** add the supported CRS registry and axis aliases ([0811462](https://github.com/DatzinDev/Atlas/commit/0811462))
* **layers:** enforce host permissions on layer metadata ([cec04ac](https://github.com/DatzinDev/Atlas/commit/cec04ac))
* **api:** expose layer catalog and metadata endpoints ([ff4b569](https://github.com/DatzinDev/Atlas/commit/ff4b569))
* **integration:** embed Atlas in FastAPI with managed lifecycle ([8bdf9da](https://github.com/DatzinDev/Atlas/commit/8bdf9da))
* **layers:** build layers from PostGIS introspection ([f267e56](https://github.com/DatzinDev/Atlas/commit/f267e56))
* **styles:** add validated geometry styles and color helpers ([ed5ead2](https://github.com/DatzinDev/Atlas/commit/ed5ead2))
* **layers:** add the in-memory layer registry ([adbe68f](https://github.com/DatzinDev/Atlas/commit/adbe68f))
* **layers:** add the layer model and service flags ([2d04e54](https://github.com/DatzinDev/Atlas/commit/2d04e54))
* **postgis:** add estimated layer extents with exact fallback ([d9e528a](https://github.com/DatzinDev/Atlas/commit/d9e528a))
* **postgis:** add spatial table and view introspection ([85a5125](https://github.com/DatzinDev/Atlas/commit/85a5125))
* **postgis:** add the async connection pool with statement timeout and streaming ([37ad744](https://github.com/zamax14/Atlas/commit/37ad744))
* **core:** add the Atlas exception hierarchy and the error response format ([87464f4](https://github.com/zamax14/Atlas/commit/87464f4))
* **core:** make AtlasConfig frozen and reject unknown fields ([da193f5](https://github.com/zamax14/Atlas/commit/da193f5))
* **core:** reject a pool minimum size above the maximum ([19082cf](https://github.com/zamax14/Atlas/commit/19082cf))
* **core:** add AtlasConfig with query limits and connection options ([67501e5](https://github.com/zamax14/Atlas/commit/67501e5))

### Bug Fixes

* **wfs:** release prefetched cursors when clients disconnect ([389673b](https://github.com/DatzinDev/Atlas/commit/389673b))
* **dev:** make the seeded layers overlap and look like a real network ([218a849](https://github.com/zamax14/Atlas/commit/218a849))
* **ci:** skip install, lint, types and tests while pyproject.toml does not exist ([7e64c12](https://github.com/zamax14/Atlas/commit/7e64c12))
