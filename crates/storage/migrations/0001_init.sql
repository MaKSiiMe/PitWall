-- PitWall schema v1. Timestamps are Unix milliseconds. JSON columns hold serde_json documents.
-- Plain SQL on purpose: it ports to Postgres with minor changes (AUTOINCREMENT → GENERATED).

CREATE TABLE players (
    login       TEXT PRIMARY KEY,
    nickname    TEXT NOT NULL,
    account_id  TEXT NOT NULL DEFAULT '',
    zone        TEXT NOT NULL DEFAULT '',
    lang        TEXT NOT NULL DEFAULT 'en',
    first_seen  INTEGER NOT NULL,
    last_seen   INTEGER NOT NULL,
    finishes    INTEGER NOT NULL DEFAULT 0,
    rating      REAL NOT NULL DEFAULT 1000,
    rated_cups  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE maps (
    uid         TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    author      TEXT NOT NULL DEFAULT '',
    file        TEXT NOT NULL DEFAULT '',
    source      TEXT NOT NULL DEFAULT 'server',
    source_id   TEXT NOT NULL DEFAULT '',
    author_time INTEGER NOT NULL DEFAULT 0,
    added_at    INTEGER NOT NULL
);

CREATE TABLE runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    map_uid     TEXT NOT NULL,
    login       TEXT NOT NULL,
    time        INTEGER NOT NULL,
    checkpoints TEXT NOT NULL,
    mode        TEXT NOT NULL DEFAULT '',
    cup_id      TEXT,
    phase_id    TEXT,
    at          INTEGER NOT NULL
);
CREATE INDEX runs_map_time ON runs (map_uid, time);
CREATE INDEX runs_login ON runs (login);

CREATE TABLE cup_runs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    cup_id      TEXT NOT NULL,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    finished_at INTEGER
);

CREATE TABLE phase_results (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    cup_run_id  INTEGER NOT NULL REFERENCES cup_runs (id),
    phase_index INTEGER NOT NULL,
    phase_id    TEXT NOT NULL,
    standings   TEXT NOT NULL,
    qualified   TEXT NOT NULL,
    finished_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    at          INTEGER NOT NULL,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    details     TEXT NOT NULL,
    success     INTEGER NOT NULL
);
CREATE INDEX audit_at ON audit_log (at);

CREATE TABLE teams (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    tag         TEXT NOT NULL DEFAULT '',
    color       TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE TABLE team_members (
    team_id     TEXT NOT NULL REFERENCES teams (id) ON DELETE CASCADE,
    login       TEXT NOT NULL,
    PRIMARY KEY (team_id, login)
);

CREATE TABLE registrations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    cup_id          TEXT NOT NULL,
    participant_id  TEXT NOT NULL,
    name            TEXT NOT NULL,
    players         TEXT NOT NULL,
    via             TEXT NOT NULL,
    status          TEXT NOT NULL,
    contact         TEXT NOT NULL DEFAULT '',
    created_at      INTEGER NOT NULL,
    UNIQUE (cup_id, participant_id)
);
