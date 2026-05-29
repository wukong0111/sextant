//! TOML parsing for `connections.toml`.

use serde::Deserialize;
use sextant_core::{Connection, Driver, SextantError};

#[derive(Debug, Deserialize)]
struct ConnectionsFile {
    connection: Vec<RawConnection>,
}

#[derive(Debug, Deserialize)]
struct RawConnection {
    name: String,
    driver: String,
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    database: Option<String>,
    #[serde(default)]
    ssl_mode: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    keyring_key: Option<String>,
}

/// Parse a TOML string into a list of [`Connection`] values.
///
/// Performs basic deserialization but **not** per-driver validation.
/// Call [`crate::validation::validate`] on each result if needed.
pub fn parse_connections(toml_str: &str) -> Result<Vec<Connection>, SextantError> {
    let file: ConnectionsFile = toml::from_str(toml_str)
        .map_err(|e| SextantError::Config(format!("failed to parse connections.toml: {e}")))?;

    file.connection.into_iter().map(raw_to_connection).collect()
}

fn raw_to_connection(raw: RawConnection) -> Result<Connection, SextantError> {
    let driver = match raw.driver.as_str() {
        "postgres" => Driver::Postgres,
        "mysql" => Driver::Mysql,
        "sqlite" => Driver::Sqlite,
        other => return Err(SextantError::Config(format!("unsupported driver: {other}"))),
    };

    Ok(Connection {
        name: raw.name,
        driver,
        host: raw.host,
        port: raw.port,
        user: raw.user,
        database: raw.database,
        ssl_mode: raw.ssl_mode,
        path: raw.path,
        keyring_key: raw.keyring_key,
    })
}
