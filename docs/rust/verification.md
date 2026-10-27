# Verificación de la entrega documental

2026-10-27. Se revisa documentación y evidencia, no el comportamiento de un motor Rust.

- OpenAPI 3.0.3 validado con JSON Schema oficial de OpenAPI usando jsonschema Draft4.
- YAML sin claves duplicadas; referencias internas resueltas, 11 paths y 14 operationId únicos.
- Ejemplos de publicación y Query válidos; SQL libre, rename por PATCH, properties vacías,
  limit cero, operador SQL y comparación ordinaria con NULL rechazados por los esquemas.
- Parámetros de paths definidos y auth obligatoria declarada en todas las operaciones admin.
- Enlaces relativos de los nuevos documentos y puntos de entrada comprobados.
- Archivo de evidencia: 17 miembros regulares sin rutas absolutas/traversal, tamaños y SHA-256
  verificados contra el manifiesto; originales byte a byte; manifiesto histórico 7/7.
- JSON HTTP: tres corridas de 240 muestras, errores registrados cero; truncamiento conocido
  preservado como limitación, no borrado por esta comprobación.
- Dependencias: 35 tareas, referencias internas existentes y grafo sin ciclos. #211 requiere
  baseline #210; #218 requiere parsing/SQL/selección espacial #220–#222.
- git diff --check y tamaños de commits/PRs documentales revisados; sin cambios de runtime.

Herramientas de revisión utilizadas: Python 3, PyYAML, jsonschema ya disponibles, tarfile,
hashlib y Git. No se añadieron dependencias ni jobs de CI como parte de esta verificación.
Esquema oficial: https://spec.openapis.org/oas/3.0/schema/2024-10-18.
El SDD y el protocolo describen las pruebas runtime futuras; no se ejecutaron localmente
pytest/PostGIS, la matriz de carga nueva, GeoServer, Rust ni una migración de consumidores.
