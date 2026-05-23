# Plan de Desarrollo: `sextant`

## Estado Actual

- `Cargo.toml`: reestructurado como workspace con 5 crates.
- `src/main.rs`: eliminado; entry point movido a `crates/sextant-cli/src/main.rs`.
- Especificación completa en `sextant-spec.md` con arquitectura, stack y roadmap.

## Progreso

| Fase | Estado | Commit |
|------|--------|--------|
| Fase 0 — Cimentación | ✅ Completada | `7cdf1cb` (initial), `1c55742` (correcciones) |
| Fase 1 — v0.1 MVP | ⬜ Pendiente | — |
| Fase 2 — v0.2 | ⬜ Pendiente | — |
| Fase 3 — v1 | ⬜ Pendiente | — |

## Principios Directores

1. **Workspace-first desde el inicio**: aunque v0.1 solo necesite 2-3 crates, se estructura como workspace para evitar refactor masivo después.
2. **Cada fase compila y es ejecutable**: nunca dejamos el repo en un estado roto.
3. **Tests desde el crate `sextant-db`**: la capa de servicios (DB, SQL, state) debe ser testeable sin TUI.
4. **Async first con `tokio`**: todas las operaciones de red/E/S usan `tokio::spawn` + `mpsc`.

---

## Fase 0 — Cimentación (Infraestructura del Workspace)

**Objetivo**: Tener el esqueleto del workspace Cargo, CI básica y un binario que arranque `ratatui` sin lógica.

### Tareas

1. ✅ **Reestructurar `Cargo.toml` raíz como workspace**:
   ```toml
   [workspace]
   members = ["crates/*"]
   resolver = "3"
   ```

2. ✅ **Crear crates vacíos** (placeholders — solo `Cargo.toml` + `lib.rs` vacío):
   - `crates/sextant-core/` — placeholder. Tipos de dominio se añaden en Fase 1 cuando se usan.
   - `crates/sextant-db/` — placeholder para drivers sqlx.
   - `crates/sextant-ui/` — loop de eventos TEA, componentes ratatui base.
   - `crates/sextant-config/` — placeholder para carga de TOML + paths XDG.
   - `crates/sextant-cli/` — entry point (`main.rs`).

3. ✅ **`sextant-cli` depende solo de `sextant-ui`**. `sextant-ui` no depende de ningún otro crate interno en Fase 0.

5. ✅ **Implementar "loop vacío" TUI**:
   - `crossterm` para input.
   - `ratatui` para render.
   - Pantalla negra con status line falso (`NOR │ no connection │ q to quit`).
   - Manejo de `Ctrl+Q` para salir limpiamente.

6. ✅ **Configurar `tracing` + `color-eyre`** en el binario para logging/errores amigables.

### Criterio de éxito ✅
`cargo run` abre una TUI negra con status line y sale con `Ctrl+Q` sin panic. Verificado con `screen` + captura de pantalla + exit code 0.

### Nota sobre testing de TUI
- **Unitarios**: usar `ratatui::backend::TestBackend` para testear el renderizado sin necesidad de un TTY real.
- **Integración**: usar `screen` para crear un pseudo-tty donde `crossterm` pueda leer eventos; enviar `Ctrl+Q` (`\x11`) y verificar exit code 0.

---

## Fase 1 — v0.1 MVP: Conexiones + Editor Básico + Grid Read-Only

**Objetivo**: Conectar a PostgreSQL y SQLite, ejecutar SQL básico, ver resultados en grid, editor modal simple.

### 1.0 Tipos base en `sextant-core`

Definir solo lo que Fase 1 necesita (nada especulativo):
- `enum Driver { Postgres, Mysql, Sqlite }`
- `struct Connection { name, driver, host, port, user, database, ssl_mode, path, keyring_key }`
- `enum CellValue { Null, Bool, I64, F64, String, Bytes }`
- `struct Column { name, type_name }`
- `struct QueryResult { columns, rows, rows_affected }`
- `trait QueryExecutor` con `async fn execute(&self, sql: &str) -> Result<QueryResult, SextantError>`
- `enum SextantError` usando `thiserror` (se añade como dependencia aquí, no antes).

### 1.1 Capa de Configuración (`sextant-config`)

- Cargar `connections.toml` desde `~/.config/sextant/connections.toml`.
- Resolver paths XDG con `dirs` crate.
- Validar esquema básico (campos requisito por driver).
- **NO implementar keyring todavía** — leer contraseña de variable de entorno `SEXTANT_<NAME>_PASSWORD` como fallback para v0.1.

### 1.2 Capa de Drivers (`sextant-db`)

- Dependencias: `sqlx` con features `runtime-tokio`, `postgres`, `sqlite`.
- `struct SqlxExecutor { pool: Pool<Any> }` (usa `Any` para unificar PG y SQLite).
- Implementar `QueryExecutor` para `SqlxExecutor`:
  - Mapear `sqlx::any::AnyRow` → `Vec<CellValue>` según el tipo SQL.
  - Soportar `SELECT` (devuelve `QueryResult`) y comandos DDL/DML (devuelve `QueryResult` vacío con `rows_affected`).
- **Pool por conexión activa**: `HashMap<String, Pool<Any>>` manejado por un `ConnectionManager`.

### 1.3 Árbol de Conexiones (sidebar)

- Componente `TreePane` en `sextant-ui`.
- Lista plana de conexiones desde `sextant-config`.
- Seleccionar con `j/k`, conectar con `Enter`.
- Una vez conectada, expandir para mostrar: `database → schemas → tablas` (introspección mínima vía `information_schema` en PG; `sqlite_master` en SQLite).
- Render: texto plano, sin iconos todavía.

### 1.4 Editor SQL Modal (básico)

- Componente `EditorModal`:
  - Usar `tui-textarea` o implementación propia ligera (recomendación: `tui-textarea` para v0.1, migrar a custom con tree-sitter en v0.2).
  - Solo **Insert mode** y **Normal mode** simplificado:
    - `i` → insert, `Esc` → normal.
    - `Ctrl+Enter` ejecuta SQL.
    - `Ctrl+S` guarda buffer (solo en memoria por ahora).
    - `Esc` cierra modal.
  - Sin syntax highlight, sin autocomplete.
  - Buffer por conexión (1 solo buffer en v0.1).

### 1.5 Grid de Resultados (read-only)

- Componente `ResultGrid`:
  - Recibe `QueryResult`, renderiza tabla con `ratatui::widgets::Table`.
  - Paginación fija: cargar todo en memoria (limit 1000 rows por ahora).
  - Navegación: `h/j/k/l` para mover celdas, `gg/G` para arriba/abajo.
  - Ajuste de ancho de columna automático (max content o 40 chars).
  - Status line actualiza: `NOR │ local-pg │ 142 rows / 38ms │ <space>e`.

### 1.6 Event Loop + Mensajes

- Mensajes clave:
  ```rust
  enum AppMsg {
      Connect(String),                    // nombre de conexión
      Disconnect,
      ExecuteSql(String),                 // SQL a ejecutar
      QueryResult(QueryResult),
      QueryError(String),
      ToggleEditor,
      EditorInput(EditorEvent),
      Quit,
  }
  ```
- `tokio::spawn` para `ExecuteSql`; resultado vuelve por `mpsc::UnboundedSender<AppMsg>`.
- Estado global (`AppState`) con modal actual (`Normal | Insert | EditorOpen`).

> **Nota de arquitectura (decisión tomada, Fase 0):** migrar de loop síncrono (`event::poll` + `event::read`) a loop **async híbrido** con `tokio::select!`. Fuentes: `crossterm::event::EventStream` (input), `mpsc` (resultados de query), timers (swap, cursor blink). Renderizar solo cuando `needs_redraw == true` o haya un tick de animación (spinner/cursor). Ver análisis completo en issue/discusión previa.

### Criterio de éxito v0.1
- Arrancar, ver lista de conexiones.
- Conectar a PG/SQLite.
- Abrir editor (`<Space>e`), escribir `SELECT * FROM users`, `Ctrl+Enter`.
- Ver resultados en grid.
- Navegar grid con `hjkl`.
- Desconectar, salir.

---

## Fase 2 — v0.2: MySQL + Autocomplete + Edición Inline + Schema Viewer

**Objetivo**: Completar los 3 drivers, editor con syntax highlighting, CRUD en grid, árbol con DDL.

### 2.1 Driver MySQL

- Añadir feature `mysql` a `sqlx` en `sextant-db`.
- Ajustar mapeo de tipos (`DECIMAL`, `DATETIME`, `JSON`).
- Introspección vía `information_schema` (MySQL 8+).

### 2.2 Syntax Highlighting

- Integrar `tree-sitter` + `tree-sitter-sql`.
- Reemplazar `tui-textarea` por editor propio (`EditorBuffer`):
  - Buffer como `Vec<Rope>` o `Vec<String>` (lineas).
  - Tree-sitter parsea en background, genera spans de estilo.
  - Render con `ratatui::text::Line` + `Span` con estilos por token (keyword, string, number, comment).

### 2.3 Autocomplete

- `AutocompleteEngine` en `sextant-sql` (usa `sqlparser-rs` para parsear AST parcial).
- Contextos:
  - Tablas/vistas del schema actual.
  - Columnas de tablas en `FROM`/`JOIN`.
  - Keywords SQL + funciones por dialecto.
- Popup flotante sobre el editor (`<C-Space>` para trigger manual; también automático tras `.` o espacio).
- Cache de metadata del schema (refrescada al conectar o manualmente).

### 2.4 Grid Editable (CRUD)

- Introspección de PK por tabla: consultar `information_schema` / `pragma table_info`.
- Si tabla tiene PK: grid editable. Sin PK: read-only con `🔒` en status line.
- **Inline editing**:
  - `Enter` en celda → modo Insert (celda resaltada).
  - Escribir valor, `Enter` o `Esc` para confirmar a "pending changes".
  - Celda modificada se marca visualmente (color diferente o `*`).
- **Operaciones**:
  - `o` — insertar fila vacía al final.
  - `dd` — marcar fila para eliminar (rojo/strikethrough).
  - `Ctrl+S` — commit (abre confirmación: ver cambios, confirmar/cancelar).
  - `Ctrl+Z` — descartar todos los pending changes.
- **Commit**:
  - Generar `UPDATE`, `INSERT`, `DELETE` con WHERE por PK.
  - Ejecutar en transacción (BEGIN → statements → COMMIT).
  - Optimistic concurrency: si la fila fue modificada por otro, mostrar error.

### 2.5 Schema Viewer + DDL

- Árbol enriquecido:
  - Expandir tabla → `Columns`, `Indexes`, `Constraints`, `Foreign Keys`.
  - `Enter` en tabla → browse rows (grid).
  - `Enter` en columna/índice → detalle en panel (a definir: split horizontal o popup).
- `D` en tabla → emitir `CREATE TABLE` skeleton al editor (abre editor modal con el DDL).
- Generar DDL básico desde metadata (tipos, defaults, constraints).

### 2.6 Buffer Management (múltiples tabs)

- Múltiples buffers por conexión.
- `<Tab>` / `<S-Tab>` para ciclar buffers dentro del editor modal.
- Dirty marker `●` en nombre de tab.
- Guardar a archivo `.sql` (`<C-s>` o `:w path`).
- Confirmar al salir si hay buffers dirty.

### Criterio de éxito v0.2
- Conectar a MySQL, PG, SQLite indistintamente.
- Editor con SQL coloreado y autocomplete funcional.
- Editar celdas, insertar filas, borrar, y commitear cambios.
- Ver schema expandido y generar DDL.

---

## Fase 3 — v0.3: Export/Import + Historial + Polish (v1)

**Objetivo**: Feature parity con la especificación v1, pulido de UX.

### 3.1 Export / Import (`sextant-db` + `sextant-ui`)

- **Export**:
  - CSV (`csv` crate, RFC 4180, delimiter configurable).
  - JSON (`serde_json`, array de objetos; opción NDJSON).
  - SQL dump (`INSERT` statements; schema-only / data-only opcional).
  - Trigger: comando `:export` o keybinding desde grid.
  - Async con barra de progreso (para tablas grandes).
- **Import**:
  - CSV/JSON/SQL → preview de mapeo de columnas.
  - Validar tipos antes de importar.
  - Async con progreso.

### 3.2 Query History + Snippets

- `state.db` (SQLite local) en `~/.local/share/sextant/state.db`.
- Tablas:
  - `query_history` (timestamp, connection, sql, duration_ms, error_msg).
  - `recent_files` (connection, path, last_opened; ring de 20).
- Comando `:history` → popup con lista ejecutable (Enter para cargar al editor).
- Recent files: `<Space>r` para popup por conexión.

### 3.3 Transacciones (Hybrid psql-style)

- Status line muestra `txn: auto` (gris) o `txn: ACTIVE` (ámbar).
- Si el usuario ejecuta `BEGIN`, pasar a `txn: ACTIVE`.
- En `ACTIVE`, cada statement va sin auto-commit hasta `COMMIT`/`ROLLBACK`.
- Grid edits siempre en transacción propia, independiente.
- Confirmación modal para operaciones destructivas (`DELETE`/`UPDATE` sin `WHERE`, DDL).

### 3.4 Keymap Remapeable + Themes

- Cargar `keys.toml` desde `~/.config/sextant/keys.toml`.
- Estructura: `[[binding]] mode = "normal" keys = "<Space>e" action = "toggle_editor"`.
- Defaults hardcodeados + merge con user overrides.
- Themes: `dark` y `light` built-in; custom `.toml` en `themes_dir`.
- `Theme` struct con colores para cada rol (background, foreground, accent, error, etc.).
- Aplicar a todos los componentes.

### 3.5 Swap Files + Recovery

- Cada 30s, si buffer dirty: escribir `.swp` en `~/.local/state/sextant/swap/`.
- Formato: contenido SQL + JSON con cursor/selection.
- Al arrancar, escanear `.swp` huérfanos → prompt de recovery.
- Borrar `.swp` en quit limpio o al guardar.
- Permisos `0600`.

### 3.6 Credentials via Keyring

- Integrar `keyring` crate.
- Al conectar, buscar password en keyring por `keyring_key`.
- Fallback a prompt interactivo (popup en TUI) si no existe.
- Guardar nueva credencial en keyring al guardar conexión.

### 3.7 Polish Final

- Help overlay (`<Space>?`) con cheatsheet dinámico.
- Command palette (`<Space>:`) con fuzzy finder (`nucleo` o `fuzzy-matcher`).
- Fuzzy find de tablas/columnas (`<Space>f`).
- Open file (`<Space>o`) con fuzzy finder sobre `queries_dir`.
- Animaciones/spinners mínimas para queries largas.
- Revisión de accesibilidad: contraste, indicadores claros.

### Criterio de éxito v1
- Todo lo descrito en `sextant-spec.md` §5 funciona.
- Tests unitarios en `sextant-db`, `sextant-sql`, `sextant-state`.
- `cargo test` pasa en CI.
- Release build funcional para Linux y macOS.

---

## Fase 4 — Post-v1 (No planificado aquí)

- ER diagrams, `EXPLAIN` visualizer.
- Plugin system.
- SSH tunneling.
- Drivers adicionales (MSSQL, Oracle, ClickHouse).

---

## Dependencias por Fase

| Fase | Nuevas dependencias (crates) |
|------|------------------------------|
| 0 | `ratatui`, `crossterm`, `tokio`, `tracing`, `tracing-subscriber`, `color-eyre` |
| 1 | `sqlx`, `serde`, `toml`, `dirs` |
| 2 | `tree-sitter`, `tree-sitter-sql`, `sqlparser`, `tui-textarea` (temporal) |
| 3 | `csv`, `serde_json`, `keyring`, `nucleo`/`fuzzy-matcher` |

---

## Notas de Implementación

- **Rendimiento**: el grid read-only de v0.1 puede cargar todo en memoria porque `ratatui::Table` requiere todas las filas. Para v0.2+, considerar virtualización (renderizar solo filas visibles) si hay tablas >10k rows.
- **SQLx `Any` driver**: simplifica mucho el código unificado, pero hay que verificar que soporta bien los tipos de los 3 backends. Si hay problemas (ej. JSON en SQLite vs PG), caer a pools tipados por driver con enum wrapper.
- **Editor propio vs `tui-textarea`**: `tui-textarea` acelera v0.1 pero no soporta syntax highlighting nativo. La migración a editor propio es necesaria en v0.2. Valorar si conviene invertir directamente en el editor propio desde v0.1 para evitar la migración.
- **Recursión en event loop**: usar `mpsc::UnboundedChannel` para evitar backpressure; el render corre en el thread principal, el trabajo pesado en `tokio` tasks.
