use crate::model::*;
use rusqlite::{Connection, params};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub type WorkspacePool = Arc<Mutex<BTreeMap<String, Vec<PathBuf>>>>;

pub type AppResult<T> = Result<T, AppError>;
#[derive(Debug)]
pub struct AppError(pub u16, pub String);
impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.1)
    }
}
impl std::error::Error for AppError {}
impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        Self(500, e.to_string())
    }
}
impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self(400, e.to_string())
    }
}
impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self(500, e.to_string())
    }
}
pub fn bad(message: impl Into<String>) -> AppError {
    AppError(400, message.into())
}
pub fn conflict(message: impl Into<String>) -> AppError {
    AppError(409, message.into())
}

pub struct Store {
    connection: Mutex<Connection>,
    pool: WorkspacePool,
    pool_id: String,
}
impl Store {
    pub fn open(path: impl AsRef<Path>) -> AppResult<Self> {
        Self::open_shared(path, WorkspacePool::default())
    }
    pub fn open_shared(path: impl AsRef<Path>, pool: WorkspacePool) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            return Err(AppError(
                500,
                format!("Database schema {version} is newer than this version of Relay"),
            ));
        }
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
          CREATE TABLE IF NOT EXISTS meta (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL, settings TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS agents (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS attempts (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), data TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS attempt_task ON attempts(task_id);
          CREATE TABLE IF NOT EXISTS artifacts (id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id), data TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS artifact_task ON artifacts(task_id);
          CREATE TABLE IF NOT EXISTS events (seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT UNIQUE NOT NULL, task_id TEXT, data TEXT NOT NULL);
          CREATE INDEX IF NOT EXISTS event_task ON events(task_id);
          CREATE TABLE IF NOT EXISTS planners (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          CREATE TABLE IF NOT EXISTS planner_notifications (id TEXT PRIMARY KEY, data TEXT NOT NULL);
          PRAGMA user_version=1;")?;
        conn.execute(
            "INSERT OR IGNORE INTO meta VALUES(1,0,?1)",
            [serde_json::to_string(&Settings::default())?],
        )?;
        let store = Self {
            connection: Mutex::new(conn),
            pool,
            pool_id: id(),
        };
        store.mutate(crate::engine::migrate_workflow)?;
        Ok(store)
    }
    fn load(conn: &Connection) -> AppResult<Snapshot> {
        fn rows<T: serde::de::DeserializeOwned>(
            conn: &Connection,
            table: &str,
        ) -> AppResult<Vec<T>> {
            let order = if table == "events" { "seq" } else { "rowid" };
            let mut stmt = conn.prepare(&format!("SELECT data FROM {table} ORDER BY {order}"))?;
            let strings = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            strings
                .into_iter()
                .map(|v| serde_json::from_str(&v).map_err(Into::into))
                .collect()
        }
        let (revision, settings): (i64, String) =
            conn.query_row("SELECT revision,settings FROM meta WHERE id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
        Ok(Snapshot {
            occupied_workspaces: vec![],
            tasks: rows(conn, "tasks")?,
            agents: rows(conn, "agents")?,
            attempts: rows(conn, "attempts")?,
            artifacts: rows(conn, "artifacts")?,
            events: rows(conn, "events")?,
            planners: rows(conn, "planners")?,
            planner_notifications: rows(conn, "planner_notifications")?,
            settings: serde_json::from_str(&settings)?,
            revision: revision as u64,
        })
    }
    pub fn read(&self) -> AppResult<Snapshot> {
        let pool = self
            .pool
            .lock()
            .map_err(|_| AppError(500, "Workspace lock poisoned".into()))?;
        let conn = self
            .connection
            .lock()
            .map_err(|_| AppError(500, "Database lock poisoned".into()))?;
        let mut state = Self::load(&conn)?;
        state.occupied_workspaces = self.foreign_workspaces(&pool);
        Ok(state)
    }
    pub fn mutate<T>(&self, change: impl FnOnce(&mut Snapshot) -> AppResult<T>) -> AppResult<T> {
        // Admission and reservation updates are atomic across projects. Execution stays concurrent.
        let mut pool = self
            .pool
            .lock()
            .map_err(|_| AppError(500, "Workspace lock poisoned".into()))?;
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| AppError(500, "Database lock poisoned".into()))?;
        let tx = conn.transaction()?;
        let mut state = Self::load(&tx)?;
        state.occupied_workspaces = self.foreign_workspaces(&pool);
        let before = serde_json::to_string(&state)?;
        let result = change(&mut state)?;
        if serde_json::to_string(&state)? == before {
            pool.insert(self.pool_id.clone(), Self::reservations(&state));
            return Ok(result);
        }
        state.revision += 1;
        for task in &state.tasks {
            tx.execute("INSERT INTO tasks VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data", params![task.id,serde_json::to_string(task)?])?;
        }
        for agent in &state.agents {
            tx.execute("INSERT INTO agents VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data", params![agent.id,serde_json::to_string(agent)?])?;
        }
        for a in &state.attempts {
            tx.execute("INSERT INTO attempts VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data",params![a.id,a.task_id,serde_json::to_string(a)?])?;
        }
        for a in &state.artifacts {
            tx.execute("INSERT INTO artifacts VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data",params![a.id,a.task_id,serde_json::to_string(a)?])?;
        }
        for e in &state.events {
            tx.execute(
                "INSERT OR IGNORE INTO events(id,task_id,data) VALUES(?1,?2,?3)",
                params![e.id, e.task_id, serde_json::to_string(e)?],
            )?;
        }
        for planner in &state.planners {
            tx.execute("INSERT INTO planners VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data", params![planner.id,serde_json::to_string(planner)?])?;
        }
        for notification in &state.planner_notifications {
            tx.execute("INSERT INTO planner_notifications VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data WHERE data!=excluded.data", params![notification.id,serde_json::to_string(notification)?])?;
        }
        tx.execute(
            "UPDATE meta SET revision=?1,settings=?2 WHERE id=1",
            params![
                state.revision as i64,
                serde_json::to_string(&state.settings)?
            ],
        )?;
        tx.commit()?;
        pool.insert(self.pool_id.clone(), Self::reservations(&state));
        Ok(result)
    }
    fn foreign_workspaces(&self, pool: &BTreeMap<String, Vec<PathBuf>>) -> Vec<PathBuf> {
        pool.iter()
            .filter(|(id, _)| *id != &self.pool_id)
            .flat_map(|(_, paths)| paths.clone())
            .collect()
    }
    fn reservations(state: &Snapshot) -> Vec<PathBuf> {
        state
            .attempts
            .iter()
            .filter(|a| ["running", "cancelling", "lost"].contains(&a.status.as_str()))
            .filter_map(|a| a.input["task"]["workspace"].as_str())
            .chain(
                state.agents.iter().filter_map(|a| {
                    Some(a.worker.as_ref()?.assignment.as_ref()?.workspace.as_str())
                }),
            )
            .filter(|p| !p.is_empty())
            .filter_map(|p| std::fs::canonicalize(p).ok())
            .collect()
    }
    pub fn backup(&self, path: &Path) -> AppResult<()> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| AppError(500, "Database lock poisoned".into()))?;
        let mut target = Connection::open(path)?;
        rusqlite::backup::Backup::new(&conn, &mut target)?.run_to_completion(
            100,
            std::time::Duration::from_millis(10),
            None,
        )?;
        Ok(())
    }
    pub fn reset_workspace(&self, confirm_name: &str, backup_path: &Path) -> AppResult<u64> {
        // Hold the same admission and database locks as mutate: no scheduler or
        // external claim may start between the activity check, backup and reset.
        let mut pool = self
            .pool
            .lock()
            .map_err(|_| AppError(500, "Workspace lock poisoned".into()))?;
        let mut conn = self
            .connection
            .lock()
            .map_err(|_| AppError(500, "Database lock poisoned".into()))?;
        let state = Self::load(&conn)?;
        if confirm_name != state.settings.project_name {
            return Err(conflict("Workspace name confirmation does not match"));
        }
        if state
            .attempts
            .iter()
            .any(|a| matches!(a.status.as_str(), "running" | "cancelling" | "lost"))
            || state.tasks.iter().any(|t| t.current_attempt.is_some())
            || state
                .agents
                .iter()
                .any(|a| a.worker.as_ref().is_some_and(|w| w.assignment.is_some()))
        {
            return Err(conflict(
                "Stop and acknowledge all active work and assignments before resetting this workspace",
            ));
        }
        if backup_path.exists() {
            return Err(conflict("Reset backup path already exists"));
        }
        if let Some(parent) = backup_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut backup = Connection::open(backup_path)?;
        rusqlite::backup::Backup::new(&conn, &mut backup)?.run_to_completion(
            100,
            std::time::Duration::from_millis(10),
            None,
        )?;
        drop(backup);
        let settings = Settings {
            project_name: state.settings.project_name,
            ..Settings::default()
        };
        let revision = state.revision + 1;
        let tx = conn.transaction()?;
        tx.execute_batch("DELETE FROM artifacts; DELETE FROM attempts; DELETE FROM events; DELETE FROM tasks; DELETE FROM agents; DELETE FROM planner_notifications; DELETE FROM planners;")?;
        tx.execute(
            "UPDATE meta SET revision=?1,settings=?2 WHERE id=1",
            params![revision as i64, serde_json::to_string(&settings)?],
        )?;
        tx.commit()?;
        pool.insert(self.pool_id.clone(), Vec::new());
        Ok(revision)
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.remove(&self.pool_id);
        }
    }
}
