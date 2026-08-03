//! Runtime tokio compartido para los backends async.
//!
//! El trait `DbAdapter` es SÍNCRONO, pero algunos drivers (`mysql_async`,
//! `tokio_postgres` + `deadpool`) son async. Cada función pública del backend
//! bloquea con `block_on`. La app corre sobre `#[tokio::main]`, así que YA
//! existe un runtime cuando llamamos a estas funciones síncronas. Intentar
//! crear OTRO runtime dentro de uno activo lanza "Cannot start a runtime
//! from within a runtime". Por eso `block_on` reutiliza el runtime actual
//! si lo hay (`Handle::try_current`), y solo crea uno nuevo como respaldo
//! cuando se llama fuera de tokio (CI, `#[tokio::test]` externo).

static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();

fn fallback_runtime() -> &'static tokio::runtime::Runtime {
    #[allow(clippy::unwrap_used)]
    RUNTIME
        .get_or_init(|| tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap())
}

/// Ejecuta un future bloqueando el hilo actual.
///
/// - Dentro de un runtime activo (el de `#[tokio::main]` de la app) usa
///   `Handle::try_current` + `tokio::task::block_in_place`: es la forma segura
///   de hacer E/S bloqueante dentro de un runtime multi-thread sin abrir un
///   runtime anidado ni bloquear el worker thread.
/// - Fuera de cualquier runtime (CI, tests async externos): usa uno propio
///   lazy-init (`fallback_runtime`).
///
/// OJO: `block_in_place` panica dentro de un runtime *current-thread*
/// (p. ej. `#[tokio::test]`). Los tests de regresión construyen su propio
/// runtime multi-thread para replicar el contexto real de la app.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(f)),
        Err(_) => fallback_runtime().block_on(f),
    }
}
