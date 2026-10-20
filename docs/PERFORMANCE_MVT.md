# Benchmark vectorial reproducible (#140)

Mide Atlas como librería ASGI, sin red TCP, proxy, navegador ni workers ASGI separados.
La matriz usa 10k/100k/1M filas **por tipo** (punto/línea/polígono), ventanas XYZ selectivas/globales, PNG 256×256, 3 repeticiones, concurrencia 1/4, 10 warmups y 100 muestras totales por lote válido.
La primera petición por caso se registra aparte: **no representa disco ni PostgreSQL fríos**; la generación, ANALYZE y los oráculos calientan los datos. No se vacían cachés del host.

## Reproducción

Instalar `pip install -e ".[dev,export]"`; crear un PostGIS descartable 16/3.4 con base `atlas_bench_vector` y extensión PostGIS. Nunca apuntar a una base de aplicación: el generador rechaza nombres fuera de `atlas_bench_*` antes de DDL y usa/retira únicamente un esquema UUID propio.

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

Pendientes de finalizar la ejecución de la matriz completa; no usar las corridas preliminares de calibración.
