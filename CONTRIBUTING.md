# Contribuir a lazydb

Gracias por contribuir.

## Flujo recomendado

1. Crea una rama por cambio.
2. Haz cambios pequenos y enfocados.
3. Ejecuta validaciones locales.
4. Abre PR con contexto claro.

## Validaciones antes de PR

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

## Convenciones

- Evitar cambios masivos no relacionados.
- Priorizar claridad de nombres y funciones cortas.
- Agregar comentarios solo cuando haya logica no obvia.
