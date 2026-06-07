# Architecture Decision Records

Este directorio registra las **decisiones de implementación** de `sextant`: una
decisión por fichero, en orden cronológico. Un ADR captura *por qué* se eligió
una opción técnica frente a otras — el código y `ARCHITECTURE.md` dicen *cómo es
el sistema ahora*; el ADR dice *cómo llegamos ahí*.

> **Frontera spec ↔ implementación.** Estos ADR son documentación **de la
> implementación**, no de la especificación. El *qué* y el *porqué de producto*
> (comportamiento que cualquier implementación debe respetar) viven en
> `SPEC.md`, que es agnóstico de lenguaje y arquitectura. Cada ADR enlaza en su
> `Contexto` el requisito de producto correspondiente en `SPEC.md` y se limita a
> registrar **cómo** se implementa aquí (almacén de credenciales, flag
> lock-free, emparejamiento de filas, etc.).

## Reglas

- **Una decisión por fichero**, numerado `NNNN-titulo-en-kebab-case.md`.
- **Inmutables.** Un ADR aceptado no se reescribe. Si la decisión cambia, se
  crea un ADR nuevo que marca al anterior como *Superseded by ADR-XXXX*, y el
  viejo se actualiza solo en su campo `Estado` para apuntar al nuevo.
- **Cortos.** Media o una pantalla. Si necesita más, probablemente son varias
  decisiones.
- **Enfocados en el *por qué*.** Contexto, decisión, alternativas descartadas,
  consecuencias. No es documentación de uso ni un tutorial.

## Formato

Plantilla [Nygard](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions):
`Estado` · `Contexto` · `Decisión` · `Alternativas consideradas` · `Consecuencias`.

## Índice

| ADR | Título | Estado |
|-----|--------|--------|
| [0001](0001-sqlx-como-capa-de-acceso-a-datos.md) | sqlx como capa de acceso a datos | Aceptado |
| [0002](0002-concurrencia-optimista-en-commits-del-grid.md) | Concurrencia optimista en los commits del grid | Aceptado |
| [0003](0003-modelo-de-transacciones-hibrido-psql.md) | Modelo de transacciones híbrido psql-style | Aceptado |
| [0004](0004-credenciales-via-keyring-del-so.md) | Credenciales vía keyring del SO | Aceptado |
