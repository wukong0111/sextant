# ADR-0004: Credenciales vía keyring del SO

## Estado

Aceptado — 2026-06-07 (`f4da375`)

## Contexto

> **Requisito de producto** (en `SPEC.md` §3.2 y §16): las contraseñas nunca en
> el fichero de config; cascada keyring → variable de entorno → prompt
> enmascarado; guardar tras conectar con éxito. Este ADR registra **cómo** se
> implementa esa cascada.

Hace falta elegir el mecanismo concreto de almacenamiento e integración con el
sistema operativo que materializa esa cascada sin bloquear CI ni la conexión a
SQLite.

## Decisión

Resolver la contraseña al conectar con esta cascada, implementada en
`sextant-config` (servicio de keyring `"sextant"`):

1. **Keyring del SO** — si la conexión declara `keyring_key`, se busca con
   `password_from_keyring`.
2. **Variable de entorno** — fallback a `SEXTANT_<NAME>_PASSWORD` cuando no hay
   `keyring_key`. Acceptable para CI y los contenedores Docker de prueba.
3. **Prompt interactivo** — si hay `keyring_key` pero no hay secreto guardado (y
   no es SQLite), se pide en un popup enmascarado en la TUI.

Tras un connect exitoso desde el prompt, la credencial se guarda en el keyring
(`store_password_in_keyring`) para no volver a pedirla.

**Divergencia respecto a la spec:** no existe un flujo de "crear/editar
conexión" en la app —las conexiones se cargan de `connections.toml`—, así que el
guardado ocurre **al conectar correctamente** con la contraseña introducida, no
al dar de alta una conexión.

## Alternativas consideradas

- **Contraseñas en el archivo de config** (texto plano o cifrado con clave
  local) — descartado por política de seguridad del proyecto: nunca en config.
- **Solo variable de entorno** — funciona para CI pero es mala UX para uso
  interactivo diario; obliga a exportar secretos en cada shell.
- **Solo keyring, sin fallbacks** — rompe CI y los contenedores de prueba, donde
  no hay keyring disponible ni práctico.

## Consecuencias

- (+) Por defecto, las contraseñas viven solo en el keyring del SO.
- (+) El fallback por env-var mantiene CI y Docker sin fricción.
- (+) SQLite y conexiones sin contraseña no se ven afectadas.
- (−) El guardado atado al "connect exitoso" (y no a un alta de conexión) es
  menos descubrible; depende de que el usuario conecte una vez vía prompt.
- (−) Dependencia del crate `keyring` y del backend de secretos del SO, que
  puede no existir en entornos headless (de ahí el fallback por env-var).
