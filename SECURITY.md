# Seguridad

## Reportar una vulnerabilidad

Usa [Report a vulnerability](https://github.com/DatzinDev/Atlas/security/advisories/new)
para enviar un reporte privado. Incluye versión, entorno, reproducción e impacto;
no publiques credenciales, datos personales ni un exploit en un issue público.
No se requiere acceso a infraestructura de producción para reproducir un fallo.

## Versiones soportadas

Las correcciones se publican en la última versión disponible de Atlas. Las ramas de
trabajo y versiones anteriores no reciben soporte de seguridad independiente.

## Releases

La publicación usa Trusted Publishing de PyPI mediante OIDC, sin tokens permanentes.
Tras integrar develop → main, crea un tag vX.Y.Z que coincida con pyproject.toml y el
commit actual de main. Ejecuta Publish to PyPI desde main indicando ese tag y aprueba
el entorno pypi. CI valida dependencias, tests, wheel y sdist antes de publicar.
No se publica automáticamente con cada push. Los tags v* no pueden modificarse ni borrarse.

## Integración

Atlas recibe la identidad y permisos de la aplicación host. La app debe configurar
su checker para datos privados, HTTPS, límites de tráfico y credenciales PostGIS de
mínimo privilegio. El contenedor Compose es de desarrollo y solo expone localhost.
