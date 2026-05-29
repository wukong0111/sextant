//! Configuration loading (TOML, XDG paths, keymaps).

use std::path::Path;

use sextant_core::{Connection, SextantError};

mod parser;
mod paths;
mod validation;

/// Load connections from the default XDG configuration path
/// (`~/.config/sextant/connections.toml`).
///
/// Each connection is parsed and validated according to its driver.
pub fn load_connections() -> Result<Vec<Connection>, SextantError> {
    let path = paths::config_dir().join("connections.toml");
    load_connections_from(path)
}

/// Load connections from an arbitrary file path.
///
/// Useful for testing or for custom configuration locations.
pub fn load_connections_from(path: impl AsRef<Path>) -> Result<Vec<Connection>, SextantError> {
    let content = std::fs::read_to_string(path.as_ref())?;
    let mut connections = parser::parse_connections(&content)?;
    for conn in &mut connections {
        validation::validate(conn)?;
    }
    Ok(connections)
}

/// Read the password for a connection from the environment.
///
/// Looks up `SEXTANT_<NAME>_PASSWORD` where `<NAME>` is the connection name
/// uppercased with spaces and hyphens replaced by underscores.
///
/// This is the v0.1 fallback; keyring integration is planned for a later
/// phase.
pub fn connection_password(name: &str) -> Option<String> {
    let key = name.to_uppercase().replace([' ', '-'], "_");
    let key = format!("SEXTANT_{key}_PASSWORD");
    std::env::var(key).ok()
}

/// Return the XDG-compliant configuration directory for sextant.
pub fn config_dir() -> std::path::PathBuf {
    paths::config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::Driver;
    use std::io::Write;

    #[test]
    fn parse_valid_postgres_and_sqlite() {
        let toml = r#"
[[connection]]
name = "local-pg"
driver = "postgres"
host = "127.0.0.1"
port = 5432
user = "dan"
database = "scratch"
ssl_mode = "prefer"
keyring_key = "sextant:local-pg"

[[connection]]
name = "scratch"
driver = "sqlite"
path = "~/db/scratch.sqlite"
"#;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml.as_bytes()).unwrap();

        let conns = load_connections_from(file.path()).unwrap();
        assert_eq!(conns.len(), 2);

        assert_eq!(conns[0].name, "local-pg");
        assert_eq!(conns[0].driver, Driver::Postgres);
        assert_eq!(conns[0].host, Some("127.0.0.1".to_string()));
        assert_eq!(conns[0].port, Some(5432));
        assert_eq!(conns[0].user, Some("dan".to_string()));
        assert_eq!(conns[0].database, Some("scratch".to_string()));
        assert_eq!(conns[0].ssl_mode, Some("prefer".to_string()));
        assert_eq!(conns[0].keyring_key, Some("sextant:local-pg".to_string()));
        assert_eq!(conns[0].path, None);

        assert_eq!(conns[1].name, "scratch");
        assert_eq!(conns[1].driver, Driver::Sqlite);
        assert_eq!(conns[1].path, Some("~/db/scratch.sqlite".to_string()));
    }

    #[test]
    fn reject_postgres_missing_host() {
        let toml = r#"
[[connection]]
name = "bad-pg"
driver = "postgres"
port = 5432
user = "dan"
database = "scratch"
"#;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml.as_bytes()).unwrap();

        let err = load_connections_from(file.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("'host' is required"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn reject_sqlite_missing_path() {
        let toml = r#"
[[connection]]
name = "bad-sqlite"
driver = "sqlite"
"#;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml.as_bytes()).unwrap();

        let err = load_connections_from(file.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("'path' is required"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn reject_unsupported_driver() {
        let toml = r#"
[[connection]]
name = "weird"
driver = "oracle"
"#;

        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(toml.as_bytes()).unwrap();

        let err = load_connections_from(file.path()).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("unsupported driver"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn connection_password_from_env() {
        // Sequential assertions to avoid races with other env-modifying tests.
        unsafe { std::env::set_var("SEXTANT_LOCAL_PG_PASSWORD", "secret123") };
        assert_eq!(
            connection_password("local-pg"),
            Some("secret123".to_string())
        );
        unsafe { std::env::remove_var("SEXTANT_LOCAL_PG_PASSWORD") };
        assert_eq!(connection_password("local-pg"), None);

        unsafe { std::env::set_var("SEXTANT_PROD_DB_PASSWORD", "hunter2") };
        assert_eq!(connection_password("prod db"), Some("hunter2".to_string()));
        unsafe { std::env::remove_var("SEXTANT_PROD_DB_PASSWORD") };
    }
}
