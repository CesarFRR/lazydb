//! Detección de servidores SQL locales (filosofía lazy: autodiscover).
//!
//! Escanea `127.0.0.1` en los puertos típicos donde arrancan servicios de
//! bases de datos y devuelve cadenas de conexión listas para el resolver.
//! El escaneo es E/S bloqueante con timeout corto; el caller lo mueve a un
//! thread para no bloquear el event loop de la UI.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

/// (puerto, label) de los servicios SQL más comunes que atcan en localhost.
/// Orden estable: se muestran en `SourcePanel` en este orden.
pub const KNOWN_SERVERS: &[(u16, &str, &str)] = &[
    (3306, "mysql", "MariaDB/MySQL"),
    (5432, "postgres", "PostgreSQL"),
    (27017, "mongodb", "MongoDB"),
    (6379, "redis", "Redis"),
    (1433, "sqlserver", "MSSQL"),
    (33060, "mysqlx", "MySQL X-Protocol"),
];

/// Detecta qué servicios tienes escuchando en `127.0.0.1`.
/// Devuelve VERSIONES de URL sin credenciales ni BD, p. ej.:
/// `["mysql://127.0.0.1:3306", "postgres://127.0.0.1:5432"]`
pub fn scan_local_servers(timeout: Duration) -> Vec<String> {
    let mut found = Vec::new();
    for (port, scheme, _label) in KNOWN_SERVERS {
        if port_is_open(*port, timeout) {
            found.push(format!("{scheme}://127.0.0.1:{port}"));
        }
    }
    found
}

fn port_is_open(port: u16, timeout: Duration) -> bool {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    TcpStream::connect_timeout(&addr, timeout).is_ok()
}

#[cfg(test)]
fn port_is_open_test(port: u16, timeout: Duration) -> bool {
    port_is_open(port, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaneo_no_panica_con_timeout_corto() {
        // timeout de 1ms: ciclo rápido, nunca panica, formato de URL válido
        let found = scan_local_servers(Duration::from_millis(1));
        for url in &found {
            let Some((scheme, _)) = url.split_once("://") else {
                panic!("URL sin esquema: {url}");
            };
            assert!(
                KNOWN_SERVERS.iter().any(|(_, s, _)| *s == scheme),
                "esquema desconocido {scheme} en {url}"
            );
        }
    }

    #[test]
    fn puerto_abierto_produce_url_correcta() {
        // Si 3306 (nuestro MariaDB local) está escuchando, la URL debe
        // existir. En CI sin servicio el test es vacuamente válido.
        if port_is_open_test(3306, Duration::from_millis(50)) {
            assert!(
                scan_local_servers(Duration::from_millis(50))
                    .iter()
                    .any(|u| u == "mysql://127.0.0.1:3306")
            );
        }
    }

    #[test]
    fn puerto_cerrado_no_detecta() {
        // Puerto alto raramente servido → seguro de probar sin red
        assert!(!port_is_open_test(59999, Duration::from_millis(10)));
    }
}
