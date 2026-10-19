# Changelog: cómo y cuándo actualizarlo

`CHANGELOG.md` está en la raíz y le habla a quien **consume** la librería: alguien que hace
`pip install atlas-geo` y necesita saber qué le cambió. No es el historial de git — el historial
cuenta commits, el changelog cuenta cambios que le importan a un usuario.

- **Formato**: [Conventional Changelog](https://github.com/conventional-changelog/conventional-changelog),
  derivado de [Conventional Commits](https://www.conventionalcommits.org/).
- **Versionado**: [Semantic Versioning](https://semver.org/).
- **Idioma**: inglés, como los commits (`docs/conventions.md`). Esta guía va en español porque es
  documentación interna.

## Qué entra y qué no

El changelog se deriva del tipo del commit. Estos son los tipos visibles y su sección:

| Tipo de commit | Sección en el CHANGELOG |
| --- | --- |
| `feat` | Features |
| `fix` | Bug Fixes |
| `perf` | Performance Improvements |
| Cualquier tipo con `!` o `BREAKING CHANGE:` | ⚠ BREAKING CHANGES |

El resto queda **oculto** por convención y no genera entrada: `docs`, `chore`, `test`, `refactor`,
`style`, `build`, `ci`. Un `chore(dev): rename compose file` no le cambia nada a quien instala la
librería, y meterlo solo diluye las entradas que sí importan.

Corolario práctico: **si tu cambio merece changelog, el commit debe ser `feat` o `fix`.** Si te
descubres queriendo listar un `chore` en el changelog, casi siempre el tipo del commit está mal
elegido, no la convención.

## Cuándo se actualiza

En el **mismo PR** que introduce el cambio, nunca en uno aparte. La entrada se agrega bajo
`## Unreleased`, en la sección que corresponda al tipo; si la sección no existe todavía, se crea.

## Cómo se escribe la entrada

Una línea por commit, con el scope en negrita, la descripción del commit y el enlace al hash:

```markdown
### Features

* **wfs:** add GetFeature endpoint with paginated GeoJSON output ([abc1234](https://github.com/zamax14/Atlas/commit/abc1234))

### Bug Fixes

* **wms:** honor bbox when the request CRS differs from the layer CRS ([def5678](https://github.com/zamax14/Atlas/commit/def5678))
```

Reglas:

1. La descripción es la del commit: presente, minúscula inicial, sin punto final.
2. El scope es el componente (`wms`, `wfs`, `postgis`, `rendering`, `exports`…), el mismo del commit.
3. Un cambio incompatible se marca con `!` en el commit (`feat(wfs)!: ...`) y aparece bajo
   BREAKING CHANGES con una nota de migración.
4. No se listan archivos tocados ni se copian cuerpos de commit — los commits son de una sola línea.
5. **Referencia**: siempre el hash enlazado. Una entrada no puede citar el hash del commit que la
   contiene —todavía no existe, y un `--amend` lo invalida—, así que el changelog va en su
   **propio commit, después** del commit del cambio:

   ```bash
   git commit -m "feat(wfs): add GetFeature endpoint"   # el cambio
   git log -1 --format=%h                               # el hash a citar
   git commit -m "docs(changelog): add the GetFeature entry"
   ```

   Los PRs se integran con merge commit, no con squash, así que ese hash sigue siendo válido en
   `develop` después del merge. Si algún día se pasa a squash, esta regla deja de funcionar y hay
   que volver a referenciar el PR.

## Cómo se publica una versión

Al preparar un release desde `develop` hacia `main`:

1. Renombra `## Unreleased` a `## X.Y.Z - AAAA-MM-DD` (fecha ISO) y abre un `## Unreleased` vacío arriba.
2. Elige `X.Y.Z` según SemVer: breaking sube MAJOR, `feat` sube MINOR, `fix`/`perf` suben PATCH.
   Mientras la versión sea `0.y.z` una MINOR puede romper compatibilidad, y eso se marca explícito.
3. Actualiza la versión en `pyproject.toml` en el mismo PR.
4. Etiqueta el merge en `main` como `vX.Y.Z`.

El alcance previsto de cada versión está en [`roadmap.md`](roadmap.md).
