//! Secrets con el almacén del SISTEMA OPERATIVO.
//!
//! Nunca guardamos contraseñas en disco (ni en `recents.json` ni en config).
//! El almacén nativo de cada SO:
//! - Linux → `secret-tool` (CLI de `libsecret` / Secret Service: `KWallet`,
//!   `GNOME Keyring`...). Preferido porque el crate `keyring` 3.x escribe en
//!   un collection que el `Secret Service` de KDE no ve (`set_password` reporta
//!   OK pero no persiste).
//! - Windows/macOS/otros → crate `keyring` (Credential Manager / Keychain).
//!
//! `secret-tool` usa attributes `service` y `user` (ambos fijos y estables).
//!
//! Flujo:
//! 1. El usuario pega una URL con `user:pass@host:port/db`.
//! 2. `save_credentials` extrae las credenciales, las guarda en el almacén
//!    bajo `user = host:port/db` y devuelve la URL SIN credenciales.
//! 3. El reciente guardado es la URL SIN password (nunca se filtra a disco).
//! 4. Al reabrir, `get_credentials` recupera user:pass del almacén.

use std::io::Write;

use crate::db::DbError;

/// Service del almacén (namespace lazydb).
const SERVICE: &str = "lazydb";

/// Clave interna (el `user` del almacén): `host:port/db`.
fn key_for(url: &str) -> String {
    strip_credentials(url)
        .replacen("postgresql://", "postgres://", 1)
        .trim_start_matches("mysql://")
        .trim_start_matches("postgres://")
        .trim_start_matches("mongodb://")
        .to_string()
}

/// ¿La URL trae credenciales embebidas (`user:pass@`)?
pub fn has_credentials(url: &str) -> bool {
    url.find("://").and_then(|i| url[i + 3..].find('@')).is_some_and(|at| at > 0)
}

/// Extrae `(user, pass)` de una URL `scheme://user:pass@host...`.
pub fn extract_credentials(url: &str) -> Option<(String, String)> {
    let at = url.find("://")?;
    let rest = &url[at + 3..];
    let at_mark = rest.find('@')?;
    if at_mark == 0 {
        return None;
    }
    let creds = &rest[..at_mark];
    let (user, pass) = creds.split_once(':').unwrap_or((creds, ""));
    Some((user.to_string(), pass.to_string()))
}

/// Quita `user:pass@` de la URL (para recents, mostrar, keys del almacén).
pub fn strip_credentials(url: &str) -> String {
    let Some(at) = url.find("://") else { return url.to_string() };
    let scheme = &url[..at + 3];
    let rest = &url[at + 3..];
    match rest.find('@') {
        Some(at_mark) if at_mark > 0 => format!("{scheme}{}", &rest[at_mark + 1..]),
        _ => url.to_string(),
    }
}

// ─── Backend: secret-tool (Linux) con fallback keyring ─────────────────

/// Ejecuta `secret-tool` con los args dados. Devuelve `Some(stdout)` si el
/// comando existe y termina con éxito; `None` si no existe o falla.
fn run_secret_tool(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("secret-tool").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn secret_store(key: &str, pass: &str) -> bool {
    // `secret-tool store` lee la password de STDIN (no de args — evita que
    // el password aparezca en ps/argv).
    let Ok(mut child) = std::process::Command::new("secret-tool")
        .args(["store", "--label", &format!("lazydb:{key}"), "service", SERVICE, "user", key])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(pass.as_bytes());
        let _ = stdin.write_all(b"\n");
    }
    child.wait().is_ok_and(|s| s.success())
}

fn secret_lookup(key: &str) -> Option<String> {
    let out = run_secret_tool(&["lookup", "service", SERVICE, "user", key])?;
    (!out.is_empty()).then_some(out)
}

fn secret_clear(key: &str) -> bool {
    run_secret_tool(&["clear", "service", SERVICE, "user", key]).is_some()
}

/// Guarda las credenciales de la URL en el almacén del SO.
pub fn save_credentials(url: &str) -> Result<Option<(String, String)>, DbError> {
    let Some((user, pass)) = extract_credentials(url) else {
        return Ok(None);
    };
    let key = key_for(url);

    // Linux: secret-tool (garantizado en KDE/KWallet). Fallback: crate keyring.
    // Se guarda `user\npass` para recuperar AMBOS al reabrir (el user no
    // viaja en la URL limpia de recents).
    let payload = format!("{user}\n{pass}");
    let ok = secret_store(&key, &payload)
        || keyring::Entry::new(SERVICE, &key).is_ok_and(|e| e.set_password(&payload).is_ok());
    if !ok {
        return Err(DbError::Open(format!(
            "no se pudieron guardar credenciales en el almacén del SO ({key})"
        )));
    }
    tracing::info!(user = %user, key = %key, "credenciales guardadas en el almacén del SO");
    Ok(Some((user, pass)))
}

/// Recupera `(user, pass)` del almacén para la URL.
// `Result<Option<...>>` es intencional: distingue "sin credenciales"
// (`Ok(None)`) de "error del almacén" (`Err`).
#[allow(clippy::unnecessary_wraps)]
pub fn get_credentials(url: &str) -> Result<Option<(String, String)>, DbError> {
    let key = key_for(url);
    // Linux: secret-tool primero; fallback keyring.
    let pass = secret_lookup(&key)
        .or_else(|| keyring::Entry::new(SERVICE, &key).ok().and_then(|e| e.get_password().ok()));
    pass.map_or(Ok(None), |payload| {
        // Payload = `user\npass` (guardado así para recuperar ambos).
        let mut lines = payload.splitn(2, '\n');
        let user = lines.next().unwrap_or("").to_string();
        let pass = lines.next().unwrap_or("").to_string();
        tracing::debug!(key = %key, user = %user, "credenciales recuperadas del almacén");
        Ok(Some((user, pass)))
    })
}

/// Elimina las credenciales guardadas para la URL.
#[allow(dead_code, clippy::unnecessary_wraps)]
pub fn forget_credentials(url: &str) -> Result<(), DbError> {
    let key = key_for(url);
    let ok = secret_clear(&key)
        || keyring::Entry::new(SERVICE, &key).is_ok_and(|e| e.delete_credential().is_ok());
    if ok {
        tracing::info!(key = %key, "credenciales eliminadas del almacén");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_user_pass_de_la_url() {
        let (user, pass) =
            extract_credentials("postgresql://uo8h6cfqdm4u5xv6lfqq:secret@host:5432/db")
                .expect("credenciales");
        assert_eq!(user, "uo8h6cfqdm4u5xv6lfqq");
        assert_eq!(pass, "secret");
    }

    #[test]
    fn sin_credenciales_devuelve_none() {
        assert!(extract_credentials("sqlite:///tmp/x.db").is_none());
        assert!(extract_credentials("mongodb://127.0.0.1:27017/db").is_none());
        assert!(!has_credentials("postgres://host:5432/db"));
    }

    #[test]
    fn strip_quita_solo_las_credenciales() {
        assert_eq!(
            strip_credentials("postgresql://user:pass@host:5432/db"),
            "postgresql://host:5432/db"
        );
        assert_eq!(strip_credentials("sqlite:///tmp/x.db"), "sqlite:///tmp/x.db");
        assert_eq!(strip_credentials("mysql://root:root@127.0.0.1:3306"), "mysql://127.0.0.1:3306");
    }

    #[test]
    fn ui_nunca_muestra_el_password() {
        // REGLA DE SEGURIDAD DE UI: cualquier string que se renderice en
        // pantalla (status bar, Meta tab, panel Fuentes) debe pasar por
        // strip_credentials. Verificamos los formatos que produce la UI.
        let url = "postgresql://uo8h6cfqdm4u5xv6lfqq:dsJBQr44561wnu9YizPLTeP1GFh0eO@host:5432/db";
        let safe = strip_credentials(url);
        // Meta tab: `db_path: <safe>`
        assert!(!format!("db_path: {safe}").contains("dsJBQr"), "Meta tab filtra password");
        // Status: `Conectando a <safe>...`
        assert!(!format!("Conectando a {safe}...").contains("dsJBQr"));
        // Error: `<safe>: fuente no soportada`
        assert!(!format!("{safe}: fuente no soportada").contains("dsJBQr"));
        // El host y la base se conservan (info útil)
        assert!(safe.contains("host:5432/db"), "el host/db no debe perderse: {safe}");
    }

    #[test]
    fn key_for_es_estable_y_sin_credenciales() {
        let k1 = key_for("postgresql://user:pass@host:5432/db");
        let k2 = key_for("postgres://host:5432/db");
        assert_eq!(k1, k2, "postgresql:// y postgres:// deben dar la misma clave");
        assert!(!k1.contains("pass"), "la clave no debe contener el password");
    }

    #[test]
    fn has_credentials_detecta_user_pass() {
        assert!(has_credentials("mysql://user:pass@host:3306/db"));
        assert!(!has_credentials("mysql://host:3306/db"));
    }

    /// Round-trip real contra el almacén del SO (requiere `secret-tool` en
    /// Linux o keyring activo en Win/macOS). Con `--ignored --nocapture`.
    #[test]
    #[ignore = "requiere el almacén del SO (secret-tool o keyring) activo"]
    fn keyring_round_trip_guarda_y_recupera() {
        let url = "postgresql://keyring_test_user:pass_test_123@host:5432/db";
        let (user, pass) = save_credentials(url).expect("guardar").expect("credenciales");
        assert_eq!(user, "keyring_test_user");
        assert_eq!(pass, "pass_test_123");

        // La URL sin credenciales recupera las guardadas
        let limpia = strip_credentials(url);
        let (u2, p2) = get_credentials(&limpia).expect("recuperar").expect("existen");
        assert_eq!(u2, "keyring_test_user");
        assert_eq!(p2, "pass_test_123");

        // Limpiar: la segunda lectura debe dar None
        forget_credentials(&limpia).expect("borrar");
        assert!(
            get_credentials(&limpia).expect("releer").is_none(),
            "tras forget_credentials no debe haber entrada"
        );
    }
}
