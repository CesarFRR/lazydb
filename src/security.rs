//! Secrets con el almacén del SISTEMA OPERATIVO (patrón keyring).
//!
//! Nunca guardamos contraseñas en disco (ni en `recents.json` ni en config):
//! se delegan al almacén nativo cifrado de cada SO, vía el crate `keyring`:
//! - Windows → `Credential Manager` (DPAPI)
//! - macOS → `Keychain`
//! - Linux → `Secret Service` (`GNOME Keyring` / `KWallet`)
//!
//! Flujo:
//! 1. El usuario pega una URL con `user:pass@host:port/db`.
//! 2. `save_credentials` extrae las credenciales, las guarda en el keyring
//!    bajo la clave `lazydb://host:port/db` y devuelve la URL SIN credenciales.
//! 3. El reciente guardado es la URL SIN password (nunca se filtra a disco).
//! 4. Al reabrir, `get_credentials` recupera user:pass del keyring y la
//!    conexión se reconstruye.

use crate::db::DbError;

/// Prefijo de servicio para las entradas del keyring (namespace lazydb).
const SERVICE: &str = "lazydb";

/// Clave interna del keyring para una URL: `host:port/db`.
fn key_for(url: &str) -> String {
    let stripped = strip_credentials(url);
    // Normaliza `postgresql://` → `postgres://` para claves estables
    stripped
        .replacen("postgresql://", "postgres://", 1)
        .trim_start_matches("mysql://")
        .trim_start_matches("postgres://")
        .trim_start_matches("mongodb://")
        .to_string()
}

/// ¿La URL trae credenciales embebidas (`user:pass@`)?
pub fn has_credentials(url: &str) -> bool {
    url.find("://")
        .and_then(|i| url[i + 3..].find('@'))
        .is_some_and(|at| at > 0)
}

/// Extrae `(user, pass)` de una URL `scheme://user:pass@host...`.
/// Devuelve `None` si no trae credenciales.
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

/// Quita `user:pass@` de la URL (para recents, mostrar, keys del keyring).
pub fn strip_credentials(url: &str) -> String {
    let Some(at) = url.find("://") else { return url.to_string() };
    let scheme = &url[..at + 3];
    let rest = &url[at + 3..];
    match rest.find('@') {
        Some(at_mark) if at_mark > 0 => format!("{scheme}{}", &rest[at_mark + 1..]),
        _ => url.to_string(),
    }
}

/// Guarda las credenciales de la URL en el keyring del SO y devuelve
/// `(user, pass)` extraídas. Si la URL no trae credenciales, no hace nada
/// y devuelve `None`.
pub fn save_credentials(url: &str) -> Result<Option<(String, String)>, DbError> {
    let Some((user, pass)) = extract_credentials(url) else {
        return Ok(None);
    };
    let key = key_for(url);
    let entry = keyring::Entry::new(SERVICE, &key)
        .map_err(|e| DbError::Open(format!("keyring (nuevo): {e}")))?;
    entry
        .set_password(&pass)
        .map_err(|e| DbError::Open(format!("keyring (guardar {user}@{key}): {e}")))?;
    tracing::info!(user = %user, key = %key, "credenciales guardadas en el almacén del SO");
    Ok(Some((user, pass)))
}

/// Recupera `(user, pass)` del keyring para la URL (con o sin credenciales).
/// Devuelve `None` si no hay credenciales guardadas (p.ej. sqlite local).
pub fn get_credentials(url: &str) -> Result<Option<(String, String)>, DbError> {
    let key = key_for(url);
    let entry = keyring::Entry::new(SERVICE, &key)
        .map_err(|e| DbError::Open(format!("keyring (nuevo): {e}")))?;
    match entry.get_password() {
        Ok(pass) => {
            // El user viaja en la URL limpia o se deriva del keyring si la
            // URL original lo tenía; si la URL ya trae user, se prefiere.
            let user = extract_credentials(url).map(|(u, _)| u);
            let user = user.unwrap_or_else(|| key.split('/').next().unwrap_or("").to_string());
            tracing::debug!(key = %key, "credenciales recuperadas del almacén");
            Ok(Some((user, pass)))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(DbError::Open(format!("keyring (leer {key}): {e}"))),
    }
}

/// Elimina las credenciales guardadas para la URL (al desconectar o "olvidar").
/// Aún no se invoca desde la UI (próximo paso), pero es parte de la API.
#[allow(dead_code)]
pub fn forget_credentials(url: &str) -> Result<(), DbError> {
    let key = key_for(url);
    let entry = keyring::Entry::new(SERVICE, &key)
        .map_err(|e| DbError::Open(format!("keyring (nuevo): {e}")))?;
    match entry.delete_credential() {
        Ok(()) => {
            tracing::info!(key = %key, "credenciales eliminadas del almacén");
            Ok(())
        }
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(DbError::Open(format!("keyring (borrar {key}): {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_user_pass_de_la_url() {
        let (user, pass) = extract_credentials(
            "postgresql://uo8h6cfqdm4u5xv6lfqq:secret@host:5432/db",
        )
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
        assert_eq!(
            strip_credentials("mysql://root:root@127.0.0.1:3306"),
            "mysql://127.0.0.1:3306"
        );
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

    /// Round-trip real contra el almacén del SO (requiere keyring de sesión:
    /// gnome-keyring/kwallet en Linux, Keychain en macOS, Credential Manager
    /// en Windows). Se ejecuta con `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore = "requiere el keyring del SO activo"]
    fn keyring_round_trip_guarda_y_recupera() {
        let url = "postgresql://keyring_test_user:pass_test_123@host:5432/db";
        let (user, pass) = save_credentials(url).expect("guardar").expect("credenciales");
        assert_eq!(user, "keyring_test_user");
        assert_eq!(pass, "pass_test_123");
        assert_eq!(user, "keyring_test_user");
        assert_eq!(pass, "pass_test_123");

        // La URL sin credenciales recupera las guardadas
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
