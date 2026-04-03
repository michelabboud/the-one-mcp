/// SQL constants for the tool catalog database tables.
pub const CREATE_TOOLS_TABLE: &str = "
CREATE TABLE IF NOT EXISTS tools (
    id              TEXT PRIMARY KEY NOT NULL,
    name            TEXT NOT NULL,
    type            TEXT NOT NULL DEFAULT 'cli',
    category        TEXT NOT NULL DEFAULT '[]',
    languages       TEXT NOT NULL DEFAULT '[]',
    description     TEXT NOT NULL DEFAULT '',
    when_to_use     TEXT NOT NULL DEFAULT '',
    what_it_finds   TEXT NOT NULL DEFAULT '',
    install_command TEXT NOT NULL DEFAULT '',
    run_command     TEXT NOT NULL DEFAULT '',
    risk_level      TEXT NOT NULL DEFAULT 'low',
    tags            TEXT NOT NULL DEFAULT '[]',
    github          TEXT NOT NULL DEFAULT '',
    trust_level     TEXT NOT NULL DEFAULT 'community',
    source          TEXT NOT NULL DEFAULT 'catalog',
    updated_at      INTEGER NOT NULL DEFAULT 0
);
";

pub const CREATE_TOOLS_FTS: &str = "
CREATE VIRTUAL TABLE IF NOT EXISTS tools_fts USING fts5(
    id,
    name,
    description,
    when_to_use,
    what_it_finds,
    tags,
    content='tools',
    content_rowid='rowid'
);
";

pub const CREATE_FTS_TRIGGER_INSERT: &str = "
CREATE TRIGGER IF NOT EXISTS tools_ai AFTER INSERT ON tools BEGIN
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END;
";

pub const CREATE_FTS_TRIGGER_UPDATE: &str = "
CREATE TRIGGER IF NOT EXISTS tools_au AFTER UPDATE ON tools BEGIN
    INSERT INTO tools_fts(tools_fts, rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES ('delete', old.rowid, old.id, old.name, old.description, old.when_to_use, old.what_it_finds, old.tags);
    INSERT INTO tools_fts(rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES (new.rowid, new.id, new.name, new.description, new.when_to_use, new.what_it_finds, new.tags);
END;
";

pub const CREATE_FTS_TRIGGER_DELETE: &str = "
CREATE TRIGGER IF NOT EXISTS tools_ad AFTER DELETE ON tools BEGIN
    INSERT INTO tools_fts(tools_fts, rowid, id, name, description, when_to_use, what_it_finds, tags)
    VALUES ('delete', old.rowid, old.id, old.name, old.description, old.when_to_use, old.what_it_finds, old.tags);
END;
";

pub const CREATE_SYSTEM_INVENTORY: &str = "
CREATE TABLE IF NOT EXISTS system_inventory (
    binary_name     TEXT PRIMARY KEY NOT NULL,
    path            TEXT NOT NULL DEFAULT '',
    version         TEXT NOT NULL DEFAULT '',
    last_checked    INTEGER NOT NULL DEFAULT 0
);
";

pub const CREATE_ENABLED_TOOLS: &str = "
CREATE TABLE IF NOT EXISTS enabled_tools (
    tool_id         TEXT NOT NULL,
    cli             TEXT NOT NULL,
    project_root    TEXT NOT NULL,
    enabled_at      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tool_id, cli, project_root)
);
";

pub const CREATE_CATALOG_META: &str = "
CREATE TABLE IF NOT EXISTS catalog_meta (
    key             TEXT PRIMARY KEY NOT NULL,
    value           TEXT NOT NULL DEFAULT ''
);
";
