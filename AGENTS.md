# AGENTS.md

Directrices de comportamiento para reducir errores comunes de codificación con LLMs. Se pueden fusionar con instrucciones específicas del proyecto según sea necesario.

**Compromiso:** Estas directrices priorizan la cautela sobre la velocidad. Para tareas triviales, usar el criterio propio.

## 1. Pensar Antes de Codificar

**No asumir. No ocultar confusiones. Plantear tradeoffs.**

Antes de implementar:
- Enunciar las suposiciones explícitamente. Si hay incertidumbre, preguntar.
- Si existen múltiples interpretaciones, presentarlas — no elegir en silencio.
- Si existe un enfoque más simple, decirlo. Oponerse cuando sea necesario.
- Si algo no está claro, parar. Nombrar qué causa confusión. Preguntar.

## 2. Simplicidad Primero

**Código mínimo que resuelva el problema. Nada especulativo.**

- Sin características más allá de lo solicitado.
- Sin abstracciones para código de un solo uso.
- Sin "flexibilidad" o "configurabilidad" que no se haya pedido.
- Sin manejo de errores para escenarios imposibles.
- Si escribes 200 líneas y podrían ser 50, reescríbelo.

Pregúntate: "¿Un ingeniero senior diría que esto está sobrecomplicado?" Si es sí, simplifica.

## 3. Cambios Quirúrgicos

**Tocar solo lo necesario. Limpiar solo el propio desorden.**

Al editar código existente:
- No "mejorar" código, comentarios o formato adyacentes.
- No refactorizar cosas que no están rotas.
- Respetar el estilo existente, incluso si se haría de otra manera.
- Si se detecta código muerto no relacionado, mencionarlo — no eliminarlo.

Cuando tus cambios generan huérfanos:
- Eliminar imports/variables/funciones que TUS cambios dejaron sin usar.
- No eliminar código muerto preexistente a menos que se solicite.

La prueba: cada línea modificada debe trazarse directamente a la petición del usuario.

## 4. Ejecución Orientada a Objetivos

**Definir criterios de éxito. Iterar hasta verificar.**

Transformar tareas en objetivos verificables:
- "Añadir validación" → "Escribir tests para entradas inválidas, luego hacerlos pasar"
- "Arreglar el bug" → "Escribir un test que lo reproduzca, luego hacerlo pasar"
- "Refactorizar X" → "Asegurar que los tests pasan antes y después"

Para tareas de varios pasos, plantear un plan breve:
```
1. [Paso] → verificar: [check]
2. [Paso] → verificar: [check]
3. [Paso] → verificar: [check]
```

Criterios de éxito fuertes permiten iterar de forma independiente. Criterios débiles ("que funcione") requieren clarificación constante.

## 5. Workflow por Fase / Punto de Plan

Cuando el usuario diga "vamos con la Fase X" o "implementa el punto Y":

1. **Leer el plan primero** — Revisar `plan.md` y marcar qué está ✅ y qué ⬜. No asumir que algo está hecho sin verificarlo en el código.

2. **Presentar opciones antes de actuar** — Si la tarea admite múltiples enfoques (librería A vs B, arquitectura X vs Y), presentar las opciones con tradeoffs y esperar decisión del usuario. No elegir en silencio.

3. **Definir el alcance de esta sesión** — Preguntar si quiere la fase completa o solo un subconjunto específico de tareas.

4. **Codificar → Verificar → Commit** — Cada tarea del plan debe:
   - Compilar sin warnings (`cargo check --workspace`)
   - Pasar tests existentes (`cargo test --workspace`)
   - Tener un criterio de éxito verificado antes de declararla "hecha"
   - Commitearse de forma atómica con mensaje descriptivo

5. **Actualizar el plan inmediatamente** — Marcar la tarea como `[x] ✅` en `plan.md` y añadir el hash del commit en la tabla de progreso. Hacer push del plan junto con el código.

6. **Si algo bloquea, parar y reportar** — No improvisar soluciones a problemas no previstos sin consultar. Documentar el bloqueo en el plan o en un issue.

---

**Estas directrices funcionan si:** hay menos cambios innecesarios en los diffs, menos reescrituras por sobrecomplicación, y las preguntas de clarificación vienen antes de la implementación en lugar de después de los errores.
