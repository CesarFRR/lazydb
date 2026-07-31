# lazydb

Terminal UI para explorar bases SQLite, inspirado en la experiencia de lazygit y lazydocker.

## Estado

Proyecto en etapa activa de desarrollo.

Seccion 1 completada:
- Shell TUI base con paneles.
- Foco por panel con teclado.
- Layout adaptable (large, medium, small).

Seccion 2 completada:
- Contexto critico visible en cabecera (foco, layout, source, object, row).
- Atajos compactos cuando el ancho de terminal baja.
- Mensaje de fallback para terminal extremadamente pequena.

Seccion 3 completada:
- Conexion SQLite real en modo read-only al abrir una base.
- Carga dinamica de tablas/vistas desde sqlite_master.
- Preview de esquema con `PRAGMA table_info(...)` para el objeto seleccionado.

Seccion 4 completada:
- Preview paginado de filas reales (SELECT * ... LIMIT/OFFSET).
- Indicador dinamico de página (Page X/Y | Row R/N).
- Navegacion con Page Up/Down entre páginas (10 filas por página).
- Scroll infinito con append/prepend (carga la página siguiente al llegar al final).
- Bajo consumo de memoria: nunca carga todas las filas a la vez.

Seccion 5 completada:
- Query runner con keybinding Ctrl+Q para ejecutar COUNT(*).
- Indicador visual de estado: [Ejecutando query...], [Query completada], [Error: ...].
- Resultado mostrado en status bar.
- Estructura para refactorizar a async real con tokio::spawn + mpsc en futuro.

Seccion 6 completada:
- Persistencia de recientes en ~/.config/lazydb/recents.json.
- Dinámicamente poblados en Sources panel.
- Automáticamente se guardan al conectar a una base.
- Favoritos con nombre (tecla `f`).
- Escaneo automatico del directorio actual: las bases `*.db` / `*.sqlite` /
  `*.sqlite3` encontradas aparecen como fuentes locales sin configuración.

Seccion 7 completada (mouse y scrollbars):
- Scrollbars verticales y barra horizontal dibujados a mano: thumb de largo
  fijo, sin los artefactos de redondeo de ratatui.
- Arrastre con mouse mapeado 1:1: el thumb sigue al cursor y recorre el 100%
  del track (click en el track = salto centrado).
- La barra vertical esta sincronizada con el item seleccionado: al arrastrarla
  hasta el final, el ultimo item de la lista queda seleccionado.
- Barra horizontal delgada (bloque de media celda) con gap sutil que separa
  las zonas interactivas: pestañas, headers y barra.
- Scroll con rueda sobre cualquier panel sin cambiar el foco.
- Click en header de columna = ordenar; click en pestañas = cambiar tab.
- Doble-click en una fila de datos = inspector de fila (modal navegable con
  las flechas mientras esta abierto).

Seccion 8 completada (datos):
- Scroll horizontal de columnas: shift+rueda o shift+h / shift+l.
- Ventana de columnas visibles con indicador `cols X-Y/Z` en el titulo.
- Ordenamiento de columnas con ciclo de 3 estados: 1er click ASC (`▴`),
  2º click DESC (`▾`), 3er click desactiva el orden (por defecto).
- Filtro de búsqueda con `/` en las listas.
- Seleccion con `▸` en todos los paneles (mismo estilo que la tabla de datos).
- Exportar la tabla actual a CSV con `e`.
- Copiar el item seleccionado con `y`.

Seccion 9 completada (seguridad):
- Ctrl+C = cierre seguro: si hay un filtro, inspector o menu abierto, primero
  los cierra; solo sale de la app cuando no queda nada abierto.
- Conexiones read-only por defecto.
- Event loop fluido: poll corto + drenado de eventos pendientes.

Alcance de la primera version:
- Soporte SQLite local.
- Navegacion de tablas y esquema.
- Ejecucion de consultas read-only.
- Layout adaptable a terminal pequena.

## Principios

- Rapido al abrir y navegar.
- Poco consumo de recursos.
- Teclas consistentes y predecibles.
- Seguridad por defecto (read-only al inicio).
- El mouse es opcional: todo es usable solo con teclado.

## Requisitos

- Rust estable (>= 1.85)
- Cargo

## Desarrollo

```bash
cargo run
```

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Controles actuales:
- `q` o `Esc`: cerrar por capas (estilo lazygit): vuelve de Detalle → cierra la DB conectada → solo con todo limpio sale.
- `Ctrl+C`: cierre seguro (cierra filtro/menus abiertos, luego sale).
- `Tab` / `Shift+Tab`: cambiar foco.
- `1` / `2` / `3` / `4` / `5`: ir a un panel especifico.
- `j` / `k` o flechas: mover seleccion.
- `Page Up` / `Page Down`: cambiar página en preview.
- `[` / `]`: cambiar pestaña del panel Detalle (Datos/Esquema/SQL/Meta).
- `Ctrl+Q`: ejecutar COUNT(*) en tabla seleccionada.
- `Enter`: saltar al panel Detalle.
- `r`: refrescar.
- `f`: en Fuentes, marcar/desmarcar favorito el item bajo el cursor; en otro panel, guardar la DB actual en favoritos.
- `d`: en Fuentes, olvidar la fuente bajo el cursor (quita de recientes/favoritos; si era la DB conectada, la cierra).
- `y`: copiar item seleccionado al portapapeles.
- `e`: exportar la tabla actual a CSV.
- `/`: iniciar filtro de búsqueda en listas.
- `x` o `b`: menu de acciones.
- `shift+h` / `shift+l`: scroll horizontal de columnas.
- `shift+rueda`: scroll horizontal con el mouse.

El panel `[1]Fuentes` agrupa las bases por secciones (`── FAVORITOS ──`, `── RECIENTES ──`, `── LOCAL DETECTADO (./) ──`), con marcas de tipo: `●` conectada, `★` favorito, `▣` sqlite local, `⊙` online. Los subtítulos de sección no son seleccionables: la navegacion los salta.

## Estructura

- `src/`: codigo fuente.
  - `src/app/`: estado global (App), controladores y paneles.
  - `src/ui/`: renderizado (widgets, layout, modales).
  - `src/db/`: adaptadores y backends de base de datos.
  - `src/keys.rs`: keymap configurable.
  - `src/storage.rs`: persistencia (recientes, favoritos).
- `docs/`: notas de arquitectura y producto.
- `.github/workflows/`: integracion continua.

## Roadmap inicial

1. [x] Shell TUI base con paneles y atajos.
2. [x] Responsive fino y degradacion progresiva.
3. [x] Conector SQLite en modo lectura.
4. [x] Explorador de tablas y preview paginado.
5. [x] Query runner con indicador visual.
6. [x] Persistencia de recientes y favoritos.
7. [x] Mouse, scrollbars arrastrables y tablas anchas.
8. [x] Ordenamiento de columnas, filtros y exportacion.
9. [x] Cierre seguro con Ctrl+C.

## Licencia

MIT.
