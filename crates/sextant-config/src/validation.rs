//! Per-driver validation for loaded [`Connection`] values.

use sextant_core::{Connection, Driver, SextantError};

/// Validate that `conn` contains the required fields for its driver.
///
/// Returns `Ok(())` if valid, or `Err(SextantError::Config(...))` with a
/// descriptive message otherwise.
pub fn validate(conn: &Connection) -> Result<(), SextantError> {
    if conn.name.trim().is_empty() {
        return Err(SextantError::Config(
            "connection 'name' is required".to_string(),
        ));
    }

    match conn.driver {
        Driver::Postgres | Driver::Mysql => validate_tcp_connection(conn),
        Driver::Sqlite => validate_sqlite_connection(conn),
    }
}

fn validate_tcp_connection(conn: &Connection) -> Result<(), SextantError> {
    let driver_name = match conn.driver {
        Driver::Postgres => "postgres",
        Driver::Mysql => "mysql",
        Driver::Sqlite => unreachable!(),
    };

    if conn.host.is_none() {
        return Err(SextantError::Config(format!(
            "connection '{}' ({}): 'host' is required",
            conn.name, driver_name,
        )));
    }
    if conn.port.is_none() {
        return Err(SextantError::Config(format!(
            "connection '{}' ({}): 'port' is required",
            conn.name, driver_name,
        )));
    }
    if conn.user.is_none() {
        return Err(SextantError::Config(format!(
            "connection '{}' ({}): 'user' is required",
            conn.name, driver_name,
        )));
    }
    if conn.database.is_none() {
        return Err(SextantError::Config(format!(
            "connection '{}' ({}): 'database' is required",
            conn.name, driver_name,
        )));
    }

    Ok(())
}

fn validate_sqlite_connection(conn: &Connection) -> Result<(), SextantError> {
    if conn.path.is_none() {
        return Err(SextantError::Config(format!(
            "connection '{}' (sqlite): 'path' is required",
            conn.name,
        )));
    }

    Ok(())
}
