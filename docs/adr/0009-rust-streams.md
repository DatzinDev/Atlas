# ADR 0009 — Composición de streams Rust

Estado: aprobado por el responsable del proyecto en esta ronda.
Seguimiento: #227 y #228.

## Decisión

Declarar `futures-util` con features mínimas como dependencia directa para componer
streams de SQLx y cuerpos Axum. Ya forma parte del árbol transitivo de ambos.
No implementar manualmente el protocolo de polling ni añadir un runtime alternativo.
La utilidad permite lectura por lotes, backpressure y comprobaciones de deadlines;
Tokio sigue siendo el runtime. Los permisos y la limpieza siguen siendo responsabilidad
de los servicios concretos. No cambia el runtime Python.
