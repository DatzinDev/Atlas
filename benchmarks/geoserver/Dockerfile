# syntax=docker/dockerfile:1.7
FROM docker.osgeo.org/geoserver:3.0.2@sha256:5a372d4988116e8f62c71ae5ff6eb8d433967697670fb1dec15379bf476721fe

ADD --checksum=sha256:289bc62c212826650f72873e8c4beac4281b4a241552a3b6dbf533022367745d \
    https://downloads.sourceforge.net/project/geoserver/GeoServer/3.0.2/extensions/geoserver-3.0.2-vectortiles-plugin.zip \
    /tmp/vectortiles.zip
RUN unzip -oj /tmp/vectortiles.zip '*.jar' -d "$GEOSERVER_LIB_DIR" && rm /tmp/vectortiles.zip

ENV SKIP_DEMO_DATA=true \
    INSTALL_EXTENSIONS=false \
    EXTRA_JAVA_OPTS="-Xms640m -Xmx640m"
