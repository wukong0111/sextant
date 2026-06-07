# ADR-0006 · Menú which-key solo para el leader

## Estado

Aceptado.

## Contexto

`SPEC.md` §12.4 exige **realimentación de chord pendiente**: pulsar un prefijo
(el leader `Espacio`, o un primer paso como `g`/`d`) nunca debe quedar sin
respuesta visible. El `ChordState` ya acumulaba la secuencia pendiente
(`crates/sextant-ui/src/keymap.rs`), pero no la exponía a la capa de render, así
que la pulsación del leader no producía ninguna señal hasta completar el chord.

## Decisión

Dos niveles de realimentación, según el prefijo:

- **Leader (`Espacio`)** → **menú emergente (*which-key*)**. `Keymap::continuations`
  lista las teclas que continúan el prefijo con la acción de cada una; se renderiza
  como un cuadro anclado **abajo-izquierda**, justo sobre la línea de estado, con
  el prefijo como título. La detección del leader es `KeySpec::is_leader`
  (`Char(' ')` sin Ctrl).
- **Cualquier otro prefijo** (`g`, `d`, …) → **eco ligero** en la línea de estado
  (`ChordState::pending_display`, p. ej. `g…`), **sin** popup.

El render se alimenta directamente de `ChordState::pending()` en cada frame, sin
estado adicional: la señal aparece mientras la secuencia está pendiente y
desaparece sola al completarla, abandonarla o cancelarla.

## Alternativas consideradas

- **Which-key para todos los prefijos.** Coherente, pero `gg`/`dd` tienen una
  única continuación obvia: un popup para ellos es ruido visual desproporcionado.
  El leader, con ~12 ramas, es el único caso donde el menú aporta.
- **Solo eco en la línea de estado, sin menú.** Mínimo, pero no resuelve el
  problema real: tras `Espacio` el usuario no recuerda las ramas disponibles. El
  menú es justamente el descubrimiento que falta.
- **Popup centrado (como el overlay de ayuda).** Tapa el contenido y compite con
  los modales. El anclaje inferior-izquierda es la convención which-key y no
  estorba al árbol/grid.

## Consecuencias

- El descubrimiento de los comandos del leader deja de depender de abrir la ayuda
  (`Espacio ?`); el menú refleja el keymap efectivo, así que respeta remapeos.
- El menú es solo de lectura: no captura teclas. La siguiente pulsación se
  resuelve por el flujo normal de `ChordState::feed`, que ya limpia el pending.
- Coste de un cuadro por frame mientras el leader está armado; nulo el resto del
  tiempo (`pending()` vacío → no se renderiza nada).
