# Medición HTTP reutilizable

`compare.py` usa solo Python 3.11+ estándar; funciona sin instalar Atlas, PostGIS ni paquetes.
Una corrida = una URL/caso/revisión/dataset, GET o POST JSON (`--body archivo.json`).
No crea datos ni configura servidores. El cierre Python conserva las métricas existentes;
este instrumento se usará al disponer del servidor Rust. GeoServer queda para después.

```bash
python3 benchmarks/compare.py --engine rust --revision COMMIT --dataset DATOS_SHA256 \
  --case geojson-page --url 'http://127.0.0.1:8080/api/v1/datasets/demo/items?limit=100' \
  --expected expected-page.json --samples 1000 --warmup 10 --concurrency 4 \
  --output rust-page-c4.jsonl
python3 -m unittest tests.benchmarks.test_compare -v
```

Preparar `expected-page.json` desde un fixture conocido/verificado, **no aprenderlo de la
respuesta a medir**. JSON completo debe coincidir: objetos ignoran orden de claves/espacios;
arrays, tipos y representación numérica se conservan. Para tiles, `--kind bytes --expected
tile.mvt` exige igualdad binaria; verificar antes su cobertura con un decoder independiente.
Si dos motores serializan distinto, revisar equivalencia y usar expectativas propias; no
interpretar un mismatch como velocidad. Registrar por separado páginas y descargas completas.

El JSONL guarda metadatos, hashes de script/expectativa/body, warmups, muestras y resumen final.
Solo `finished=true` y todos los samples `valid` habilitan comparar. Errores, cuerpos distintos,
truncamientos y rechazos 413/429/503 no entran en p50/p95 ni throughput válido. Salida 0 éxito;
1 corrida fallida; interrupción deja `finished=false` o ausencia del resumen final. No sobrescribe.
Latencia hasta cuerpo recibido (máximo 32 MiB), validación fuera del tiempo HTTP. Throughput
incluye validación/escritura del cliente; conexiones nuevas por petición, carga cerrada por
samples, timeout de socket por operación, sin reintentos/redirecciones ni p99. No mide CPU/RSS
del servidor ni certifica saturación/ausencia de fugas; ampliar solo cuando la comparación lo exija.

## Motor Rust (#230)

Con `--engine rust`, `--case` debe ser un caso canónico de `RUST_CASES` (`catalog`, `geojson-page`,
`geojson-query` con `--body`, `mvt-tile` con `--kind mvt`, `tilejson`, `export`) y la ruta de
`--url` debe coincidir con la API REST de [api.md](../docs/rust/api.md) (se admite el prefijo de
`public_url`); un caso mal configurado falla antes de medir. `--kind mvt` decodifica protobuf con
biblioteca estándar: ignora el orden de features, claves y valores del encoder, pero conserva capas,
extent, ids, tipos, propiedades y coordenadas cuantizadas exactas. Un sample inválido registra
`mismatch`: `count`, `ids`, `geometry_or_properties` o `content`; los errores registran además el
`code` del cuerpo `{code,message,request_id}`. Preparar la expectativa desde PostGIS
(`json_build_object`/`ST_AsMVT` con `ST_TileEnvelope`, extent 4096, buffer 64), nunca desde Rust.
Compilar con `cargo build --release --locked` y pasar `--pin` con el binario, `atlas.toml` y el
script del oráculo: se guardan sus SHA-256 en los metadatos. `--server-cgroup` y `--db-cgroup`
(directorios cgroup v2: el binario en `systemd-run --user --scope`, PostGIS en
`system.slice/docker-<id>.scope`) guardan CPU y memoria actual/pico al inicio (`resources_before`) y
en el resumen (`resources`), también al interrumpir (`null` si falta un cgroup); el informe resta
ambos. Cliente y servidor no comparten cgroup; PostgreSQL incluye caché/shmem.

Repetir tres veces a c1/c4 con mismos datos/índices, propiedades, filtros, orden, página, caché,
hardware y límites. Guardar junto al JSONL las expectativas, seed/hash de datos, configuración,
versiones y comandos. Usar fixture pública sin tokens/secretos en URL/body. Python HTTP nuevo
y Rust release son comparables entre sí; el informe histórico ASGI no lo es directamente.

## Referencia GeoServer (#209)

`benchmarks/geoserver/` fija GeoServer 3.0.2 por digest y la extensión vector-tiles de la misma
versión por SHA-256 en el build. El servicio `geoserver` del perfil `bench` usa 2 CPU, 1 GiB y heap
fijo de 640 MiB en loopback. `python3 -m benchmarks.geoserver.setup --database atlas_bench_... \
--schema ... --layer nombre=tabla --output geoserver.json`, con `GEOSERVER_ADMIN_PASSWORD` y
`ATLAS_BENCH_DB_PASSWORD` por entorno, rechaza bases fuera de `atlas_bench_*` y salidas existentes.
Recrea solo el workspace `atlas_bench` (pool máximo 10, bbox exacto, capas EPSG:3857), quita la
caché GWC de cada capa y espera a WFS. El manifiesto guarda versiones, hashes del Dockerfile y de la
configuración, JDK, heap y cuotas, nunca contraseñas.

`python3 -m benchmarks.geoserver.cases --case CASO --layer capa [--query query.json] [--tile z/x/y]`
imprime una línea JSON con la URL para `compare.py --engine geoserver`, o `status: n/a` con motivo
para `catalog` y `tilejson`, que no tienen equivalente y se informan como N/A. La Query de Atlas se
traduce solo en el generador: `filter` a ECQL, `bbox` y polígonos en EPSG:4326, `properties` a
`propertyName`, `sort` más el id como desempate final a `sortBy`, `limit`/`offset` a
`count`/`startIndex`; `export` es WFS completo sin paginar. `mvt-tile` pide WMS GetMap MVT sin GWC
sobre la envolvente XYZ: 256 px dan extent 4096 y `buffer=4` equivale a 64 unidades. Medido con
GeoServer 3.0.2: el extent MVT es 16 veces el ancho en píxeles (4096 px darían 65536) y el buffer
WMS escala igual (`buffer=64` a 256 px agrega features fuera del margen de `ST_AsMVT`).

Conteo y precisión se alinean con Rust: cada capa lleva `skipNumberMatched` (sin COUNT, sin
`numberMatched`) y `numDecimals=9`, como `ST_AsGeoJSON`. `equivalence.py` compara por contenido:
`geojson()` quita la envoltura de GeoServer (`timeStamp`, `totalFeatures`, `crs`, `bbox`), toma el
id de `capa.N` y quita el id repetido en properties; `difference()` nombra `count`, `ids`,
`properties` o `geometry` (tolerancia 1.5e-9 grados: GeoTools y PROJ difieren en el último dígito y
ese cuanto puede pasar de 1e-9 en flotante); `coverage()` compara capas, extent, número e ids de
features de un MVT contra `ST_AsMVT` e informa aparte las geometrías cuantizadas distintas, que no
hacen fallar la cobertura. Con `ATLAS_GEOSERVER_URL`, `GEOSERVER_ADMIN_PASSWORD` y
`ATLAS_TEST_DATABASE_URL`, `test_geoserver.py` verifica contra GeoServer y PostGIS reales una página
con bbox, sort y properties y la cobertura de una tesela no vacía; sin ellas se salta. `compare.py`
no cambia: integrar estas normalizaciones a la medición es parte de #231.

## Matriz y GeoServer (#231)

`python3 -m benchmarks.matrix --plan plan.json --output-dir evidencia/` expande un plan
(`engine`, `revision`, `dataset`, `concurrency`, `repetitions`, `options` comunes y `cases` con
`case`, `url`, `expected` y `options` propias; cada opción es un flag de `compare.py`, y una lista
repite el flag) en un JSONL por caso, concurrencia y repetición. Reanuda: salta las corridas con
resumen `finished` y se detiene ante un archivo incompleto, que hay que apartar a mano. Sale con 1
si alguna corrida falla.

`--engine geoserver` exige `--kind gs-json` (contenido por `equivalence.geojson` y `difference`) o
`gs-mvt` (cobertura de capas, extent e ids por `coverage`; la geometría cuantizada no invalida). El
resumen incluye `p99_ms` solo con 1.000 o más muestras válidas; si no, `null`.

`compare.py --duration SEGUNDOS` (hasta 7200) reemplaza `--samples`: los trabajadores repiten
peticiones hasta el plazo y la corrida es válida si todas las muestras lo son. La matriz lo usa
para las corridas de 60 s (`options: {"duration": 60}`).

`python3 -m benchmarks.soak --plan soak.json --output soak.jsonl` ejecuta la mezcla del soak:
plan con `engine`, `revision`, `dataset`, `concurrency`, `duration`, `seed`, `warmup`, cgroups
opcionales y `mix` (cada entrada con `case`, `url`, `expected`, `kind`, `body` y `weight`; 40/40/20
para página, teselas y filtros). El caso de cada petición sale de la semilla y su número, así que
la secuencia es reproducible. Cada muestra lleva su `case` y el resumen trae `by_case`.

## Planes de #231

`benchmarks/plans/` trae `matrix-{rust,geoserver}.json` (c1/c4/c16, tres repeticiones, 60 s por
corrida) y `soak-{rust,geoserver}.json` (30 min a c16, 40% página, 40% teselas, 20% filtros,
semilla 231). Cada `FILL:` se reemplaza antes de medir con el commit o digest, el SHA-256 de los
datos, las URL (en GeoServer, la salida de `benchmarks.geoserver.cases`), las expectativas desde
PostGIS y los cgroups. Se copian a `docs/rust/evidence/` con los valores y ahí se ejecutan
(`benchmarks.matrix`, luego `benchmarks.soak`), un motor a la vez. `catalog` y `tilejson` no existen
en GeoServer y se informan como N/A. El plan de Python queda fuera hasta fijar su SHA y sus rutas.
