# Benchmark vectorial reproducible (#140)

Mide Atlas como librería ASGI, sin red TCP, proxy, navegador ni workers ASGI separados.
La matriz usa 10k/100k/1M filas **por tipo** (punto/línea/polígono), ventanas XYZ selectivas/globales, PNG 256×256, 3 repeticiones, concurrencia 1/4, 10 warmups y 100 muestras totales por lote válido.
La primera petición por caso se registra aparte: **no representa disco ni PostgreSQL fríos**; la generación, ANALYZE y los oráculos calientan los datos. No se vacían cachés del host.

## Reproducción

Ejecutar desde checkout Git en Linux. Instalar `pip install -e ".[dev,export]"`; crear un PostGIS descartable 16/3.4 con base `atlas_bench_vector` y extensión PostGIS. Nunca apuntar a una base de aplicación: el generador rechaza nombres fuera de `atlas_bench_*` antes de DDL y usa/retira únicamente un esquema UUID propio.

```bash
export ATLAS_BENCH_DATABASE_URL='postgresql://atlas:atlas@127.0.0.1:55436/atlas_bench_vector'
# Opcional: ruta real del cgroup v2 de ese PostgreSQL; sin ella no se reportan sus recursos.
export ATLAS_BENCH_POSTGRES_CGROUP='/sys/fs/cgroup/system.slice/docker-ID.scope'
python benchmarks/mvt.py --samples 100 --warmup 10 --concurrency 1 4 --repetitions 3 --rows 10000 100000 1000000 --output /tmp/atlas-vector-benchmark.json
```

## Integridad y métricas

- MVT se decodifica contra IDs candidatos y codificables mediante ST_AsMVTGeom; se comprueban identidad y coordenadas dentro del buffer. El mosaico de 16 tiles z7 debe cubrir todos los IDs, permitiendo duplicados por buffer. Cuantización/colapso se informa aparte, no como LIMIT silencioso ni descarga íntegra.
- Cada payload MVT distinto se valida; las repeticiones comprueban SHA-256 para evitar que el decoder distorsione la carga. La latencia termina al recibir el cuerpo HTTP; CPU/RSS incluyen el cliente y la validación del proceso compartido.
- WFS comprueba IDs y numberMatched/numberReturned. Una primera página incompleta conserva métricas `page`, separadas de `complete`; no se extrapola al coste de descargar todo.
- WMS contrasta el conteo SQL con OperationLog y verifica PNG/dimensiones. Una respuesta truncada registra el preflight, `samples=0` y el motivo de exclusión; no se repite como un supuesto mapa completo rápido.
- 413, errores e integridad inválida tienen contadores separados. p50/p95 usan rango más próximo y solo su grupo; no se mezclan resultados completos, páginas y rechazos.
- CPU, RSS actual/máxima, threads y fds del proceso se registran antes/después y se muestrea RSS cada 50 ms. No es una garantía de detectar todos los picos.
- El cgroup opcional mide CPU y memoria PostgreSQL; memoria incluye caché de archivos/shared memory, **no es RSS**. Los picos del proceso son acumulados; se distingue el máximo muestreado de cada lote.
- Hay 10 rondas de cancelación por protocolo/dataset y observaciones de reposo a 0/0,2/1,2 s: sesiones DB activas, cola del pool, tareas, CPU, RSS y handles. No certifica ausencia de fugas a largo plazo.
- El JSON registra configuración sin credenciales, versiones instaladas/PostGIS, commit y SHA-256 de los scripts. Persiste progreso por dataset y al salir; `finished=false` identifica ejecuciones interrumpidas.

## Resultados de referencia

Ejecución 2026-10-20: Ryzen 5 7640HS (12 CPUs lógicas), ~14,8 GiB RAM, Python 3.11.14, PostgreSQL 16.4/PostGIS 3.4.3; cgroup sin cuota CPU ni límite de memoria. Runtime base `63a12a3`; versiones y hashes exactos de scripts en JSON.
324 casos: 16.200 muestras completas, 7.200 páginas WFS, 1.800 rechazos MVT y cero errores/invalidez calientes; 72 preflights WMS incompletos excluidos (no son éxitos). Cobertura de todos los IDs en los nueve mosaicos.

Ventana selectiva z10: mediana de los p95 de tres repeticiones, milisegundos **c1/c4**. WFS es primera página; con 1M filas contiene 1.000 de 1.024 candidatos. MVT no incluye renderizado cliente.

| Filas y geometría | MVT | WFS | WMS |
|---|---:|---:|---:|
| 10,000 puntos | 1.11/3.33 | 3.25/7.82 | 23.14/65.09 |
| 10,000 líneas | 1.32/4.19 | 2.81/7.52 | 10.74/23.20 |
| 10,000 polígonos | 1.27/3.31 | 2.83/8.01 | 24.43/58.80 |
| 100,000 puntos | 1.19/3.66 | 5.00/16.30 | 42.80/156.38 |
| 100,000 líneas | 1.43/3.05 | 6.30/19.04 | 28.78/99.82 |
| 100,000 polígonos | 2.16/4.07 | 8.51/45.32 | 59.67/245.87 |
| 1,000,000 puntos | 3.72/7.58 | 52.70/161.75 | incompleto/incompleto |
| 1,000,000 líneas | 3.44/6.02 | 60.91/170.26 | incompleto/incompleto |
| 1,000,000 polígonos | 8.01/10.67 | 71.69/230.10 | incompleto/incompleto |

Primeras respuestas: 0,82–2.333,81 ms entre casos distintos (no comparar como disco frío). RSS muestreado del proceso: 90,18–267,35 MiB; pico cgroup PostgreSQL: 1.110,27 MiB. CPU de los lotes/preflights registrados: Python 491,95 s y PostgreSQL 489,71 s; son cargas/contratos distintos, no una comparación de motores.
Cancelación: 270 rondas, 1.074 tareas canceladas y 6 finalizadas antes de cancelar. Al reposo final: cero sesiones DB activas, cola y tareas extra; hasta 33 threads. El pool registró 36 returns_bad y 3 requests_errors, y recuperó disponibilidad.
La CPU del proceso siguió activa en el segundo final del reposo tras cancelar WMS: no se certifica drenado nativo ni ausencia de acumulación sostenida. Seguimientos P0: [truncamiento #193](https://github.com/DatzinDev/Atlas/issues/193) y [trabajo pendiente #194](https://github.com/DatzinDev/Atlas/issues/194).
Datos sintéticos con ID y geometrías sencillas nativas 3857; no evalúa reproyección desde CRS locales, geometrías complejas, navegador, producción ni GeoServer. Primero corregir integridad/presupuestos y concurrencia; repetir esta matriz antes de atribuir mejoras a caché/renderizado.
JSON local `/tmp/atlas-vector-benchmark.json` (821.574 bytes), SHA-256 `82ea823f1187fa7e4245d4a82ffb2fe0ef7d5d5600278b4e98427863d13e6937`. Las corridas preliminares de calibración se excluyeron.


## Validación de correcciones WMS — 2026-10-23 (#196)

Stack #201, PRs #197–#200. Repetición **selectiva**, tres repeticiones, 20 muestras por lote,
2 warmups, concurrencia solicitada 1/4, 10k/100k/1M filas por tipo. Misma máquina y PostGIS;
Python 3.11.14, Pillow 13.0.0, Shapely 2.2.0, psycopg 3.3.6.
Las dependencias cambiaron respecto al histórico: no atribuir diferencias de latencia únicamente al código.
Sin caché; max_render_features=100000, max_render_bytes=67108864 y max_render_concurrency=2.

```bash
python benchmarks/mvt.py --windows selective --samples 20 --warmup 2 --concurrency 1 4 --repetitions 3 --rows 10000 100000 1000000 --output /tmp/atlas-wms-urgent-final.json
```

162 casos y nueve mosaicos completos: 2394 muestras completas, 360 páginas WFS,
486 rechazos explícitos por saturación WMS; cero errores, respuestas incompletas o integridad inválida.
En c4, cada lote WMS admite 2 de 20 muestras y rechaza 18 sin cola: **no es una mejora de throughput**
ni un p95 representativo de 20 mapas completos. El host debe espaciar/reintentar o dimensionar el cupo.

WMS c1: mediana de p95 (ms) entre tres repeticiones, 20 mapas completos por lote.

| Tabla y geometría | p95 WMS c1 (ms) |
|---|---:|
| 10,000 puntos | 12.43 |
| 10,000 líneas | 11.64 |
| 10,000 polígonos | 13.32 |
| 100,000 puntos | 40.46 |
| 100,000 líneas | 28.45 |
| 100,000 polígonos | 55.26 |
| 1,000,000 puntos | 392.25 |
| 1,000,000 líneas | 211.96 |
| 1,000,000 polígonos | 468.66 |

Con 1M filas, la ventana contiene 1024 geometrías y WMS representa todas; WFS conserva su página
1000/1024. La fidelidad y los presupuestos globales/multicapa se cubren además en regresiones
1000/1001/cap/cap+1 y bytes exactos/excesivos. No se repitió aquí toda la matriz global histórica.

Cancelación: 270 rondas, 425 cancelaciones;
observaciones de reposo hasta 6,2 s después del drenaje. Cero sesiones activas, cola del pool o tareas extra
al final de los 27 perfiles. CPU máxima del proceso en el intervalo final de 5 s tras WMS: 0.0009 s.
La cancelación espera al worker real; los tests bloqueables verifican retención del cupo incluso con
cancelaciones asyncio repetidas/AnyIO y excepciones del renderer. No certifica ausencia de fugas a horas.
El driver sigue descartando conexiones durante cancelaciones duras; el pool recupera disponibilidad.
RSS máximo muestreado: 274.34 MiB, incluido cliente/validación ASGI; sin cgroup PostgreSQL en esta repetición.
La latencia de renderizado sigue siendo un área de optimización: este bloque corrige integridad y recursos.

JSON local `/tmp/atlas-wms-urgent-final.json` (365,788 bytes), SHA-256 `022242c38bbe7a7313538724effc3f9089329bf9256a72e0fa94d6462149cca6`.
Se excluye la corrida preliminar con cursor de una fila. El JSON conserva commit, configuración y hashes de scripts.
