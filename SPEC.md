# sextant — Especificación de producto (agnóstica)

> **Propósito.** Este documento describe *qué es* sextant y *cómo se comporta*,
> de forma **independiente de lenguaje, librerías y arquitectura**. Una
> implementación en cualquier stack que satisfaga esta especificación —y, en
> particular, los **criterios de aceptación** de la §17— es una réplica fiel de
> sextant.
>
> **Reglas de este documento.** No nombra lenguajes, frameworks, crates ni
> estructuras de código. Todo lo que aparece aquí pasa el test: *dos
> implementaciones correctas e independientes estarían obligadas a coincidir en
> ello.* Las decisiones de implementación viven aparte (junto al código), no
> aquí.
>
> Derivado por ingeniería inversa del comportamiento observable, no de
> documentación previa.

---

## 1. Identidad del producto

sextant es un **cliente de base de datos para terminal, gobernado por teclado**.
Da soporte a tres motores: **PostgreSQL**, **MySQL** y **SQLite**. Su modelo de
interacción es **modal** (al estilo de editores como vi/Helix): el usuario no
usa el ratón; navega, consulta y edita con secuencias de teclas.

El producto presenta tres superficies simultáneas:

1. **Árbol lateral (tree)** — conexiones → esquemas → tablas → columnas.
2. **Rejilla de resultados (grid)** — filas devueltas por una consulta o por el
   *browse* de una tabla; editable bajo ciertas condiciones (§7).
3. **Editor SQL modal** — un editor flotante con múltiples buffers (pestañas)
   para escribir y ejecutar consultas.

Una **línea de estado** siempre visible es la fuente de verdad sobre el modo
activo, la conexión, el resultado de la última consulta y el estado
transaccional (§11).

---

## 2. Conceptos y modelo de datos

Estos tipos son el **contrato observable**; sus nombres son orientativos, su
semántica es obligatoria.

### 2.1 Driver

Enumeración cerrada: `Postgres`, `Mysql`, `Sqlite`. Determina el dialecto de
*quoting* (§6), los campos de conexión requeridos (§3) y el modo de conexión.

### 2.2 Connection

Una conexión guardada tiene: `name` (obligatorio, no vacío), `driver`
(obligatorio) y campos opcionales a nivel de estructura cuya obligatoriedad
depende del driver: `host`, `port`, `user`, `database`, `ssl_mode`, `path`,
`keyring_key`.

### 2.3 CellValue

El valor de una celda es uno de: **Null**, **Bool**, entero de 64 bits, real de
64 bits, **String** o **Bytes** (binario). Reglas de presentación obligatorias:

- **Null** se muestra y se trata mediante el centinela textual `NULL` (esto es
  observable: el grid lo muestra así y los generadores de SQL lo interpretan).
- **Bytes** se representa en **hexadecimal** allí donde se serializa a texto.

### 2.4 Column y QueryResult

Una **Column** tiene `name` y `type_name` (el nombre de tipo reportado por el
motor). Un **QueryResult** tiene `columns`, `rows` (matriz de `CellValue`) y un
`rows_affected` opcional. Convención:

- Las sentencias que devuelven filas (`SELECT`, `WITH`, `EXPLAIN`, `VALUES`)
  pueblan `columns`/`rows` y dejan `rows_affected` vacío.
- El resto (`INSERT`/`UPDATE`/`DELETE`/DDL/control de transacción) dejan
  `columns`/`rows` vacíos y pueblan `rows_affected`.

### 2.5 Metadatos de esquema (introspección)

Para cada tabla, la app conoce: columnas (`name`, `type_name`, `nullable`,
`default`, `is_primary_key`), la clave primaria (lista de columnas), índices
(`name`, columnas, `unique`) y claves foráneas (`name`, columnas, tabla y
columnas referenciadas).

---

## 3. Conexiones y credenciales

### 3.1 Validación por driver

Al cargar conexiones se valida:

- **PostgreSQL y MySQL**: requieren `host`, `port`, `user` y `database`. La
  ausencia de cualquiera es un error de configuración con mensaje descriptivo.
- **SQLite**: requiere `path`. No requiere credenciales.
- Cualquier `driver` distinto de los tres soportados es un error ("unsupported
  driver").

### 3.2 Resolución de contraseña (cascada obligatoria)

Las contraseñas **nunca** se almacenan en el fichero de conexiones. Al conectar,
la contraseña se resuelve en este orden:

1. **Almacén de credenciales del sistema operativo**, si la conexión declara
   `keyring_key`.
2. **Variable de entorno** `SEXTANT_<NAME>_PASSWORD`, donde `<NAME>` es el
   nombre de la conexión en mayúsculas con espacios y guiones convertidos en
   guiones bajos. Es el *fallback* cuando no hay `keyring_key`.
3. **Prompt interactivo enmascarado** dentro de la TUI, cuando hay `keyring_key`
   pero no existe secreto guardado (no aplica a SQLite).

Tras una conexión exitosa establecida desde el prompt interactivo, la
contraseña **se guarda** en el almacén del SO para no volver a pedirla. SQLite y
conexiones sin contraseña no pasan por esta cascada.

### 3.3 Listado y conexión

Al arrancar, el árbol lista las conexiones definidas. La acción *Activate*
(`Enter` por defecto) sobre una conexión la establece; al completarse la
introspección, sus esquemas y tablas aparecen en el árbol.

---

## 4. Navegación del esquema (árbol)

El árbol es jerárquico: **conexión → esquema → tabla → columnas**. Acciones de
producto:

- *Down/Up* mueven la selección; *Top/Bottom* van al extremo.
- *Right* (sobre un nodo plegable) lo expande; *Left* lo pliega o sube de nivel.
- Expandir una tabla muestra sus **columnas** con marca visible de clave
  primaria.
- *Activate* sobre una tabla hace **browse** (§7.1).
- *EmitDdl* sobre una tabla inserta en el editor un **esqueleto `CREATE TABLE`**
  (§6.2).

---

## 5. Editor SQL

### 5.1 Naturaleza

Editor **modal flotante** sobre el viewport, con **dos modos**: *Normal* (las
teclas navegan/comandan) e *Insert* (las teclas escriben texto). Indicador de
modo en la línea de estado (`NOR` / `INS`).

### 5.2 Buffers (pestañas)

- Soporta **múltiples buffers** simultáneos, mostrados como pestañas.
- Un buffer con cambios sin guardar se marca con un indicador visible (`●`).
- Existe acción para **ciclar** entre buffers (hacia delante y atrás) y para
  **crear** un buffer nuevo.

### 5.3 Ejecución

- Acción **Run**: ejecuta el contenido del buffer activo. Dos atajos
  equivalentes la disparan (`Ctrl+E` y `Ctrl+Enter` por defecto).
- El resultado se vuelca al grid; la línea de estado muestra recuento de filas y
  duración (§11).
- **Toda** ejecución se registra en el historial (§9.1), tanto si tiene éxito
  como si falla (en cuyo caso se registra el error).
- Antes de ejecutar una **sentencia destructiva** (§8) se interpone un modal de
  confirmación.

### 5.4 Autocompletado

En modo Insert, una acción (`Ctrl+Space` por defecto) abre un *popup* de
autocompletado alimentado por el esquema de la conexión activa (**nombres de
tablas y de columnas**). El usuario puede insertar un candidato.

### 5.5 Guardado

- Acción **Save** (`Ctrl+S`): guarda el buffer activo en un fichero `.sql`. El
  primer guardado pide un nombre; los siguientes sobrescriben en silencio el
  fichero ya vinculado.
- Un buffer guardado queda **vinculado** a su ruta.
- Los buffers son **volátiles en memoria** hasta que se guardan; la única
  escritura automática a disco es el *swap* de recuperación (§10).

---

## 6. Generación de SQL (dialecto)

### 6.1 Quoting de identificadores

Obligatorio por dialecto: **MySQL** usa backticks (`` `x` ``); **PostgreSQL** y
**SQLite** usan comillas dobles (`"x"`). Los caracteres de comilla incrustados
se **duplican** para que el resultado sea siempre un único token seguro. Una
referencia a tabla con esquema se cualifica (`"esquema"."tabla"`); sin esquema,
solo la tabla.

### 6.2 Esqueleto CREATE TABLE

A partir de los metadatos cacheados se genera un `CREATE TABLE` con: nombre y
tipo declarado de cada columna, `NOT NULL` cuando aplique, `DEFAULT <expr>`
cuando exista, y una cláusula `PRIMARY KEY (...)` si hay clave primaria. Es un
**esqueleto editable**, no un round-trip exacto del DDL original.

### 6.3 Literales

Un valor de celda se serializa como literal SQL así: el centinela `NULL` produce
`NULL` sin comillas; cualquier otro valor se envuelve en comillas simples con las
comillas internas duplicadas. (Los backends coercionan literales entre comillas
para columnas numéricas/booleanas, lo que mantiene el generador agnóstico de
tipo.)

---

## 7. Rejilla de resultados (grid)

### 7.1 Browse

*Activate* sobre una tabla ejecuta `SELECT * FROM <tabla> LIMIT 500` y muestra
las filas. La SQL de browse se recuerda para poder **refrescar** tras un commit.

### 7.2 Navegación (siempre disponible)

*Down/Up/Left/Right* mueven la celda seleccionada; *Top/Bottom* van al extremo.
La rejilla **se desplaza horizontalmente** para mantener visible la celda
seleccionada, de modo que se puede recorrer cualquier columna aunque la tabla
sea más ancha que la pantalla.

### 7.3 Editabilidad

- El grid es **editable** solo si el resultado proviene de una tabla **con clave
  primaria**.
- Es **de solo lectura** para resultados ad-hoc del editor o tablas sin clave
  primaria; en ese caso la línea de estado muestra un indicador de bloqueo
  (🔒).

### 7.4 Edición (cuando es editable)

- *Activate* sobre una celda entra en edición (modo Insert); confirmar fija el
  cambio como **pendiente**, cancelar lo descarta. Las celdas modificadas se
  resaltan.
- *AddRow* añade una fila vacía al final (resaltada como nueva).
- *DeleteRow* marca/desmarca una fila para borrado (resaltada como tachada).
- *Discard* (`Ctrl+Z`) descarta todos los cambios pendientes.
- *Commit* (`Ctrl+S`) abre un modal de confirmación que muestra las sentencias a
  ejecutar; al aceptar, **todas se aplican en una única transacción** y el grid
  se refresca reejecutando el browse.

### 7.5 Concurrencia optimista (obligatoria)

Las sentencias de commit no emparejan filas solo por clave primaria:

- **UPDATE**: `WHERE` = clave primaria **más los valores originales de las
  columnas editadas**.
- **DELETE**: `WHERE` = **la fila original completa**.
- Un valor original `NULL` se empareja con `col IS NULL` (no `col = NULL`).

Efecto: si otra sesión modificó la fila entremedias, la sentencia afecta a **0
filas** en lugar de sobrescribir o borrar cambios ajenos. *(Limitación de
producto conocida: hoy el conflicto se manifiesta como "0 filas afectadas" — la
fila no cambia tras refrescar — sin un mensaje de error explícito.)*

---

## 8. Operaciones destructivas

Antes de ejecutarse, una sentencia se clasifica como destructiva por su palabra
clave inicial (heurística, no parsing completo) y, si lo es, requiere
**confirmación modal**:

- `DELETE` o `UPDATE` **sin** cláusula `WHERE` (afectan a todas las filas).
- Cualquier DDL: `DROP`, `TRUNCATE`, `ALTER`, `CREATE`, `RENAME`.

`WHERE` se reconoce como palabra completa (un identificador como `wherehouse` no
cuenta). La confirmación es obligatoria **con independencia del régimen
transaccional** activo.

---

## 9. Transacciones

Modelo **híbrido al estilo psql**, a nivel de sesión:

- **Autocommit** (por defecto): cada sentencia se confirma sola. *No se muestra
  indicador alguno* en la línea de estado.
- **Transacción activa**: se entra al ejecutar `BEGIN` / `START TRANSACTION`. A
  partir de ahí, cada sentencia queda **sin confirmar** hasta `COMMIT` / `END` /
  `ROLLBACK`. Mientras está activa, la línea de estado muestra `txn: ACTIVE`
  (resaltado). Los `SELECT` dentro de la transacción **ven los cambios no
  confirmados**.
- `END` es sinónimo de `COMMIT`. Un `COMMIT`/`ROLLBACK` suelto fuera de
  transacción, o un `BEGIN` anidado, se delega al backend para que él reporte el
  resultado.

Los **edits del grid** (§7.4) van **siempre** en su propia transacción
independiente, sin importar el régimen de sesión.

---

## 10. Recuperación ante caídas (swap)

- Mientras un buffer del editor tiene cambios sin guardar, su contenido (y la
  ruta vinculada, si la hay) se escribe periódicamente en un fichero de *swap*
  de sesión (cadencia aproximada de 30 s).
- Un **cierre limpio elimina** el swap. Por tanto, cualquier swap encontrado al
  arrancar es **huérfano** de una sesión que cayó.
- Al arrancar, los swaps huérfanos disparan un prompt de **recuperación** que
  permite restaurar o descartar ese trabajo.
- Es la **única** escritura a disco de contenido de buffer sin acción explícita
  del usuario.

---

## 11. Línea de estado (contrato)

Siempre visible. Comunica, como mínimo:

- **Modo** actual (`NOR` / `INS`).
- **Conexión** activa, o indicación de que no hay ninguna.
- Tras una consulta: **recuento de filas y duración**, en la forma
  `<n> rows / <ms>ms`.
- `txn: ACTIVE` (resaltado) cuando hay una transacción de sesión abierta; nada
  en autocommit.
- Indicador de **solo lectura** (🔒) cuando el grid enfocado no es editable.
- Un **spinner** mientras hay trabajo asíncrono en curso.
- La **pista de ayuda** (chord que abre la pantalla de ayuda) **siempre
  visible**, en cualquier foco o modo; las demás pistas son contextuales, pero
  esta es el punto de entrada al resto y no debe desaparecer.

---

## 12. Modelo de interacción y teclado

### 12.1 Modos y foco

Dos **modos**: *Normal* e *Insert*. El foco principal alterna entre **árbol** y
**grid** (acción *FocusNext*). El editor, cuando está abierto, captura la
entrada.

### 12.2 Acciones y remapeo

Las teclas de modo Normal se resuelven a **acciones con nombre**, no a
comportamientos fijos: la misma acción (p. ej. *Down*) opera sobre el árbol o
sobre el grid según el foco. El keymap es **remapeable** por el usuario:

- Un *chord* puede ser de una tecla, de dos teclas (p. ej. `gg`, `dd`), con
  *leader* (`Espacio`) o con `Ctrl`.
- Una asociación del usuario **reemplaza** la asociación por defecto con el mismo
  chord; puede añadir chords alternativos. Acciones desconocidas se ignoran.
- Tras un prefijo sin salida, el emparejamiento se reintenta desde la tecla
  siguiente (un chord válido tras un prefijo muerto aún dispara).

### 12.3 Keymap por defecto (contrato de producto)

| Acción | Chord por defecto | Significado |
|--------|-------------------|-------------|
| Quit | `Ctrl+Q` | salir |
| FocusNext | `Tab` | alternar foco árbol/grid |
| ToggleEditor | `Espacio e` | abrir editor SQL |
| OpenHistory | `Espacio h` | historial de consultas |
| OpenRecent | `Espacio r` | ficheros recientes |
| Export | `Espacio x` | exportar resultado |
| Import | `Espacio i` | importar a tabla |
| Down / Up / Left / Right | `j` / `k` / `h` / `l` | mover (árbol o grid) |
| Top / Bottom | `gg` / `G` | ir al principio / final |
| Activate | `Enter` | conectar / browse / editar celda |
| AddRow | `o` | añadir fila (grid) |
| DeleteRow | `dd` | marcar borrado (grid) |
| Commit | `Ctrl+S` | confirmar edits (grid) |
| Discard | `Ctrl+Z` | descartar edits (grid) |
| EmitDdl | `D` | emitir `CREATE TABLE` (árbol) |
| Help | `Espacio ?` | ayuda |
| CommandPalette | `Espacio :` | paleta de comandos |
| FindTable | `Espacio f` | buscar tabla |
| OpenFile | `Espacio o` | abrir `.sql` |
| Snippets | `Espacio s` | insertar snippet |
| SaveSnippet | `Espacio S` | guardar buffer como snippet |

Teclas internas del editor (alternar Insert/Normal, Run, Save) y de los modales
(confirmar/cancelar) se gestionan en su propio contexto y no forman parte del
keymap remapeable.

### 12.4 Realimentación de chord pendiente

Mientras una secuencia de teclas está **pendiente** (es prefijo de uno o más
chords pero aún no completa), la app indica que el modo está **armado**, de modo
que pulsar el prefijo nunca queda sin respuesta visible:

- Al pulsar el **leader** (`Espacio`) aparece un **menú emergente** (*which-key*)
  con las teclas que pueden continuarlo y la acción a la que lleva cada una.
- Para cualquier otro prefijo (p. ej. `g` de `gg`, `d` de `dd`) se muestra una
  **realimentación más ligera** (eco de la secuencia pendiente), **sin** menú.

La realimentación refleja el keymap efectivo (respeta los remapeos del usuario)
y desaparece al completar el chord, abandonarlo (tecla sin salida) o cancelarlo.

### 12.5 Salida con cambios sin guardar

*Quit* con buffers sucios abre un prompt **guardar / descartar / cancelar**.

---

## 13. Selectores difusos y modales

La app ofrece varios selectores con **filtrado difuso (fuzzy)**:

- **Historial** (*OpenHistory*): lista consultas, **más reciente primero**;
  seleccionar una la inserta en el editor.
- **Ficheros recientes** (*OpenRecent*): por conexión, más reciente primero.
- **Abrir fichero** (*OpenFile*): fuzzy sobre el directorio de queries.
- **Buscar tabla** (*FindTable*): fuzzy sobre las tablas del esquema.
- **Insertar snippet** (*Snippets*): fuzzy sobre los snippets guardados.
- **Paleta de comandos** (*CommandPalette*) y **Ayuda** (*Help*, lista los
  keybindings).

---

## 14. Export / Import

### 14.1 Export

La acción *Export* ofrece cuatro formatos: **CSV**, **TSV**, **JSON** y **SQL (INSERT)**.
El resultado se escribe a un fichero en el directorio de exports. Contratos de
serialización de `CellValue`:

| Formato | Null | Bytes | Booleano |
|---------|------|-------|----------|
| CSV | celda vacía | hex | `true`/`false` |
| TSV | celda vacía | hex | `true`/`false` |
| JSON | `null` | string hex | booleano JSON |
| SQL | `NULL` | hex | `TRUE`/`FALSE` |

JSON es un array de objetos por fila; SQL es una sentencia `INSERT` por fila.
CSV y TSV difieren solo en el delimitador (coma vs. tabulación); ambos incluyen
una fila de cabecera.

### 14.2 Import

La acción *Import* sobre una tabla:

1. Acepta **CSV** (la primera fila es cabecera), **JSON** (array de objetos) o un
   fichero de **sentencias SQL** (separadas por `;`, respetando comillas).
2. Empareja las columnas de origen con las de la tabla destino.
3. Muestra una **previsualización** que cuenta posibles problemas de tipo.
4. Al confirmar, genera **un `INSERT` por fila** rellenando solo las columnas
   emparejadas y los ejecuta.

---

## 15. Persistencia y layout en disco (contrato observable)

Las rutas siguen XDG (con *fallback* a `~/.config`, `~/.local/share`,
`~/.local/state`). La app distingue **configuración** (que el usuario edita) de
**estado** (que la app gestiona):

| Contenido | Ubicación lógica |
|-----------|------------------|
| Conexiones (`connections.toml`) | dir. de configuración |
| Keymap del usuario | dir. de configuración |
| Temas | dir. de configuración `/themes` |
| Queries guardadas (`*.sql`) | dir. de datos `/queries` |
| Resultados exportados | dir. de datos `/exports` |
| Estado local (historial, recientes, snippets) | dir. de datos `/state.db` |
| Swaps de recuperación | dir. de estado `/swap` |

**Estado local** persiste: historial de consultas (conexión, SQL, duración,
error, timestamp); ficheros recientes (por conexión, **ring de los 20 más
recientes**, deduplicado por ruta, más reciente primero); y snippets (globales,
nombre→cuerpo, guardar sobrescribe el mismo nombre). El estado local degrada con
elegancia: si no se puede abrir, equivale a "historial deshabilitado", no a un
error fatal.

---

## 16. Seguridad (requisitos)

- Las contraseñas **nunca** se escriben en ficheros de configuración; solo en el
  almacén del SO (§3.2).
- Las cadenas de conexión se **redactan** en logs (sin contraseña).
- Las operaciones destructivas requieren confirmación (§8).
- **Permisos de fichero restrictivos** al crear, en sistemas que los soporten:
  - estado local: `0600`
  - swaps: `0600`
  - directorio de queries: `0700`; ficheros `.sql`: `0600`
  - exports: directorio `0700`, fichero `0600`
- El texto de queries en disco (queries guardadas, swaps, historial) **no está
  cifrado**. El modelo de amenaza asume acceso solo local a la máquina.

---

## 17. Criterios de aceptación (Given / When / Then)

Contrato verificable. Cualquier implementación que los cumpla replica el
comportamiento. Son **agnósticos**: el mapeo de cada escenario a un test
concreto vive en cada implementación.

**Arranque y salida limpia**
- *Given* una configuración con al menos una conexión
- *When* se arranca la app
- *Then* el árbol lista esa conexión y la línea de estado indica que no hay
  conexión activa
- *And* la acción Quit cierra el proceso limpiamente cuando no hay buffers sucios

**Conectar y consultar**
- *Given* la app arrancada con una conexión seleccionada
- *When* se ejecuta Activate sobre ella
- *Then* la introspección revela sus tablas en el árbol
- *When* se abre el editor, se escribe una consulta `SELECT` y se ejecuta Run
- *Then* el grid muestra las filas y la línea de estado muestra
  `<n> rows / <ms>ms`

**El historial registra toda ejecución**
- *Given* una conexión activa
- *When* se ejecuta cualquier consulta desde el editor
- *Then* queda registrada en el estado local con su conexión, y aparece en el
  selector de historial **más reciente primero**
- *And* una consulta que falla se registra igualmente, con su mensaje de error

**Browse de tabla**
- *Given* una tabla en el árbol
- *When* se ejecuta Activate sobre ella
- *Then* el grid muestra el resultado de `SELECT * FROM <tabla> LIMIT 500`

**Navegación horizontal del grid**
- *Given* un grid con más columnas de las que caben en pantalla
- *When* se mueve la selección con Right hacia una columna fuera de vista
- *Then* la rejilla se desplaza horizontalmente y la columna seleccionada queda
  visible

**Redimensionamiento de columnas del grid**
- *Given* un grid con resultados visibles y el foco en el grid
- *When* se ejecuta WidenColumn o NarrowColumn sobre la columna seleccionada
- *Then* el ancho de esa columna aumenta o disminuye de forma visible, empujando
  las columnas siguientes o mostrando su contenido truncado
- *And* AutoFitColumn restaura el ancho de la columna seleccionada al auto-fit
- *And* AutoFitAll restaura todos los anchos sobrescritos

**Edición del grid con concurrencia optimista**
- *Given* una tabla **con clave primaria** abierta en el grid
- *When* se edita una celda, se añade una fila y se marca otra para borrar, y se
  ejecuta Commit
- *Then* un modal muestra las sentencias y, al aceptar, se aplican en **una sola
  transacción** y el grid se refresca
- *And* el `WHERE` de UPDATE/DELETE incluye los valores originales, de modo que
  una fila cambiada por otra sesión afecta a 0 filas en vez de sobrescribirla

**Grid de solo lectura**
- *Given* un resultado ad-hoc del editor, o una tabla sin clave primaria
- *When* el grid tiene el foco
- *Then* no es editable y la línea de estado muestra el indicador de bloqueo

**Transacción de sesión psql-style**
- *Given* una conexión en autocommit (sin indicador transaccional)
- *When* se ejecuta `BEGIN`
- *Then* la línea de estado muestra `txn: ACTIVE`
- *When* se ejecuta una escritura y luego un `SELECT` dentro de la transacción
- *Then* el `SELECT` ve el cambio no confirmado
- *When* se ejecuta `ROLLBACK`
- *Then* el indicador desaparece y el cambio no persiste

**Guardia de operaciones destructivas**
- *Given* el editor con una conexión activa
- *When* se ejecuta un `DELETE`/`UPDATE` sin `WHERE`, o cualquier DDL
- *Then* se interpone un modal de confirmación antes de tocar la base de datos

**Resolución de credenciales**
- *Given* una conexión TCP con `keyring_key` sin secreto guardado
- *When* se intenta conectar
- *Then* se pide la contraseña en un prompt enmascarado, y tras conectar con
  éxito se guarda en el almacén del SO
- *And* en ningún caso la contraseña se escribe en el fichero de configuración

**Export**
- *Given* un resultado en el grid
- *When* se ejecuta Export y se elige un formato (CSV/TSV/JSON/SQL)
- *Then* se escribe un fichero con los datos, respetando las reglas de
  serialización de la §14.1

**Import**
- *Given* una tabla seleccionada y un fichero CSV con una fila nueva
- *When* se ejecuta Import, se carga el fichero y se confirma
- *Then* se inserta la fila en la base de datos del usuario

**Recuperación ante caída**
- *Given* un buffer con cambios sin guardar y una sesión que terminó sin cierre
  limpio (swap huérfano)
- *When* se arranca la app
- *Then* se ofrece recuperar ese trabajo
- *And* un cierre limpio no deja swaps

**Remapeo de teclas**
- *Given* una asociación de usuario que reasigna un chord existente a otra acción
- *When* se pulsa ese chord
- *Then* se ejecuta la acción del usuario, no la de por defecto

**Autocomplete de tablas y columnas**
- *Given* el editor abierto en modo Insert con una conexión activa
- *When* se escribe el inicio de una consulta y se ejecuta la acción de
  autocompletado (`Ctrl+Space`)
- *Then* aparece un popup con candidatos del esquema de la conexión: nombres de
  tablas y, tras un calificador `tabla.`, las columnas de esa tabla
- *And* al aceptar un candidato, este se inserta sustituyendo el prefijo escrito

**Schema viewer (columnas en el árbol)**
- *Given* una conexión introspeccionada con sus tablas en el árbol
- *When* se expande una tabla
- *Then* se listan sus columnas con su tipo declarado y una marca visible de
  clave primaria

**Pista de ayuda siempre visible**
- *Given* un grid editable con el foco puesto en él
- *When* la línea de estado muestra las pistas contextuales de edición
- *Then* la pista de ayuda sigue presente al final de la línea, no la sustituyen
  las pistas contextuales

**Realimentación de chord pendiente**
- *Given* la app en modo Normal sin overlays
- *When* se pulsa el leader (`Espacio`)
- *Then* aparece un menú which-key con las continuaciones (`e` editor, `h`
  historial, …) y la acción de cada una
- *When* se pulsa un primer paso que no es leader (p. ej. `g` de `gg`)
- *Then* se muestra un eco de la secuencia pendiente, sin menú
- *And* al completar, abandonar o cancelar el chord, la realimentación desaparece

**Selección rectangular de celdas del grid**
- *Given* el foco está en el grid y hay resultados visibles
- *When* se pulsa `v`
- *Then* el modo pasa a Visual, la celda actual se convierte en el ancla, y el
  status bar muestra `VIS`
- *When* se mueve el cursor con `h/j/k/l`
- *Then* el rango rectangular entre el ancla y el cursor se resalta en el grid
- *When* se pulsa `<Ctrl-c>`
- *Then* aparece un picker con opciones CSV, TSV, JSON, SQL INSERT
- *When* se selecciona un formato y pulsa Enter
- *Then* el contenido del rango seleccionado se copia al portapapeles en ese
  formato y aparece una notificación transitoria
- *When* se pulsa `Esc` o `v` en modo Visual
- *Then* se abandona el modo Visual y el resaltado desaparece

---

## 18. Rationale de las decisiones de producto

El *porqué* de las decisiones de comportamiento (las de implementación viven con
el código):

- **Concurrencia optimista en vez de locks.** Una TUI interactiva puede dejar
  una edición a medias indefinidamente; mantener locks o transacciones largas
  bloquearía a otras sesiones. El emparejamiento por valores originales evita
  perder cambios ajenos en silencio sin requerir columnas de versión, que no
  existen en esquemas arbitrarios.
- **Transacciones híbridas psql-style; autocommit sin indicador.** Familiar para
  quien viene de psql. No se pinta nada en autocommit para no añadir ruido a la
  línea de estado y porque la ausencia de marca *es* la convención psql; solo lo
  excepcional (transacción abierta) merece señal.
- **Edits del grid en transacción propia.** El commit en lote del grid es una
  unidad atómica conceptualmente independiente de las consultas ad-hoc del
  editor; mezclarlo con el régimen de sesión sorprendería al usuario.
- **Editabilidad atada a clave primaria.** Sin PK no hay forma segura de
  identificar una fila para UPDATE/DELETE; el grid se degrada a solo lectura en
  lugar de arriesgar ediciones ambiguas.
- **Contraseñas solo en el almacén del SO, con fallback por entorno.** El
  almacén es seguro por defecto; la variable de entorno mantiene utilizable CI y
  entornos sin almacén disponible, sin meter secretos en config.
- **Buffers volátiles + swap.** Coherente con los editores reales (vim/helix): el
  contenido vive en memoria y solo se persiste explícitamente, salvo el swap,
  cuya única misión es sobrevivir a una caída.
- **Keymap como acciones con nombre, remapeable.** Desacopla la tecla del efecto
  y permite que la misma acción sirva en árbol y grid según foco; remapear es un
  requisito de un cliente gobernado por teclado.
- **Browse acotado a 500 filas.** Cota fija que mantiene la UI responsiva sin
  pedir paginación al usuario para la inspección habitual de una tabla.
```
