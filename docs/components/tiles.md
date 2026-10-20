# tiles

Servicio Python y ruta HTTP implementados; [ADR 0006](../adr/0006-native-mvt.md) es la especificación normativa.

- Publicará GET /tiles/{name}/{z}/{x}/{y}.mvt y get_vector_tile con grid XYZ compartido.
- Importa core, layers y postgis; nunca wfs, wms ni rendering. core calcula el grid.
- Autoriza view + query y habilitación MVT antes de pedir SQL; postgis recibe TableInfo.
- Properties y predicados usan whitelist y ADR 0005; presupuestos sin truncamiento.
- Devuelve bytes MVT con caché deshabilitada; no incorpora estilos, WMTS ni GWC.
- Las pruebas futuras decodifican geometría/atributos, vacío/bordes y errores de límites.

## Servicio (#134)

`get_vector_tile(ctx, name, tile, *, properties=None, predicate=None, principal=None)`
devuelve bytes. `predicate` es un modelo previamente validado por core, igual que WFS;
HTTP se encargará de parsear JSON con los límites configurados. Propiedades pasan por
whitelist SQL. `LayerServices.mvt=False` exige habilitación explícita del host.
Capa habilitada, servicio MVT, permisos view y query se comprueban antes de SQL,
incluso para tiles vacíos. WMS/WFS mantienen sus defaults. No se usa caché de imágenes.

## HTTP (#138)

GET `{prefix}/tiles/{name}/{z}/{x}/{y}.mvt`, por defecto prefix=/atlas.
MIME application/vnd.mapbox-vector-tile, Cache-Control: no-store, 200 incluso vacío.
Acepta `properties=nombre,id` y `ATLAS_FILTER` JSON con presupuestos de AtlasConfig;
rechaza duplicados, propiedades vacías y parámetros desconocidos (incluidos CQL/SLD).
Errores de coordenadas/filtros fallan antes del servicio. El checker recibe el
principal de la dependencia del host. La whitelist de atributos se verifica en postgis.
Errores de permisos/servicio/presupuestos usan la taxonomía Atlas, sin SQL ni URL de BD.
