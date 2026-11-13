//! SQLite persistence for the mining bank.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// Lightweight row data for listing entries (no image blobs).
#[derive(Clone, Debug, PartialEq)]
pub struct BankEntryMeta {
    pub id: i64,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
    pub term: String,
    pub reading: String,
    pub definition_text: String,
    /// Serialized dictionary entry (see `bank::entry_from_json`).
    pub definition_json: String,
    pub sentence: String,
    /// Character range `[start, end)` of the mined word within `sentence`.
    pub word_range: (usize, usize),
    pub has_screenshot: bool,
    /// Optional single tag (e.g. the game the word was mined from).
    pub tag: Option<String>,
    /// Optional Twitch clip URL if a clip was captured for this word.
    pub twitch_clip_url: Option<String>,
}

impl BankEntryMeta {
    /// Lowercased copy of `self.tag` for allocation-free substring filtering.
    pub fn compute_tag_lower(&self) -> Option<String> {
        self.tag.as_deref().map(str::to_lowercase)
    }
}

/// A fully-prepared entry ready to be inserted.
#[derive(Clone, Debug, Default)]
pub struct NewBankEntry {
    pub request_id: Option<i64>,
    pub created_at: i64,
    pub term: String,
    pub reading: String,
    pub definition_text: String,
    pub definition_json: String,
    pub sentence: String,
    pub word_range: (usize, usize),
    pub source_text: String,
    /// Encoded (JPEG) screenshot, capped to 720p.
    pub screenshot: Option<Vec<u8>>,
    /// Encoded (JPEG) thumbnail for list previews.
    pub thumbnail: Option<Vec<u8>>,
    pub tag: Option<String>,
    pub twitch_clip_url: Option<String>,
}

/// A custom name dictionary entry.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomNameEntry {
    pub id: i64,
    /// Unix timestamp in milliseconds.
    pub created_at: i64,
    pub kanji: String,
    pub furigana: String,
    pub source_tag: Option<String>,
    pub notes: Option<String>,
}

/// A new custom name ready to be inserted.
#[derive(Clone, Debug, Default)]
pub struct NewCustomName {
    pub created_at: i64,
    pub kanji: String,
    pub furigana: String,
    pub source_tag: Option<String>,
    pub notes: Option<String>,
}

pub struct BankDatabase {
    conn: Connection,
}

impl BankDatabase {
    /// Opens (creating if necessary) the bank database at `path`.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let existed_on_disk = path.exists();
        let conn = Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        if existed_on_disk && Self::has_column(&conn, "mined_words", "screenshot")? {
            let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let mut bak = path.as_os_str().to_os_string();
            bak.push(".pre_media_split.bak");
            let bak_path = Path::new(&bak);
            if !bak_path.exists() {
                std::fs::copy(path, bak_path).with_context(|| {
                    format!("backing up {} to {}", path.display(), bak_path.display())
                })?;
            }
        }
        Self::from_conn(conn)
    }

    /// Opens a transient in-memory database (used by tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let sql = format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1");
        Ok(conn.prepare(&sql)?.exists(params![column])?)
    }

    fn migrate_legacy_inline_media(conn: &Connection) -> Result<()> {
        if !Self::has_column(conn, "mined_words", "tag")? {
            conn.execute_batch("ALTER TABLE mined_words ADD COLUMN tag TEXT;")?;
        }
        if !Self::has_column(conn, "mined_words", "twitch_clip_url")? {
            conn.execute_batch("ALTER TABLE mined_words ADD COLUMN twitch_clip_url TEXT;")?;
        }
        if !Self::has_column(conn, "mined_words", "request_id")? {
            conn.execute_batch("ALTER TABLE mined_words ADD COLUMN request_id INTEGER;")?;
        }

        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let mig_res = (|| -> Result<()> {
            let prev_seq: Option<i64> = conn
                .query_row(
                    "SELECT seq FROM sqlite_sequence WHERE name = 'mined_words'",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .unwrap_or(None);

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS mined_word_media (
                     word_id    INTEGER PRIMARY KEY,
                     screenshot BLOB,
                     thumbnail  BLOB
                 );
                 INSERT OR REPLACE INTO mined_word_media (word_id, screenshot, thumbnail)
                 SELECT id, screenshot, thumbnail
                 FROM mined_words
                 WHERE screenshot IS NOT NULL OR thumbnail IS NOT NULL;
                 DROP TABLE IF EXISTS mined_words_v2;
                 CREATE TABLE mined_words_v2 (
                     id               INTEGER PRIMARY KEY AUTOINCREMENT,
                     created_at       INTEGER NOT NULL,
                     term             TEXT NOT NULL,
                     reading          TEXT NOT NULL,
                     definition_text  TEXT NOT NULL,
                     definition_json  TEXT NOT NULL,
                     sentence         TEXT NOT NULL,
                     word_start       INTEGER NOT NULL,
                     word_end         INTEGER NOT NULL,
                     source_text      TEXT NOT NULL,
                     has_screenshot   INTEGER NOT NULL DEFAULT 0,
                     tag              TEXT,
                     twitch_clip_url  TEXT,
                     request_id       INTEGER,
                     sequence         INTEGER
                 );
                 INSERT INTO mined_words_v2 (
                     id, created_at, term, reading, definition_text, definition_json,
                     sentence, word_start, word_end, source_text, has_screenshot,
                     tag, twitch_clip_url, request_id, sequence
                 )
                 SELECT
                     id, created_at, term, reading, definition_text, definition_json,
                     sentence, word_start, word_end, source_text,
                     CASE WHEN screenshot IS NOT NULL THEN 1 ELSE 0 END,
                     tag, twitch_clip_url, request_id, NULL
                 FROM mined_words;
                 DROP TABLE mined_words;
                 ALTER TABLE mined_words_v2 RENAME TO mined_words;
                 CREATE INDEX IF NOT EXISTS idx_mined_words_created_id
                     ON mined_words(created_at DESC, id DESC);
                 CREATE INDEX IF NOT EXISTS idx_mined_words_tag
                     ON mined_words(tag);
                 CREATE INDEX IF NOT EXISTS idx_mined_words_request_id
                     ON mined_words(request_id);",
            )?;

            if let Some(seq) = prev_seq {
                let current_seq: Option<i64> = conn
                    .query_row(
                        "SELECT seq FROM sqlite_sequence WHERE name = 'mined_words'",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                match current_seq {
                    Some(cur) if cur < seq => {
                        conn.execute(
                            "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'mined_words'",
                            params![seq],
                        )?;
                    }
                    None => {
                        conn.execute(
                            "INSERT INTO sqlite_sequence (name, seq) VALUES ('mined_words', ?1)",
                            params![seq],
                        )?;
                    }
                    _ => {}
                }
            }
            Ok(())
        })();

        match mig_res {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        let _ = conn.pragma_update(None, "temp_store", "MEMORY");
        conn.busy_timeout(std::time::Duration::from_secs(5))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS bank_metadata (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;

        if Self::has_column(&conn, "mined_words", "screenshot")? {
            Self::migrate_legacy_inline_media(&conn)?;
        } else {
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS mined_words (
                     id               INTEGER PRIMARY KEY AUTOINCREMENT,
                     created_at       INTEGER NOT NULL,
                     term             TEXT NOT NULL,
                     reading          TEXT NOT NULL,
                     definition_text  TEXT NOT NULL,
                     definition_json  TEXT NOT NULL,
                     sentence         TEXT NOT NULL,
                     word_start       INTEGER NOT NULL,
                     word_end         INTEGER NOT NULL,
                     source_text      TEXT NOT NULL,
                     has_screenshot   INTEGER NOT NULL DEFAULT 0,
                     tag              TEXT,
                     twitch_clip_url  TEXT,
                     request_id       INTEGER,
                     sequence         INTEGER
                 );
                 CREATE TABLE IF NOT EXISTS mined_word_media (
                     word_id    INTEGER PRIMARY KEY,
                     screenshot BLOB,
                     thumbnail  BLOB
                 );",
            )?;
            if !Self::has_column(&conn, "mined_words", "has_screenshot")? {
                conn.execute_batch(
                    "ALTER TABLE mined_words ADD COLUMN has_screenshot INTEGER NOT NULL DEFAULT 0;",
                )?;
            }
            if !Self::has_column(&conn, "mined_words", "tag")? {
                conn.execute_batch("ALTER TABLE mined_words ADD COLUMN tag TEXT;")?;
            }
            if !Self::has_column(&conn, "mined_words", "twitch_clip_url")? {
                conn.execute_batch("ALTER TABLE mined_words ADD COLUMN twitch_clip_url TEXT;")?;
            }
            if !Self::has_column(&conn, "mined_words", "request_id")? {
                conn.execute_batch("ALTER TABLE mined_words ADD COLUMN request_id INTEGER;")?;
            }
            if !Self::has_column(&conn, "mined_words", "sequence")? {
                conn.execute_batch("ALTER TABLE mined_words ADD COLUMN sequence INTEGER;")?;
            }
        }

        conn.execute_batch(
            "DROP INDEX IF EXISTS idx_mined_words_created;
             CREATE INDEX IF NOT EXISTS idx_mined_words_created_id
                 ON mined_words(created_at DESC, id DESC);
             CREATE INDEX IF NOT EXISTS idx_mined_words_tag ON mined_words(tag);
             CREATE INDEX IF NOT EXISTS idx_mined_words_request_id ON mined_words(request_id);
             CREATE TABLE IF NOT EXISTS custom_names (
                 id         INTEGER PRIMARY KEY AUTOINCREMENT,
                 created_at INTEGER NOT NULL,
                 kanji      TEXT NOT NULL,
                 furigana   TEXT NOT NULL,
                 source_tag TEXT,
                 notes      TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_custom_names_kanji ON custom_names(kanji);
             CREATE INDEX IF NOT EXISTS idx_custom_names_furigana ON custom_names(furigana);
             CREATE INDEX IF NOT EXISTS idx_custom_names_created ON custom_names(created_at DESC);",
        )?;
        Ok(Self { conn })
    }

    /// Inserts an entry and returns its listing metadata.
    pub fn insert(&self, entry: &NewBankEntry) -> Result<BankEntryMeta> {
        let sequence = super::extract_sequence_from_json(&entry.definition_json);
        let has_screenshot = i64::from(entry.screenshot.is_some());
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                 sentence, word_start, word_end, source_text, has_screenshot, tag, twitch_clip_url,
                 request_id, sequence)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                entry.created_at,
                entry.term,
                entry.reading,
                entry.definition_text,
                entry.definition_json,
                entry.sentence,
                entry.word_range.0 as i64,
                entry.word_range.1 as i64,
                entry.source_text,
                has_screenshot,
                entry.tag,
                entry.twitch_clip_url,
                entry.request_id,
                sequence,
            ],
        )?;
        let id = tx.last_insert_rowid();
        if entry.screenshot.is_some() || entry.thumbnail.is_some() {
            tx.execute(
                "INSERT INTO mined_word_media (word_id, screenshot, thumbnail)
                 VALUES (?1, ?2, ?3)",
                params![id, entry.screenshot, entry.thumbnail],
            )?;
        }
        tx.commit()?;
        Ok(BankEntryMeta {
            id,
            created_at: entry.created_at,
            term: entry.term.clone(),
            reading: entry.reading.clone(),
            definition_text: entry.definition_text.clone(),
            definition_json: entry.definition_json.clone(),
            sentence: entry.sentence.clone(),
            word_range: entry.word_range,
            has_screenshot: entry.screenshot.is_some(),
            tag: entry.tag.clone(),
            twitch_clip_url: entry.twitch_clip_url.clone(),
        })
    }

    /// Lists all entries, most recent first.
    #[cfg(test)]
    pub fn list_meta(&self) -> Result<Vec<BankEntryMeta>> {
        Ok(self
            .list_meta_with_sequences()?
            .into_iter()
            .map(|(meta, _)| meta)
            .collect())
    }

    /// Lists all entries along with their cached `sequence` column (`None` for un-backfilled legacy rows).
    pub(crate) fn list_meta_with_sequences(&self) -> Result<Vec<(BankEntryMeta, Option<i64>)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, term, reading, definition_text, sentence, word_start, word_end,
                    has_screenshot != 0, definition_json, tag, twitch_clip_url, sequence
             FROM mined_words ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            let meta = BankEntryMeta {
                id: row.get(0)?,
                created_at: row.get(1)?,
                term: row.get(2)?,
                reading: row.get(3)?,
                definition_text: row.get(4)?,
                sentence: row.get(5)?,
                word_range: (
                    row.get::<_, i64>(6)? as usize,
                    row.get::<_, i64>(7)? as usize,
                ),
                has_screenshot: row.get(8)?,
                definition_json: row.get(9)?,
                tag: row.get(10)?,
                twitch_clip_url: row.get(11)?,
            };
            let sequence: Option<i64> = row.get(12)?;
            Ok((meta, sequence))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Batch-updates the cached `sequence` column for existing entries.
    pub(crate) fn update_sequences(&self, updates: &[(i64, i64)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare_cached("UPDATE mined_words SET sequence = ?1 WHERE id = ?2")?;
            for (id, sequence) in updates {
                stmt.execute(params![sequence, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn get_meta_value(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM bank_metadata WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn set_meta_value(&self, key: &str, val: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO bank_metadata (key, value) VALUES (?1, ?2)",
            params![key, val],
        )?;
        Ok(())
    }

    pub fn thumbnail(&self, id: i64) -> Result<Option<Vec<u8>>> {
        self.blob(id, "thumbnail")
    }

    pub fn screenshot(&self, id: i64) -> Result<Option<Vec<u8>>> {
        self.blob(id, "screenshot")
    }

    fn blob(&self, id: i64, column: &str) -> Result<Option<Vec<u8>>> {
        let sql = match column {
            "thumbnail" => "SELECT thumbnail FROM mined_word_media WHERE word_id = ?1",
            "screenshot" => "SELECT screenshot FROM mined_word_media WHERE word_id = ?1",
            _ => anyhow::bail!("invalid media column: {column}"),
        };
        let mut stmt = self.conn.prepare_cached(sql)?;
        Ok(stmt
            .query_row(params![id], |row| row.get::<_, Option<Vec<u8>>>(0))
            .optional()?
            .flatten())
    }

    /// Sets (or, with `None`, removes) the tag of an entry.
    pub fn set_tag(&self, id: i64, tag: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE mined_words SET tag = ?1 WHERE id = ?2",
            params![tag, id],
        )?;
        Ok(())
    }

    /// Sets (or, with `None`, removes) the Twitch clip URL of an entry by ID.
    pub fn set_twitch_clip_url(&self, id: i64, clip_url: Option<&str>) -> Result<()> {
        let updated = self.conn.execute(
            "UPDATE mined_words SET twitch_clip_url = ?1 WHERE id = ?2",
            params![clip_url, id],
        )?;
        if updated == 0 {
            anyhow::bail!("mined word entry id {id} not found");
        }
        Ok(())
    }

    /// Sets the Twitch clip URL for the entry with matching unique `request_id`.
    /// Returns the ID of the matched entry if found.
    pub fn set_twitch_clip_url_by_request_id(
        &self,
        request_id: i64,
        clip_url: &str,
    ) -> Result<Option<i64>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id FROM mined_words WHERE request_id = ?1 ORDER BY id DESC LIMIT 1",
        )?;
        let id: Option<i64> = stmt
            .query_row(params![request_id], |row| row.get(0))
            .optional()?;
        if let Some(id) = id {
            self.set_twitch_clip_url(id, Some(clip_url))?;
        }
        Ok(id)
    }

    /// Sets the Twitch clip URL for the entry with matching `created_at` when unambiguous.
    /// Returns the ID of the matched entry if found.
    pub fn set_twitch_clip_url_by_created_at(
        &self,
        created_at: i64,
        clip_url: &str,
    ) -> Result<Option<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM mined_words WHERE created_at = ?1 LIMIT 2")?;
        let ids: Vec<i64> = stmt
            .query_map(params![created_at], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        match ids.as_slice() {
            [] => Ok(None),
            [id] => {
                self.set_twitch_clip_url(*id, Some(clip_url))?;
                Ok(Some(*id))
            }
            _ => anyhow::bail!(
                "ambiguous created_at timestamp {created_at} matches multiple mined entries"
            ),
        }
    }

    /// Updates `definition_json` for multiple entries in a single batch.
    pub fn update_definition_jsons(&self, updates: &[(i64, String)]) -> Result<()> {
        if updates.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt =
                tx.prepare_cached("UPDATE mined_words SET definition_json = ?1 WHERE id = ?2")?;
            for (id, def_json) in updates {
                stmt.execute(params![def_json, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM mined_word_media WHERE word_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM mined_words WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Inserts a new custom name entry and returns it with its generated id.
    pub fn insert_custom_name(&self, entry: &NewCustomName) -> Result<CustomNameEntry> {
        self.conn.execute(
            "INSERT INTO custom_names (created_at, kanji, furigana, source_tag, notes)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.created_at,
                entry.kanji,
                entry.furigana,
                entry.source_tag,
                entry.notes,
            ],
        )?;
        Ok(CustomNameEntry {
            id: self.conn.last_insert_rowid(),
            created_at: entry.created_at,
            kanji: entry.kanji.clone(),
            furigana: entry.furigana.clone(),
            source_tag: entry.source_tag.clone(),
            notes: entry.notes.clone(),
        })
    }

    /// Lists all custom names, most recently created first.
    pub fn list_custom_names(&self) -> Result<Vec<CustomNameEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, created_at, kanji, furigana, source_tag, notes
             FROM custom_names ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CustomNameEntry {
                id: row.get(0)?,
                created_at: row.get(1)?,
                kanji: row.get(2)?,
                furigana: row.get(3)?,
                source_tag: row.get(4)?,
                notes: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Deletes a custom name entry by id.
    pub fn delete_custom_name(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM custom_names WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, created_at: i64, shot: bool) -> NewBankEntry {
        NewBankEntry {
            request_id: None,
            created_at,
            term: term.into(),
            reading: "よみ".into(),
            definition_text: "① meaning".into(),
            definition_json: "[]".into(),
            sentence: format!("{term}を見た。"),
            word_range: (0, term.chars().count()),
            source_text: format!("前の文。{term}を見た。"),
            screenshot: shot.then(|| vec![1, 2, 3]),
            thumbnail: shot.then(|| vec![4, 5]),
            tag: None,
            twitch_clip_url: None,
        }
    }

    #[test]
    fn round_trip_orders_newest_first_and_deletes() {
        let db = BankDatabase::open_in_memory().unwrap();
        let a = db.insert(&entry("猫", 1_000, true)).unwrap();
        let b = db.insert(&entry("犬", 2_000, false)).unwrap();

        let list = db.list_meta().unwrap();
        assert_eq!(
            list.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![b.id, a.id]
        );
        assert!(list[1].has_screenshot);
        assert!(!list[0].has_screenshot);
        assert_eq!(list[1].word_range, (0, 1));

        assert_eq!(db.screenshot(a.id).unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(db.thumbnail(a.id).unwrap(), Some(vec![4, 5]));
        assert_eq!(db.screenshot(b.id).unwrap(), None);

        db.delete(a.id).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].term, "犬");
        assert_eq!(db.screenshot(a.id).unwrap(), None);
    }

    #[test]
    fn tags_round_trip_and_can_be_edited() {
        let db = BankDatabase::open_in_memory().unwrap();
        let a = db
            .insert(&NewBankEntry {
                tag: Some("Final Fantasy 7".into()),
                ..entry("猫", 1_000, false)
            })
            .unwrap();
        assert_eq!(a.tag.as_deref(), Some("Final Fantasy 7"));
        let b = db.insert(&entry("犬", 2_000, false)).unwrap();
        assert_eq!(b.tag, None);

        let list = db.list_meta().unwrap();
        assert_eq!(list[1].tag.as_deref(), Some("Final Fantasy 7"));
        assert_eq!(list[0].tag, None);

        db.set_tag(b.id, Some("Dragon Quest")).unwrap();
        db.set_tag(a.id, None).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list[0].tag.as_deref(), Some("Dragon Quest"));
        assert_eq!(list[1].tag, None);
    }

    #[test]
    fn twitch_clip_urls_round_trip_and_can_be_edited() {
        let db = BankDatabase::open_in_memory().unwrap();
        let a = db
            .insert(&NewBankEntry {
                twitch_clip_url: Some("https://clips.twitch.tv/ClipA".into()),
                ..entry("猫", 1_000, false)
            })
            .unwrap();
        assert_eq!(
            a.twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/ClipA")
        );
        let b = db.insert(&entry("犬", 2_000, false)).unwrap();
        assert_eq!(b.twitch_clip_url, None);

        let list = db.list_meta().unwrap();
        assert_eq!(
            list[1].twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/ClipA")
        );
        assert_eq!(list[0].twitch_clip_url, None);

        db.set_twitch_clip_url(b.id, Some("https://clips.twitch.tv/ClipB"))
            .unwrap();
        db.set_twitch_clip_url_by_created_at(1_000, "https://clips.twitch.tv/ClipAUpdated")
            .unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(
            list[0].twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/ClipB")
        );
        assert_eq!(
            list[1].twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/ClipAUpdated")
        );
    }

    #[test]
    fn databases_without_tag_column_are_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE mined_words (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, created_at INTEGER NOT NULL,
                 term TEXT NOT NULL, reading TEXT NOT NULL, definition_text TEXT NOT NULL,
                 definition_json TEXT NOT NULL, sentence TEXT NOT NULL,
                 word_start INTEGER NOT NULL, word_end INTEGER NOT NULL,
                 source_text TEXT NOT NULL, screenshot BLOB, thumbnail BLOB);
             INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                 sentence, word_start, word_end, source_text)
             VALUES (1, '猫', 'ねこ', 'cat', '[]', '猫だ', 0, 1, '猫だ');",
        )
        .unwrap();
        let db = BankDatabase::from_conn(conn).unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].tag, None);
        assert_eq!(list[0].twitch_clip_url, None);
        db.set_tag(list[0].id, Some("Grandia 2")).unwrap();
        db.set_twitch_clip_url(list[0].id, Some("https://clips.twitch.tv/Grandia"))
            .unwrap();
        let list = db.list_meta().unwrap();
        assert_eq!(list[0].tag.as_deref(), Some("Grandia 2"));
        assert_eq!(
            list[0].twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/Grandia")
        );
    }

    #[test]
    fn custom_names_round_trip_and_can_be_deleted() {
        let db = BankDatabase::open_in_memory().unwrap();
        let name1 = db
            .insert_custom_name(&NewCustomName {
                created_at: 1_000,
                kanji: "田中".into(),
                furigana: "たなか".into(),
                source_tag: Some("Game A".into()),
                notes: Some("Protagonist".into()),
            })
            .unwrap();
        let name2 = db
            .insert_custom_name(&NewCustomName {
                created_at: 2_000,
                kanji: "田中".into(),
                furigana: "たない".into(),
                source_tag: None,
                notes: None,
            })
            .unwrap();

        let list = db.list_custom_names().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, name2.id);
        assert_eq!(list[0].kanji, "田中");
        assert_eq!(list[0].furigana, "たない");
        assert_eq!(list[0].source_tag, None);
        assert_eq!(list[0].notes, None);

        assert_eq!(list[1].id, name1.id);
        assert_eq!(list[1].kanji, "田中");
        assert_eq!(list[1].furigana, "たなか");
        assert_eq!(list[1].source_tag.as_deref(), Some("Game A"));
        assert_eq!(list[1].notes.as_deref(), Some("Protagonist"));

        db.delete_custom_name(name1.id).unwrap();
        let list = db.list_custom_names().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, name2.id);
    }

    #[test]
    fn test_bank_database_open_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("bank.db");
        let db = BankDatabase::open(&db_path).unwrap();
        let sync_mode: i64 = db
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync_mode, 1, "PRAGMA synchronous should be NORMAL (1)");
        let journal_mode: String = db
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert!(journal_mode.eq_ignore_ascii_case("wal"));
        let list = db.list_meta().unwrap();
        assert!(list.is_empty());
        let resolved = crate::bank::resolve_bank_directory();
        assert!(!resolved.as_os_str().is_empty());
    }

    #[test]
    fn legacy_inline_media_migrates_on_disk_with_backup_and_preserves_ids() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("legacy_bank.db");

        // 1. Seed a legacy on-disk WAL database with inline screenshot/thumbnail BLOBs
        //    and a deleted trailing row to test AUTOINCREMENT sequence preservation.
        {
            let conn = Connection::open(&db_path).unwrap();
            let _ = conn.pragma_update(None, "journal_mode", "WAL");
            conn.execute_batch(
                "CREATE TABLE mined_words (
                     id               INTEGER PRIMARY KEY AUTOINCREMENT,
                     created_at       INTEGER NOT NULL,
                     term             TEXT NOT NULL,
                     reading          TEXT NOT NULL,
                     definition_text  TEXT NOT NULL,
                     definition_json  TEXT NOT NULL,
                     sentence         TEXT NOT NULL,
                     word_start       INTEGER NOT NULL,
                     word_end         INTEGER NOT NULL,
                     source_text      TEXT NOT NULL,
                     screenshot       BLOB,
                     thumbnail        BLOB,
                     tag              TEXT,
                     twitch_clip_url  TEXT,
                     request_id       INTEGER
                 );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                     sentence, word_start, word_end, source_text, screenshot, thumbnail, tag, twitch_clip_url, request_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    1_000i64,
                    "猫",
                    "ねこ",
                    "cat",
                    r#"{"sequence":101}"#,
                    "猫がいる。",
                    0i64,
                    1i64,
                    "猫がいる。",
                    vec![10u8, 20, 30],
                    vec![40u8, 50],
                    Some("Game 1"),
                    Some("https://clips.twitch.tv/LegacyCat"),
                    11i64,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                     sentence, word_start, word_end, source_text, screenshot, thumbnail, tag, twitch_clip_url, request_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL, NULL, NULL, ?10)",
                params![
                    2_000i64,
                    "犬",
                    "いぬ",
                    "dog",
                    r#"{"sequence":202}"#,
                    "犬がいる。",
                    0i64,
                    1i64,
                    "犬がいる。",
                    12i64,
                ],
            )
            .unwrap();
            // Insert id = 3 and delete it so sqlite_sequence = 3 while MAX(id) = 2
            conn.execute(
                "INSERT INTO mined_words (created_at, term, reading, definition_text, definition_json,
                     sentence, word_start, word_end, source_text)
                 VALUES (3000, '鳥', 'とり', 'bird', '[]', '鳥だ', 0, 1, '鳥だ')",
                [],
            )
            .unwrap();
            conn.execute("DELETE FROM mined_words WHERE id = 3", [])
                .unwrap();
        }

        // 2. Open via BankDatabase::open -> should checkpoint, create .pre_media_split.bak, and migrate
        let db = BankDatabase::open(&db_path).unwrap();

        let bak_path = dir.path().join("legacy_bank.db.pre_media_split.bak");
        assert!(
            bak_path.exists(),
            "backup file {:?} should be created before table rebuild",
            bak_path
        );

        // Verify backup still has the original legacy inline schema & data
        {
            let bak_conn = Connection::open(&bak_path).unwrap();
            assert!(BankDatabase::has_column(&bak_conn, "mined_words", "screenshot").unwrap());
            let bak_shot: Vec<u8> = bak_conn
                .query_row("SELECT screenshot FROM mined_words WHERE id = 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(bak_shot, vec![10u8, 20, 30]);
        }

        // Verify migrated schema no longer has inline BLOB columns on mined_words
        assert!(!BankDatabase::has_column(&db.conn, "mined_words", "screenshot").unwrap());
        assert!(!BankDatabase::has_column(&db.conn, "mined_words", "thumbnail").unwrap());
        assert!(BankDatabase::has_column(&db.conn, "mined_words", "has_screenshot").unwrap());
        assert!(BankDatabase::has_column(&db.conn, "mined_words", "sequence").unwrap());

        // Verify list_meta, screenshot, thumbnail
        let list = db.list_meta().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, 2);
        assert_eq!(list[0].term, "犬");
        assert!(!list[0].has_screenshot);
        assert_eq!(list[1].id, 1);
        assert_eq!(list[1].term, "猫");
        assert!(list[1].has_screenshot);
        assert_eq!(list[1].tag.as_deref(), Some("Game 1"));
        assert_eq!(
            list[1].twitch_clip_url.as_deref(),
            Some("https://clips.twitch.tv/LegacyCat")
        );

        assert_eq!(db.screenshot(1).unwrap(), Some(vec![10u8, 20, 30]));
        assert_eq!(db.thumbnail(1).unwrap(), Some(vec![40u8, 50]));
        assert_eq!(db.screenshot(2).unwrap(), None);
        assert_eq!(db.thumbnail(2).unwrap(), None);

        // Verify AUTOINCREMENT ID continues after deleted id = 3 (new entry gets id = 4)
        let inserted = db
            .insert(&NewBankEntry {
                definition_json: r#"{"sequence":404}"#.into(),
                ..entry("魚", 4_000, true)
            })
            .unwrap();
        assert_eq!(inserted.id, 4);
        assert_eq!(db.screenshot(4).unwrap(), Some(vec![1, 2, 3]));

        // Verify sequence extraction on insert and update_sequences on migrated rows
        let with_seqs = db.list_meta_with_sequences().unwrap();
        assert_eq!(with_seqs[0].0.id, 4);
        assert_eq!(with_seqs[0].1, Some(404));
        assert_eq!(with_seqs[1].1, None);
        assert_eq!(with_seqs[2].1, None);
        db.update_sequences(&[(1, 101), (2, 202)]).unwrap();
        let with_seqs = db.list_meta_with_sequences().unwrap();
        assert_eq!(with_seqs[1].1, Some(202));
        assert_eq!(with_seqs[2].1, Some(101));

        // Verify delete removes both mined_words and mined_word_media rows
        db.delete(1).unwrap();
        assert_eq!(db.screenshot(1).unwrap(), None);
        assert_eq!(db.thumbnail(1).unwrap(), None);
        let media_exists: bool = db
            .conn
            .prepare("SELECT 1 FROM mined_word_media WHERE word_id = 1")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(!media_exists);
    }
}
