# Meeting Processing Status Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persistent `processing_status` field to meeting history entries, shown as a colored badge (Starting → Processing → Completed) on the history card.

**Architecture:** A new `ProcessingStatus` enum and DB column (migration 6, default `'completed'`) persists status. The meeting segment worker creates a placeholder entry with `Starting` status before spawning, transitions to `Processing` on the first transcribed segment, and marks `Completed` when the channel drains. The frontend renders a Tailwind chip badge on `entry_type === "meeting"` entries only.

**Tech Stack:** Rust/rusqlite (backend), React/TypeScript/Tailwind (frontend), tauri-specta (bindings auto-generation), i18next (i18n)

---

### Task 1: Add ProcessingStatus to history.rs + update audio.rs callers

**Files:**
- Modify: `src-tauri/src/managers/history.rs`
- Modify: `src-tauri/src/managers/audio.rs` (callers only — logic rewrite is Task 2)

- [ ] **Step 1: Write the failing test**

Add this test inside the `#[cfg(test)] mod tests` block in `src-tauri/src/managers/history.rs` (after the `entry_type_round_trips_for_meeting` test):

```rust
#[test]
fn processing_status_round_trips() {
    let conn = setup_conn();
    conn.execute(
        "INSERT INTO transcription_history (
            file_name, timestamp, saved, title,
            transcription_text, post_process_requested, entry_type, processing_status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            "meeting_2026-05-25.txt",
            100i64,
            false,
            "May 25, 2026",
            "hello",
            false,
            "meeting",
            "starting",
        ],
    )
    .unwrap();

    let entry = HistoryManager::get_latest_entry_with_conn(&conn)
        .unwrap()
        .unwrap();

    assert_eq!(entry.processing_status, ProcessingStatus::Starting);
}
```

- [ ] **Step 2: Run test to confirm it fails to compile**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml 2>&1 | Select-String "error"
```

Expected: compile error — `ProcessingStatus` not found, `HistoryEntry` has no field `processing_status`.

- [ ] **Step 3: Add `ProcessingStatus` enum**

In `src-tauri/src/managers/history.rs`, after the `EntryType` enum (after line 42), add:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingStatus {
    Starting,
    Processing,
    Completed,
}
```

- [ ] **Step 4: Add `processing_status` field to `HistoryEntry`**

In the `HistoryEntry` struct (currently ends at line 75), add the field after `entry_type`:

```rust
pub entry_type: EntryType,
pub processing_status: ProcessingStatus,
```

- [ ] **Step 5: Update `map_history_entry`**

Replace the current `map_history_entry` function (lines 208–227) entirely:

```rust
fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let entry_type_str: String = row.get("entry_type")?;
    let entry_type = if entry_type_str == "meeting" {
        EntryType::Meeting
    } else {
        EntryType::Normal
    };
    let processing_status_str: String = row.get("processing_status")?;
    let processing_status = match processing_status_str.as_str() {
        "starting" => ProcessingStatus::Starting,
        "processing" => ProcessingStatus::Processing,
        _ => ProcessingStatus::Completed,
    };
    Ok(HistoryEntry {
        id: row.get("id")?,
        file_name: row.get("file_name")?,
        timestamp: row.get("timestamp")?,
        saved: row.get("saved")?,
        title: row.get("title")?,
        transcription_text: row.get("transcription_text")?,
        post_processed_text: row.get("post_processed_text")?,
        post_process_prompt: row.get("post_process_prompt")?,
        post_process_requested: row.get("post_process_requested")?,
        entry_type,
        processing_status,
    })
}
```

- [ ] **Step 6: Add migration 6**

In the `MIGRATIONS` slice (after the last `M::up` at line 34), add:

```rust
M::up("ALTER TABLE transcription_history ADD COLUMN processing_status TEXT NOT NULL DEFAULT 'completed';"),
```

- [ ] **Step 7: Update all SELECT queries to include `processing_status`**

Every query that calls `Self::map_history_entry` needs `processing_status` in its column list. There are six places — replace each SELECT column list as shown.

**`get_history_entries` — three branches** (the SELECT string appears three times, around lines 481, 494, 507). In each, replace the column list:

```sql
SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, entry_type, processing_status
```

(i.e., append `, processing_status` to every SELECT that feeds `map_history_entry`.)

**`get_latest_entry_with_conn`** (around line 528):

```rust
fn get_latest_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT
            id,
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            entry_type,
            processing_status
         FROM transcription_history
         ORDER BY timestamp DESC
         LIMIT 1",
    )?;

    let entry = stmt.query_row([], Self::map_history_entry).optional()?;
    Ok(entry)
}
```

**`get_latest_completed_entry_with_conn`** (around line 555):

```rust
fn get_latest_completed_entry_with_conn(conn: &Connection) -> Result<Option<HistoryEntry>> {
    let mut stmt = conn.prepare(
        "SELECT
            id,
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            entry_type,
            processing_status
         FROM transcription_history
         WHERE transcription_text != ''
         ORDER BY timestamp DESC
         LIMIT 1",
    )?;

    let entry = stmt.query_row([], Self::map_history_entry).optional()?;
    Ok(entry)
}
```

**`get_entry_by_id`** (around line 610):

```rust
pub async fn get_entry_by_id(&self, id: i64) -> Result<Option<HistoryEntry>> {
    let conn = self.get_connection()?;
    let mut stmt = conn.prepare(
        "SELECT
            id,
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            entry_type,
            processing_status
         FROM transcription_history
         WHERE id = ?1",
    )?;

    let entry = stmt.query_row([id], Self::map_history_entry).optional()?;
    Ok(entry)
}
```

**`update_transcription`** — the SELECT after the UPDATE (around line 328):

```rust
let entry = conn
    .query_row(
        "SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, entry_type, processing_status
         FROM transcription_history WHERE id = ?1",
        params![id],
        Self::map_history_entry,
    )?;
```

- [ ] **Step 8: Update `save_entry` to accept `processing_status`**

Replace the `save_entry` signature and body. New signature adds `processing_status: ProcessingStatus` as the last parameter:

```rust
pub fn save_entry(
    &self,
    file_name: String,
    transcription_text: String,
    post_process_requested: bool,
    post_processed_text: Option<String>,
    post_process_prompt: Option<String>,
    entry_type: EntryType,
    processing_status: ProcessingStatus,
) -> Result<HistoryEntry> {
    let timestamp = Utc::now().timestamp();
    let title = self.format_timestamp_title(timestamp);

    let conn = self.get_connection()?;
    conn.execute(
        "INSERT INTO transcription_history (
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            entry_type,
            processing_status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            &file_name,
            timestamp,
            false,
            &title,
            &transcription_text,
            &post_processed_text,
            &post_process_prompt,
            post_process_requested,
            if entry_type == EntryType::Meeting { "meeting" } else { "normal" },
            match processing_status {
                ProcessingStatus::Starting => "starting",
                ProcessingStatus::Processing => "processing",
                ProcessingStatus::Completed => "completed",
            },
        ],
    )?;

    let entry = HistoryEntry {
        id: conn.last_insert_rowid(),
        file_name,
        timestamp,
        saved: false,
        title,
        transcription_text,
        post_processed_text,
        post_process_prompt,
        post_process_requested,
        entry_type,
        processing_status,
    };

    debug!("Saved history entry with id {}", entry.id);

    self.cleanup_old_entries()?;

    if let Err(e) = (HistoryUpdatePayload::Added {
        entry: entry.clone(),
    })
    .emit(&self.app_handle)
    {
        error!("Failed to emit history-updated event: {}", e);
    }

    Ok(entry)
}
```

- [ ] **Step 9: Add `update_processing_status` method**

Add this method to `impl HistoryManager`, after `update_transcription`:

```rust
pub fn update_processing_status(&self, id: i64, status: ProcessingStatus) -> Result<()> {
    let conn = self.get_connection()?;
    let updated = conn.execute(
        "UPDATE transcription_history SET processing_status = ?1 WHERE id = ?2",
        params![
            match status {
                ProcessingStatus::Starting => "starting",
                ProcessingStatus::Processing => "processing",
                ProcessingStatus::Completed => "completed",
            },
            id
        ],
    )?;

    if updated == 0 {
        return Err(anyhow!("History entry {} not found", id));
    }

    let entry = conn.query_row(
        "SELECT id, file_name, timestamp, saved, title, transcription_text,
                post_processed_text, post_process_prompt, post_process_requested,
                entry_type, processing_status
         FROM transcription_history WHERE id = ?1",
        params![id],
        Self::map_history_entry,
    )?;

    debug!("Updated processing_status for history entry {} to {:?}", id, entry.processing_status);

    if let Err(e) = (HistoryUpdatePayload::Updated {
        entry,
    })
    .emit(&self.app_handle)
    {
        error!("Failed to emit history-updated event: {}", e);
    }

    Ok(())
}
```

- [ ] **Step 10: Update test infrastructure**

Replace `setup_conn` (around line 679) to include the new column:

```rust
fn setup_conn() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    conn.execute_batch(
        "CREATE TABLE transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL,
            post_processed_text TEXT,
            post_process_prompt TEXT,
            post_process_requested BOOLEAN NOT NULL DEFAULT 0,
            entry_type TEXT NOT NULL DEFAULT 'normal',
            processing_status TEXT NOT NULL DEFAULT 'completed'
        );",
    )
    .expect("create transcription_history table");
    conn
}
```

Replace `insert_entry` (around line 699) to include `processing_status`:

```rust
fn insert_entry(conn: &Connection, timestamp: i64, text: &str, post_processed: Option<&str>) {
    conn.execute(
        "INSERT INTO transcription_history (
            file_name,
            timestamp,
            saved,
            title,
            transcription_text,
            post_processed_text,
            post_process_prompt,
            post_process_requested,
            entry_type,
            processing_status
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            format!("handy-{}.wav", timestamp),
            timestamp,
            false,
            format!("Recording {}", timestamp),
            text,
            post_processed,
            Option::<String>::None,
            false,
            "normal",
            "completed",
        ],
    )
    .expect("insert history entry");
}
```

- [ ] **Step 11: Update `audio.rs` callers of `save_entry` to pass `ProcessingStatus::Completed`**

In `src-tauri/src/managers/audio.rs`, there are two calls to `hm.save_entry` inside the worker thread. Add `crate::managers::history::ProcessingStatus::Completed` as the final argument to each:

**First call** (around line 700, inside the `None =>` branch of the `match history_entry_id`):

```rust
match hm.save_entry(
    history_file_name.clone(),
    full_text,
    false,
    None,
    None,
    crate::managers::history::EntryType::Meeting,
    crate::managers::history::ProcessingStatus::Completed,
) {
```

**Second call** (around line 762, inside the `None =>` branch at worker teardown):

```rust
if let Err(e) = hm.save_entry(
    history_file_name,
    full_text,
    false,
    None,
    None,
    crate::managers::history::EntryType::Meeting,
    crate::managers::history::ProcessingStatus::Completed,
) {
```

- [ ] **Step 12: Run tests to confirm all pass**

```powershell
cargo test --manifest-path src-tauri/Cargo.toml 2>&1
```

Expected: all tests pass, including the new `processing_status_round_trips` test.

- [ ] **Step 13: Commit**

```bash
git add src-tauri/src/managers/history.rs src-tauri/src/managers/audio.rs
git commit -m "feat: add ProcessingStatus enum and DB migration to history"
```

---

### Task 2: Rewrite `begin_meeting_segment` to use placeholder entry

**Files:**
- Modify: `src-tauri/src/managers/audio.rs`

- [ ] **Step 1: Create placeholder entry before spawning the worker**

In `begin_meeting_segment` (around line 608), after the `*self.meeting_chunk_tx.lock().unwrap() = Some(tx);` line (and after `history_file_name` and `app_handle` are set), but **before** `let worker = std::thread::spawn(...)`, add:

```rust
// Create a placeholder entry immediately so the history panel shows "starting" status.
let placeholder_id: Option<i64> = if let Some(hm) = self
    .app_handle
    .try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
{
    match hm.save_entry(
        history_file_name.clone(),
        String::new(),
        false,
        None,
        None,
        crate::managers::history::EntryType::Meeting,
        crate::managers::history::ProcessingStatus::Starting,
    ) {
        Ok(entry) => Some(entry.id),
        Err(e) => {
            error!("begin_meeting_segment: failed to create placeholder history entry: {e}");
            None
        }
    }
} else {
    error!("begin_meeting_segment: HistoryManager not available");
    None
};
```

- [ ] **Step 2: Rewrite the worker thread body**

Replace the entire `let worker = std::thread::spawn(move || { ... });` block with the following. Key changes: `placeholder_id` moves into the closure, `history_entry_id` is removed, status transitions are added.

```rust
let worker = std::thread::spawn(move || {
    let tm = match app_handle.try_state::<std::sync::Arc<crate::managers::transcription::TranscriptionManager>>() {
        Some(s) => (*s).clone(),
        None => {
            error!("Meeting mode worker: TranscriptionManager not available");
            return;
        }
    };

    let file = match std::fs::OpenOptions::new().append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            error!("Meeting mode worker: failed to open transcript file: {e}");
            return;
        }
    };
    let mut writer = std::io::BufWriter::new(file);

    let mut segments: Vec<(chrono::DateTime<chrono::Local>, String)> = Vec::new();
    let mut first_segment = true;
    let mut index: u32 = 0;

    while let Ok(chunk) = rx.recv() {
        match tm.transcribe(chunk) {
            Ok(text) if !text.is_empty() => {
                let ts = chrono::Local::now();
                let line = format!("[{}] {}\n", ts.format("%H:%M:%S"), text);

                if let Err(e) = writer.write_all(line.as_bytes()) {
                    error!("Meeting mode worker: failed to write segment: {e}");
                } else {
                    let _ = writer.flush();
                }

                segments.push((ts, text.clone()));

                let full_text: String = segments
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");

                if let Some(hm) = app_handle
                    .try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
                {
                    if let Some(id) = placeholder_id {
                        if let Err(e) = hm.update_transcription(id, full_text, None, None) {
                            error!("Meeting mode worker: failed to update history entry: {e}");
                        }
                        if first_segment {
                            if let Err(e) = hm.update_processing_status(
                                id,
                                crate::managers::history::ProcessingStatus::Processing,
                            ) {
                                error!("Meeting mode worker: failed to set processing status: {e}");
                            }
                            first_segment = false;
                        }
                    }
                }

                let _ = app_handle.emit(
                    "meeting-segment-transcribed",
                    MeetingSegmentEvent {
                        text,
                        timestamp: format!("[{}]", ts.format("%H:%M:%S")),
                        index,
                    },
                );
                index += 1;
            }
            Ok(_) => {}
            Err(e) => error!("Meeting mode worker: transcription error: {e}"),
        }
    }

    // Channel closed — write footer and mark entry as completed.
    let end = chrono::Local::now();
    let duration = end.signed_duration_since(start_time);
    let hours = duration.num_hours().abs();
    let minutes = (duration.num_minutes() % 60).abs();
    let seconds = (duration.num_seconds() % 60).abs();

    if let Ok(file) = std::fs::OpenOptions::new().append(true).open(&path) {
        let mut f = file;
        let _ = writeln!(f);
        let _ = writeln!(f, "Duration: {:02}:{:02}:{:02}", hours, minutes, seconds);
    }

    let full_text: String = segments
        .iter()
        .map(|(_, t)| t.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    if let Some(hm) = app_handle
        .try_state::<std::sync::Arc<crate::managers::history::HistoryManager>>()
    {
        if let Some(id) = placeholder_id {
            if let Err(e) = hm.update_transcription(id, full_text, None, None) {
                error!("Meeting mode worker: failed to update history entry on stop: {e}");
            }
            if let Err(e) = hm.update_processing_status(
                id,
                crate::managers::history::ProcessingStatus::Completed,
            ) {
                error!("Meeting mode worker: failed to set completed status: {e}");
            }
        }
    }

    info!("Meeting transcript saved to: {}", path.display());
    debug!("Meeting mode worker thread finished");
});
```

- [ ] **Step 3: Verify compilation**

```powershell
cargo check --manifest-path src-tauri/Cargo.toml 2>&1
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/managers/audio.rs
git commit -m "feat: create meeting placeholder entry on PTT start, track processing status in worker"
```

---

### Task 3: Add i18n keys for processing status

**Files:**
- Modify: `src/i18n/locales/en/translation.json`
- Modify: all other 19 locale files (same keys, English text as fallback)

- [ ] **Step 1: Add keys to English locale**

In `src/i18n/locales/en/translation.json`, inside `"settings"` → `"history"` object, add a `"status"` key after `"openTranscript"`:

```json
"openTranscript": "Open transcript",
"status": {
  "starting": "Starting...",
  "processing": "Processing...",
  "completed": "Completed"
}
```

- [ ] **Step 2: Add keys to all other locale files**

For each of the following files, find the `"settings"` → `"history"` object and add the same `"status"` block (with English text as fallback):

- `src/i18n/locales/ar/translation.json`
- `src/i18n/locales/bg/translation.json`
- `src/i18n/locales/cs/translation.json`
- `src/i18n/locales/de/translation.json`
- `src/i18n/locales/es/translation.json`
- `src/i18n/locales/fr/translation.json`
- `src/i18n/locales/he/translation.json`
- `src/i18n/locales/it/translation.json`
- `src/i18n/locales/ja/translation.json`
- `src/i18n/locales/ko/translation.json`
- `src/i18n/locales/pl/translation.json`
- `src/i18n/locales/pt/translation.json`
- `src/i18n/locales/ru/translation.json`
- `src/i18n/locales/sv/translation.json`
- `src/i18n/locales/tr/translation.json`
- `src/i18n/locales/uk/translation.json`
- `src/i18n/locales/vi/translation.json`
- `src/i18n/locales/zh-TW/translation.json`
- `src/i18n/locales/zh/translation.json`

In each file, find the closing `}` of the `"history"` object and insert before it:

```json
"status": {
  "starting": "Starting...",
  "processing": "Processing...",
  "completed": "Completed"
}
```

- [ ] **Step 3: Verify no JSON syntax errors**

```powershell
Get-ChildItem src/i18n/locales -Recurse -Filter translation.json | ForEach-Object { try { Get-Content $_.FullName | ConvertFrom-Json | Out-Null; Write-Host "OK: $($_.FullName)" } catch { Write-Host "ERR: $($_.FullName) - $_" } }
```

Expected: all files print `OK`.

- [ ] **Step 4: Commit**

```bash
git add src/i18n/locales/
git commit -m "feat: add i18n keys for meeting processing status badge"
```

---

### Task 4: Add ProcessingStatusBadge to the frontend

**Files:**
- Modify: `src/components/settings/history/HistorySettings.tsx`

Note: `bindings.ts` is auto-generated by tauri-specta on `cargo build`. The new `ProcessingStatus` type will appear there after the Rust tasks compile. In this task we reference it as a local type alias so TypeScript compiles correctly even before the first tauri build.

- [ ] **Step 1: Add `ProcessingStatusBadge` component**

In `src/components/settings/history/HistorySettings.tsx`, add this component **before** the `MeetingHistoryEntry` component definition (i.e., before line 314):

```tsx
type ProcessingStatus = "starting" | "processing" | "completed";

const ProcessingStatusBadge: React.FC<{ status: ProcessingStatus }> = ({
  status,
}) => {
  const { t } = useTranslation();

  if (status === "starting") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium bg-amber-100 text-amber-700 dark:bg-amber-900/30 dark:text-amber-400">
        <span className="h-1.5 w-1.5 rounded-full bg-amber-500 animate-pulse" />
        {t("settings.history.status.starting")}
      </span>
    );
  }

  if (status === "processing") {
    return (
      <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium bg-blue-100 text-blue-700 dark:bg-blue-900/30 dark:text-blue-400">
        <span className="h-1.5 w-1.5 rounded-full bg-blue-500 animate-pulse" />
        {t("settings.history.status.processing")}
      </span>
    );
  }

  return (
    <span className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400">
      <span className="h-1.5 w-1.5 rounded-full bg-green-500" />
      {t("settings.history.status.completed")}
    </span>
  );
};
```

- [ ] **Step 2: Render badge inside `MeetingHistoryEntry`**

In `MeetingHistoryEntry` (currently at line 342–369), replace the header row to include the badge next to the timestamp. Replace this block:

```tsx
<div className="flex justify-between items-center">
  <p className="text-sm font-medium">
    {formatDateTime(String(entry.timestamp), i18n.language)}
  </p>
  <IconButton onClick={onDelete} title={t("settings.history.delete")}>
    <Trash2 width={16} height={16} />
  </IconButton>
</div>
```

With:

```tsx
<div className="flex justify-between items-center">
  <div className="flex items-center gap-2">
    <p className="text-sm font-medium">
      {formatDateTime(String(entry.timestamp), i18n.language)}
    </p>
    <ProcessingStatusBadge
      status={(entry.processing_status ?? "completed") as ProcessingStatus}
    />
  </div>
  <IconButton onClick={onDelete} title={t("settings.history.delete")}>
    <Trash2 width={16} height={16} />
  </IconButton>
</div>
```

The `?? "completed"` fallback handles the brief window before `bindings.ts` is regenerated, and ensures safety if the field is absent on older cached entries.

- [ ] **Step 3: Verify TypeScript compilation**

```powershell
bun run build 2>&1
```

Expected: no TypeScript errors. (If `processing_status` is not yet in bindings.ts, the `?? "completed"` fallback and `as ProcessingStatus` cast prevent errors.)

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/history/HistorySettings.tsx
git commit -m "feat: add ProcessingStatusBadge to meeting history entries"
```

---

### Task 5: Manual end-to-end verification

- [ ] **Step 1: Start the app in dev mode**

```powershell
bun run tauri dev
```

Wait for the Rust build to complete (this regenerates `bindings.ts` with the new `ProcessingStatus` type). Confirm the app opens.

- [ ] **Step 2: Verify "Starting..." badge on PTT press**

1. Enable meeting mode in settings.
2. Press the PTT shortcut.
3. Check the History panel — a new meeting entry should appear immediately with a yellow "Starting..." badge.

- [ ] **Step 3: Verify "Processing..." badge on first segment**

4. Speak a sentence. Wait for the model to transcribe the first segment.
5. The badge on that history entry should change to blue "Processing..." with a pulsing dot.

- [ ] **Step 4: Verify "Completed" badge after stop**

6. Release PTT.
7. Wait a moment for the worker to finish.
8. The badge should change to green "Completed" (no pulse).

- [ ] **Step 5: Verify existing entries**

9. Open the History panel — all pre-existing meeting entries should show a green "Completed" badge (from the DB migration default).
10. Normal (non-meeting) entries should show no badge at all.

- [ ] **Step 6: Commit if any adjustments were made during testing**

```bash
git add -p
git commit -m "fix: adjust meeting processing status badge after manual verification"
```
