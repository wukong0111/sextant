//! Build sqlx connection URLs from [`Connection`] values.

use sextant_core::{Connection, Driver, SextantError};

/// Build a sqlx-compatible connection URL for the given connection.
///
/// For PostgreSQL: `postgres://user:pass@host:port/database?sslmode=...`
/// For SQLite: `sqlite:///absolute/path` or `sqlite::memory:`
pub fn build_connection_url(
    conn: &Connection,
    password: Option<&str>,
) -> Result<String, SextantError> {
    match conn.driver {
        Driver::Postgres => build_postgres_url(conn, password),
        Driver::Sqlite => build_sqlite_url(conn),
        Driver::Mysql => Err(SextantError::Config(
            "MySQL is not supported in v0.1".to_string(),
        )),
    }
}

fn build_postgres_url(conn: &Connection, password: Option<&str>) -> Result<String, SextantError> {
    let host = conn.host.as_ref().ok_or_else(|| {
        SextantError::Config(format!("connection '{}' missing host", conn.name))
    })?;
    let port = conn.port.ok_or_else(|| {
        SextantError::Config(format!("connection '{}' missing port", conn.name))
    })?;
    let user = conn.user.as_ref().ok_or_else(|| {
        SextantError::Config(format!("connection '{}' missing user", conn.name))
    })?;
    let database = conn.database.as_ref().ok_or_else(|| {
        SextantError::Config(format!("connection '{}' missing database", conn.name))
    })?;

    let mut url = format!("postgres://{user}");
    if let Some(pass) = password {
        url.push(':');
        url.push_str(pass);
    }
    url.push_str("@");
    url.push_str(host);
    url.push(':');
    url.push_str(&port.to_string());
    url.push('/');
    url.push_str(database);

    if let Some(ssl) = &conn.ssl_mode {
        url.push_str("?sslmode=");
        url.push_str(ssl);
    }

    Ok(url)
}

fn build_sqlite_url(conn: &Connection) -> Result<String, SextantError> {
    let path = conn.path.as_ref().ok_or_else(|| {
        SextantError::Config(format!("connection '{}' missing path", conn.name))
    })?;

    if path == ":memory:" {
        return Ok("sqlite::memory:".to_string());
    }

    let expanded = if path.starts_with("~/") {
        let home = dirs::home_dir().ok_or_else(|| {
            SextantError::Config("could not resolve home directory".to_string())
        })?;
        home.join(&path[2..])
    } else {
        std::path::PathBuf::from(path)
    };

    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(SextantError::Io)?
            .join(expanded)
    };

    Ok(format!("sqlite://{}", absolute.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sextant_core::Driver;

    fn pg_conn() -> Connection {
        Connection {
            name: "local-pg".to_string(),
            driver: Driver::Postgres,
            host: Some("127.0.0.1".to_string()),
            port: Some(5432),
            user: Some("dan".to_string()),
            database: Some("scratch".to_string()),
            ssl_mode: Some("prefer".to_string()),
            path: None,
            keyring_key: None,
        }
    }

    fn sqlite_conn(path: &str) -> Connection {
        Connection {
            name: "scratch".to_string(),
            driver: Driver::Sqlite,
            host: None,
            port: None,
            user: None,
            database: None,
            ssl_mode: None,
            path: Some(path.to_string()),
            keyring_key: None,
        }
    }

    #[test]
    fn postgres_url_with_password() {
        let url = build_connection_url(&pg_conn(), Some("secret")).unwrap();
        assert_eq!(url, "postgres://dan:secret@127.0.0.1:5432/scratch?sslmode=prefer");
    }

    #[test]
    fn postgres_url_without_password() {
        let mut conn = pg_conn();
        conn.ssl_mode = None;
        let url = build_connection_url(&conn, None).unwrap();
        assert_eq!(url, "postgres://dan@127.0.0.1:5432/scratch");
    }

    #[test]
    fn sqlite_memory_url() {
        let url = build_connection_url(&sqlite_conn(":memory:"), None).unwrap();
        assert_eq!(url, "sqlite::memory:");
    }

    #[test]
    fn sqlite_file_url_expands_tilde() {
        let url = build_connection_url(&sqlite_conn("~/db/scratch.sqlite"), None).unwrap();
        assert!(url.starts_with("sqlite://"));
        assert!(url.ends_with("db/scratch.sqlite"));
        assert!(!url.contains("~"));
    }

    #[test]
    fn mysql_not_supported() {
        let conn = Connection {
            name: "bad".to_string(),
            driver: Driver::Mysql,
            host: Some("localhost".to_string()),
            port: Some(3306),
            user: Some("root".to_string()),
            database: Some("db".to_string()),
            ssl_mode: None,
            path: None,
            keyring_key: None,
        };
        let err = build_connection_url(&conn, None).unwrap_err();
        assert!(format!("{err}").contains("MySQL is not supported"));
    }
}
