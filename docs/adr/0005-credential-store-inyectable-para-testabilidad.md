# ADR-0005: `CredentialStore` inyectable para testabilidad

## Estado

Aceptado — 2026-06-07

Refina **ADR-0004** (no lo sustituye): la cascada de credenciales no cambia; se
hace inyectable y testeable.

## Contexto

> **Requisito de producto** (`SPEC.md` §3.2 y §17 "Resolución de credenciales"):
> cascada keyring → env → prompt enmascarado, y guardar tras conectar con éxito.
> El comportamiento observable **no cambia** con este ADR.

La implementación de **ADR-0004** quedó imposible de testear de forma hermética:
el orden de la cascada vivía inline en `App::start_connection`, llamando directo a
`password_from_keyring` (pega al keyring **real** del SO, sin costura para un
doble); y el guardar-tras-conectar estaba enterrado dentro del `tokio::spawn` de
`App::spawn_connect`, tras un `ConnectionManager::connect` real (inalcanzable sin
una BD). Solo se cubrían la rama env y la UI del prompt (hueco **#9** de
`docs/coverage.md`).

## Decisión

Introducir tres costuras que hacen el flujo testeable sin tocar el keyring real ni
una BD:

1. **Trait `CredentialStore`** (en `sextant-core`, junto a `QueryExecutor`):
   `get(key) -> Option<String>` / `set(key, password)`. Object-safe → se sostiene
   como `Arc<dyn CredentialStore>`. La impl de producción `KeyringStore`
   (`sextant-config`) es un adaptador fino sobre las funciones keyring de
   ADR-0004; `App` la inyecta por defecto y los tests sustituyen un doble en
   memoria.
2. **Función pura `resolve_password`** (`sextant-config`): recibe los valores ya
   buscados (keyring, env) y decide `Connect(Option<pw>)` o `Prompt{keyring_key}`.
   El **orden** de la cascada queda en código sin I/O → unit-testable.
   `start_connection` solo hace los dos lookups y un `match`.
3. **Guardado reubicado a un seam síncrono**: en vez de escribir el keyring dentro
   del connect async, `confirm_password_prompt` deja la pw en
   `App.pending_credential` y, al llegar `AppMsg::Connected`, el handler llama
   `persist_pending_credential`, que guarda vía el `CredentialStore` inyectado y
   lo limpia (un `ConnectionFailed` lo descarta). Así el guardado se testea en
   seco, sin construir el `SqlxExecutor` real del mensaje.

La infra genuinamente compleja —el backend keyring real del SO y el
connect+introspección reales a BD— se queda como **QA manual** detrás de estas
costuras (ver `docs/coverage.md`).

## Alternativas consideradas

- **Mock del crate `keyring`** (feature `mock`): testea la librería, no nuestra
  lógica, y su builder global de proceso complica los tests en paralelo.
- **Abstraer `ConnectionManager` tras un trait** para testear el guardado end-to-
  end con un connector falso: cruza fronteras de crate (la introspección vive en
  el `SqlxExecutor` concreto) y es desproporcionado; reubicar el efecto a un seam
  síncrono logra lo mismo sin ese coste.

## Consecuencias

- (+) Orden de la cascada, lookup/escritura de credenciales y guardar-tras-
  conectar quedan cubiertos por tests herméticos (sin Docker, sin keyring real,
  sin pool de BD).
- (+) El punto de credenciales es inyectable: futuros backends de secretos se
  añaden implementando `CredentialStore`.
- (−) La pw introducida vive transitoriamente en `App.pending_credential` hasta el
  `Connected` (misma ventana de exposición que antes; se limpia de inmediato).
- (−) Que un `Connected` real dispare el guardado sigue siendo verificación
  manual (depende del connect real), aunque el efecto en sí ya es testeable.
