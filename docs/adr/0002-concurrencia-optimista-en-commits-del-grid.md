# ADR-0002: Concurrencia optimista en los commits del grid

## Estado

Aceptado — 2026-06-07 (`d35734e`)

## Contexto

> **Requisito de producto** (en `SPEC.md` §7.5): las ediciones concurrentes del
> grid no deben perderse en silencio, y sin usar locks ni transacciones largas.
> Este ADR registra **cómo** se implementa ese requisito en este código.

El grid editable confirma en lote varias filas en una única transacción. Entre
cargar la tabla y confirmar pueden pasar minutos, así que hay que detectar si la
fila cambió bajo nuestros pies en el momento del commit.

## Decisión

Aplicar **concurrencia optimista** en el `WHERE` de cada statement de commit:

- `UPDATE` empareja la PK **más los valores originales de las columnas editadas**.
- `DELETE` empareja la **fila original completa**.
- `where_match` emite `col IS NULL` para los valores NULL (no `col = NULL`).

No se mantiene ningún lock ni transacción abierta mientras el usuario edita.

## Alternativas consideradas

- **Locking pesimista (`SELECT … FOR UPDATE`)** — exigiría una transacción
  abierta durante toda la edición interactiva; mal encaje con una TUI donde el
  usuario puede dejar el grid a medias indefinidamente, y bloquea a otros.
- **Last-write-wins (WHERE solo por PK)** — trivial, pero pierde en silencio los
  cambios de la otra sesión. Inaceptable para una herramienta de base de datos.
- **Columna de versión / timestamp optimista** — robusto, pero asume que cada
  tabla tiene esa columna; `sextant` opera sobre esquemas arbitrarios del usuario.

## Consecuencias

- (+) No se sobrescriben ni borran cambios ajenos de forma silenciosa.
- (+) Sin transacciones de larga duración ni locks que afecten a otras sesiones.
- (+) Funciona sobre cualquier tabla con PK, sin requerir columnas especiales.
- (−) El `WHERE` es más ancho y depende de manejar NULL con `IS NULL`.
- (−) **Limitación conocida**: hoy un conflicto se manifiesta como "0 filas
  afectadas" (la fila no cambia tras refrescar), sin un error explícito al
  usuario. Surfacing del conflicto queda como follow-up.
