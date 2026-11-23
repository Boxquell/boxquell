//! The network log: every event of every sandbox, kept in SQLite.

use anyhow::Result;
use boxquell_net::NetEvent;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;

pub struct EventStore {
    db: Mutex<Connection>,
}

#[derive(Debug, Default, Clone, Deserialize)]
pub struct EventFilter {
    pub sandbox: Option<String>,
    /// Only events with a sequence number above this one.
    pub after: Option<i64>,
    #[serde(default)]
    pub blocked_only: bool,
    pub limit: Option<u32>,
}

impl EventStore {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(db: Connection) -> Result<Self> {
        db.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS events (
                 seq INTEGER PRIMARY KEY AUTOINCREMENT,
                 sandbox TEXT NOT NULL,
                 ts_ms INTEGER NOT NULL,
                 allowed INTEGER NOT NULL,
                 event TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS events_sandbox ON events (sandbox, seq);",
        )?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// Store an event and return it the way [`query`](Self::query) would.
    pub fn insert(&self, sandbox: &str, event: &NetEvent) -> Result<Value> {
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO events (sandbox, ts_ms, allowed, event) VALUES (?1, ?2, ?3, ?4)",
            params![sandbox, event.ts_ms as i64, event.allowed, serde_json::to_string(event)?],
        )?;
        Ok(row(db.last_insert_rowid(), sandbox, serde_json::to_value(event)?))
    }

    /// The newest matching events, oldest first.
    pub fn query(&self, filter: &EventFilter) -> Result<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut statement = db.prepare(
            "SELECT seq, sandbox, event FROM events
             WHERE (?1 IS NULL OR sandbox = ?1) AND seq > ?2 AND (?3 = 0 OR allowed = 0)
             ORDER BY seq DESC LIMIT ?4",
        )?;
        let rows = statement
            .query_map(params![filter.sandbox, filter.after.unwrap_or(0), filter.blocked_only, filter.limit.unwrap_or(200).min(5000)], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })?;
        let mut events = Vec::new();
        for item in rows {
            let (seq, sandbox, event) = item?;
            events.push(row(seq, &sandbox, serde_json::from_str(&event)?));
        }
        events.reverse();
        Ok(events)
    }

    pub fn delete_sandbox(&self, sandbox: &str) -> Result<()> {
        self.db.lock().unwrap().execute("DELETE FROM events WHERE sandbox = ?1", params![sandbox])?;
        Ok(())
    }
}

fn row(seq: i64, sandbox: &str, mut event: Value) -> Value {
    event["seq"] = seq.into();
    event["sandbox"] = sandbox.into();
    event
}

#[cfg(test)]
mod tests {
    use super::*;
    use boxquell_net::EventKind;

    #[test]
    fn filters_and_order() {
        let store = EventStore::in_memory().unwrap();
        store.insert("a", &NetEvent::new(EventKind::Dns, true, "allow x").host("x")).unwrap();
        store.insert("a", &NetEvent::new(EventKind::Tcp, false, "no matching rule").host("y")).unwrap();
        let third = store.insert("b", &NetEvent::new(EventKind::Tcp, true, "allow z").host("z")).unwrap();
        assert_eq!(third["seq"], 3);
        assert_eq!(third["sandbox"], "b");

        let all = store.query(&EventFilter::default()).unwrap();
        assert_eq!(all.iter().map(|e| e["seq"].as_i64().unwrap()).collect::<Vec<_>>(), [1, 2, 3]);
        let blocked = store.query(&EventFilter { sandbox: Some("a".into()), blocked_only: true, ..Default::default() }).unwrap();
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0]["host"], "y");
        assert_eq!(store.query(&EventFilter { after: Some(2), ..Default::default() }).unwrap().len(), 1);
        assert_eq!(store.query(&EventFilter { limit: Some(1), ..Default::default() }).unwrap()[0]["seq"], 3);

        store.delete_sandbox("a").unwrap();
        assert_eq!(store.query(&EventFilter::default()).unwrap().len(), 1);
    }
}
