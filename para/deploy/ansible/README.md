# Kit de Ansible

Provisiona una máquina Ubuntu y despliega la plataforma con los mismos
comandos que usarías a mano (`make preflight`, `make up`, `make verify`).
Dos roles:

| Rol | Qué hace |
| --- | --- |
| `common` | Paquetes, UTC y chrony, parches de seguridad sin reinicio automático, fail2ban, Docker (con tope de logs), Tailscale opcional y ufw (22, 80, 443; todo en `tailscale0`) |
| `para` | Clona el fork de vlpds (rama `para/platform`), WatZappa y mubEZ en su revisión fijada, escribe `.env` y `env/*.env`, un archivo por secreto (`root:10001`, `0440`), genera la config de Matrix la primera vez, corre `make preflight`, `make up` y `make verify` |

vlpds se descarga (`VLPDS_IMAGE`, una versión fija). Las imágenes de
WatZappa, mubEZ y el relay se construyen en el servidor mientras
`WATZAPPA_VERSION` o `MUBEZ_VERSION` sean `local` (nadie las publica todavía):
el rol corre `make preflight BUILD=1` y `make up BUILD=1`. Cuando un CI las
publique, pon `sha-<commit>` o `X.Y.Z` y se descargan.

La llave de rotación PLC de vlpds va envuelta con la KEK: genera
`vlpds_kek` y `vlpds_plc_rotation_key` en tu máquina con `make secrets` y
copia los dos archivos al inventario cifrado.

## Primera vez

```bash
cd deploy/ansible
ansible-galaxy collection install -r requirements.yml
cp -r inventories/example inventories/prod          # inventories/prod no se versiona
cp .sops.yaml.example .sops.yaml                     # tu llave pública age
cp inventories/prod/group_vars/para.sops.yaml.example inventories/prod/group_vars/para.sops.yaml
$EDITOR inventories/prod/hosts.yml inventories/prod/group_vars/para/vars.yml
$EDITOR inventories/prod/group_vars/para.sops.yaml   # los secretos
sops --encrypt --in-place inventories/prod/group_vars/para.sops.yaml
```

Si la máquina solo acepta contraseña, pásala a llaves una vez:

```bash
ansible-playbook -i inventories/prod/hosts.yml playbooks/bootstrap.yml \
  -u root --ask-pass -e bootstrap_pubkey="$(cat ~/.ssh/id_ed25519.pub)"
```

Luego, en seco y de verdad:

```bash
ansible-playbook -i inventories/prod/hosts.yml playbooks/site.yml --check --diff
ansible-playbook -i inventories/prod/hosts.yml playbooks/site.yml
```

(o desde la raíz: `make deploy INVENTORY=deploy/ansible/inventories/prod`).

En una máquina nueva `--check` se detiene en "Docker Engine y Compose": el
repositorio de apt todavía no existe.

## Lo que comprueba antes y después

- Antes de levantar, `make preflight`: dominio, versiones fijadas, secretos
  (presentes, largos, distintos), el bucket (escribe/lee/lista/borra y que no
  sea público) y DNS. Si falla, no levanta nada.
- Al final, `make verify` y su resumen: contenedores sanos, cada endpoint
  público con HTTPS, mubEZ replicando al bucket, respaldos corriendo. El
  playbook falla si algo de eso falla.

## Secretos

- Viven cifrados en `group_vars/para.sops.yaml` (al lado de `group_vars/para/`,
  no adentro). El rol escribe cada uno como archivo; nada secreto pasa por
  variables de entorno ni aparece en `docker inspect`.
- El servidor nunca inventa un secreto que falte: solo deriva las URLs de
  Postgres. Si falta uno, preflight falla y te dice cuál.
- Si cambia un secreto, el rol recrea los contenedores (`UP_ARGS=--force-recreate`).
- Los cinco que no tienen arreglo si se pierden (`scripts/secrets.sh list`,
  columna OFFLINE) guárdalos también fuera de la máquina y fuera del bucket.

## Workers de AppView

Agrega hosts al grupo `para_appview` con su `para_private_ip`. El rol escribe
en la principal `APPVIEW_WORKERS` (Caddy reparte entre todos) y en cada worker
solo los secretos de bsky, con `postgres_url` apuntando a la principal por la
red privada. Ver [docs/ESCALADO.md](../../docs/ESCALADO.md).
