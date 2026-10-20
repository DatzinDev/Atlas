# tiles — contrato propuesto

**No implementado.** [ADR 0006](../adr/0006-native-mvt.md) es la especificación normativa.

- Publicará GET /tiles/{name}/{z}/{x}/{y}.mvt y get_vector_tile con grid XYZ compartido.
- Importa core, layers y postgis; nunca wfs, wms ni rendering. core calcula el grid.
- Autoriza view + query y habilitación MVT antes de pedir SQL; postgis recibe TableInfo.
- Properties y predicados usan whitelist y ADR 0005; presupuestos sin truncamiento.
- Devuelve bytes MVT con caché deshabilitada; no incorpora estilos, WMTS ni GWC.
- Las pruebas futuras decodifican geometría/atributos, vacío/bordes y errores de límites.
