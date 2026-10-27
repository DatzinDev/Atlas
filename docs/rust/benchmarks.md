# Cierre Python: métricas y medición posterior de Rust

Fecha de inventario: 2026-10-27. Esta entrega **no ejecuta mediciones nuevas**.
Requisitos R09–R17 y épica [#203](https://github.com/DatzinDev/Atlas/issues/203).

Este es el archivo consolidado de métricas para cerrar la etapa Python (#210).
El segundo entregable es [el benchmark HTTP reutilizable](../../benchmarks/COMPARE.md) (#208).
El cierre no exige nuevas corridas, optimizaciones Python ni GeoServer. Tras revisar e integrar
ambos entregables se puede comenzar Rust; el contraste con GeoServer se difiere a Rust 3 (#209).
No se eliminan fuentes, releases ni consumidores Python al cerrar esta etapa.

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
concurrencias 1/4 y geometrías sintéticas 3857. Ventana selectiva z10, mediana de p95 de tres
repeticiones; ms c1/c4. Referencia publicada en develop 254b0d6; runtime medido base 63a12a3.
El commit del informe no sustituye la identidad del runtime/paquetes descrita en él.

| Filas | Geometría | MVT c1/c4 | WFS página c1/c4 | WMS c1/c4 |
|---:|---|---|---|---|
| 10.000 | puntos | 1,11 / 3,33 | 3,25 / 7,82 | 23,14 / 65,09 |
| 10.000 | líneas | 1,32 / 4,19 | 2,81 / 7,52 | 10,74 / 23,20 |
| 10.000 | polígonos | 1,27 / 3,31 | 2,83 / 8,01 | 24,43 / 58,80 |
| 100.000 | puntos | 1,19 / 3,66 | 5,00 / 16,30 | 42,80 / 156,38 |
| 100.000 | líneas | 1,43 / 3,05 | 6,30 / 19,04 | 28,78 / 99,82 |
| 100.000 | polígonos | 2,16 / 4,07 | 8,51 / 45,32 | 59,67 / 245,87 |
| 1.000.000 | puntos | 3,72 / 7,58 | 52,70 / 161,75 | incompleto |
| 1.000.000 | líneas | 3,44 / 6,02 | 60,91 / 170,26 | incompleto |
| 1.000.000 | polígonos | 8,01 / 10,67 | 71,69 / 230,10 | incompleto |

WFS a 1M devuelve 1.000 de 1.024 candidatos. CPU acumulada de lotes/preflights: Python
491,95 s y PostgreSQL 489,71 s; pico cgroup PostgreSQL 1.110,27 MiB, incluye caché/shared memory.
Sin cuota CPU/memoria en esta matriz. 270 rondas de cancelación; reposo final sin sesiones DB
activas, cola ni tareas extra, pero CPU aún activa tras WMS: no certifica drenado nativo.

No comparar WFS/MVT como trabajos equivalentes. MVT no incluye render del cliente y puede
colapsar geometrías tras cuantización. RSS vectorial muestreado: 90,18–267,35 MiB del proceso
compartido con generador/validación, no memoria exclusiva del servidor.

## Piloto HTTP anterior: referencia distinta

2026-10-19, Python 3.13.11/Uvicorn un worker; mismo Ryzen, ~14,8 GiB RAM. PostGIS 16.4/3.4.3
limitado a 1 CPU/512 MiB en RAM; cliente y servidor en el mismo host, sin aislamiento de ruido.
Medianas ms c1/c4; 12 samples por combinación, no tres repeticiones independientes:

| Caso | Base 7b53a1c | Cambios locales sin caché | Caché caliente local |
|---|---|---|---|
| WMS puntos 256² | 1.386,9 / 4.922,7 | 47,1 / 201,1 | 1,11 / 3,07 |
| WMS puntos 1024² | 15.507,2 / 61.745,5 | 161,2 / 414,6 | 2,06 / 26,94 |
| WMS líneas 256² | 160,3 / 613,0 | 36,9 / 137,2 | 1,22 / 4,20 |
| WMS líneas 1024² | 4.277,1 / 17.625,2 | 169,8 / 424,3 | 1,88 / 17,85 |
| WMS polígonos 256² | 485,1 / 2.053,3 | 163,7 / 693,0 | 1,22 / 3,82 |
| WMS polígonos 1024² | 26.414,8 / 81.164,3 | 334,0 / 843,5 | 1,89 / 22,04 |

Base puntos dibujaba 1.000/1.111 candidatos; local 1.111/1.111: cargas distintas. HIT no mide
generación de mapas nuevos. Pico RSS acumulado worker base/sin caché/HIT: 374,2/178,6/132,9 MiB.
WFS página 1.000 con geometría, base c1/c4: 34,3/120,2 ms; offset 90.000: 100,1/289,4 ms;
sin geometría: 23,6/73,7 ms. WFS no se optimizó en ese piloto.
Soak local adicional: 300 ventanas únicas, 1.000 polígonos, 100,2 s; medianas por bloque
318,0–333,6 ms y RSS final 170,1 MiB. No demuestra ausencia de fugas ni capacidad sostenida.
Fuentes de estas cifras: PERFORMANCE_BASELINE/OPTIMIZATION dentro del archivo original.

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

## Comparación futura (PERF01–PERF06; no bloquea el cierre Python)

El instrumento mínimo entregado mide samples HTTP y expectativas conocidas; su uso y límites
están en COMPARE.md. El protocolo ampliado siguiente pertenece a #230/#231; duración/soak,
oráculos espaciales y recursos del servidor aún no están implementados en compare.py.
No comparar directamente esta referencia ASGI con números TCP de Rust. Si se quiere cuantificar
la ganancia del cambio, ejecutar Python fijado y Rust release con el mismo instrumento HTTP,
dataset/índices/configuración y trabajo equivalente. GeoServer se incorpora después mediante #209.

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
