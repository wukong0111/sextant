# ADR-0007: Renderizado manual del grid en lugar de `ratatui::widgets::Table`

## Estado

Aceptado — 2026-06-09

## Contexto

> **Requisito de producto** (en `SPEC.md` §17.19): el grid debe permitir
> redimensionar columnas interactivamente con `>` / `<`, y las columnas que no
> caben completamente deben mostrarse truncadas en lugar de desaparecer.

Inicialmente el grid usaba `ratatui::widgets::Table` con `Constraint::Length`.
Esto funcionaba bien para pocas columnas, pero cuando el resultado tenía
muchas columnas (ej. la tabla `users` de `seeds/sqlite.sql` con 13 columnas)
surgieron dos problemas:

1. **Compresión silenciosa**: `Table` con `Flex::Legacy` (el default) y
   `Flex::Start` distribuyen el espacio sobrante entre todas las columnas.
   Cuando hay muchas columnas, cada una se comprime proporcionalmente para
   caber en el área. Como resultado, cambiar el `Constraint::Length` de una
   columna en 2 celdas no producía ningún cambio visual perceptible.

2. **Desaparición de columnas parciales**: al limitar las columnas pasadas al
   `Table` solo a las que cabían completamente (`visible_columns_count`), una
   columna que parcialmente cabía simplemente no se renderizaba.

## Decisión

Reemplazar `ratatui::widgets::Table` por **renderizado celda a celda** con
`Paragraph`:

- Iterar columna por columna desde `first_visible_column`.
- Para cada celda (header + data rows) renderizar un `Paragraph` en un
  `Rect` cuyo ancho es `col_width.min(remaining)`, donde `remaining` es el
  espacio que queda en el área del grid.
- Dejar que `Paragraph` trunque el texto automáticamente cuando el rectángulo
  es más estrecho que el contenido.
- Rellenar el fondo del grid con un `Block` vacío antes de pintar las celdas.

Esto da control total sobre el ancho de cada columna y el truncamiento de la
última columna visible.

## Alternativas consideradas

- **Seguir usando `Table` con `Flex::Start` y `visible_columns_count`** —
  resolvía la compresión, pero las columnas parciales desaparecían. No
  aceptable para el requisito de truncamiento.

- **Seguir usando `Table` y pasar todas las columnas** — `Flex::Start`
  comprime todas las columnas proporcionalmente cuando la suma excede el área,
  anulando cualquier override de ancho.

- **Renderizar la columna parcial como overlay manual sobre `Table`** —
  posible, pero introduce complejidad de alineación (coordenadas X/Y, estilos
  de fila, etc.) sin ganancia clara sobre renderizar todo manualmente.

- **Usar `Constraint::Min` o `Max` en `Table`** — no evitan la compresión
  cuando el total de constraints excede el área; solo cambian la prioridad de
  distribución del espacio sobrante.

## Consecuencias

- (+) Los cambios de ancho de columna (`>` / `<`) son **siempre visibles**,
  independientemente del número de columnas.
- (+) La última columna visible se **trunca** limpiamente en lugar de
  desaparecer.
- (+) No hay layout flex involucrado; cada columna ocupa exactamente el ancho
  que le corresponde.
- (−) Perdemos el scroll vertical automático de `Table`. El grid actual ya no
  lo usaba (no hay `TableState`), así que la equivalencia funcional se mantiene.
- (−) El código de renderizado es más largo (~60 líneas) que la construcción
  de un `Table`, pero sigue estando contenido en un solo método.
