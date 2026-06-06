# Plan de Desarrollo: `sextant`

## Estado Actual

- `Cargo.toml`: reestructurado como workspace con 5 crates.
- `src/main.rs`: eliminado; entry point movido a `crates/sextant-cli/src/main.rs`.
- Especificación completa en `sextant-spec.md` con arquitectura, stack y roadmap.

## Progreso

| Fase | Estado | Commit |
|------|--------|--------|
| Fase 0 — Cimentación | ✅ Completada | `7cdf1cb` (initial), `1c55742` (correcciones) |
| Fase 1 — v0.1 MVP | ✅ Completada | `fbee360` (1.1 config), `6dfb9cf` (1.2 db), `6315a32` (1.3 sidebar), `aa94722` (1.4 editor), `3b14373` (1.5 grid), `afb16cc` (1.5 fixes), `9615337` (1.6 event loop), `53f57a7` (fix grid highlight + cursor), `4a2636e` (fix SQLite BOOLEAN) |
| Fase 2 — v0.2 | ✅ Completada | `432d8df` (2.1 MySQL), `2778f89` (2.1 Docker tests + tipos), `ce9aa8d` (2.1 PG 18.4 / MySQL 9.7), `e576d47` (2.1 fix PG imagen), `4f8cd49` (2.1 conexiones Docker TUI), `8682aba` (2.1 fix passwords Docker), `b634a43` (2.1 fix .env + SQLite), `ae628b4` (2.1 fix MySQL introspection column names), `b4aae24` (2.1 Docker DB seeds), `91ea5c6` (2.1 SQLite seed + file conn), `b3d7e43` (2.1 untrack test.db), `e354de5` (2.1 rich type seeds), `207770b` (2.1 test schema cleanup), `76858c7` (chore: normalización fmt/clippy toolchain 1.96), `2de33b5` (base: introspección de columnas + PK + cache), `c826b0c` (base: quote_ident + DDL `CREATE TABLE`), `b8bc78f` (2.4 columnas en árbol + browse rows + DDL), `95b4427` (2.3 autocomplete), `aa662b0` (2.2 base: transacciones + DML gen), `4d7bfac` (2.2 grid editable CRUD), `bf1a892` (2.5 multi-buffer tabs), `f0c5232` (2.4 índices/FKs en árbol), `b7e6148` (2.5 guardado .sql + prompt al salir) |
| Fase 3 — v1 | 🚧 En progreso | `d88ddc3` (3.2 query history + recent files), `96eea9f` (3.1 export CSV/JSON/SQL) |

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

### 1.0 Tipos base en `sextant-core` ✅ (`da4546e`)

Definir solo lo que Fase 1 necesita (nada especulativo):
- `enum Driver { Postgres, Mysql, Sqlite }`
- `struct Connection { name, driver, host, port, user, database, ssl_mode, path, keyring_key }`
- `enum CellValue { Null, Bool, I64, F64, String, Bytes }`
- `struct Column { name, type_name }`
- `struct QueryResult { columns, rows, rows_affected }`
- `trait QueryExecutor` con `fn execute(&self, sql: &str) -> impl Future<Output = Result<QueryResult, SextantError>> + Send` (desugared para garantizar bounds `Send` sin warnings del compilador)
- `enum SextantError` usando `thiserror` (se añade como dependencia aquí, no antes).

### 1.1 Capa de Configuración (`sextant-config`) ✅ (`fbee360`)

- Cargar `connections.toml` desde `~/.config/sextant/connections.toml`.
- Resolver paths XDG con `dirs` crate.
- Validar esquema básico (campos requisito por driver).
- **NO implementar keyring todavía** — leer contraseña de variable de entorno `SEXTANT_<NAME>_PASSWORD` como fallback para v0.1.

### 1.2 Capa de Drivers (`sextant-db`) ✅ (`6dfb9cf`)

- Dependencias: `sqlx` con features `runtime-tokio`, `postgres`, `sqlite`.
- `struct SqlxExecutor { pool: DbPool }` donde `DbPool` es un enum con variantes tipadas (`PgPool`, `SqlitePool`).
  - Decisión técnica: `sqlx::Any` no soporta el tipo `BOOLEAN` de SQLite (`SqliteTypeInfo(Bool)`), lo que hacía fallar `fetch_all` directamente. Usar pools tipados por backend resuelve el problema y prepara el terreno para MySQL en Fase 2.
- Implementar `QueryExecutor` para `SqlxExecutor`:
  - Mapear filas según el tipo SQL, con fallback ordenado: `bool` (solo si `type_info` indica booleano) → `i64` → `f64` → `String` → `Bytes`.
  - Soportar `SELECT` (devuelve `QueryResult`) y comandos DDL/DML (devuelve `QueryResult` vacío con `rows_affected`).
- **Pool por conexión activa**: `HashMap<String, DbPool>` manejado por un `ConnectionManager`.

### 1.3 Árbol de Conexiones (sidebar) ✅

- Componente `TreePane` en `sextant-ui`.
- Lista plana de conexiones desde `sextant-config`.
- Seleccionar con `j/k`, conectar con `Enter`.
- Una vez conectada, expandir para mostrar: `database → schemas → tablas` (introspección mínima vía `information_schema` en PG; `sqlite_master` en SQLite).
- Render: texto plano, sin iconos todavía.

### 1.4 Editor SQL Modal (básico) ✅

- Componente `EditorModal`:
  - Usar `tui-textarea` o implementación propia ligera (recomendación: `tui-textarea`; la migración a editor propio con tree-sitter se pospuso a post-v1).
  - Solo **Insert mode** y **Normal mode** simplificado:
    - `i` → insert, `Esc` → normal.
    - `Ctrl+Enter` ejecuta SQL.
    - `Ctrl+S` guarda buffer (solo en memoria por ahora).
    - `Esc` cierra modal.
  - Sin syntax highlight, sin autocomplete.
  - Buffer por conexión (1 solo buffer en v0.1).

### 1.5 Grid de Resultados (read-only) ✅

- Componente `ResultGrid`:
  - Recibe `QueryResult`, renderiza tabla con `ratatui::widgets::Table`.
  - Paginación fija: cargar todo en memoria (limit 1000 rows por ahora).
  - Navegación: `h/j/k/l` para mover celdas, `gg/G` para arriba/abajo.
  - Ajuste de ancho de columna automático (max content o 40 chars).
  - Status line actualiza: `NOR │ local-pg │ 142 rows / 38ms │ <space>e`.

### 1.6 Event Loop + Mensajes ✅

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

**Objetivo**: Completar los 3 drivers, autocomplete, CRUD en grid, y árbol con DDL + browse rows. (El syntax highlighting se movió a post-v1.)

### 2.1 Driver MySQL ✅ (`432d8df`)

- [x] Añadir feature `mysql` a `sqlx` en `sextant-db`.
- [x] Ajustar mapeo de tipos (`DECIMAL`, `DATETIME`, `JSON` → `String` vía `BigDecimal`/`Decimal`, `chrono`, `JsonValue`).
- [x] Introspección vía `information_schema` (MySQL 8+).
- [x] Entorno Docker (`compose.yml` + `Makefile`) para tests de integración PG/MySQL.

> **Orden de prioridad (revisado).** La Fase 2 se reordenó por ratio **valor/coste**: primero lo que aporta más valor funcional con menos riesgo. El syntax highlighting **salió de la fase** (movido a post-v1, Fase 4) porque es lo más caro (obliga a un editor propio) y lo de menor valor (su utilidad es *leer*, no escribir). El autocomplete se mantiene porque su metadata ya está resuelta por la introspección existente de `sextant-db`.

### 2.2 Grid Editable (CRUD) ✅ (`aa662b0`, `4d7bfac`)

- [x] ✅ Introspección de PK por tabla (reusa `introspect_columns` de la base; `pk_columns` en `EditContext`).
- [x] ✅ Con PK: grid editable. Sin PK / resultado ad-hoc del editor: read-only con `🔒` en status line.
- [x] ✅ **Inline editing**: `Enter` en celda → modo edición; teclear, `Enter` confirma a pending, `Esc` cancela; celda modificada marcada (color), filas nuevas en verde, borradas tachadas en rojo.
- [x] ✅ **Operaciones**: `o` fila vacía al final, `dd` marcar borrado (toggle), `Ctrl+S` commit (modal de confirmación con los statements), `Ctrl+Z` descartar.
- [x] ✅ **Commit**: `build_update`/`build_insert`/`build_delete` con WHERE por PK (valores originales), ejecutado en transacción (`execute_transaction`); refresco re-ejecutando el browse.
- [ ] ⬜ Optimistic concurrency (chequeo de valores originales en WHERE) — **diferido a Fase 3** por decisión de alcance (v0.2 usa WHERE solo por PK).

### 2.3 Autocomplete (básico) ✅ (`95b4427`)

- [x] ✅ `autocomplete` (`sextant-ui`) basado en **tokenización ligera + heurísticas** (token anterior al cursor), **sin** AST/`sqlparser`.
- [x] ✅ Contextos: tras `FROM`/`JOIN`/`UPDATE`/`INTO` → tablas; tras `tabla.`/`alias.` → columnas; en otro caso → keywords SQL.
- [x] ✅ Resolución de alias (`FROM users u … u.`) escaneando el buffer completo (el *stretch* sí se implementó; subqueries no).
- [x] ✅ Popup flotante sobre el editor cerca del cursor (`<C-Space>` manual + automático tras `.`); navegación ↑/↓, aceptar Tab/Enter, descartar Esc, live-filter al teclear.
- [x] ✅ Metadata reutilizando el cache de introspección (`App.table_meta`), pasada al editor al abrirlo.

### 2.4 Schema Viewer + DDL ✅

> **Base compartida** (`2de33b5`, `c826b0c`): `SqlxExecutor::introspect_columns` (columnas + tipos + nullable + default + PK por dialecto) cacheado en `App.table_meta` al conectar, y helpers `quote_ident`/`generate_create_table` en `sextant-db::sql`.

- [x] ✅ Árbol enriquecido — **columnas**: `l`/`→` expande la tabla y muestra columnas (nombre, tipo, marca `PK`) desde el cache; `h` colapsa. (`b8bc78f`)
- [x] ✅ `Enter` en tabla → browse rows: `SELECT * FROM {tabla} LIMIT 500` en el grid (helper `run_sql`). (`b8bc78f`)
- [x] ✅ `D` en tabla → emitir `CREATE TABLE` skeleton al editor desde metadata (tipos, NOT NULL, defaults, PK). (`b8bc78f`)
- [x] ✅ **Indexes / Foreign Keys** en el árbol (`introspect_table_detail` perezoso al expandir + `AppMsg::TableDetailLoaded`, render bajo las columnas con `⚿`/`→`). (`f0c5232`)
- [ ] ⬜ `Enter` en columna/índice → detalle en panel (a definir; post-v0.2).

> **Divergencia respecto al plan original**: `l`=expandir y `Enter`=browse se separan (antes el plan decía "Enter o l → browse"), siguiendo el spec §9 (Tree Pane: `h`/`l` colapsar/expandir, `<Enter>` abrir objeto). Indexes/FKs se sacan del primer commit y quedan como sub-tarea pendiente de 2.4.

### 2.5 Buffer Management (múltiples tabs) ✅ (`bf1a892`, `b7e6148`)

- [x] ✅ Múltiples buffers (`Vec<Buffer>` sobre `tui-textarea`), buffer activo.
- [x] ✅ `<Tab>` / `<S-Tab>` ciclan buffers (modo Normal); `<C-t>` abre uno nuevo.
- [x] ✅ Barra de tabs arriba del modal con nombre de archivo / índice + marca `●` de dirty y resaltado del activo (dirty por buffer).
- [x] ✅ Guardar a archivo `.sql` (`<C-s>` con prompt de nombre la primera vez; `sextant-config::write_query` con perms 0600 / dir 0700).
- [x] ✅ Confirmar al salir (`<C-q>`) si hay buffers dirty (save/discard/cancel).

### Criterio de éxito v0.2
- Conectar a MySQL, PG, SQLite indistintamente.
- Editar celdas, insertar filas, borrar, y commitear cambios.
- Autocomplete funcional (tablas + columnas).
- Ver schema expandido, browse rows (`SELECT * ... LIMIT 500`) y generar DDL.

---

## Fase 3 — v0.3: Export/Import + Historial + Polish (v1)

**Objetivo**: Feature parity con la especificación v1, pulido de UX.

### 3.1 Export / Import (`sextant-db` + `sextant-ui`)

> **Base compartida**: módulo puro `sextant-db::export` (`ExportFormat` +
> `to_csv`/`to_json`/`to_sql`), sin I/O, sobre el `QueryResult` en memoria.
> Reutiliza `sql::quote_ident` para el dump SQL (literales type-aware: números y
> booleanos sin comillas, NULL desnudo, hex para `Bytes`). El destino se escribe
> en `sextant-config::exports_dir()` (`$XDG_DATA_HOME/sextant/exports`) con
> permisos `0700`/`0600` vía `write_export`.

- **Export** ✅:
  - [x] ✅ CSV (`csv` crate, RFC 4180; NULL → campo vacío).
  - [x] ✅ JSON (`serde_json`, array de objetos por fila, valores tipados).
  - [x] ✅ SQL dump (`INSERT` statements por fila, identificadores por dialecto).
  - [x] ✅ Trigger: `<Space>x` → picker de formato (CSV/JSON/SQL); export
    asíncrono (`tokio::spawn`), ruta confirmada en la status line.
  - [ ] ⬜ Delimitador CSV configurable / NDJSON / schema-only — diferido.
  - [ ] ⬜ Barra de progreso (results en memoria ≤500–1000 filas; no crítico).
- **Import** ⬜ (tarea siguiente):
  - CSV/JSON/SQL → preview de mapeo de columnas.
  - Validar tipos antes de importar.
  - Async con progreso.

> **Divergencias respecto al plan original**:
> - **`:export` → `<Space>x`.** Igual que en 3.2, la command-line `:` se difiere
>   a la command-palette de 3.7; el export se dispara con la leader key
>   `<Space>x` (consistente con `<Space>e`/`<Space>h`/`<Space>r`).
> - **Export-first.** Export e import se separan en commits distintos por
>   tamaño: el import necesita UI de preview de mapeo de columnas + validación
>   de tipos. Este commit cubre solo export.

### 3.2 Query History + Snippets ✅ (`d88ddc3`)

> **Crate nuevo** `sextant-state`: dueño de la persistencia local de la app
> (`state.db`), separada de las BD del usuario. `StateStore` (sqlx/sqlite,
> async, `Clone` barato) con migración idempotente al abrir. El path vive en
> `sextant-config::state_db_path()` junto al resto de paths XDG. Permisos
> `0700` (dir) / `0600` (fichero).

- [x] ✅ `state.db` (SQLite local) en `~/.local/share/sextant/state.db`.
- [x] ✅ Tablas:
  - [x] ✅ `query_history` (timestamp, connection, sql, duration_ms, error_msg).
  - [x] ✅ `recent_files` (connection, path, last_opened; ring de 20 por conexión, prune en cada insert).
- [x] ✅ Historial: popup con lista ejecutable (Enter carga el SQL al editor).
- [x] ✅ Recent files: `<Space>r` popup por conexión (Enter lee el `.sql` y lo carga al editor).
- [ ] ⬜ **Snippets** — diferido (fuera del alcance de este commit; a planificar dentro de la Fase 3).

> **Divergencias respecto al plan original**:
> - **`:history` → `<Space>h`.** La línea de comando `:` se difiere a la
>   command-palette de 3.7 para no introducir un input `:` a medias que esa
>   tarea reescribiría; el historial se dispara con la leader key `<Space>h`
>   (consistente con `<Space>e`/`<Space>r`).
> - **Grabación selectiva.** Sólo se registran las queries ejecutadas desde el
>   editor; el browse de tablas y el refresco post-commit pasan `record=false`
>   para no ensuciar el historial.

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

- **Syntax highlighting (editor propio + tree-sitter)**: requiere **reemplazar `tui-textarea` por un editor propio** (`EditorBuffer` con `Vec<String>`: cursor, insert/delete, UTF-8 multibyte, scroll, selección), porque `tui-textarea 0.7` tiene `line_spans` en `pub(crate)` y no permite colorear por token. Sobre ese editor, integrar `tree-sitter` + `tree-sitter-sql` (parseo síncrono; los buffers SQL son pequeños) y render con `ratatui::text::Line` + `Span` por token. Migrar es la tarea de peor ratio valor/coste; al hacerla habrá que reintegrar autocomplete (2.3) y tabs (2.5) sobre el nuevo editor.
- **Asistente IA opcional**: generar/explicar queries en lenguaje natural. Conexión **opt-in**: el usuario configura proveedor/modelo + token API (`config.toml` para el modelo; keyring/env para el token). Sin configurar → la feature no aparece y `sextant` funciona 100% offline. **Complementa** (no sustituye) al autocomplete local, que sigue siendo la fuente fiable de nombres del schema (la IA puede alucinar columnas).
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
| 2 | Ninguna nueva imprescindible: se mantiene `tui-textarea`. `sqlparser` solo si el autocomplete heurístico no basta. (`tree-sitter` + `tree-sitter-sql` se posponen a post-v1 junto con el highlighting.) |
| 3 | `csv`, `serde_json`, `keyring`, `nucleo`/`fuzzy-matcher` |

---

## Notas de Implementación

- **Rendimiento**: el grid read-only de v0.1 puede cargar todo en memoria porque `ratatui::Table` requiere todas las filas. Para v0.2+, considerar virtualización (renderizar solo filas visibles) si hay tablas >10k rows.
- ~~**SQLx `Any` driver**: simplifica mucho el código unificado, pero hay que verificar que soporta bien los tipos de los 3 backends. Si hay problemas (ej. JSON en SQLite vs PG), caer a pools tipados por driver con enum wrapper.~~
  - **Actualizado (post-Fase 1)**: Se migró directamente a pools tipados (`DbPool` enum) porque `Any` falla con `BOOLEAN` en SQLite. El código ahora usa `PgPool`/`SqlitePool` directamente, eliminando `AnyPool` y `install_drivers`.
- **Editor propio vs `tui-textarea`**: `tui-textarea` no expone styling por token (`line_spans` es `pub(crate)`), así que el syntax highlighting obliga a un editor propio. Decisión (revisada Fase 2): **mantener `tui-textarea`** durante toda la v0.2 (CRUD, autocomplete y tabs se construyen encima: cursor vía `cursor()`, multi-buffer vía `Vec<TextArea>`) y **mover el editor propio + highlighting a post-v1 (Fase 4)**, por ser la tarea de peor ratio valor/coste.
- **Recursión en event loop**: usar `mpsc::UnboundedChannel` para evitar backpressure; el render corre en el thread principal, el trabajo pesado en `tokio` tasks.
