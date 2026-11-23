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
             CREATE INDEX IF NOT EXISTS events_sandbox ON events (sandbox, seq);
             CREATE TABLE IF NOT EXISTS usage (
                 ts_ms INTEGER NOT NULL,
                 sandbox TEXT NOT NULL,
                 secret TEXT NOT NULL,
                 host TEXT NOT NULL,
                 model TEXT NOT NULL,
                 input INTEGER NOT NULL,
                 output INTEGER NOT NULL,
                 cache_read INTEGER NOT NULL,
                 cache_write INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS usage_time ON usage (ts_ms);
             CREATE TABLE IF NOT EXISTS traffic (
                 ts_ms INTEGER NOT NULL,
                 sandbox TEXT NOT NULL,
                 up INTEGER NOT NULL,
                 down INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS traffic_sandbox ON traffic (sandbox, ts_ms);",
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

    /// Note what a sandbox sent and received in the last stretch of time.
    pub fn record_traffic(&self, sandbox: &str, ts_ms: u64, up: u64, down: u64) -> Result<()> {
        self.db.lock().unwrap().execute(
            "INSERT INTO traffic (ts_ms, sandbox, up, down) VALUES (?1, ?2, ?3, ?4)",
            params![ts_ms as i64, sandbox, up as i64, down as i64],
        )?;
        Ok(())
    }

    /// A sandbox's traffic since `since_ms`: bytes sent and received per
    /// stretch of `bucket_ms`, oldest first, and the destinations that carried
    /// most of it, from the connection log.
    pub fn traffic(&self, sandbox: &str, since_ms: u64, bucket_ms: u64) -> Result<Value> {
        let db = self.db.lock().unwrap();
        let mut statement =
            db.prepare("SELECT (ts_ms / ?3) * ?3, SUM(up), SUM(down) FROM traffic WHERE sandbox = ?1 AND ts_ms >= ?2 GROUP BY 1 ORDER BY 1")?;
        let buckets: Vec<Value> = statement
            .query_map(params![sandbox, since_ms as i64, bucket_ms.max(1) as i64], |row| {
                Ok(serde_json::json!({ "t": row.get::<_, i64>(0)?, "up": row.get::<_, i64>(1)?, "down": row.get::<_, i64>(2)? }))
            })?
            .collect::<std::result::Result<_, _>>()?;
        // A connection's bytes are known when it closes.
        let mut statement = db.prepare(
            "SELECT COALESCE(json_extract(event, '$.host'), json_extract(event, '$.ip')), SUM(json_extract(event, '$.bytes_up')), SUM(json_extract(event, '$.bytes_down')), COUNT(*)
             FROM events WHERE sandbox = ?1 AND ts_ms >= ?2 AND json_extract(event, '$.phase') = 'close'
             GROUP BY 1 ORDER BY SUM(json_extract(event, '$.bytes_up')) + SUM(json_extract(event, '$.bytes_down')) DESC LIMIT 12",
        )?;
        let top: Vec<Value> = statement
            .query_map(params![sandbox, since_ms as i64], |row| {
                Ok(serde_json::json!({
                    "host": row.get::<_, Option<String>>(0)?, "up": row.get::<_, Option<i64>>(1)?.unwrap_or(0), "down": row.get::<_, Option<i64>>(2)?.unwrap_or(0), "connections": row.get::<_, i64>(3)?,
                }))
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(serde_json::json!({ "bucket_ms": bucket_ms, "buckets": buckets, "top": top }))
    }

    /// Note the tokens one request to a model API used.
    pub fn record_usage(&self, sandbox: &str, event: &NetEvent) -> Result<()> {
        let Some(usage) = &event.usage else { return Ok(()) };
        self.db.lock().unwrap().execute(
            "INSERT INTO usage (ts_ms, sandbox, secret, host, model, input, output, cache_read, cache_write) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.ts_ms as i64,
                sandbox,
                event.secret.as_deref().unwrap_or(""),
                event.host.as_deref().unwrap_or(""),
                usage.model,
                usage.input as i64,
                usage.output as i64,
                usage.cache_read as i64,
                usage.cache_write as i64
            ],
        )?;
        Ok(())
    }

    /// Token use since `since_ms`, summed per stretch of `bucket_ms`, sandbox,
    /// secret and model, oldest first. A bucket's `t` is its start.
    pub fn usage(&self, since_ms: u64, bucket_ms: u64) -> Result<Vec<Value>> {
        let db = self.db.lock().unwrap();
        let mut statement = db.prepare(
            "SELECT (ts_ms / ?2) * ?2, sandbox, secret, model, SUM(input), SUM(output), SUM(cache_read), SUM(cache_write), COUNT(*)
             FROM usage WHERE ts_ms >= ?1 GROUP BY 1, 2, 3, 4 ORDER BY 1",
        )?;
        let rows = statement.query_map(params![since_ms as i64, bucket_ms.max(1) as i64], |row| {
            Ok(serde_json::json!({
                "t": row.get::<_, i64>(0)?, "sandbox": row.get::<_, String>(1)?, "secret": row.get::<_, String>(2)?, "model": row.get::<_, String>(3)?,
                "input": row.get::<_, i64>(4)?, "output": row.get::<_, i64>(5)?, "cache_read": row.get::<_, i64>(6)?, "cache_write": row.get::<_, i64>(7)?,
                "requests": row.get::<_, i64>(8)?,
            }))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
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
        self.db.lock().unwrap().execute("DELETE FROM traffic WHERE sandbox = ?1", params![sandbox])?;
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
