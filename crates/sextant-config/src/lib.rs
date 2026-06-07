//! Configuration loading (TOML, XDG paths, keymaps).

use std::path::Path;

use sextant_core::{Connection, CredentialStore, Driver, SextantError};

mod keymap;
mod parser;
mod paths;
mod theme;
mod validation;

pub use keymap::{RawBinding, load_keybindings, load_keybindings_from};
pub use theme::{Theme, load_theme, load_theme_from};

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
/// uppercased with spaces and hyphens replaced by underscores. This is the
/// fallback used when a connection has no `keyring_key` (see
/// [`password_from_keyring`]).
pub fn connection_password(name: &str) -> Option<String> {
    let key = name.to_uppercase().replace([' ', '-'], "_");
    let key = format!("SEXTANT_{key}_PASSWORD");
    std::env::var(key).ok()
}

/// The OS keyring service name under which sextant stores credentials.
const KEYRING_SERVICE: &str = "sextant";

/// Look up a stored password in the OS keyring by its `keyring_key`.
///
/// Returns `None` when no entry exists or the keyring is unavailable.
pub fn password_from_keyring(keyring_key: &str) -> Option<String> {
    keyring::Entry::new(KEYRING_SERVICE, keyring_key)
        .ok()?
        .get_password()
        .ok()
}

/// Store a password in the OS keyring under its `keyring_key`.
pub fn store_password_in_keyring(keyring_key: &str, password: &str) -> Result<(), SextantError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, keyring_key)
        .map_err(|e| SextantError::Config(format!("keyring: {e}")))?;
    entry
        .set_password(password)
        .map_err(|e| SextantError::Config(format!("keyring: {e}")))
}

/// The production [`CredentialStore`]: reads and writes the OS keyring.
///
/// A thin adapter over [`password_from_keyring`] / [`store_password_in_keyring`];
/// its real I/O is verified manually (it talks to the OS secret store).
pub struct KeyringStore;

impl CredentialStore for KeyringStore {
    fn get(&self, key: &str) -> Option<String> {
        password_from_keyring(key)
    }

    fn set(&self, key: &str, password: &str) -> Result<(), SextantError> {
        store_password_in_keyring(key, password)
    }
}

/// Outcome of resolving a connection password before connecting (§3.2).
#[derive(Debug, PartialEq, Eq)]
pub enum PasswordResolution {
    /// Connect with this password (`None` for SQLite or a passwordless driver).
    Connect(Option<String>),
    /// No secret available: prompt the user, then store it under `keyring_key`.
    Prompt { keyring_key: String },
}

/// Decide how to obtain the password for a connection, given the already-looked-up
/// keyring and environment values. Pure (no I/O) so the cascade *order* is testable.
///
/// Cascade (§3.2): keyring wins over env; if neither yields a secret, a TCP driver
/// that declares a `keyring_key` prompts the user, otherwise connect with no
/// password (SQLite, or a connection without a `keyring_key`).
pub fn resolve_password(
    driver: Driver,
    keyring_key: Option<&str>,
    from_keyring: Option<String>,
    from_env: Option<String>,
) -> PasswordResolution {
    if let Some(password) = from_keyring.or(from_env) {
        return PasswordResolution::Connect(Some(password));
    }
    match keyring_key {
        Some(key) if driver != Driver::Sqlite => PasswordResolution::Prompt {
            keyring_key: key.to_string(),
        },
        _ => PasswordResolution::Connect(None),
    }
}

/// Return the XDG-compliant configuration directory for sextant.
pub fn config_dir() -> std::path::PathBuf {
    paths::config_dir()
}

/// Return the directory where saved `.sql` queries live.
pub fn queries_dir() -> std::path::PathBuf {
    paths::queries_dir()
}

/// Return the directory where custom theme `.toml` files live.
pub fn themes_dir() -> std::path::PathBuf {
    paths::themes_dir()
}

/// Return the directory where editor swap files live.
pub fn swap_dir() -> std::path::PathBuf {
    paths::swap_dir()
}

/// Write an editor swap file with restrictive permissions (`0700` dir, `0600`
/// file), matching the security model for unencrypted query text on disk.
pub fn write_swap(path: &Path, content: &str) -> Result<(), SextantError> {
    write_secure(path, content)
}

/// Return the directory where exported result sets are written.
pub fn exports_dir() -> std::path::PathBuf {
    paths::exports_dir()
}

/// Resolve a query name to a `.sql` path inside [`queries_dir`].
pub fn query_path(name: &str) -> std::path::PathBuf {
    paths::query_path(name)
}

/// Return the path to the local application state database (`state.db`).
pub fn state_db_path() -> std::path::PathBuf {
    paths::state_db_path()
}

/// Write `content` to a `.sql` file, creating the parent directory if needed.
///
/// Enforces restrictive permissions on Unix per the security model: the
/// queries directory is `0700` and the `.sql` file is `0600` (query text on
/// disk is not encrypted; the threat model assumes local-machine access only).
pub fn write_query(path: &Path, content: &str) -> Result<(), SextantError> {
    write_secure(path, content)
}

/// Write an exported result set to `path`, creating the parent directory if
/// needed.
///
/// Enforces the same restrictive permissions as [`write_query`] (`0700`
/// directory, `0600` file): exported data on disk is not encrypted, and the
/// threat model assumes local-machine access only.
pub fn write_export(path: &Path, content: &str) -> Result<(), SextantError> {
    write_secure(path, content)
}

/// Write `content` to `path` with restrictive Unix permissions: the parent
/// directory is `0700` and the file itself is `0600`.
fn write_secure(path: &Path, content: &str) -> Result<(), SextantError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_mode(parent, 0o700);
    }
    std::fs::write(path, content)?;
    set_mode(path, 0o600);
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

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

    #[test]
    fn resolve_password_prefers_keyring_over_env() {
        // Both present: the keyring value wins (cascade order).
        let r = resolve_password(
            Driver::Postgres,
            Some("k"),
            Some("from-keyring".into()),
            Some("from-env".into()),
        );
        assert_eq!(r, PasswordResolution::Connect(Some("from-keyring".into())));
    }

    #[test]
    fn resolve_password_falls_back_to_env() {
        // No keyring_key, env set: env is the fallback.
        let r = resolve_password(Driver::Postgres, None, None, Some("from-env".into()));
        assert_eq!(r, PasswordResolution::Connect(Some("from-env".into())));
    }

    #[test]
    fn resolve_password_prompts_when_keyring_key_but_no_secret() {
        let r = resolve_password(Driver::Mysql, Some("k"), None, None);
        assert_eq!(
            r,
            PasswordResolution::Prompt {
                keyring_key: "k".into()
            }
        );
    }

    #[test]
    fn resolve_password_sqlite_never_prompts() {
        // SQLite has no credentials even if a (spurious) keyring_key is present.
        let r = resolve_password(Driver::Sqlite, Some("k"), None, None);
        assert_eq!(r, PasswordResolution::Connect(None));
    }

    #[test]
    fn resolve_password_tcp_without_keyring_key_connects_passwordless() {
        // A TCP driver without a keyring_key and no env: connect with no password
        // (no prompt — there is nowhere to store it).
        let r = resolve_password(Driver::Postgres, None, None, None);
        assert_eq!(r, PasswordResolution::Connect(None));
    }

    #[test]
    fn query_path_appends_sql_extension() {
        assert!(
            query_path("report")
                .to_string_lossy()
                .ends_with("report.sql")
        );
        assert!(
            query_path("report.sql")
                .to_string_lossy()
                .ends_with("report.sql")
        );
    }

    #[test]
    fn write_query_creates_file_with_restrictive_perms() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("q.sql");

        write_query(&path, "SELECT 1;").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "SELECT 1;");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "file should be 0600");
            let dir_mode = std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(dir_mode, 0o700, "queries dir should be 0700");
        }
    }
}
