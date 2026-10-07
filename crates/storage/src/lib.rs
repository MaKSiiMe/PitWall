//! Persistence (SQLite through sqlx; plain SQL, portable to Postgres).
//!
//! Queries are checked at runtime (no database needed to compile). Migrations are embedded and run
//! when the database is opened.

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

/// Storage errors.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("invalid stored JSON: {0}")]
    Json(#[from] serde_json::Error),
}

type Result<T> = std::result::Result<T, StorageError>;

/// A player seen on the server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerRecord {
    pub login: String,
    pub nickname: String,
    pub account_id: String,
    pub zone: String,
    pub lang: String,
}

/// Player profile with statistics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerProfile {
    pub login: String,
    pub nickname: String,
    pub zone: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub finishes: i64,
    pub rating: f64,
    pub rated_cups: i64,
}

/// A finished run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub map_uid: String,
    pub login: String,
    pub time: i32,
    pub checkpoints: Vec<i32>,
    pub mode: String,
    pub cup_id: Option<String>,
    pub phase_id: Option<String>,
    pub at: i64,
}

/// Best time of a player on a map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BestTime {
    pub login: String,
    pub name: String,
    pub time: i32,
}

/// A map known to the controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapRecord {
    pub uid: String,
    pub name: String,
    pub author: String,
    pub file: String,
    /// `server`, `tmx`, `nadeo`, `upload`.
    pub source: String,
    pub source_id: String,
    pub author_time: i32,
}

/// An audit log line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub action: String,
    pub details: serde_json::Value,
    pub success: bool,
}

/// A played (or running) cup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CupRun {
    pub id: i64,
    pub cup_id: String,
    pub name: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Stored result of a phase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhaseRecord {
    pub phase_index: i64,
    pub phase_id: String,
    pub standings: serde_json::Value,
    pub qualified: Vec<String>,
    pub finished_at: i64,
}

/// A registration request (web, Discord, in game).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub id: i64,
    pub cup_id: String,
    pub participant_id: String,
    pub name: String,
    pub players: Vec<String>,
    pub via: String,
    /// `pending`, `accepted`, `refused`.
    pub status: String,
    pub contact: String,
    pub created_at: i64,
}

/// A team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub tag: String,
    pub color: String,
    pub players: Vec<String>,
}

/// Database handle. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Storage {
    pool: SqlitePool,
}

/// Elo constant for cup ratings.
const ELO_K: f64 = 32.0;

impl Storage {
    /// Opens (creating if needed) a SQLite database file and runs the migrations.
    pub async fn open(path: &std::path::Path) -> Result<Storage> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;
        Self::from_pool(pool).await
    }

    /// In-memory database (tests).
    pub async fn in_memory() -> Result<Storage> {
        let options = SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true);
        // A single connection: every connection to `:memory:` is a different database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await?;
        Self::from_pool(pool).await
    }

    async fn from_pool(pool: SqlitePool) -> Result<Storage> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Storage { pool })
    }

    // --- players -------------------------------------------------------------------------------

    pub async fn upsert_player(&self, player: &PlayerRecord, now: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO players (login, nickname, account_id, zone, lang, first_seen, last_seen)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (login) DO UPDATE SET
                nickname = excluded.nickname,
                account_id = CASE WHEN excluded.account_id = '' THEN players.account_id ELSE excluded.account_id END,
                zone = CASE WHEN excluded.zone = '' THEN players.zone ELSE excluded.zone END,
                lang = excluded.lang,
                last_seen = excluded.last_seen",
        )
        .bind(&player.login)
        .bind(&player.nickname)
        .bind(&player.account_id)
        .bind(&player.zone)
        .bind(&player.lang)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn player(&self, login: &str) -> Result<Option<PlayerProfile>> {
        let row = sqlx::query(
            "SELECT login, nickname, zone, first_seen, last_seen, finishes, rating, rated_cups
             FROM players WHERE login = ?",
        )
        .bind(login)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| PlayerProfile {
            login: r.get("login"),
            nickname: r.get("nickname"),
            zone: r.get("zone"),
            first_seen: r.get("first_seen"),
            last_seen: r.get("last_seen"),
            finishes: r.get("finishes"),
            rating: r.get("rating"),
            rated_cups: r.get("rated_cups"),
        }))
    }

    /// Players ordered by rating.
    pub async fn leaderboard(&self, limit: i64) -> Result<Vec<PlayerProfile>> {
        let rows = sqlx::query(
            "SELECT login, nickname, zone, first_seen, last_seen, finishes, rating, rated_cups
             FROM players WHERE rated_cups > 0 ORDER BY rating DESC, login LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| PlayerProfile {
                login: r.get("login"),
                nickname: r.get("nickname"),
                zone: r.get("zone"),
                first_seen: r.get("first_seen"),
                last_seen: r.get("last_seen"),
                finishes: r.get("finishes"),
                rating: r.get("rating"),
                rated_cups: r.get("rated_cups"),
            })
            .collect())
    }

    /// Updates ratings from a final ranking (`(login, rank)`, best first) with pairwise Elo.
    pub async fn update_ratings(&self, ranking: &[(String, u32)]) -> Result<()> {
        if ranking.len() < 2 {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let mut ratings = Vec::with_capacity(ranking.len());
        for (login, _) in ranking {
            let rating: Option<f64> =
                sqlx::query_scalar("SELECT rating FROM players WHERE login = ?")
                    .bind(login)
                    .fetch_optional(&mut *tx)
                    .await?;
            ratings.push(rating.unwrap_or(1000.0));
        }
        let n = ranking.len();
        let opponents = (n - 1) as f64;
        for (i, (login, rank)) in ranking.iter().enumerate() {
            let mut delta = 0.0;
            for (j, (_, other_rank)) in ranking.iter().enumerate() {
                if i == j {
                    continue;
                }
                let expected = 1.0 / (1.0 + 10f64.powf((ratings[j] - ratings[i]) / 400.0));
                let score = match rank.cmp(other_rank) {
                    std::cmp::Ordering::Less => 1.0,
                    std::cmp::Ordering::Equal => 0.5,
                    std::cmp::Ordering::Greater => 0.0,
                };
                delta += ELO_K * (score - expected);
            }
            let new_rating = ratings[i] + delta / opponents;
            sqlx::query(
                "INSERT INTO players (login, nickname, first_seen, last_seen, rating, rated_cups)
                 VALUES (?, ?, 0, 0, ?, 1)
                 ON CONFLICT (login) DO UPDATE SET rating = excluded.rating, rated_cups = players.rated_cups + 1",
            )
            .bind(login)
            .bind(login)
            .bind(new_rating)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // --- maps and runs ------------------------------------------------------------------------------

    pub async fn upsert_map(&self, map: &MapRecord, now: i64) -> Result<()> {
        sqlx::query(
            "INSERT INTO maps (uid, name, author, file, source, source_id, author_time, added_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (uid) DO UPDATE SET
                name = excluded.name, author = excluded.author, file = excluded.file,
                author_time = excluded.author_time,
                source = CASE WHEN excluded.source = 'server' THEN maps.source ELSE excluded.source END,
                source_id = CASE WHEN excluded.source_id = '' THEN maps.source_id ELSE excluded.source_id END",
        )
        .bind(&map.uid)
        .bind(&map.name)
        .bind(&map.author)
        .bind(&map.file)
        .bind(&map.source)
        .bind(&map.source_id)
        .bind(map.author_time)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn maps(&self) -> Result<Vec<MapRecord>> {
        let rows = sqlx::query("SELECT uid, name, author, file, source, source_id, author_time FROM maps ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| MapRecord {
                uid: r.get("uid"),
                name: r.get("name"),
                author: r.get("author"),
                file: r.get("file"),
                source: r.get("source"),
                source_id: r.get("source_id"),
                author_time: r.get("author_time"),
            })
            .collect())
    }

    /// Stores a finished run and counts the finish.
    pub async fn record_run(&self, run: &RunRecord) -> Result<i64> {
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO runs (map_uid, login, time, checkpoints, mode, cup_id, phase_id, at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&run.map_uid)
        .bind(&run.login)
        .bind(run.time)
        .bind(serde_json::to_string(&run.checkpoints)?)
        .bind(&run.mode)
        .bind(&run.cup_id)
        .bind(&run.phase_id)
        .bind(run.at)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query("UPDATE players SET finishes = finishes + 1 WHERE login = ?")
            .bind(&run.login)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Best time per player on a map.
    pub async fn best_times(&self, map_uid: &str, limit: i64) -> Result<Vec<BestTime>> {
        let rows = sqlx::query(
            "SELECT r.login AS login, COALESCE(p.nickname, r.login) AS name, MIN(r.time) AS time
             FROM runs r LEFT JOIN players p ON p.login = r.login
             WHERE r.map_uid = ?
             GROUP BY r.login
             ORDER BY time ASC, r.login
             LIMIT ?",
        )
        .bind(map_uid)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| BestTime {
                login: r.get("login"),
                name: r.get("name"),
                time: r.get("time"),
            })
            .collect())
    }

    /// Checkpoints of a player's best run on a map.
    pub async fn best_run_checkpoints(
        &self,
        map_uid: &str,
        login: &str,
    ) -> Result<Option<Vec<i32>>> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT checkpoints FROM runs WHERE map_uid = ? AND login = ? ORDER BY time ASC LIMIT 1",
        )
        .bind(map_uid)
        .bind(login)
        .fetch_optional(&self.pool)
        .await?;
        raw.map(|r| serde_json::from_str(&r))
            .transpose()
            .map_err(Into::into)
    }

    /// Every run of a cup (for exports).
    pub async fn cup_runs(&self, cup_id: &str) -> Result<Vec<RunRecord>> {
        let rows = sqlx::query(
            "SELECT map_uid, login, time, checkpoints, mode, cup_id, phase_id, at FROM runs
             WHERE cup_id = ? ORDER BY at",
        )
        .bind(cup_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let checkpoints: String = r.get("checkpoints");
                Ok(RunRecord {
                    map_uid: r.get("map_uid"),
                    login: r.get("login"),
                    time: r.get("time"),
                    checkpoints: serde_json::from_str(&checkpoints)?,
                    mode: r.get("mode"),
                    cup_id: r.get("cup_id"),
                    phase_id: r.get("phase_id"),
                    at: r.get("at"),
                })
            })
            .collect()
    }

    // --- cups ------------------------------------------------------------------------------------------

    /// Starts recording a cup; returns its run id.
    pub async fn start_cup_run(&self, cup_id: &str, name: &str, now: i64) -> Result<i64> {
        Ok(sqlx::query(
            "INSERT INTO cup_runs (cup_id, name, status, started_at) VALUES (?, ?, 'running', ?)",
        )
        .bind(cup_id)
        .bind(name)
        .bind(now)
        .execute(&self.pool)
        .await?
        .last_insert_rowid())
    }

    /// Latest unfinished run of a cup.
    pub async fn current_cup_run(&self, cup_id: &str) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM cup_runs WHERE cup_id = ? AND finished_at IS NULL ORDER BY id DESC LIMIT 1",
        )
        .bind(cup_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn finish_cup_run(&self, run_id: i64, status: &str, now: i64) -> Result<()> {
        sqlx::query("UPDATE cup_runs SET status = ?, finished_at = ? WHERE id = ?")
            .bind(status)
            .bind(now)
            .bind(run_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn record_phase(
        &self,
        run_id: i64,
        phase_index: i64,
        phase_id: &str,
        standings: &serde_json::Value,
        qualified: &[String],
        finished_at: i64,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO phase_results (cup_run_id, phase_index, phase_id, standings, qualified, finished_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(run_id)
        .bind(phase_index)
        .bind(phase_id)
        .bind(serde_json::to_string(standings)?)
        .bind(serde_json::to_string(qualified)?)
        .bind(finished_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Played cups, most recent first.
    pub async fn cup_history(&self, limit: i64) -> Result<Vec<CupRun>> {
        let rows = sqlx::query(
            "SELECT id, cup_id, name, status, started_at, finished_at FROM cup_runs ORDER BY id DESC LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| CupRun {
                id: r.get("id"),
                cup_id: r.get("cup_id"),
                name: r.get("name"),
                status: r.get("status"),
                started_at: r.get("started_at"),
                finished_at: r.get("finished_at"),
            })
            .collect())
    }

    pub async fn cup_run(&self, run_id: i64) -> Result<Option<CupRun>> {
        let row = sqlx::query(
            "SELECT id, cup_id, name, status, started_at, finished_at FROM cup_runs WHERE id = ?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| CupRun {
            id: r.get("id"),
            cup_id: r.get("cup_id"),
            name: r.get("name"),
            status: r.get("status"),
            started_at: r.get("started_at"),
            finished_at: r.get("finished_at"),
        }))
    }

    pub async fn phase_results(&self, run_id: i64) -> Result<Vec<PhaseRecord>> {
        let rows = sqlx::query(
            "SELECT phase_index, phase_id, standings, qualified, finished_at FROM phase_results
             WHERE cup_run_id = ? ORDER BY phase_index",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let standings: String = r.get("standings");
                let qualified: String = r.get("qualified");
                Ok(PhaseRecord {
                    phase_index: r.get("phase_index"),
                    phase_id: r.get("phase_id"),
                    standings: serde_json::from_str(&standings)?,
                    qualified: serde_json::from_str(&qualified)?,
                    finished_at: r.get("finished_at"),
                })
            })
            .collect()
    }

    // --- audit -----------------------------------------------------------------------------------------

    pub async fn audit(
        &self,
        at: i64,
        actor: &str,
        action: &str,
        details: &serde_json::Value,
        success: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO audit_log (at, actor, action, details, success) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(at)
        .bind(actor)
        .bind(action)
        .bind(serde_json::to_string(details)?)
        .bind(success)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn audit_log(&self, limit: i64) -> Result<Vec<AuditRecord>> {
        let rows = sqlx::query("SELECT id, at, actor, action, details, success FROM audit_log ORDER BY id DESC LIMIT ?")
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter()
            .map(|r| {
                let details: String = r.get("details");
                Ok(AuditRecord {
                    id: r.get("id"),
                    at: r.get("at"),
                    actor: r.get("actor"),
                    action: r.get("action"),
                    details: serde_json::from_str(&details)?,
                    success: r.get("success"),
                })
            })
            .collect()
    }

    // --- registrations and teams -----------------------------------------------------------------

    /// Adds a registration request; `Ok(None)` if this participant already asked for this cup.
    pub async fn add_registration(&self, reg: &Registration) -> Result<Option<i64>> {
        let result = sqlx::query(
            "INSERT INTO registrations (cup_id, participant_id, name, players, via, status, contact, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (cup_id, participant_id) DO NOTHING",
        )
        .bind(&reg.cup_id)
        .bind(&reg.participant_id)
        .bind(&reg.name)
        .bind(serde_json::to_string(&reg.players)?)
        .bind(&reg.via)
        .bind(&reg.status)
        .bind(&reg.contact)
        .bind(reg.created_at)
        .execute(&self.pool)
        .await?;
        Ok((result.rows_affected() > 0).then(|| result.last_insert_rowid()))
    }

    pub async fn set_registration_status(&self, id: i64, status: &str) -> Result<()> {
        sqlx::query("UPDATE registrations SET status = ? WHERE id = ?")
            .bind(status)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn registrations(&self, cup_id: &str) -> Result<Vec<Registration>> {
        let rows = sqlx::query(
            "SELECT id, cup_id, participant_id, name, players, via, status, contact, created_at
             FROM registrations WHERE cup_id = ? ORDER BY id",
        )
        .bind(cup_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|r| {
                let players: String = r.get("players");
                Ok(Registration {
                    id: r.get("id"),
                    cup_id: r.get("cup_id"),
                    participant_id: r.get("participant_id"),
                    name: r.get("name"),
                    players: serde_json::from_str(&players)?,
                    via: r.get("via"),
                    status: r.get("status"),
                    contact: r.get("contact"),
                    created_at: r.get("created_at"),
                })
            })
            .collect()
    }

    pub async fn registration(&self, id: i64) -> Result<Option<Registration>> {
        let row = sqlx::query(
            "SELECT id, cup_id, participant_id, name, players, via, status, contact, created_at
             FROM registrations WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|r| {
            let players: String = r.get("players");
            Ok(Registration {
                id: r.get("id"),
                cup_id: r.get("cup_id"),
                participant_id: r.get("participant_id"),
                name: r.get("name"),
                players: serde_json::from_str(&players)?,
                via: r.get("via"),
                status: r.get("status"),
                contact: r.get("contact"),
                created_at: r.get("created_at"),
            })
        })
        .transpose()
    }

    pub async fn save_team(&self, team: &Team, now: i64) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO teams (id, name, tag, color, created_at) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, tag = excluded.tag, color = excluded.color",
        )
        .bind(&team.id)
        .bind(&team.name)
        .bind(&team.tag)
        .bind(&team.color)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM team_members WHERE team_id = ?")
            .bind(&team.id)
            .execute(&mut *tx)
            .await?;
        for login in &team.players {
            sqlx::query("INSERT INTO team_members (team_id, login) VALUES (?, ?)")
                .bind(&team.id)
                .bind(login)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn teams(&self) -> Result<Vec<Team>> {
        let rows = sqlx::query("SELECT id, name, tag, color FROM teams ORDER BY name")
            .fetch_all(&self.pool)
            .await?;
        let mut teams = Vec::with_capacity(rows.len());
        for r in rows {
            let id: String = r.get("id");
            let players: Vec<String> = sqlx::query_scalar(
                "SELECT login FROM team_members WHERE team_id = ? ORDER BY login",
            )
            .bind(&id)
            .fetch_all(&self.pool)
            .await?;
            teams.push(Team {
                id,
                name: r.get("name"),
                tag: r.get("tag"),
                color: r.get("color"),
                players,
            });
        }
        Ok(teams)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(login: &str) -> PlayerRecord {
        PlayerRecord {
            login: login.into(),
            nickname: login.to_uppercase(),
            account_id: String::new(),
            zone: "World|Europe|France".into(),
            lang: "fr".into(),
        }
    }

    fn run(login: &str, time: i32) -> RunRecord {
        RunRecord {
            map_uid: "map1".into(),
            login: login.into(),
            time,
            checkpoints: vec![time / 2, time],
            mode: "timeattack".into(),
            cup_id: Some("mini".into()),
            phase_id: Some("quali".into()),
            at: 1,
        }
    }

    #[tokio::test]
    async fn players_runs_and_best_times() {
        let db = Storage::in_memory().await.unwrap();
        db.upsert_player(&player("a"), 10).await.unwrap();
        db.upsert_player(&player("b"), 10).await.unwrap();
        db.upsert_player(&player("a"), 20).await.unwrap();
        for (login, time) in [("a", 50_000), ("a", 48_000), ("b", 49_000)] {
            db.record_run(&run(login, time)).await.unwrap();
        }
        let best = db.best_times("map1", 10).await.unwrap();
        assert_eq!(
            best,
            vec![
                BestTime {
                    login: "a".into(),
                    name: "A".into(),
                    time: 48_000
                },
                BestTime {
                    login: "b".into(),
                    name: "B".into(),
                    time: 49_000
                },
            ]
        );
        assert_eq!(
            db.best_run_checkpoints("map1", "a").await.unwrap(),
            Some(vec![24_000, 48_000])
        );
        let profile = db.player("a").await.unwrap().unwrap();
        assert_eq!(
            (profile.first_seen, profile.last_seen, profile.finishes),
            (10, 20, 2)
        );
        assert_eq!(db.cup_runs("mini").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn cups_phases_and_ratings() {
        let db = Storage::in_memory().await.unwrap();
        let run_id = db.start_cup_run("mini", "Mini cup", 1).await.unwrap();
        assert_eq!(db.current_cup_run("mini").await.unwrap(), Some(run_id));
        let standings = serde_json::json!([{"id": "a", "rank": 1}, {"id": "b", "rank": 2}]);
        db.record_phase(run_id, 0, "quali", &standings, &["a".into()], 5)
            .await
            .unwrap();
        db.finish_cup_run(run_id, "finished", 9).await.unwrap();
        assert_eq!(db.current_cup_run("mini").await.unwrap(), None);
        let phases = db.phase_results(run_id).await.unwrap();
        assert_eq!(phases[0].qualified, ["a"]);
        assert_eq!(db.cup_history(10).await.unwrap()[0].status, "finished");

        db.update_ratings(&[("a".into(), 1), ("b".into(), 2), ("c".into(), 3)])
            .await
            .unwrap();
        let board = db.leaderboard(10).await.unwrap();
        assert_eq!(board[0].login, "a");
        assert!(board[0].rating > 1000.0 && board[2].rating < 1000.0);
        let total: f64 = board.iter().map(|p| p.rating).sum();
        assert!((total - 3000.0).abs() < 1e-6, "Elo is zero-sum");
    }

    #[tokio::test]
    async fn audit_registrations_teams_maps() {
        let db = Storage::in_memory().await.unwrap();
        db.audit(
            1,
            "player:boss",
            "kick",
            &serde_json::json!({"login": "x"}),
            true,
        )
        .await
        .unwrap();
        let log = db.audit_log(10).await.unwrap();
        assert_eq!(log[0].action, "kick");
        assert_eq!(log[0].details["login"], "x");

        let reg = Registration {
            id: 0,
            cup_id: "league".into(),
            participant_id: "bw".into(),
            name: "Blue Wings".into(),
            players: vec!["p1".into(), "p2".into()],
            via: "web".into(),
            status: "pending".into(),
            contact: "discord#1".into(),
            created_at: 3,
        };
        let id = db.add_registration(&reg).await.unwrap().unwrap();
        assert_eq!(
            db.add_registration(&reg).await.unwrap(),
            None,
            "duplicates are ignored"
        );
        db.set_registration_status(id, "accepted").await.unwrap();
        assert_eq!(
            db.registration(id).await.unwrap().unwrap().status,
            "accepted"
        );
        assert_eq!(db.registrations("league").await.unwrap().len(), 1);

        let team = Team {
            id: "bw".into(),
            name: "Blue Wings".into(),
            tag: "BW".into(),
            color: "0af".into(),
            players: vec!["p2".into(), "p1".into()],
        };
        db.save_team(&team, 1).await.unwrap();
        assert_eq!(db.teams().await.unwrap()[0].players, ["p1", "p2"]);

        let map = MapRecord {
            uid: "u".into(),
            name: "A01".into(),
            author: "nadeo".into(),
            file: "Campaigns/A01.Map.Gbx".into(),
            source: "tmx".into(),
            source_id: "123".into(),
            author_time: 30_000,
        };
        db.upsert_map(&map, 1).await.unwrap();
        // A later sighting from the server playlist keeps the TMX origin.
        db.upsert_map(
            &MapRecord {
                source: "server".into(),
                source_id: String::new(),
                ..map.clone()
            },
            2,
        )
        .await
        .unwrap();
        assert_eq!(db.maps().await.unwrap()[0].source_id, "123");
    }
}
