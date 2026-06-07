# ADR-0003: Modelo de transacciones híbrido psql-style

## Estado

Aceptado — 2026-06-06 (`2a8a1e6`)

## Contexto

> **Comportamiento de producto** (en `SPEC.md` §9 y §11): modelo híbrido
> psql-style, `txn: ACTIVE` solo cuando hay transacción abierta, los SELECT ven
> los cambios no confirmados, y los edits del grid van en transacción propia.
> Este ADR registra **cómo** se implementa ese comportamiento.

Hay que decidir la mecánica del régimen transaccional del editor SQL (consultas
ad-hoc), independiente del commit en lote del grid (ver
[ADR-0002](0002-concurrencia-optimista-en-commits-del-grid.md)).

## Decisión

Adoptar un modelo **híbrido al estilo `psql`** con dos regímenes:

- **Autocommit** (por defecto): cada statement se confirma solo.
- **ACTIVE**: se entra al ejecutar `BEGIN`/`START TRANSACTION`. `SqlxExecutor`
  saca una conexión del pool (`PoolConnection`) y la **retiene**; cada statement
  posterior va a esa conexión sin auto-commit hasta `COMMIT`/`END`/`ROLLBACK`,
  que la cierran y la devuelven al pool. Los `SELECT` dentro de la transacción
  ven los cambios no confirmados.

La **status line es la autoridad**: el usuario nunca debe sorprenderse del
régimen activo. Los grid edits siempre van en su propia transacción aparte.

**Divergencias respecto a la spec original:**

1. Solo se pinta `txn: ACTIVE` (ámbar) cuando hay transacción abierta; el modo
   autocommit **no muestra nada** (la spec proponía `txn: auto` en gris). Razón:
   no desbordar la status line a 80 columnas y reducir ruido — igual que `psql`,
   que solo marca cuando hay transacción.
2. El flag de estado es **lock-free** (`SqlxExecutor::in_transaction`,
   `AtomicBool`), consultado en cada render sin bloquear.

## Alternativas consideradas

- **Siempre autocommit** — simple, pero impide al usuario agrupar statements
  manualmente, algo esperado en cualquier cliente serio.
- **Siempre transaccional con commit explícito** — seguro, pero molesto para el
  flujo habitual de consultas sueltas y sorprendente para quien viene de psql.
- **Mostrar ambos estados en la status line** (`txn: auto` / `txn: ACTIVE`) —
  descartado por ruido visual a 80 columnas (ver divergencia 1).

## Consecuencias

- (+) Comportamiento familiar para usuarios de `psql`.
- (+) El estado de transacción se consulta sin locks en el hot path de render.
- (+) Los edits del grid quedan aislados del régimen del editor.
- (−) Mantener una `PoolConnection` retenida durante una transacción ACTIVE
  consume una conexión del pool mientras dure.
- (−) La ausencia de indicador en autocommit asume que el usuario conoce la
  convención psql; se documenta en la spec y la ayuda.

## Relacionado

Las operaciones destructivas (`DELETE`/`UPDATE` sin `WHERE`, DDL) pasan por un
modal de confirmación **independientemente del régimen** (`sql::dangerous_reason`).
