use rusqlite::{Connection, Result};

const INITIAL_SCHEMA_VERSION: i64 = 1;

pub struct Repository {
    connection: Connection,
}

impl Repository {
    pub fn open_in_memory() -> Result<Self> {
        let connection = Connection::open_in_memory()?;
        let migration = format!("PRAGMA user_version = {INITIAL_SCHEMA_VERSION};");
        connection.execute_batch(&format!(
            "
                CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at_unix_ms INTEGER NOT NULL
                );
                {migration}
                "
        ))?;

        Ok(Self { connection })
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
    }
}

#[cfg(test)]
mod tests {
    use super::Repository;

    #[test]
    fn fresh_database_applies_the_first_schema_migration() {
        let repository = Repository::open_in_memory().expect("database should open");

        assert_eq!(
            repository
                .schema_version()
                .expect("schema version should be readable"),
            super::INITIAL_SCHEMA_VERSION
        );
    }
}
