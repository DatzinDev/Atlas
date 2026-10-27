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

Repetir tres veces a c1/c4 con mismos datos/índices, propiedades, filtros, orden, página, caché,
hardware y límites. Guardar junto al JSONL las expectativas, seed/hash de datos, configuración,
versiones y comandos. Usar fixture pública sin tokens/secretos en URL/body. Python HTTP nuevo
y Rust release son comparables entre sí; el informe histórico ASGI no lo es directamente.
