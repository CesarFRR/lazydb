# lazydb

Terminal UI para explorar bases SQLite, inspirado en la experiencia de lazygit y lazydocker.

## Estado

Proyecto en etapa inicial.

Seccion 1 completada:
- Shell TUI base con paneles.
- Foco por panel con teclado.
- Layout adaptable (large, medium, small).

Seccion 2 completada:
- Contexto critico visible en cabecera (foco, layout, source, object, row).
- Atajos compactos cuando el ancho de terminal baja.
- Mensaje de fallback para terminal extremadamente pequena.

Seccion 3 completada:
- Conexion SQLite real en modo read-only al abrir `sakila.db`.
- Carga dinamica de tablas/vistas desde sqlite_master.
- Preview de esquema con `PRAGMA table_info(...)` para el objeto seleccionado.

Seccion 4 completada:
- Preview paginado de filas reales (SELECT * ... LIMIT/OFFSET).
- Indicador dinamico de página (Page X/Y | Row R/N).
- Navegacion con Page Up/Down entre páginas (10 filas por página).
- Bajo consumo de memoria: nunca carga todas las filas a la vez.

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
- `q` o `Esc`: salir.
- `Tab` / `Shift+Tab`: cambiar foco.
- `h` / `l`: cambiar foco estilo vim.
- `1` / `2` / `3`: ir a panel especifico.
- `j` / `k` o flechas: mover seleccion.
- `Page Up` / `Page Down`: cambiar página en preview.
- `Enter`: ejecutar accion en panel de Sources.
- `r`: refrescar.

## Estructura

- `src/`: codigo fuente.
- `docs/`: notas de arquitectura y producto.
- `.github/workflows/`: integracion continua.

## Roadmap inicial

1. [x] Shell TUI base con paneles y atajos.
2. [x] Responsive fino y degradacion progresiva.
3. [x] Conector SQLite en modo lectura.
4. [x] Explorador de tablas y preview paginado.
5. Query runner asincrono y cancelable.
6. Persistencia de recientes y favoritos.

## Licencia

MIT.
