use crate::model::{Component, Confidence, SensorReading};
use crate::sampler::Snapshot;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use std::time::Duration as StdDuration;

const DEFAULT_RETENTION_DAYS: i64 = 7;
const RETENTION_ENV: &str = "POWERWATCH_RETENTION_DAYS";

#[derive(Debug)]
pub enum StorageError {
    OpenFailed(String),
    QueryFailed(String),
}

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: &std::path::Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path).map_err(open_error)?;
        configure_file_connection(&conn)?;
        let storage = Self { conn };
        storage.create_schema()?;
        Ok(storage)
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let conn =
            Connection::open_in_memory().map_err(|e| StorageError::OpenFailed(e.to_string()))?;
        let storage = Self { conn };
        storage.create_schema()?;
        Ok(storage)
    }

    fn create_schema(&self) -> Result<(), StorageError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS readings (
                    component   TEXT NOT NULL,
                    watts       REAL NOT NULL,
                    confidence  TEXT NOT NULL,
                    ts          TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_readings_ts ON readings(ts);",
            )
            .map_err(query_error)?;
        Ok(())
    }

    pub fn insert_reading(&self, reading: &SensorReading) -> Result<(), StorageError> {
        insert_row(&self.conn, reading)
    }

    pub fn insert_snapshot(&self, snapshot: &Snapshot) -> Result<usize, StorageError> {
        let transaction = self.conn.unchecked_transaction().map_err(query_error)?;

        let mut inserted = 0;
        for reading in snapshot.readings() {
            insert_row(&transaction, reading)?;
            inserted += 1;
        }
        if let Some(total) = snapshot.total() {
            insert_row(&transaction, &total)?;
            inserted += 1;
        }

        transaction.commit().map_err(query_error)?;
        Ok(inserted)
    }

    pub fn delete_older_than(&self, cutoff: DateTime<Utc>) -> Result<usize, StorageError> {
        self.conn
            .execute("DELETE FROM readings WHERE ts < ?1", [cutoff.to_rfc3339()])
            .map_err(query_error)
    }

    pub fn prune_expired(&self) -> Result<usize, StorageError> {
        match retention_from_env() {
            Some(retention) => self.delete_older_than(Utc::now() - retention),
            None => Ok(0),
        }
    }

    pub fn readings_since(&self, since: DateTime<Utc>) -> Result<Vec<SensorReading>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT component, watts, confidence, ts FROM readings WHERE ts >= ?1 ORDER BY ts ASC")
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        let rows = stmt
            .query_map([since.to_rfc3339()], |row| {
                let component_json: String = row.get(0)?;
                let watts: f64 = row.get(1)?;
                let confidence_text: String = row.get(2)?;
                let ts_text: String = row.get(3)?;
                Ok((component_json, watts, confidence_text, ts_text))
            })
            .map_err(|e| StorageError::QueryFailed(e.to_string()))?;

        let mut readings = Vec::new();
        for row in rows {
            let (component_json, watts, confidence_text, ts_text) =
                row.map_err(|e| StorageError::QueryFailed(e.to_string()))?;

            let component: Component = serde_json::from_str(&component_json)
                .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
            let confidence = if confidence_text == "measured" {
                Confidence::Measured
            } else {
                Confidence::Estimated
            };
            let timestamp = DateTime::parse_from_rfc3339(&ts_text)
                .map_err(|e| StorageError::QueryFailed(e.to_string()))?
                .with_timezone(&Utc);

            readings.push(SensorReading {
                component,
                watts,
                confidence,
                timestamp,
            });
        }

        Ok(readings)
    }
}

fn open_error(error: rusqlite::Error) -> StorageError {
    StorageError::OpenFailed(error.to_string())
}

fn query_error(error: rusqlite::Error) -> StorageError {
    StorageError::QueryFailed(error.to_string())
}

fn configure_file_connection(conn: &Connection) -> Result<(), StorageError> {
    conn.busy_timeout(StdDuration::from_secs(5))
        .map_err(open_error)?;
    conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })
    .map_err(open_error)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(open_error)?;
    Ok(())
}

fn insert_row(conn: &Connection, reading: &SensorReading) -> Result<(), StorageError> {
    let component_json = serde_json::to_string(&reading.component)
        .map_err(|e| StorageError::QueryFailed(e.to_string()))?;
    let confidence = match reading.confidence {
        Confidence::Measured => "measured",
        Confidence::Estimated => "estimated",
    };

    conn.prepare_cached(
        "INSERT INTO readings (component, watts, confidence, ts) VALUES (?1, ?2, ?3, ?4)",
    )
    .and_then(|mut statement| {
        statement.execute(rusqlite::params![
            component_json,
            reading.watts,
            confidence,
            reading.timestamp.to_rfc3339()
        ])
    })
    .map_err(query_error)?;

    Ok(())
}

pub fn retention_from_env() -> Option<chrono::Duration> {
    retention_from(std::env::var(RETENTION_ENV).ok().as_deref())
}

fn retention_from(value: Option<&str>) -> Option<chrono::Duration> {
    let default = Some(chrono::Duration::days(DEFAULT_RETENTION_DAYS));
    match value.map(str::trim) {
        None | Some("") => default,
        Some(text) => match text.parse::<u32>() {
            Ok(0) => None,
            Ok(days) => Some(chrono::Duration::days(i64::from(days))),
            Err(_) => default,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn reading(
        component: Component,
        watts: f64,
        confidence: Confidence,
        timestamp: DateTime<Utc>,
    ) -> SensorReading {
        SensorReading {
            component,
            watts,
            confidence,
            timestamp,
        }
    }

    #[test]
    fn a_stored_reading_can_be_read_back() {
        let storage = Storage::open_in_memory().unwrap();
        let now = Utc::now();
        storage
            .insert_reading(&reading(Component::Cpu, 12.5, Confidence::Measured, now))
            .unwrap();

        let readings = storage.readings_since(now - Duration::seconds(1)).unwrap();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].component, Component::Cpu);
        assert_eq!(readings[0].watts, 12.5);
        assert_eq!(readings[0].confidence, Confidence::Measured);
    }

    #[test]
    fn a_reading_with_data_carrying_component_round_trips_correctly() {
        let storage = Storage::open_in_memory().unwrap();
        let now = Utc::now();
        storage
            .insert_reading(&reading(
                Component::Disk("sda".to_string()),
                3.0,
                Confidence::Estimated,
                now,
            ))
            .unwrap();

        let readings = storage.readings_since(now - Duration::seconds(1)).unwrap();

        assert_eq!(readings[0].component, Component::Disk("sda".to_string()));
    }

    #[test]
    fn readings_before_the_cutoff_are_excluded() {
        let storage = Storage::open_in_memory().unwrap();
        let old = Utc::now() - Duration::hours(2);
        let recent = Utc::now();

        storage
            .insert_reading(&reading(Component::Cpu, 5.0, Confidence::Measured, old))
            .unwrap();
        storage
            .insert_reading(&reading(Component::Cpu, 8.0, Confidence::Measured, recent))
            .unwrap();

        let readings = storage
            .readings_since(recent - Duration::minutes(1))
            .unwrap();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].watts, 8.0);
    }

    #[test]
    fn readings_come_back_oldest_first() {
        let storage = Storage::open_in_memory().unwrap();
        let t0 = Utc::now() - Duration::seconds(10);
        let t1 = Utc::now();

        storage
            .insert_reading(&reading(Component::Cpu, 8.0, Confidence::Measured, t1))
            .unwrap();
        storage
            .insert_reading(&reading(Component::Cpu, 5.0, Confidence::Measured, t0))
            .unwrap();

        let readings = storage.readings_since(t0 - Duration::seconds(1)).unwrap();

        assert_eq!(readings[0].watts, 5.0);
        assert_eq!(readings[1].watts, 8.0);
    }

    fn file_storage() -> (tempfile::TempDir, std::path::PathBuf, Storage) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let storage = Storage::open(&path).unwrap();
        (dir, path, storage)
    }

    fn index_names(conn: &Connection) -> Vec<String> {
        let mut statement = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'index'")
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|name| name.unwrap())
            .collect()
    }

    fn snapshot_of(readings: &[(&str, f64)]) -> Snapshot {
        Snapshot {
            timestamp: Utc::now(),
            results: readings
                .iter()
                .map(|(name, watts)| {
                    (
                        name.to_string(),
                        Ok(reading(
                            Component::Cpu,
                            *watts,
                            Confidence::Measured,
                            Utc::now(),
                        )),
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn a_file_database_uses_write_ahead_logging() {
        let (_dir, path, _storage) = file_storage();

        let conn = Connection::open(path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();

        assert_eq!(mode, "wal");
    }

    #[test]
    fn the_timestamp_column_is_indexed() {
        let storage = Storage::open_in_memory().unwrap();

        let plan: String = storage
            .conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT component, watts, confidence, ts FROM readings WHERE ts >= ?1 ORDER BY ts ASC",
                ["2026-01-01T00:00:00+00:00"],
                |row| row.get(3),
            )
            .unwrap();

        assert!(plan.contains("idx_readings_ts"), "unexpected plan: {plan}");
    }

    #[test]
    fn an_existing_database_without_the_index_gets_one_and_keeps_its_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute(
                "CREATE TABLE readings (component TEXT NOT NULL, watts REAL NOT NULL, confidence TEXT NOT NULL, ts TEXT NOT NULL)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO readings VALUES ('\"Cpu\"', 5.0, 'measured', '2026-01-01T00:00:00+00:00')",
                [],
            )
            .unwrap();
        }

        let storage = Storage::open(&path).unwrap();

        let since = DateTime::parse_from_rfc3339("2025-12-31T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(storage.readings_since(since).unwrap().len(), 1);
        assert!(index_names(&storage.conn).contains(&"idx_readings_ts".to_string()));
    }

    #[test]
    fn a_snapshot_is_stored_together_with_its_total() {
        let storage = Storage::open_in_memory().unwrap();
        let snapshot = snapshot_of(&[("cpu", 10.0), ("ram", 5.0)]);

        let inserted = storage.insert_snapshot(&snapshot).unwrap();

        assert_eq!(inserted, 3);
        let since = Utc::now() - chrono::Duration::minutes(1);
        let stored = storage.readings_since(since).unwrap();
        assert_eq!(stored.len(), 3);
        assert!(stored
            .iter()
            .any(|r| r.component == Component::Total && r.watts == 15.0));
    }

    #[test]
    fn a_snapshot_with_nothing_to_report_inserts_nothing() {
        let storage = Storage::open_in_memory().unwrap();
        let snapshot = Snapshot {
            timestamp: Utc::now(),
            results: Vec::new(),
        };

        assert_eq!(storage.insert_snapshot(&snapshot).unwrap(), 0);
    }

    #[test]
    fn deleting_older_than_removes_only_the_old_readings() {
        let storage = Storage::open_in_memory().unwrap();
        let now = Utc::now();
        storage
            .insert_reading(&reading(
                Component::Cpu,
                1.0,
                Confidence::Measured,
                now - chrono::Duration::days(10),
            ))
            .unwrap();
        storage
            .insert_reading(&reading(Component::Cpu, 2.0, Confidence::Measured, now))
            .unwrap();

        let removed = storage
            .delete_older_than(now - chrono::Duration::days(7))
            .unwrap();

        assert_eq!(removed, 1);
        let left = storage
            .readings_since(now - chrono::Duration::days(30))
            .unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].watts, 2.0);
    }

    #[test]
    fn several_connections_can_write_to_the_same_file_at_once() {
        let (_dir, path, _storage) = file_storage();

        let writers: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || {
                    let storage = Storage::open(&path).unwrap();
                    for _ in 0..50 {
                        storage
                            .insert_snapshot(&snapshot_of(&[("cpu", 10.0)]))
                            .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }

        let storage = Storage::open(&path).unwrap();
        let since = Utc::now() - chrono::Duration::minutes(1);
        assert_eq!(storage.readings_since(since).unwrap().len(), 4 * 50 * 2);
    }

    #[test]
    fn retention_defaults_to_seven_days() {
        assert_eq!(retention_from(None), Some(chrono::Duration::days(7)));
        assert_eq!(retention_from(Some("")), Some(chrono::Duration::days(7)));
    }

    #[test]
    fn retention_can_be_set_in_days() {
        assert_eq!(retention_from(Some("30")), Some(chrono::Duration::days(30)));
        assert_eq!(retention_from(Some(" 3 ")), Some(chrono::Duration::days(3)));
    }

    #[test]
    fn zero_days_keeps_everything() {
        assert_eq!(retention_from(Some("0")), None);
    }

    #[test]
    fn an_invalid_retention_falls_back_to_the_default() {
        for text in ["abc", "-3", "1.5"] {
            assert_eq!(retention_from(Some(text)), Some(chrono::Duration::days(7)));
        }
    }
}
