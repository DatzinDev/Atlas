# Tareas y trazabilidad

Épica [#203](https://github.com/DatzinDev/Atlas/issues/203). Fuente del estado real: issues de GitHub.
Esta tabla es un índice de contratos, no una segunda lista de checks que pueda quedar desactualizada.
`blocked` se retira después de integrar dependencias; `needs-spec` nunca autoriza implementación.

| Etapa | Issue | Requisitos | Depende de |
|---|---|---|---|
| 0 | [#204 SDD y arquitectura](https://github.com/DatzinDev/Atlas/issues/204) | R01–R17 | — |
| 0 | [#205 REST/OpenAPI](https://github.com/DatzinDev/Atlas/issues/205) | R01–R16 | #204 |
| 0 | [#206 Evidencia y protocolo](https://github.com/DatzinDev/Atlas/issues/206) | PERF01–PERF06 | #204 |
| 0 | [#207 Roadmap y seguimiento](https://github.com/DatzinDev/Atlas/issues/207) | R17 | #204,#205,#206 |
| 0 | [#208 Herramienta HTTP reutilizable](https://github.com/DatzinDev/Atlas/issues/208) | Instrumento mínimo; benchmarks.md | #206 |
| 0 | [#210 Métricas y cierre Python](https://github.com/DatzinDev/Atlas/issues/210) | Evidencia histórica,R17 | #206,#208 |
| 1 | [#211 Paquete y CI](https://github.com/DatzinDev/Atlas/issues/211) | R16,R17 | #210,#207 |
| 1 | [#212 Modelos y límites](https://github.com/DatzinDev/Atlas/issues/212) | R01–R08,R12–R16 | #211 |
| 1 | [#213 Pool y transacciones](https://github.com/DatzinDev/Atlas/issues/213) | R15,R16 | #212 |
| 1 | [#214 Introspección](https://github.com/DatzinDev/Atlas/issues/214) | R02,R04,R08 | #213 |
| 1 | [#215 Catálogo persistente](https://github.com/DatzinDev/Atlas/issues/215) | R01,R02,R17 | #213,#212 |
| 1 | [#216 JWT externos](https://github.com/DatzinDev/Atlas/issues/216) | R12,R14 | #212 |
| 1 | [#217 Políticas](https://github.com/DatzinDev/Atlas/issues/217) | R03,R07,R13,R14 | #216 |
| 1 | [#218 Administración](https://github.com/DatzinDev/Atlas/issues/218) | R01,R02,R12–R14 | #215,#214,#217,#205,#220,#221,#222 |
| 1 | [#219 Descubrimiento](https://github.com/DatzinDev/Atlas/issues/219) | R03,R13,R14 | #218 |
| 1 | [#220 AST acotado](https://github.com/DatzinDev/Atlas/issues/220) | R04–R07,R15 | #212,#205 |
| 1 | [#221 Predicados SQL](https://github.com/DatzinDev/Atlas/issues/221) | R04,R05,R07 | #220,#214 |
| 1 | [#222 Selección espacial](https://github.com/DatzinDev/Atlas/issues/222) | R06,R07,R15 | #221 |
| 2 | [#223 Filas y paginación](https://github.com/DatzinDev/Atlas/issues/223) | R04,R08,R09 | #222,#214 |
| 2 | [#224 API GeoJSON](https://github.com/DatzinDev/Atlas/issues/224) | R04–R09,R13–R15 | #223,#219 |
| 2 | [#225 SQL MVT](https://github.com/DatzinDev/Atlas/issues/225) | R04–R07,R10,R15 | #222 |
| 2 | [#226 API tiles/TileJSON](https://github.com/DatzinDev/Atlas/issues/226) | R03,R10,R13–R15 | #225,#219 |
| 2 | [#227 Exportación GeoJSON](https://github.com/DatzinDev/Atlas/issues/227) | R07,R09,R11,R13–R15 | #223,#224 |
| 2 | [#228 Admisión y cancelación](https://github.com/DatzinDev/Atlas/issues/228) | R11,R15,R16 | #224,#226,#227,#213 |
| 2 | [#229 Aceptación y paquete MVP](https://github.com/DatzinDev/Atlas/issues/229) | R01–R17 | #228,#226,#219 |
| 3 | [#230 Adaptador Rust](https://github.com/DatzinDev/Atlas/issues/230) | PERF01–PERF06,R17 | #229,#210 |
| 3 | [#209 Entorno GeoServer, diferido](https://github.com/DatzinDev/Atlas/issues/209) | PERF01–PERF04 | #230 |
| 3 | [#231 Comparación publicada](https://github.com/DatzinDev/Atlas/issues/231) | PERF01–PERF06,R15,R17 | #230,#209 |
| 3 | [#232 OIDC externo](https://github.com/DatzinDev/Atlas/issues/232) | R12–R14 | #216,#217,#229 |
| 3 | [#233 Consumidor y retorno](https://github.com/DatzinDev/Atlas/issues/233) | R17; needs-spec | #231,#232 |
| 4 | [#234 ADR formatos](https://github.com/DatzinDev/Atlas/issues/234) | R11,R13–R15; needs-spec | #229 |
| 4 | [#235 GeoParquet](https://github.com/DatzinDev/Atlas/issues/235) | R11,R13–R15; needs-spec | #234,#227 |
| 4 | [#236 FlatGeobuf](https://github.com/DatzinDev/Atlas/issues/236) | R11,R13–R15; needs-spec | #234,#235 |
| 5 | [#237 ADR snapshots](https://github.com/DatzinDev/Atlas/issues/237) | R10,R13–R15; needs-spec | #231 |
| 5 | [#238 CLI PMTiles](https://github.com/DatzinDev/Atlas/issues/238) | R10,R13–R15; needs-spec | #237,#226 |

Cada issue define archivos previstos, aceptación y verificación. Antes de superar su tamaño,
se parte sin ampliar alcance. Las extensiones needs-spec requieren SDD/ADR y tareas atómicas
antes de código; el catálogo privado y formatos estáticos no pueden filtrar datos por atajos.
