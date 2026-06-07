# ADR-0001: sqlx como capa de acceso a datos

## Estado

Aceptado — 2026-05-24 (`6dfb9cf`)

## Contexto

`sextant` debe hablar con tres motores —PostgreSQL, MySQL y SQLite— detrás de
un único trait `QueryExecutor` en `sextant-core`. La capa de drivers
(`sextant-db`) tiene que ser asíncrona (`tokio`), manejar pools por conexión y
poder ejecutar SQL arbitrario introducido por el usuario, no solo consultas
conocidas en tiempo de compilación. Toda la I/O de base de datos debe compilar y
testearse sin depender de `sextant-ui`.

## Decisión

Usar **`sqlx` 0.8** con las features `postgres`, `mysql` y `sqlite` como única
capa de acceso a datos, en modo **runtime** (no las macros `query!` chequeadas
en compilación). El mismo crate cubre además el `state.db` local
(`sextant-state`) vía `sqlx-sqlite`.

## Alternativas consideradas

- **Diesel** — ORM con DSL y migraciones potentes, pero orientado a esquemas
  conocidos en compilación. `sextant` ejecuta SQL ad-hoc del usuario; un ORM
  estorba más que ayuda y su soporte async era inmaduro.
- **Drivers nativos por motor** (`tokio-postgres`, `mysql_async`, `rusqlite`) —
  máximo control, pero obligan a tres APIs distintas detrás del trait y a
  reimplementar pooling y mapeo de tipos tres veces.
- **Macros `query!` de sqlx (compile-time checked)** — descartadas: requieren
  una base de datos accesible en build time y un esquema fijo, incompatible con
  consultas arbitrarias del usuario.

## Consecuencias

- (+) Una sola API async y un solo modelo de pooling para los tres motores.
- (+) `sqlx-sqlite` reutilizado para el estado local sin dependencias extra.
- (+) `sextant-db` y `sextant-state` compilan y se testean sin la TUI (regla de
  dependencias del workspace).
- (−) El mapeo de tipos por motor (`CellValue`) es manual, al no usar las macros
  chequeadas; hay que cuidar los casos por driver (NULL, tipos exóticos).
- (−) Sin verificación de SQL en compilación: los errores de query aparecen en
  runtime, lo cual es inevitable dado que el SQL lo escribe el usuario.
