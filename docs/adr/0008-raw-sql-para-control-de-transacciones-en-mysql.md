# ADR-0008: `raw_sql` para control de transacciones en MySQL

## Estado

Aceptado — 2026-06-14

## Contexto

> Relacionado con [ADR-0003](0003-modelo-de-transacciones-hibrido-psql.md).

El modelo híbrido psql-style de sextant retiene una `PoolConnection` mientras
hay una transacción de sesión abierta y ejecuta `BEGIN` / `START TRANSACTION`,
`COMMIT` / `END` y `ROLLBACK` sobre esa conexión. En PostgreSQL y SQLite el
camino original usando `sqlx::query(...)` funciona sin problemas.

En MySQL, el servidor **rechaza comandos de control transaccional cuando se
envían a través del protocolo de prepared statements**:

```text
error returned from database: 1295 (HY000):
This command is not supported in the prepared statement protocol yet
```

Esto afecta a `BEGIN`, `START TRANSACTION`, `COMMIT` y `ROLLBACK`, que son
exactamente los comandos que abren y cierran la transacción de sesión en
`SqlxExecutor`.

## Decisión

Usar `sqlx::raw_sql(...)` (envuelto en `AssertSqlSafe`) para ejecutar
sentencias de control transaccional sobre la conexión retenida en
`SqlxExecutor::begin_held` y `close_held`. `raw_sql` envía el texto SQL
directamente sin prepararlo, evitando la limitación de MySQL.

Las sentencias normales (SELECT, INSERT, UPDATE, DELETE, DDL, etc.) siguen
usando `sqlx::query`/`sqlx::query_as` con su comportamiento habitual.

## Alternativas consideradas

- **Detectar el driver y usar `raw_sql` solo para MySQL** — descartado porque
  `raw_sql` es igual de válido para PostgreSQL y SQLite, y unificar el camino
  reduce complejidad y riesgo de divergencia entre drivers.
- **Delegar el control transaccional a sqlx `Transaction`** — descartado porque
  el modelo de sesión psql-style requiere mantener la transacción abierta a
  través de múltiples llamadas independientes a `QueryExecutor::execute`, algo
  que `PoolConnection` retenida expresa mejor que una `Transaction` de corta
  vida.
- **Parsear el SQL y ejecutar un stored procedure equivalente** — innecesario;
  `raw_sql` resuelve el problema con una sola línea de cambio.

## Consecuencias

- (+) Las transacciones de sesión funcionan en MySQL exactamente igual que en
  PostgreSQL y SQLite.
- (+) No se introduce lógica condicional por driver en el código de control
  transaccional.
- (−) `raw_sql` no realiza verificación estática de tipos en tiempo de
  compilación; usamos `AssertSqlSafe` solo para literales reconocidos
  explícitamente (`BEGIN`, `START TRANSACTION`, `COMMIT`, `END`, `ROLLBACK`).
- (−) Las sentencias de control transaccional no devuelven `rows_affected`, pero
  ese valor ya no se usa para este tipo de comandos.

## Relacionado

- `SPEC.md` §17.7 (Transacción de sesión psql-style).
- `crates/sextant-db/src/executor.rs`: `begin_held`, `close_held`,
  `txn_control`.
