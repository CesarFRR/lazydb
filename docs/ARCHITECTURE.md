# Arquitectura inicial (Fase 1 SQLite)

Objetivo: entregar una TUI util para SQLite con bajo consumo y buena ergonomia.

## Modulos planeados

- `app`: estado global y rutas de navegacion.
- `ui`: renderizado de paneles y barra de atajos.
- `sqlite`: introspeccion de esquema y ejecucion de consultas.
- `jobs`: ejecucion asincrona de consultas y cancelacion.
- `store`: persistencia de recientes y favoritos.

## Reglas de diseno

- No cargar resultados completos en memoria.
- Paginacion o streaming en previews de tablas.
- Consulta en segundo plano para mantener la UI fluida.
- Priorizar modo read-only en MVP.

## Layout responsivo

- Pantalla grande: 3 paneles (fuentes, objetos, resultado).
- Pantalla mediana: 2 paneles con foco.
- Pantalla pequena: 1 panel principal + overlay de acciones.

## Fuera de alcance inicial

- Conexion remota por SSH.
- Soporte multibase (PostgreSQL/MySQL/etc.).
- Edicion de datos en caliente.
