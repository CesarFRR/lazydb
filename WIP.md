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
- Popup de error global · historial de queries persistente

## FASE 2 — Multi-backend local

- **DuckDB** (`.duckdb`, crate `duckdb` bundled, mismo patrón que sqlite.rs)
- **MySQL localhost** (crate `mysql`) y **PostgreSQL** (crate `postgres`)
- Trait de backend **SÍNCRONO** + `spawn_blocking` en el servicio (patrón lazysql, no gobang)
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
