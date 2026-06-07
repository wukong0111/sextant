# Roadmap — sextant

**v1 entregada** (Fases 0–3: workspace, conexiones PG/MySQL/SQLite, editor SQL,
grid editable con concurrencia optimista, autocomplete, schema viewer + DDL,
export/import, historial + snippets, transacciones psql-style, themes, keymap
remapeable, swap/recovery, keyring, command palette).

La historia fase-por-fase y los hashes viven en **`git log`**. El *qué* canónico
está en `SPEC.md`, las decisiones de implementación en `docs/adr/`, y la
cobertura de tests en `docs/coverage.md`. Este documento mira **hacia delante**:
backlog diferido y trabajo post-v1.

---

## Backlog diferido (v1+)

Items recortados de la v1 por ratio valor/coste; ninguno bloquea el release.

- **Detalle de columna/índice en panel** — `Enter` sobre una columna o índice en
  el árbol abre un panel de detalle. Sin diseño aún.
- **Export: opciones** — delimitador CSV configurable, NDJSON, schema-only.
- **Import: remapeo interactivo de columnas** — hoy el mapeo es por nombre
  (case-insensitive) y read-only; falta poder remapear a mano.
- **Barra de progreso** en export/import — no crítica mientras el result set
  viva en memoria (≤500–1000 filas).

---

## Fase 4 — Post-v1

- **Syntax highlighting (editor propio + tree-sitter).** La tarea de peor ratio
  valor/coste. `tui-textarea 0.7` tiene `line_spans` en `pub(crate)`, así que
  colorear por token obliga a **reemplazarlo por un editor propio**
  (`EditorBuffer` con `Vec<String>`: cursor, insert/delete, UTF-8 multibyte,
  scroll, selección) y montar encima `tree-sitter` + `tree-sitter-sql` (parseo
  síncrono; los buffers SQL son pequeños). Al migrar habrá que reintegrar el
  autocomplete (2.3) y los tabs (2.5) sobre el nuevo editor.
- **Asistente IA opt-in.** Generar/explicar queries en lenguaje natural.
  Proveedor/modelo en `config.toml`, token en keyring/env. Sin configurar → la
  feature no aparece y `sextant` es 100% offline. **Complementa** (no sustituye)
  al autocomplete local, que sigue siendo la fuente fiable de nombres del schema
  (la IA puede alucinar columnas).
- **Otros**: ER diagrams, visualizador de `EXPLAIN`, sistema de plugins, SSH
  tunneling, drivers adicionales (MSSQL, Oracle, ClickHouse).

---

## Notas a futuro

- **Virtualización del grid.** Hoy el grid carga todo en memoria porque
  `ratatui::Table` requiere todas las filas. Para tablas >10k filas, considerar
  renderizar solo las visibles.
