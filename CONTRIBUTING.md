# Cómo contribuir a Atlas

El repositorio está organizado para que cada issue sea implementable de forma aislada —por una
persona o por un agente— y revisable en pocos minutos. Eso solo funciona si los issues y los PRs
se mantienen pequeños.

## Trabajar un issue

1. **Elige un issue con label `ai-ready`.** Significa que el contrato, los archivos y los criterios
   de aceptación están completos y no hace falta preguntar nada para empezar. Un issue
   `needs-spec` hay que especificarlo antes de tocarlo; uno `blocked` espera a sus dependencias.
2. **Lee sus dos documentos**: el `docs/components/*.md` del componente y, si toca SQL, filtros o
   permisos, `docs/components/security.md`.
3. **Crea la rama desde `develop`**: `type/nombre-corto` (`feat/wfs-get-feature`,
   `fix/bbox-axis-order`). Nunca se commitea directo a `develop` ni a `main`.
4. **Implementa solo lo que pide el issue.** La sección *Fuera de alcance* del issue es vinculante.
   Si encuentras otra cosa que arreglar, abre otro issue.
5. **Escribe el test.** Un módulo, un archivo de test. Sin test no está terminado.
6. **Verifica** con el comando que el propio issue indica en su sección de verificación.
7. **Commit de una línea**, Conventional Commits, sin cuerpo y sin co-autores.
8. **Abre el PR contra `develop`** con `Closes #N` y la plantilla rellena.

## Ramas

Flujo: `feature → develop → main`. Las ramas de trabajo conservan el prefijo por tipo
(`feat/`, `fix/`, `docs/`, etc.); los stacks pueden encadenarlas antes de integrar en `develop`.
Los PRs a `main` deben salir de `develop`; no se publican features directamente en `main`.

```
main   ── release ─────────────────────────────►   solo recibe releases desde develop
              ▲
develop  ─────┴── feat/... ── fix/... ── docs/...    integración: aquí llegan los issues
```

- `main` — estable. Cada merge es una versión publicable, etiquetada (`v0.1.0`).
- `develop` — integración. Todos los PRs de issues apuntan aquí.
- `type/nombre-corto` — una por issue, cortada desde `develop` y borrada al mergear.

Ambas ramas base son protegidas **por convención**, no por reglas de GitHub: no hay nada que te
impida técnicamente empujar a `develop`, y aun así no se hace.

## Entorno de desarrollo

```bash
cp .env.example .env     # opcional: solo si necesitas cambiar puerto o credenciales
docker compose up -d     # PostGIS 16-3.4 en localhost:5433 con el dataset de scripts/seed.sql
```

## Definición de terminado

Un issue está terminado cuando:

- [ ] Todos los criterios de aceptación del issue están marcados.
- [ ] `pytest` pasa, incluidos los tests nuevos.
- [ ] `ruff check .` y `mypy src/` pasan.
- [ ] Si cambió un contrato público, su `docs/components/*.md` está actualizado en el mismo PR.
- [ ] No se añadieron dependencias (o hay un ADR que lo justifica).
- [ ] El diff no excede el `size:` del issue. Si lo excede, el issue estaba mal dimensionado:
      dilo en el PR y propón partirlo.

## Checklist de revisión

Quien revisa mira estas cinco cosas, en este orden:

1. **¿El diff hace solo lo que dice el issue?** Alcance ampliado = se devuelve, aunque el código
   extra esté bien.
2. **¿Hay SQL construido con f-strings o concatenación?** Es un bloqueo automático.
   Toda columna que entra en una query debe venir de la whitelist de la capa.
3. **¿Se respetan las direcciones de dependencia de `docs/architecture.md`?**
   Un import de `postgis` dentro de `rendering/` es un bloqueo.
4. **¿Hay abstracción especulativa?** Una interfaz con una sola implementación, un parámetro de
   configuración que nadie usa, un `Protocol` que el issue no pedía: se quita.
5. **¿El test falla si se rompe la lógica?** Un test que pasa con la función vacía no es un test.

## Dimensionar issues

- `size:xs` — menos de 50 líneas de diff.
- `size:s` — menos de 150.
- `size:m` — menos de 300.

**No existe `size:l`.** Si algo no cabe en `size:m`, se parte en issues que sí quepan. El límite no
es estético: un PR de 800 líneas no se revisa, se aprueba por cansancio.

## Añadir dependencias

Se responde primero, en el issue:

1. ¿Lo resuelve la biblioteca estándar?
2. ¿Lo resuelve una dependencia que ya tenemos (Pydantic, shapely, psycopg, Pillow)?
3. ¿Son menos de ~50 líneas escribirlo?

Si tras esas tres preguntas la dependencia sigue en pie, va un ADR a `docs/adr/` **antes** del PR
que la instala.

## Estructura del repositorio

```
src/atlas/<componente>/     código + CLAUDE.md local con las reglas del componente
tests/<componente>/         un archivo de test por módulo
docs/components/            contrato e invariantes de cada componente
docs/adr/                   decisiones y alternativas descartadas
examples/                   apps FastAPI de ejemplo
```
