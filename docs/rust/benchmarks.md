# Rendimiento: evidencia conservada y comparación futura

Fecha de inventario: 2026-10-27. Esta entrega **no ejecuta mediciones nuevas**.
Requisitos R09–R17 y épica [#203](https://github.com/DatzinDev/Atlas/issues/203).

## Evidencia que ya existe

[Archivo original comprimido](evidence/python-2026-10.tar.gz) y [manifiesto SHA-256](evidence/manifest.json).
17 archivos, 81.577 bytes comprimidos; originales sin reformatear. Incluye muestras, paquetes,
perfiles, verificación y tres informes locales, además de informes vectoriales fijados por commit.
No contiene el código fuente completo del checkout histórico ni constituye un nuevo baseline.
Solo aparecen credenciales ficticias de bases locales de benchmark en los ejemplos originales.

| Grupo | Muestras y origen | Lectura correcta |
|---|---|---|
| HTTP histórico | 20 combinaciones × 12 muestras = 240; JSON original disponible | Parte de WMS puntos era truncada; 200/PNG válido no garantiza cobertura. |
| HTTP optimizado sin caché | 240, JSON final disponible; cambios locales sobre 7b53a1c | No inferir el runtime completo solo del commit: se registran hashes de fuentes. |
| HTTP con caché | 240, JSON final disponible | HIT mide repetición, no generación de ventanas nuevas. |
| Vectorial integrado | 324 casos, 16.200 completas, 7.200 páginas y 1.800 rechazos | ASGI in-process, sin TCP; excluye 72 preflights WMS incompletos. |
| WMS corregido candidato | PR #200: 162 casos, 2.394 completas, 360 páginas y 486 rechazos | No integrado en develop al inventariar; c4 rechaza saturación, no demuestra mayor throughput. |

El manifiesto histórico conserva sus siete coincidencias SHA-256. Los tres JSON HTTP final/histórico
tienen cero errores de transporte/validación registrados; esto no borra el truncamiento conocido.
Con doce muestras p95 es el máximo: estos pilotos no justifican p99 ni capacidad sostenida.

Vectorial: Ryzen 5 7640HS, Python 3.11.14, PostgreSQL 16.4/PostGIS 3.4.3; tres repeticiones,
concurrencias 1/4 y geometrías sintéticas 3857. Con un millón de filas, ventana selectiva:

| Mediana de p95 entre repeticiones | c1 ms | c4 ms |
|---|---:|---:|
| MVT, puntos/líneas/polígonos | 3,44–8,01 | 6,02–10,67 |
| WFS, primera página 1.000/1.024 | 52,70–71,69 | 161,75–230,10 |
| WMS histórico | incompleto | incompleto |

No comparar WFS/MVT como trabajos equivalentes. MVT no incluye render del cliente y puede
colapsar geometrías tras cuantización. RSS vectorial muestreado: 90,18–267,35 MiB del proceso
compartido con generador/validación, no memoria exclusiva del servidor.

## Fuentes fijadas y faltantes

- [Informe integrado](https://github.com/DatzinDev/Atlas/blob/254b0d6f100fc2f6ae2eb8d97f9f1014d96479c3/docs/PERFORMANCE_MVT.md): también dentro del archivo.
- [Informe candidato](https://github.com/DatzinDev/Atlas/blob/ed9fdda002483832ae435fd7ba990f29524d2c2b/docs/PERFORMANCE_MVT.md): WMS c1 completo con 1M filas p95 211,96–468,66 ms; dependencias distintas del histórico.
- Faltan /tmp/atlas-vector-benchmark.json y /tmp/atlas-wms-urgent-final.json. Sus hashes esperados
  constan en manifest.json; no reconstruir muestras desde tablas ni marcar originales como verificados.
- PRs #197–#200 estaban abiertos; no integrar cambios para obtener una cifra conveniente.
- Los archivos locales optimizados incluyen código no integrado. Son evidencia histórica, no
  la versión distribuida de Python. Una comparación nueva parte de un commit reproducible.

Para comprobar el archivo sin extraerlo, verificar archive_sha256, abrir tar.gz con Python
tarfile y calcular SHA-256 de cada miembro contra files del manifiesto. Solo miembros regulares,
rutas relativas y sin '..'. Las muestras originales se preservan byte a byte.

## Protocolo nuevo (PERF01–PERF06)

PERF01 Referencias: congelar Python develop por SHA, lock/resolución de dependencias e imagen.
El candidato WMS es una referencia adicional explícita, nunca sustitución silenciosa.
Rust se compila release. GeoServer usa versión estable fijada con extensión vector-tiles de
la misma versión; guardar digest, JDK, configuración, heap y plugins. BD PG16/PostGIS3.4.
No medir el deployment productivo ni usar sus credenciales/datos privados.

PERF02 Aislamiento: servidores secuenciales por TCP HTTP real, generador en otro proceso,
2 CPU/1 GiB por motor y PostGIS separado 2 CPU/2 GiB; mismo host/cotas para cada corrida.
Python un worker; documentar hilos Rust/Java y pool máximo 10 en los tres. No ejecutar tests
ni otros benchmarks simultáneamente. Registrar ruido y limitaciones del host.

PERF03 Corpus: reutilizar generador/oráculos vectoriales con seed, 10k/100k/1M por geometría,
ventanas selectivas/densas, nueve mosaicos y un corpus real redistribuible con procedencia/licencia.
Misma BD, índices GiST, ANALYZE, filtros, proyección y propiedades. Semántica de conteo idéntica
o experimento separado; Rust sin COUNT no se presenta como mejora de lenguaje frente a WFS con COUNT.

PERF04 Escenarios: catálogo, página GeoJSON, selección por bbox/polígono, filtros compuestos,
MVT/TileJSON y descarga completa. GeoServer WFS→GeoJSON y extensión vector-tiles→MVT se
adaptan solo en el generador; Rust no incorpora OGC. Alinear extent=4096, buffer=64, CRS,
propiedades, cobertura, estilo sin clasificación y simplificación; si no se logra equivalencia,
informar N/A o diferencias, nunca declarar ganador. Las funciones solo Atlas van en matriz separada.

PERF05 Carga: 10 warmups excluidos; tres repeticiones de 60 s por caso a c1/c4/c16; soak 30 min
a c16 con mezcla fija 40% página, 40% tiles, 20% filtros; export/cancelación se evalúan aparte.
Primero sin caché de resultados, después experimento separado de caché si existe en los tres.
PostgreSQL caliente tras warmup no significa disco frío; no vaciar cachés del host por defecto.
Reportar p50/p95; p99 solo con >=1.000 muestras válidas por grupo. Registrar muestras individuales.

PERF06 Oráculos y recursos: validar cuerpos completos, IDs, tipos/atributos y geometrías;
decodificar MVT distinto fuera del temporizador y comprobar candidatos codificables/bordes.
Latencia hasta consumir cuerpo; throughput válido separado de errores/rechazos. Medir CPU/RSS
servidor y recursos DB separados del generador, red/payload, conexiones, espera pool y timeout.
Cancelar consultas/descargas y verificar reposo, tareas y conexiones; no solo HTTP 200.

## Publicación y aceptación

Cada run conserva JSON crudo, summary, manifest, hashes de datos/fuentes/configuración,
versiones/digests, hardware, cuotas, comandos y estado finished. No secretos ni tokens.
Resultados pequeños y resúmenes quedan en Git; grandes se adjuntan a una release de evidencia
con checksum y URL permanente, no únicamente como artifacts con caducidad de Actions ni /tmp.
Una corrida con integridad inválida no permite concluir mejoras de rendimiento. Sin SLO absoluto
acordado, publicar diferencias y tradeoffs; no inventar un objetivo de superioridad.

Fuentes: [GeoServer GeoJSON](https://docs.geoserver.org/main/en/user/services/wfs/outputformats/),
[vector tiles](https://docs.geoserver.org/main/en/user/extensions/vectortiles/tutorial/),
[PostGIS MVT](https://postgis.net/docs/ST_AsMVT.html). GeoServer no ha sido medido en esta entrega.
