# QA Manual — sextant (v0.2 / Fase 2)

Guía para verificar a mano, dentro de la TUI, las features de la Fase 2:
**2.2 grid editable (CRUD)**, **2.3 autocomplete**, **2.4 schema viewer + DDL**
y **2.5 buffers/tabs + guardado**. Complementa a los tests automáticos
(`cargo test --workspace`): cubre lo que no se puede ejercitar sin un TTY real.

> La TUI es interactiva y necesita una terminal real. Ejecuta `cargo run` en
> **tu** terminal (no a través de un agente). Los comandos de preparación
> (`make …`, `sqlite3 …`) sí son no interactivos.

---

## 1. Requisitos

- Rust toolchain del proyecto (MSRV 1.85+).
- Docker + Docker Compose v2 (solo para PostgreSQL/MySQL; SQLite no lo necesita).
- `sqlite3` CLI (para sembrar/verificar `test.db`).

## 2. Preparación

```bash
# 1. Contenedores PG/MySQL (omitir si solo pruebas SQLite)
make test-db-up

# 2. Conexiones de prueba -> ~/.config/sextant/connections.toml
make setup-docker-conns

# 3. Datos de ejemplo (PG, MySQL y test.db de SQLite)
make seed
```

Tras esto el sidebar mostrará 4 conexiones:

| Conexión             | Driver   | Notas                                  |
|----------------------|----------|----------------------------------------|
| `docker-pg`          | postgres | `127.0.0.1:5433`, requiere contraseña  |
| `docker-mysql`       | mysql    | `127.0.0.1:3307`, requiere contraseña  |
| `local-sqlite`       | sqlite   | `:memory:` (vacía)                     |
| `local-sqlite-file`  | sqlite   | `test.db` (sembrada) — **la más fácil**|

El esquema sembrado incluye `users`, `orders`, `products`, `type_samples`
(todas con PK; `orders` tiene claves foráneas).

## 3. Arranque

SQLite no necesita contraseña:

```bash
cargo run
```

Para PostgreSQL / MySQL, exporta las contraseñas en la misma shell antes:

```bash
export SEXTANT_DOCKER_PG_PASSWORD=sextant SEXTANT_DOCKER_MYSQL_PASSWORD=sextant
cargo run
```

Salir en cualquier momento: `Ctrl+Q` (si hay buffers sin guardar, pedirá
confirmación — ver 2.5).

---

## 4. Checklist por feature

Usa `local-sqlite-file` para el recorrido principal; luego repítelo con
`docker-pg` y `docker-mysql` para validar los 3 drivers.

### Conectar + 2.4 Schema viewer

- [ ] `j`/`k` hasta `local-sqlite-file`, `Enter` → conecta y expande a `main`.
- [ ] `l` (o `→`) sobre `main` muestra las tablas.
- [ ] `l` sobre `users` muestra **columnas** (`id … PK`, `name`, …) y, debajo,
      **índices** (`⚿ …`) y **foreign keys** (`→ …`). Prueba con `orders`
      para ver FKs hacia `users`/`products`.
- [ ] `h` colapsa el nodo.

### 2.2 Browse rows + grid editable

- [ ] Sobre `users`, `Enter` → ejecuta `SELECT * … LIMIT 500`; el foco salta al
      grid. La status line muestra `N rows / Xms` y **no** aparece 🔒
      (la tabla tiene PK → editable).
- [ ] `h j k l` mueven de celda; `gg` / `G` van arriba/abajo.
- [ ] `Enter` en una celda (p. ej. `name`) → modo edición; escribe un valor,
      `Enter` confirma (`Esc` cancela). La celda queda marcada y la status line
      muestra `✎ 1 pending`.
- [ ] `o` añade una fila vacía al final (en verde). Edita su `name`
      (`Enter` → escribir → `Enter`); deja `id` vacío (es AUTOINCREMENT).
- [ ] `dd` sobre una fila la marca para borrar (tachada en rojo); `dd` de nuevo
      la desmarca.
- [ ] `Ctrl+S` abre el **modal de confirmación** con los `UPDATE/INSERT/DELETE`.
      `y`/`Enter` confirma (corre en una transacción y refresca el grid);
      `n`/`Esc` cancela.
- [ ] `Ctrl+Z` descarta los cambios pendientes sin commitear.
- [ ] **Read-only**: ejecuta un `SELECT` ad-hoc desde el editor (ver 2.3) y
      comprueba que el grid muestra 🔒 y rechaza la edición.

Verificación en disco (opcional):

```bash
sqlite3 test.db "SELECT id, name FROM users ORDER BY id;"
```

### 2.3 Autocomplete (en el editor)

- [ ] `<Espacio>` y luego `e` abren el editor. `i` entra en modo insert.
- [ ] Escribe `SELECT * FROM ` → popup con **tablas**; sigue con `u` → filtra a
      `users`; `Enter` o `Tab` acepta (reemplaza la palabra parcial).
- [ ] Escribe `users.` → popup con **columnas** de `users`
      (`Ctrl+Espacio` lo dispara manualmente; `.` lo abre automáticamente).
- [ ] Alias: `... FROM users u` y luego `u.` también ofrece columnas de `users`.
- [ ] `↑`/`↓` navegan el popup; `Esc` lo descarta; seguir tecleando lo filtra.
- [ ] `Ctrl+E` (o `Ctrl+Enter`) ejecuta el buffer → resultados en el grid.

### 2.4 DDL

- [ ] En el árbol, sobre una tabla, pulsa `D` → abre el editor con un
      `CREATE TABLE …` generado desde la metadata (tipos, NOT NULL, PK).

### 2.5 Buffers / tabs + guardado

- [ ] `Ctrl+T` abre un **buffer nuevo** (barra de tabs arriba del editor).
- [ ] `Tab` / `Shift+Tab` (en modo Normal) ciclan entre buffers; un buffer con
      cambios sin guardar muestra `●` en su tab.
- [ ] `Ctrl+S` en un buffer nuevo pide **nombre de archivo**; escribe `mi_query`
      y `Enter` → guarda `~/.local/share/sextant/queries/mi_query.sql`.
      El tab pasa a mostrar el nombre y desaparece el `●`.
- [ ] Comprueba permisos restrictivos (Unix):
      ```bash
      ls -l ~/.local/share/sextant/queries/mi_query.sql   # -rw------- (0600)
      stat -c '%a' ~/.local/share/sextant/queries          # 700
      ```
- [ ] Haz un cambio sin guardar, `Esc` (Normal) cierra el editor, y pulsa
      `Ctrl+Q` → aparece el prompt **"Unsaved buffers"**:
      `s` guardar · `d` descartar y salir · `c`/`Esc` cancelar.

### Repetir con PostgreSQL y MySQL

- [ ] Repite *Conectar → expandir (columnas/índices/FKs) → browse → editar +
      commit → autocomplete* con `docker-pg`.
- [ ] Lo mismo con `docker-mysql`.

---

## 5. Referencia rápida de teclas

**Global**: `<Espacio>e` editor · `Tab` alterna foco árbol/grid · `Ctrl+Q` salir.

**Árbol**: `j`/`k` mover · `l`/`→` expandir · `h` colapsar ·
`Enter` conectar (conexión) / browse (tabla) · `D` emitir DDL.

**Grid (editable)**: `h j k l` mover · `gg`/`G` extremos · `Enter` editar celda ·
`o` nueva fila · `dd` borrar fila · `Ctrl+S` commit · `Ctrl+Z` descartar.

**Editor**: `i` insert · `Esc` normal/cerrar · `Ctrl+E`/`Ctrl+Enter` ejecutar ·
`Ctrl+Espacio` autocomplete · `Tab`/`Shift+Tab` ciclar buffers ·
`Ctrl+T` buffer nuevo · `Ctrl+S` guardar `.sql`.

---

## 6. Limpieza

```bash
make test-db-down    # para y elimina contenedores + volúmenes
```

`test.db` (SQLite) puede re-sembrarse cuando quieras con `make seed-sqlite`.
