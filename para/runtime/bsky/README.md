# Entradas del AppView, montadas sobre la imagen de WatZappa

`api.js` y `dataplane.js` son copias de
[WatZappa#13](https://github.com/mexico-parasocial/WatZappa/pull/13). En
`main` de WatZappa, `api.js` no arranca (usa `require()` en un paquete ESM) y
nada sirve el dataplane ni indexa el firehose. Los compose de `para/` las
montan en `/app/services/bsky/` de la imagen, sin cambiar WatZappa.

La imagen del AppView todavía no se puede construir desde Git: el build de
`@atproto/oauth-provider-ui` falla porque pnpm no instala el binario de
rolldown para la plataforma (un problema que ya existía, aparte del PR). Por
eso se usa una imagen local (`BSKY_IMAGE`, por defecto `para-core-bsky:latest`).
Cuando ese build funcione y el PR se mezcle, estos montajes sobran y el
AppView se construye desde Git, como ya se hace con bsync.
