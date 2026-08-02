# WIP — lazydb: hoja de ruta de implementación

> Documento de trabajo (work in progress). Aquí vive TODO lo que vamos a hacer en esta etapa:
> decisiones, fases, bugs y estado. Se actualiza a medida que avanzamos.
> Complemento de `analisis-comunidad/brechas-lazydb.md` (informe de brechas comunidad → lazydb).

---

## Objetivo de la etapa

Consolidar la base actual de lazydb con **estándares de clean code** (como recomienda la
comunidad para TUIs con filosofía "lazy": lazydocker/lazygit/gobang/lazysql) y preparar la
arquitectura para la **siguiente etapa multi-backend**:

- **Locales:** SQLite (actual), DuckDB (`.duckdb`, archivo local)
- **Semi-online (localhost):** MySQL, PostgreSQL en `127.0.0.1`
- **Online:** MySQL/PostgreSQL en plataformas (Azure, CleverCloud, etc.) con credenciales
  seguras vía `pass`

## Filosofía (por qué Rust)

- Se eligió **Rust sobre Go**: velocidad y eficiencia extrema, necesarias para trabajar con
  bases de datos en terminal.
- Regla de oro: optimizar como en videojuegos — **renderizar/calcular solo lo visible
  (culling)**. Nada de O(n) completo por frame: virtualización, paginación lazy, probes
  perezosos con caché. La UI nunca bloquea.

---

## Estado actual (verificado en código, 2026-07-31)

✅ Tabs Sources: `All | Local | Online` (controller.rs:869)
✅ Marcas: `●` conectada · `★` favorito · `▣` sqlite local · `⊙` online (controller.rs:225-258)
✅ Secciones: FAVORITOS → RECIENTES → LOCAL DETECTADO (./) (controller.rs:284-376)
✅ Scrollbars manuales drag 1:1 · mouse · cierre por capas · CSV · Yank · filtro `/`
✅ Query async `tokio::spawn_blocking` + rusqlite read-only (query.rs)
✅ Trait `DbAdapter` + resolver (db/adapter.rs, resolver.rs)

---

## FASE ACTUAL — Panel de Fuentes (en curso)

### Bug 1 (RESUELTO ✅ commit `44a91bf`): la DB conectada no se marca `●`

- **Causa:** rutas sin normalizar. `scan_cwd_databases()` devuelve paths absolutos
  (controller.rs:381-402) pero `connect_sqlite("sakila.db")` guarda el relativo
  (controller.rs:992). La marca `●` compara strings exactos (controller.rs:313) → nunca
  coinciden.
- **Efecto colateral:** la misma DB aparece duplicada (RECIENTES relativo vs DETECTADO
  absoluto); el `seen` set no deduplica.
- **Fix aplicado:** nuevo módulo `src/paths.rs` con `normalize_path()` (expande `~`, hace
  absoluta, canonicaliza si existe, limpieza léxica de `./` `..` como fallback; las URLs
  `mysql://`, `postgres://`, `sqlite://`… no se tocan). Choke points: `connect_sqlite()` y
  `SourceList::entry()` normalizan antes de comparar/deduplicar; `forget_source()` también
  purga variantes relativas viejas del storage. 9 tests en `paths.rs` + 3 de regresión en
  controller. `cargo test` 16/16, clippy `-D warnings` limpio.

### Rediseño del panel Fuentes (propuesta)

```
┌─[1]─ Fuentes ─────────────── sakila.db ● ──────┐
│  [Todos|Local|Online]                            │
│  ── FAVORITOS ──────────────                    │
│    ● ★ sakila.db                                │
│  ── RECIENTES ──────────────                    │
│    ● ▣ base_ancha.db                            │
│  ── ARCHIVOS (./) ──────────                    │
│    ▣ sakila.db                                  │
│    ▣ base_ancha.db                              │
│  ── LOCALHOST ──────────────                    │
│    M mysql://127.0.0.1:3306/lazy  (servicio)    │
│  ── ONLINE ──────────────────                   │
│    P postgres://db.azure.com:5432/prod          │
│                                                 │
│  Buscar archivo .db                             │
│  Abrir sakila.db                                │
└─────────────────────────────────────────────────┘
```

1. **Enum `SourceKind { File, Localhost, Online }`** — ✅ HECHO (commit `7f889f2`): reemplaza
   el heuristic de strings `is_online_source` (que confundía `sqlite://` con online y ocultaba
   `mysql://localhost` del tab Local). Con `url_host()` para distinguir localhost de remoto.
2. **Marcas por tipo de DB** — ✅ HECHO (`7f889f2`, simplificado en `19b4054`): `▣` SQLite
   (azul) · `D` DuckDB (verde) · `M` MySQL (rojo) · `P` PostgreSQL (magenta) · `⊙` genérico.
   **Sin marca = todo bien**: el `✓` se eliminó (era redundante); solo `✗` rojo cuando el
   probe falla. La `★` identifica al favorito y la **sección FAVORITOS se eliminó**: los
   favoritos encabezan la lista sin encabezado.
3. **Health probe perezoso (filosofía culling):** ✅ HECHO (`6372a76` + `19b4054`) — probar
   SOLO la fuente seleccionada (NUNCA toda la lista), en segundo plano (`tokio::spawn` +
   canal mpsc, aplicado en `compute_layout()` por frame) con caché `App.health:
   HashMap<String, bool>`. **Se re-verifica en cada selección** (click o flechas): sin TTL ni
   re-probe por tiempo (simplificado en `19b4054` tras feedback del usuario). Archivos →
   `metadata is_file`; URLs → `TcpStream::connect_timeout(2s)` con puertos default (MySQL
   3306, PG 5432, http 80, https 443) vía `source_host_port()`. Marca: solo `✗` (rojo) /
   nada = bien. Rebuild solo si cambió el estado. 4 tests.
4. **Secciones por tipo:** ARCHIVOS (./) / LOCALHOST / ONLINE — parcial: sección LOCAL DETECTADO
   renombrada a ARCHIVOS (./) ✅; las secciones LOCALHOST/ONLINE llegan en Fase 2 con la
   detección real de servicios (hoy no hay nada que listar ahí).
5. **Título del panel:** ~~muestra la DB conectada~~ — 🔄 PROBADO Y REVERTIDO (`4f6ef1e`):
   quedaba redundante con el resumen del panel colapsado (ítem 7); el título vuelve a ser
   `Fuentes ([Todo] Local Online)`.
6. El render de `source_line` (panel.rs:431-451) — ✅ adaptado a las marcas nuevas.
7. **Resumen del panel colapsado** — ✅ HECHO (`89a0240` + `009c163`): el ítem único de
   resumen (patrón lazydocker contenedor seleccionado: 1º DB conectada `●`, 2º fuente bajo el
   cursor, 3º primer entry real) se muestra SOLO en el **colapso responsive** — altura 3 del
   layout (`collapse_stack`: "Sources mínimo: borde + 1 ítem", layout.rs:253), es decir cuando
   la terminal se achica. Expandido, la lista completa se muestra siempre, enfocado o no, con
   cursor y scrollbar. Función `source_summary()` en controller.rs con 3 tests.

---

## FASE 0 — Fundamentos clean code (después de Fuentes) — ✅ COMPLETA

Prepara todo para escalar; cero cambios visuales. Ítems 1-8 todos HECHO
(1 errores tipados `45c5c94`+`2e8af90` · 2 modelos de dominio `8e52f31` ·
3 Theme `4db3ae8` · 4 keymap+ayuda `204ccf8` · 5 query runner `27b6e83` ·
6 limpieza layout `8f3c2a0` · 7 robustez/tracing `d25f6f2` · 8 config
proyecto `en curso`). 51 tests verdes, clippy `-D warnings` limpio.

> Pendiente candidato a Fase 1: colapso MANUAL de paneles vía
> `ToggleCurrentPanel`/`PanelMode` (el layout hoy decide el colapso solo por
> altura; los `PanelMode` de cada panel ya no se consultan tras el ítem 6).

1. **Errores tipados** — ✅ HECHO (`45c5c94`): `DbError` (thiserror) en `src/db/error.rs` con
   variantes `Open`/`Sqlite`/`Io`/`Join`, `From` impls (rusqlite, io, JoinError → `?` mágico)
   y mensajes String planos (Display directo al status bar, `PartialEq` para tests). Migrado
   todo el dominio: sqlite.rs, adapter.rs, sqlite_adapter.rs, service.rs, query.rs, storage.rs.
   Fuera el `Result<_, String>` del dominio. 2 tests del error.
2. **Modelos de dominio** — ✅ HECHO (en curso de commit): `Row { cells }`, `Column { name, dtype }`,
   `ColumnInfo { cid, name, dtype, notnull, pk }` y `TableData { columns, rows }` en
   `src/db/model.rs`. El contrato `DbAdapter` habla en modelos tipados — **nunca** en strings
   formateados `"a | b | c"` (el bug que motivó esto: un valor con `|` dentro de una celda se
   rompía con `split('|')` en el inspector). La UI convierte con view-models (`to_lines()` /
   `to_line(" | ")`). Migrados: `table_columns`→`Vec<ColumnInfo>`, `column_names`→`Vec<Column>`,
   `table_data_rows`→`Vec<Row>`, `table_rows[_sorted]`→`TableData` (dtype vacío en filas porque
   rusqlite no tiene `column_decltype`; el dtype declarado vive en `ColumnInfo` vía PRAGMA).
   **Bug preexistente destapado por los tests**: `row.get::<_, String>` falla en columnas
   numéricas → todas las celdas INTEGER/REAL se mostraban `[NULL]` falsos en la UI. Fix:
   `cell_value_to_string()` convierte por tipo real (Null/Integer/Real/Text/Blob). 41 tests.
3. **`Theme` centralizado** — ✅ HECHO (en curso de commit): `src/ui/theme.rs` con paleta
   semántica (`selection` cyan, `unfocused` gray, `dim` darkgray, `border`, `ok` verde,
   `error` rojo, `favorite` amarillo) + `SourceColors` por tipo de backend (▣ D M P ⊙) para
   Fase 2. Const global `THEME` (zero-cost, single source of truth). Cero `Color::` fuera del
   módulo: migrados panel.rs (8 sitios), modal.rs (3), ui/mod.rs. 2 tests (paleta
   diferenciada, marcas únicas).
4. **Keymap con grupos** — ✅ HECHO (en curso de commit): `KeyGroup` (Navegación/Foco/Fuentes/
   Pestañas/Datos/Acciones) + `AppAction::group()` y `description()` (español, mnemotécnicas).
   Ayuda autogenerada desde bindings REALES (`Keymap::help_sections()`): una fila por acción
   con todas sus teclas ("j, ↓" → Mover abajo), si el usuario remapea en config.toml la ayuda
   muestra lo que funciona. Modal `?` (nueva acción `ToggleHelp`): modal con scroll y
   scrollbar, títulos de grupo destacados, cierra con `?`/esc/q; `render_lines()` nuevo en
   modal.rs para contenido estilizado por spans. Hint `?` añadido al footer. 3 tests.
5. **Query runner** — ✅ HECHO (`27b6e83`): `COUNT(*)` REAL con rusqlite + `spawn_blocking`
   (fuera el `sh -c sqlite3` bloqueante del sistema, que además era un agujero de inyección
   por comillas). Cancelación con **generation counter**: `query_gen++` al lanzar/limpiar/
   desconectar; resultados stale descartados en el tick (`apply_count_result` pura, testeada).
   `QueryState` vivo (`Running`/`Done`/`Error`, fuera `allow(dead_code)`) y **spinner braille
   en el status bar** mientras corre (patrón lazy: feedback inmediato, UI nunca se congela).
   Pendiente menor: el tab Query real (hoy el resultado solo vive en el status bar).
6. **Limpieza layout.rs:** `panel_modes` recibidos y no usados (`_panel_modes`,
   layout.rs:175) — integrar `PanelMode` real o eliminar params muertos.
7. **Robustez** — ✅ HECHO (en curso de commit): `tracing` a archivo rotativo diario
   (`~/.config/lazydb/logs/lazydb.YYYY-MM-DD.log`, non-blocking; la app jamás imprime en
   stdout). Instrumentados: arranque/cierre, conexión/desconexión, probes (lanzados y
   resueltos), COUNT(*). `ratatui::init()`/`restore()` + panic hook propio que loggea y
   restaura la terminal antes de morir (adiós raw-mode colgado tras un panic). El filtro
   `KeyEventKind::Press` ya existía. Dependencias: tracing, tracing-subscriber,
   tracing-appender.
8. **Config por proyecto** — ✅ HECHO (en curso de commit): `Config`/`UiConfig` con serde
   derive y `#[serde(default)]` (config mínima válida; adiós parseo manual a mano). `Config::load()`
   = global (`~/.config/lazydb/config.toml`) + **por proyecto** (`lazydb.toml` buscado desde el
   CWD hacia arriba, se detiene en la raíz del repo — el dir con `.git` — o la raíz del fs);
   fusión recursiva de tablas TOML (el proyecto gana campo a campo). El keymap también lee la
   config del proyecto: `[keys]` del proyecto sobreescribe al global (`set_binding` reemplaza).
   Clamp `rows_per_page` 1..=500. 5 tests (búsqueda desde el fondo del árbol, no escapa del
   repo, overlay gana, defaults, fusión recursiva).

## FASE 1 — Pulido UX

- **Data tab con celdas 2D** — ✅ HECHO (en curso de commit): `TableState` real con
  `highlight_symbol("▎")` + `row_highlight_style` (fila seleccionada cyan bold, patrón
  lazygit; fuera el ▸/BOLD manual). Celdas TIPADAS: el controller guarda
  `preview_data: Option<TableData>` (sincronizada con el scroll infinito) y el render lee
  `Row.cells` — sin `split(" | ")` en la UI (el bug de pipes que se colaba en el render).
  Se reserva 1 columna para el símbolo (sin desbordar el borde). Fallback strings para
  mensajes/List de 1 columna. Función pura `data_cell_str` + 3 tests (pipes intactos,
  formato │, truncado). La ayuda autogenerada del ítem 4 de Fase 0 ya cubre "ayuda".
- **FK Jump** — ✅ HECHO: `Enter` sobre una fila de datos resuelve la primera FK
  con valor no nulo (`PRAGMA foreign_key_list`) y salta a la tabla referenciada,
  posicionado en la página y fila exactas (offset por rowid, `COUNT(*) WHERE rowid <= ?`).
  FK sin columna explícita (`REFERENCES t`) → PK de la tabla destino. Si la tabla no está
  cargada, se recarga la lista. Sin FK o valor nulo → inspector de fila (como antes).
  `to: Option<String>` en `ForeignKey`; helpers `foreign_keys()` y `row_offset_of()`
  en sqlite.rs + 4 tests de contrato (FK list, tabla sin FKs, offset, tabla vacía).
- **Popup de error global** — ✅ HECHO: modal rojo (`THEME.error`) encima de todo,
  título `✗` + body con wrap; se cierra con Enter/Esc/q y **ignora el resto de
  teclas** (patrón lazygit: el error nunca se pierde bajo navegación). Helper
  `App::show_error()` → `tracing::error!` + estado `error: Option<ErrorPopup>`;
  migrados 10 callers: apertura de DB, SQL/contar/obtener filas, schema, export
  y cli sqlite3. `render_lines` gana `border_style: Option<Style>` opcional.
  2 tests (cierre con Enter/Esc/q, otras teclas ignoradas).
- **Historial de queries persistente** — ✅ HECHO: input SQL modal estilo `:` (vim) con
  cursor real del terminal (`set_cursor_position`), historial navegable ↑/↓ (rellena el
  buffer, selección resaltada cyan, estilo fish). Enter ejecuta la query asíncrona
  (`tokio::spawn`, generación anti-stale) usando `query::execute_query` (re-desterileizado
  de `dead_code`). Canal unificado `QueryMsg::Count | QueryMsg::Free`. Resultado en
  preview (modo query, sin scroll infinito). Historial persistente en `recents.json`
  (`query_history: Vec<String>` deduplicado, trim, reposiciona, máx 50 entradas).
  `Theme` gana `text`/`bg` para inversión semántica; `inner_area` hecho pub(crate).
  `Render_query_input` en ui/mod.rs (modal con prompt `❯`, historial debajo, hint al pie).
  13 tests (storage 4, controller 8, theme se mantiene).

## FASE 2 — Multi-backend local

### ✅ DuckDB backend (HECHO, 2026-08-02)

- Crate `duckdb 1.4.5` con feature `bundled` (API estilo rusqlite, tal como previó la tabla de drivers)
- `src/db/backends/duckdb.rs` replica el patrón de `sqlite.rs` (funciones puras path → Result):
  - `list_objects_by_type` vía `information_schema.tables/views` + `duckdb_indexes()/duckdb_triggers()`
    (filtrando `table_type = 'BASE TABLE'` para que las vistas no aparezcan como tablas)
  - `object_sql` vía `duckdb_tables()`/`duckdb_views()` (DuckDB guarda el SQL original en el catálogo)
  - `column_names`/`table_columns` vía `PRAGMA table_info` — OJO: `notnull`/`pk` son **BOOLEAN** en DuckDB
  - `table_rows[_sorted]`, `table_row_count`, `table_data_rows` (mismo LIMIT/OFFSET que sqlite)
  - `foreign_keys` vía `duckdb_constraints()` + `array_to_string()` — NO existe `PRAGMA foreign_key_list`
    en DuckDB; las columnas de constraints son `varchar[]` y hay que aplanarlas
  - `row_offset_of` con `ROW_NUMBER() OVER ()` (DuckDB no tiene rowid)
  - `cell_value_to_string` con todas las variantes de `ValueRef` de arrow (Boolean, UTinyInt..UBigInt,
    HugeInt, Decimal, Date32, Time64, Timestamp, Interval, List, Enum, Struct, Map, Union, Array);
    tipos compuestos → `<list>`/`<struct>`/etc.
  - Apertura **read-only** con `Config::access_mode(AccessMode::ReadOnly)` (no hay OpenFlags)
- `src/db/backends/duckdb_adapter.rs` implementa `DbAdapter` (misma delegación que SqliteAdapter)
- Resolver ampliado: `duckdb://` + extensiones `.duckdb`/`.ddb` → DuckdbAdapter
- 9 tests + 1 smoke test `#[ignore]` contra `/home/cesar/dev/lazydb/fw2-aai_Latn.duckdb` (tabla `data`
  con 432 filas, PK compuesta `(dump VARCHAR, id UUID)`; DDL, filas y tipos verificados)

**Notas para próximos backends (lecciones DuckDB):**
- No asumir PRAGMAs de SQLite: verificar con la CLI real antes (`duckdb file.duckdb "PRAGMA..."`)
- Los tipos del driver pueden diferir (bool vs int en PRAGMA, List en constraints)
- `column_names()` del Statement panica si la query no se ejecutó (distinto de rusqlite)
- ⚠️ **FORMATO NO BACKWARD-COMPATIBLE**: un `.duckdb` creado por una versión X NO lo abre
  un crate embebido de versión < X (DuckDB 1.4.5 no lee archivos de 1.5.x → "catálogo
  ilegible"). Regla: el crate `duckdb` debe ir siempre a la última (~1.10505 ≈ CLI 1.5.x).
  Verificar antes: `duckdb --version` de la CLI del sistema vs `cargo search duckdb`.

### ✅ DuckDB integrado en la UI (HECHO, 2026-08-02)

- **Trait `DbAdapter` ampliado** al contrato completo que el controller usaba directo contra
  sqlite: `column_names`, `table_data_rows`, `table_rows_sorted`, `foreign_keys`,
  `row_offset_of`, `query(sql, limit)` y `count(sql)` (query libre del modal `:`).
  Ahora es el ÚNICO punto de acceso a datos desde la UI (16 call sites migrados).
- `connect_sqlite` (ahora genérico) resuelve el backend por extensión vía `db::resolver`.
- `query.rs` (modal `:`) despacha por extensión: `resolve_backend` + `spawn_blocking`.
  Añadidos `query_free`/`count_free` a sqlite.rs y duckdb.rs (duckdb: `column_count()`
  solo tras ejecutar `query()`, vía `rows.as_ref()` por el borrow).
- **Panel de fuentes**: `scan_cwd_databases` detecta `.duckdb`/`.ddb` además de
  `.db`/`.sqlite`/`.sqlite3`; `connect_selected_source` acepta las extensiones nuevas;
  la marca `D` ya existía en `db_type_mark`. El smoke test `#[ignore]` abre
  `fw2-aai_Latn.duckdb` completo desde la UI (tablas/vistas/índices/datos/DDL/FK).
- 4 tests nuevos (count duckdb, query duckdb, query error, scan cwd) → 86 verdes.

### ✅ Fix "catálogo ilegible" DuckDB 1.5 (HECHO, 2026-08-02)

- **Síntoma**: tras el bump a duckdb 1.10505 (lib 1.5.5), la UI seguía fallando con
  "no se pudo leer el catálogo" en TODOS los `.duckdb`, aunque el smoke test de tablas pasaba.
- **Causa raíz**: `connect_sqlite` exige que tablas+vistas+avanzados devuelvan `Ok` los
  tres. En 1.5.x, `duckdb_triggers()` **ya no existe** (DuckDB eliminó los triggers del
  motor) → `list_advanced_objects` fallaba siempre → todo el connect abortaba.
  El smoke test solo probaba tablas: por eso pasaba mientras la UI fallaba.
- **Fix**: `list_advanced_objects` ahora solo lista índices; "trigger" devuelve `[]`.
- **Bonus encontrado**: en 1.5.x las vistas internas (`duckdb_*`, `sqlite_master`,
  `pragma_*`) viven en schema `main` (antes `system`) y se colaban en la lista de
  vistas. Ahora se filtran con `NOT internal` de `duckdb_views()`.
- **Lección**: verificar SIEMPRE el catálogo completo contra la versión real del motor
  (`duckdb -c "SELECT ... FROM duckdb_functions() ..."`), no solo la ruta feliz.
  El smoke test de la UI (`smoke_flujo_ui_completo`) replica el flujo completo
  (normalize_path → resolver → tablas/vistas/índices/triggers/avanzados).

### ✅ Inspector de fila + tipos avanzados DuckDB (HECHO, 2026-08-02)

- **Panic "The statement was not executed yet"**: `table_data_rows` (inspector de fila)
  llamaba `stmt.column_count()` ANTES de ejecutar la query → panic en duckdb-rs.
  Ahora obtiene el nº de columnas del catálogo (`column_names(...).len()`), igual que
  `table_rows_sorted`. **Lección**: en duckdb-rs NUNCA tocar `column_count()`/
  `column_names()` del Statement sin haber ejecutado; sacar la metadata del catálogo.
- **Tipos avanzados ya no son placeholders**: `cell_value_to_string` ahora renderiza
  - Timestamp/Date32/Time64 → fecha civil real (algoritmo `civil_from_days` de Hinnant,
    sin chrono) con fracción `.ffffff` cuando aplica
  - Interval → `Xm Yd HH:MM:SS` (partes no nulas)
  - Enum → valor del diccionario vía `ValueRef::as_str()`
  - Blob → `0x<hex>` · Geometry → `WKB[nB]` · List → `<list[n]>` (longitud real)
  - Struct/Map/Union/Array → placeholder (requieren Arrow API; pendiente)
- Tests: `render_fechas_y_horas_usa_fecha_civil` (epoch real verificada con `date +%s`,
  incluye fecha pre-1970) + smoke ampliado con el path del inspector sobre ambos `.duckdb`
  reales → 87 verdes.

### ✅ Tipos compuestos expandidos en el inspector de fila (HECHO, 2026-08-02)

- **Idea del usuario**: los placeholders `<struct>`/`<map>`/`<union>`/`<array>` no caben
  en una celda, pero el inspector de fila (modal con scroll) sí tiene espacio → mostrar
  el contenido COMPLETO con indentación al navegar filas con ↑/↓.
- `cell_value_to_pretty(row, i)` → `ValueRef::to_owned()` → `value_to_pretty` recursivo:
  - List/Array → `[elem, ...]` un elemento por línea (anidado funciona)
  - Struct → `{ campo: valor, ... }` con indentación por nivel (structs anidados ok)
  - Map → `{ clave: valor, ... }`
  - Union → `<union>` + contenido indentado
  - Escalares → mismo formato que `cell_value_to_string`
- Contrato: nuevo método `DbAdapter::table_data_rows_pretty` (default delega al compacto;
  sqlite no cambia). El controller lo usa SOLO en `refresh_row_inspector`.
- Modal `render_table` ahora parte por `\n` explícito ANTES de `wrap_text` (los valores
  expandidos son multilínea; sin esto se rompía el wrap por caracteres).
- Smoke ampliado: `estructuras_complejas` de `mi_test_db.duckdb` muestra list de listas,
  struct con struct anidado (contacto), map y union expandidos.

### ✅ Formato numpy/pandas + JSON pretty + scrollbar interior del modal (HECHO, 2026-08-02)

- **Regla numpy/pandas (idea del usuario)**: el PRIMER nivel de una lista/array son
  "filas" → una por línea; los niveles internos van COMPACTOS (una línea). Resultado:
  - Matriz 2D `[[1,2],[3,4]]` → `[ [1, 2], [3, 4] ]` (2 líneas, no 6)
  - Lista 1D `[dev, test, v1]` → una sola línea
  - Matriz 3D → solo el primer nivel en líneas (`[[1, 2], [3, 4]]` inline por fila)
  - Structs/maps dentro de listas aún se expanden (re-sangrados con `replace('\n', ...)`)
  - Union: escalar → `union(valor)` inline · compuesto → bloque indentado
- **JSON strings**: un `Value::Text` que empieza por `{`/`[` se formatea con
  `serde_json::to_string_pretty` (nueva dependencia `serde_json = "1"`) → `payload_json`
  de `estructuras_complejas` ya no es una línea kilométrica. Texto normal intacto.
- **Scrollbar DENTRO del modal** (decisión final del usuario): el scrollbar vive en la
  última columna del INNER del modal (no en el borde), donde el hit-testing es
  inequívoco → un click en esa columna SOLO puede significar scroll del modal.
- `modal.rs` refactorizado con geometría COMPARTIDA entre render y controller:
  `geometry(area, w_pct, h_pct)` const fn, `table_geometry(inner) -> (key_w, val_w)`
  const fn (key = `(w-3)*40/100` máx 8), `expand_pairs()` (split por `\n` + wrap) —
  el controller mide `content_len` con la MISMA fórmula que dibuja el render.
- `render_lines`/`render_table`: `content_rect = rect - 1 columna` + scrollbar interior
  (`panel::draw_v_scrollbar`); `render_table` usa `TableState::with_offset(offset)` +
  `with_selected(Some(offset))` → tabla y scrollbar SIEMPRE sincronizados (el
  "scroll raro" anterior era el auto-scroll de ratatui divergiendo del scrollbar manual).
- Drag del scrollbar del modal: `DragState::InspectorScroll { rect, content_len }`
  (hit-testing con `geometry(70,70)` idéntica al render), jump-to-position + 1:1,
  misma matemática que los paneles.
- Tests: `render_compuestos_usa_regla_numpy` (lista 1D, matriz 2D/3D, vacía, union) +
  `render_texto_json_se_formatea_pretty` (JSON válido → pretty; texto normal y JSON
  roto intactos) → 89 verdes.

### Drivers verificados (jul 2026, Gemini + crates.io cruzados)

| Motor | Crate | Tipo | Estado 2026 | Nota lazydb |
|---|---|---|---|---|
| SQLite | `rusqlite` | sync, FFI C | ✅ vigente (0.38) | YA EN USO. bundled; read-only (WAL no aplica) |
| DuckDB | `duckdb` (~1.10505) | sync, FFI | ✅ muy activo | API estilo rusqlite → copia directa del patrón sqlite.rs |
| PostgreSQL | `postgres` (sync) / `tokio-postgres` (async) | protocolo nativo | ✅ vigente | **sync para nosotros** + `spawn_blocking`. Pool bb8/deadpool si hace falta. Pipelining nativo (tip gemini) |
| MySQL/MariaDB | `mysql` (sync) / `mysql_async` | protocolo binario puro | ✅ vigente | sync para nosotros; protocolo binario sin capa de red |
| MongoDB | `mongodb` (oficial) | async, 100% Rust | ✅ | Fase 3; bson para bytes |
| Redis | `redis-rs` (+ `fred` para clustering masivo) | async | ✅ | Fase 3; redis-rs basta (fred = overkill para TUI) |
| ClickHouse | `clickhouse` | async, columnar | ✅ | Fase 3; formato nativo de bloques |
| ScyllaDB | `scylla` | async | ✅ | opcional, solo si el usuario lo pide |
| MSSQL | `tiberius` | async, TDS puro | ✅ | opcional (evita ODBC en Linux) |
| Embebido KV | `redb` (100% Rust) / `rust-rocksdb` | sync | ✅ | candidato caché local; JSON simple basta por ahora |
| ❌ Evitar | `sqlx` | async | — | compile-time queries inútiles para SQL dinámico de usuario; overhead de parsing AST; decisión de arquitectura ya tomada (trait sync + spawn_blocking) |

### Estrategia lazy (de Gemini, ya aplicada en parte)

- Trait de backend **SÍNCRONO** + `spawn_blocking` en el servicio (patrón lazysql, no gobang)
- Queries en task + `tokio::sync::mpsc` → la UI nunca se congela (query runner actual)
- Culling: solo la página visible (LIMIT/OFFSET ya implementado); los bytes crudos
  on-demand (`Arc<[u8]>` hasta el render) son un refinamiento futuro, no bloqueante:
  el modelo tipado `Row.cells` ya decodifica a nivel de filas/páginas visibles
- `BackendCapabilities { has_schemas, supports_limit_offset, quoting }` para adaptar la UI
  sin `if backend == sqlite`
- Selector de fuente: popup de conexiones (gobang connections.rs)

## FASE 3 — Online

- `ConnectionProfile { name, kind, host, port, user?, db? }` — credenciales NUNCA en repo:
  vía `pass` o prompt
- Conexiones TLS (Azure, CleverCloud) · árbol con schemas si el backend los tiene
- Tabs Sources con pestaña "Conexiones" (además de Recientes/Favoritos)

---

## Orden de ejecución

1. **Panel Fuentes:** fix normalización + rediseño (fase actual)
2. **Fase 0** — fundamentos
3. **Fase 1** — pulido UX
4. **Fase 2** — DuckDB → MySQL → PostgreSQL
5. **Fase 3** — online + `pass`

> Regla: cada ítem = commit pequeño y enfocado con test. Los tests del dominio nunca
> necesitan terminal (lección de gobang `database-tree`).
