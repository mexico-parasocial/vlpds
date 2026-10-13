# Respaldos de la plataforma hacia el bucket. Misma versión mayor que el
# Postgres de la plataforma: pg_dump no puede respaldar un servidor más nuevo.
FROM postgres:15-alpine
RUN apk add --no-cache rclone sqlite tar zstd busybox-extras tini
COPY --chmod=755 backup.sh restore.sh /usr/local/bin/
ENTRYPOINT ["/sbin/tini", "--"]
CMD ["backup.sh", "loop"]
