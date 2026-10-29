# secrets/

Un archivo por secreto, sin salto de línea al final. Nada de aquí se versiona
salvo este README.

    make secrets          # genera los que faltan (nunca pisa los existentes)
    make secrets-import   # los trae de los .env de antes
    scripts/secrets.sh list    # qué es cada uno, qué perfil lo usa, cuáles van offline
    scripts/secrets.sh check   # valida: presentes, largos, distintos, sin valores de desarrollo

Los servicios los reciben como `NAME_FILE=/run/secrets/<archivo>`; vlpds los
lee él mismo, y para los de WatZappa runtime/env-from-files.sh los carga
antes de arrancar y se niega a hacerlo si el mismo secreto también
llega como variable. En el servidor el directorio es `root:10001` con `0750`
y cada archivo `0440` (lo hace `make secrets` corriendo como root, o Ansible).

Seis no tienen arreglo si se pierden y no viven en el bucket: respáldalos
fuera de la máquina (docs/RESPALDOS.md):

- `vlpds_kek` — envuelve las llaves de firma de cada repo, que sí están en el
  bucket: sin ella, el bucket no sirve
- `vlpds_plc_rotation_key` — controla los DIDs de todas las cuentas (va
  envuelta con la KEK: `make secrets` la crea así)
- `bsky_service_signing_key`, `ozone_signing_key`
- `m8_curp_pepper` — sin él no se puede deduplicar a nadie por CURP
- `m8_issuer_private_jwk` — firma las credenciales de identidad
