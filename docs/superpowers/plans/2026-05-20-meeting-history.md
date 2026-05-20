# Meeting History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface meeting mode transcriptions in the History tab with a dedicated card layout (title + date + open-file button), driven by an explicit `entry_type` DB column.

**Architecture:** Add an `entry_type TEXT` column (migration 4) to `transcription_history`; introduce a Rust `EntryType` enum; plumb it through `save_entry` and all callers; add an `open_meeting_file` Tauri command; update `bindings.ts` manually; render two distinct card variants in `HistorySettings.tsx`; remove the folder shortcut from `MeetingModeToggle`.

**Tech Stack:** Rust / rusqlite / rusqlite_migration / tauri-specta / React / TypeScript / i18next

---

## File Map

| File | What changes |
|---|---|
| `src-tauri/src/managers/history.rs` | Migration, `EntryType` enum, `HistoryEntry.entry_type`, all SELECTs, `map_history_entry`, `save_entry` signature + INSERT, test helpers |
| `src-tauri/src/actions.rs` | Pass `EntryType::Normal` at two existing `save_entry` call sites |
| `src-tauri/src/managers/audio.rs` | Pass `EntryType::Meeting` in `stop_meeting_mode`, add error log |
| `src-tauri/src/commands/audio.rs` | Add `open_meeting_file` command |
| `src-tauri/src/lib.rs` | Register `open_meeting_file` in the `collect_commands![]` list |
| `src/bindings.ts` | Add `entry_type` to `HistoryEntry` type; add `openMeetingFile` command binding |
| `src/i18n/locales/en/translation.json` | Add `settings.history.openTranscript` key |
| `src/components/settings/history/HistorySettings.tsx` | Add `MeetingHistoryEntry` component, global folder button, replace filename heuristic |
| `src/components/settings/MeetingModeToggle.tsx` | Remove folder button, `savedPath` toast, `handleOpenFolder`, `FolderOpen` import |

---

### Task 1: DB migration, EntryType enum, HistoryEntry, and tests

**Files:**
- Modify: `src-tauri/src/managers/history.rs`

**Context:**
- The `MIGRATIONS` slice is at lines 20–34. Each element is an `M::up(sql)`. Add a fifth migration at the end.
- `HistoryEntry` struct is at lines 55–66.
- `map_history_entry` is at lines 199–211.
- All SELECT queries use named column access via `Self::map_history_entry`, so every SELECT must include `entry_type` once the function reads it.
- SELECTs that need updating: `get_history_entries` (3 branches, lines ~460–497), `update_transcription` (line ~310), `get_latest_entry_with_conn` (line ~508), `get_latest_completed_entry_with_conn` (line ~535), `get_entry_by_id` (line ~589).
- `save_entry` is at lines 219–280. The `INSERT` lists 8 params (`?1`–`?8`); the new param will be `?9`.
- Test helpers `setup_conn` and `insert_entry` live in the `#[cfg(test)]` block at the bottom of the file.

- [ ] **Step 1: Write the failing test for EntryType round-trip**

Add inside the `mod tests` block at the bottom of `src-tauri/src/managers/history.rs`:

```rust
#[test]
fn entry_type_round_trips_for_meeting() {
    let conn = setup_conn();
    conn.execute(
        "INSERT INTO transcription_history (
            file_name, timestamp, saved, title,
            transcription_text, post_process_requested, entry_type
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            "meeting_2026-05-20_10-00-00.txt",
            100i64,
            false,
            "May 20, 2026",
            "hello world",
            false,
            "meeting",
        ],
    )
    .unwrap();

    let entry = HistoryManager::get_latest_entry_with_conn(&conn)
        .unwrap()
        .unwrap();

    assert_eq!(entry.entry_type, EntryType::Meeting);
}
```

- [ ] **Step 2: Run test to confirm it fails to compile (EntryType not defined yet)**

```powershell
cd src-tauri && cargo test managers::history::tests::entry_type_round_trips_for_meeting 2>&1 | Select-String "error|FAILED|test result"
```

Expected: compile error — `EntryType` and `entry_type` field don't exist yet.

- [ ] **Step 3: Add the migration**

In `src-tauri/src/managers/history.rs`, change the end of `MIGRATIONS`:

```rust
static MIGRATIONS: &[M] = &[
    M::up(
        "CREATE TABLE IF NOT EXISTS transcription_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            file_name TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            saved BOOLEAN NOT NULL DEFAULT 0,
            title TEXT NOT NULL,
            transcription_text TEXT NOT NULL
        );",
    ),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_processed_text TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_prompt TEXT;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN post_process_requested BOOLEAN NOT NULL DEFAULT 0;"),
    M::up("ALTER TABLE transcription_history ADD COLUMN entry_type TEXT NOT NULL DEFAULT 'normal';"),
];
```

- [ ] **Step 4: Add the EntryType enum**

Add directly above the `PaginatedHistory` struct (around line 36):

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Normal,
    Meeting,
}
```

- [ ] **Step 5: Add entry_type field to HistoryEntry**

Change `HistoryEntry` struct to add the field after `post_process_requested`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Type)]
pub struct HistoryEntry {
    pub id: i64,
    pub file_name: String,
    pub timestamp: i64,
    pub saved: bool,
    pub title: String,
    pub transcription_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
    pub post_process_requested: bool,
    pub entry_type: EntryType,
}
```

- [ ] **Step 6: Update map_history_entry**

Replace the existing `map_history_entry` function body:

```rust
fn map_history_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    let entry_type_str: String = row.get("entry_type").unwrap_or_else(|_| "normal".to_string());
    let entry_type = if entry_type_str == "meeting" {
        EntryType::Meeting
    } else {
        EntryType::Normal
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
    })
}
```

- [ ] **Step 7: Add entry_type to every SELECT in history.rs**

There are 7 SELECT statements that use `Self::map_history_entry`. In each one, append `, entry_type` to the column list. The column list currently ends with `post_process_requested` everywhere.

Find each occurrence of this pattern and add `, entry_type` after `post_process_requested`:

Locations to update (search for `post_process_requested` inside SELECT strings):
1. `get_history_entries` cursor+limit branch (~line 462)
2. `get_history_entries` no-cursor+limit branch (~line 476)
3. `get_history_entries` no-limit branch (~line 487)
4. `update_transcription` re-fetch SELECT (~line 310)
5. `get_latest_entry_with_conn` test helper (~line 510)
6. `get_latest_completed_entry_with_conn` (~line 537)
7. `get_entry_by_id` (~line 591)

Each SELECT currently looks like:
```sql
SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested
FROM transcription_history
```

Change to:
```sql
SELECT id, file_name, timestamp, saved, title, transcription_text, post_processed_text, post_process_prompt, post_process_requested, entry_type
FROM transcription_history
```

- [ ] **Step 8: Update save_entry**

Replace the `save_entry` signature and body to add `entry_type`:

```rust
pub fn save_entry(
    &self,
    file_name: String,
    transcription_text: String,
    post_process_requested: bool,
    post_processed_text: Option<String>,
    post_process_prompt: Option<String>,
    entry_type: EntryType,
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
            entry_type
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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

- [ ] **Step 9: Update test helpers**

In the `#[cfg(test)]` block, update `setup_conn`:

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
            entry_type TEXT NOT NULL DEFAULT 'normal'
        );",
    )
    .expect("create transcription_history table");
    conn
}
```

Update `insert_entry` to include `entry_type`:

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
            entry_type
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
        ],
    )
    .expect("insert history entry");
}
```

- [ ] **Step 10: Run tests to confirm they pass**

```powershell
cd src-tauri && cargo test managers::history::tests 2>&1 | Select-String "test result|FAILED|error\[E"
```

Expected: `test result: ok. N passed; 0 failed`

- [ ] **Step 11: Confirm the full crate still compiles (callers of save_entry will fail — that's expected here)**

```powershell
cd src-tauri && cargo check 2>&1 | Select-String "error\[E" | Select-Object -First 20
```

Expected: errors about `save_entry` missing the `entry_type` argument in `actions.rs` and `audio.rs`. That's fine — fixed in Task 2.

- [ ] **Step 12: Commit**

```bash
git add src-tauri/src/managers/history.rs
git commit -m "feat: add entry_type to history DB, EntryType enum, and update save_entry"
```

---

### Task 2: Update all save_entry callers

**Files:**
- Modify: `src-tauri/src/actions.rs` (two call sites)
- Modify: `src-tauri/src/managers/audio.rs` (one call site in stop_meeting_mode)

**Context:**
- `actions.rs` line ~591: successful transcription save — pass `EntryType::Normal`
- `actions.rs` line ~635: failed transcription save — pass `EntryType::Normal`
- `audio.rs` line ~790: meeting mode save — pass `EntryType::Meeting`, add error-case log

- [ ] **Step 1: Fix the two call sites in actions.rs**

At line ~591 (successful transcription):
```rust
if let Err(err) = hm.save_entry(
    file_name,
    transcription,
    post_process,
    processed.post_processed_text.clone(),
    processed.post_process_prompt.clone(),
    crate::managers::history::EntryType::Normal,
) {
```

At line ~635 (failed transcription):
```rust
if let Err(save_err) = hm.save_entry(
    file_name,
    String::new(),
    post_process,
    None,
    None,
    crate::managers::history::EntryType::Normal,
) {
```

- [ ] **Step 2: Fix the call site in audio.rs (stop_meeting_mode)**

Replace the existing `save_entry` block (around line 785–794) with:

```rust
if !file_name.is_empty() {
    if let Some(hm) = self
        .app_handle
        .try_state::<Arc<crate::managers::history::HistoryManager>>()
    {
        if let Err(e) = hm.save_entry(
            file_name,
            full_text,
            false,
            None,
            None,
            crate::managers::history::EntryType::Meeting,
        ) {
            error!("Meeting mode: failed to save history entry: {e}");
        }
    } else {
        error!("Meeting mode: HistoryManager state not available — entry not saved");
    }
}
```

- [ ] **Step 3: Confirm cargo check passes**

```powershell
cd src-tauri && cargo check 2>&1 | Select-String "error\[E"
```

Expected: no output (zero errors).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/actions.rs src-tauri/src/managers/audio.rs
git commit -m "fix: pass EntryType to all save_entry callers; Meeting uses EntryType::Meeting"
```

---

### Task 3: Add open_meeting_file command

**Files:**
- Modify: `src-tauri/src/commands/audio.rs`
- Modify: `src-tauri/src/lib.rs`

**Context:**
- `open_meetings_folder` is the last command in `audio.rs` (line ~377). Add `open_meeting_file` after it.
- In `lib.rs`, `commands::audio::open_meetings_folder` is at line ~423. Add `open_meeting_file` on the next line.

- [ ] **Step 1: Add the command to audio.rs**

After the closing `}` of `open_meetings_folder`, add:

```rust
#[tauri::command]
#[specta::specta]
pub fn open_meeting_file(app: AppHandle, file_name: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let docs_dir = app
        .path()
        .document_dir()
        .map_err(|e| format!("Failed to resolve Documents directory: {e}"))?;
    let file_path = docs_dir.join("Handy").join("meetings").join(&file_name);

    app.opener()
        .open_path(file_path.to_string_lossy().as_ref(), None::<String>)
        .map_err(|e| format!("Failed to open meeting file: {e}"))
}
```

- [ ] **Step 2: Register the command in lib.rs**

In the `collect_commands![]` list, after `commands::audio::open_meetings_folder,`, add:

```rust
commands::audio::open_meeting_file,
```

- [ ] **Step 3: Confirm cargo check passes**

```powershell
cd src-tauri && cargo check 2>&1 | Select-String "error\[E"
```

Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/audio.rs src-tauri/src/lib.rs
git commit -m "feat: add open_meeting_file Tauri command"
```

---

### Task 4: Update bindings.ts

**Files:**
- Modify: `src/bindings.ts`

**Context:**
- `HistoryEntry` type is at line 879. It's a single-line `export type HistoryEntry = { ... }`.
- `openMeetingsFolder` binding is at lines ~758–765. Add `openMeetingFile` after it.
- tauri-specta auto-generates this file in debug mode, but we update it manually here to avoid needing a full `bun run tauri dev` session.

- [ ] **Step 1: Add entry_type to HistoryEntry type**

Find line 879:
```ts
export type HistoryEntry = { id: number; file_name: string; timestamp: number; saved: boolean; title: string; transcription_text: string; post_processed_text: string | null; post_process_prompt: string | null; post_process_requested: boolean }
```

Replace with:
```ts
export type HistoryEntry = { id: number; file_name: string; timestamp: number; saved: boolean; title: string; transcription_text: string; post_processed_text: string | null; post_process_prompt: string | null; post_process_requested: boolean; entry_type: "normal" | "meeting" }
```

- [ ] **Step 2: Add openMeetingFile command binding**

After the closing `},` of `openMeetingsFolder` (around line 765), add:

```ts
async openMeetingFile(file_name: string) : Promise<Result<null, string>> {
    try {
    return { status: "ok", data: await TAURI_INVOKE("open_meeting_file", { file_name }) };
} catch (e) {
    if(e instanceof Error) throw e;
    else return { status: "error", error: e  as any };
}
},
```

- [ ] **Step 3: Verify TypeScript compiles**

```powershell
bun run build 2>&1 | Select-String "error TS|Error:"
```

Expected: no errors (or only pre-existing errors unrelated to this change).

- [ ] **Step 4: Commit**

```bash
git add src/bindings.ts
git commit -m "feat: add entry_type to HistoryEntry binding and openMeetingFile command"
```

---

### Task 5: Add i18n key

**Files:**
- Modify: `src/i18n/locales/en/translation.json`

**Context:**
- The `settings.history` section already has keys like `"loading"`, `"empty"`, `"openFolder"`, etc.
- Add `"openTranscript"` to that section.

- [ ] **Step 1: Add the key**

Inside `settings.history` object, add:

```json
"openTranscript": "Open transcript"
```

- [ ] **Step 2: Verify frontend still compiles**

```powershell
bun run build 2>&1 | Select-String "error TS|Error:"
```

Expected: no new errors.

- [ ] **Step 3: Commit**

```bash
git add src/i18n/locales/en/translation.json
git commit -m "i18n: add openTranscript key for meeting history entries"
```

---

### Task 6: HistorySettings.tsx — MeetingHistoryEntry component and global folder button

**Files:**
- Modify: `src/components/settings/history/HistorySettings.tsx`

**Context:**
- `FolderOpen` is already imported from `lucide-react` (line 4). Add `FileText` to that import.
- `HistoryEntryComponent` (line ~304) currently has `const isMeetingEntry = entry.file_name.endsWith(".txt")`. Replace this heuristic with `entry.entry_type === "meeting"` and branch to a new component.
- The global header at lines ~276–286 shows `OpenRecordingsButton` for recordings folder. Add a second `OpenRecordingsButton` for the meetings folder — conditionally rendered.
- `openRecordingsFolder` handler already exists at line ~226. Add an `openMeetingsFolder` handler below it.
- The existing two `.endsWith(".txt")` checks (retranscribe button ~line 389 and AudioPlayer ~line 445) become unreachable once we branch early in `HistoryEntryComponent`, so they can be removed.

- [ ] **Step 1: Add FileText to lucide-react import**

Change:
```tsx
import { Check, Copy, FolderOpen, RotateCcw, Star, Trash2 } from "lucide-react";
```
To:
```tsx
import { Check, Copy, FileText, FolderOpen, RotateCcw, Star, Trash2 } from "lucide-react";
```

- [ ] **Step 2: Add openMeetingsFolder handler**

After the `openRecordingsFolder` handler (around line 235), add:

```tsx
const openMeetingsFolder = async () => {
  try {
    const result = await commands.openMeetingsFolder();
    if (result.status !== "ok") {
      throw new Error(String(result.error));
    }
  } catch (error) {
    console.error("Failed to open meetings folder:", error);
  }
};
```

- [ ] **Step 3: Add conditional meetings folder button to the header**

In the `return` of `HistorySettings`, the header block (lines ~276–286) currently is:

```tsx
<div className="px-4 flex items-center justify-between">
  <div>
    <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
      {t("settings.history.title")}
    </h2>
  </div>
  <OpenRecordingsButton
    onClick={openRecordingsFolder}
    label={t("settings.history.openFolder")}
  />
</div>
```

Replace with:

```tsx
<div className="px-4 flex items-center justify-between">
  <div>
    <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
      {t("settings.history.title")}
    </h2>
  </div>
  <div className="flex items-center gap-2">
    {entries.some((e) => e.entry_type === "meeting") && (
      <OpenRecordingsButton
        onClick={openMeetingsFolder}
        label={t("settings.meetingMode.openFolder")}
      />
    )}
    <OpenRecordingsButton
      onClick={openRecordingsFolder}
      label={t("settings.history.openFolder")}
    />
  </div>
</div>
```

- [ ] **Step 4: Add MeetingHistoryEntry component**

Add this new component just above `HistoryEntryComponent` (around line 303):

```tsx
const MeetingHistoryEntry: React.FC<{
  entry: HistoryEntry;
  onDelete: () => void;
}> = ({ entry, onDelete }) => {
  const { t, i18n } = useTranslation();

  const handleOpenFile = async () => {
    try {
      await commands.openMeetingFile(entry.file_name);
    } catch (error) {
      console.error("Failed to open meeting file:", error);
    }
  };

  const handleOpenFolder = async () => {
    try {
      await commands.openMeetingsFolder();
    } catch (error) {
      console.error("Failed to open meetings folder:", error);
    }
  };

  return (
    <div className="px-4 py-2 pb-4 flex flex-col gap-2">
      <div className="flex justify-between items-center">
        <p className="text-sm font-medium">
          {formatDateTime(String(entry.timestamp), i18n.language)}
        </p>
        <IconButton onClick={onDelete} title={t("settings.history.delete")}>
          <Trash2 width={16} height={16} />
        </IconButton>
      </div>
      <div className="flex items-center gap-2">
        <button
          onClick={handleOpenFile}
          className="flex items-center gap-1.5 text-xs text-text/60 hover:text-text/90 transition-colors"
        >
          <FileText width={13} height={13} />
          <span>{t("settings.history.openTranscript")}</span>
        </button>
        <button
          onClick={handleOpenFolder}
          className="flex items-center gap-1.5 text-xs text-text/40 hover:text-text/70 transition-colors"
        >
          <FolderOpen width={13} height={13} />
          <span>{t("settings.meetingMode.openFolder")}</span>
        </button>
      </div>
    </div>
  );
};
```

- [ ] **Step 5: Update HistoryEntryComponent to branch on entry_type**

In `HistoryEntryComponent`, replace:
```tsx
const isMeetingEntry = entry.file_name.endsWith(".txt");
```
With:
```tsx
const isMeetingEntry = entry.entry_type === "meeting";
```

Then add an early return **as a JavaScript statement** between the last hook/handler and the `return (` statement (i.e., after `formattedDate` and before `return (`):

```tsx
if (isMeetingEntry) {
  return (
    <MeetingHistoryEntry
      entry={entry}
      onDelete={handleDeleteEntry}
    />
  );
}
```

In the normal render path below (which now only runs for non-meeting entries), remove the two dead guards:
- Remove the `{!isMeetingEntry && ...}` wrapper around the retranscribe `IconButton` — render the button unconditionally.
- Replace `{!entry.file_name.endsWith(".txt") && <AudioPlayer .../>}` with just `<AudioPlayer .../>`.

- [ ] **Step 6: Verify TypeScript compiles**

```powershell
bun run build 2>&1 | Select-String "error TS|Error:"
```

Expected: no errors.

- [ ] **Step 7: Commit**

```bash
git add src/components/settings/history/HistorySettings.tsx
git commit -m "feat: add MeetingHistoryEntry card and global meetings folder button in History tab"
```

---

### Task 7: MeetingModeToggle cleanup

**Files:**
- Modify: `src/components/settings/MeetingModeToggle.tsx`

**Context:**
- Current file: `src/components/settings/MeetingModeToggle.tsx` (83 lines).
- Remove: `FolderOpen` import, `savedPath` state, `toastTimer` ref, `handleOpenFolder` function, the secondary `<div>` row that contains the folder button and saved-path paragraph.
- Keep: the `ToggleSwitch` and the `loading`/`active` state logic, the `useEffect` init call, `handleToggle`.
- The `result.data` check in `handleToggle` can also be removed since we no longer show a toast with the path.

- [ ] **Step 1: Rewrite MeetingModeToggle.tsx**

Replace the entire file with:

```tsx
import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { commands } from "@/bindings";

export const MeetingModeToggle: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const [supported, setSupported] = useState(false);
  const [active, setActive] = useState(false);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    commands.isMeetingModeSupported().then(setSupported);
    commands.getMeetingModeState().then(setActive);
  }, []);

  if (!supported) return null;

  const handleToggle = async (enabled: boolean) => {
    setLoading(true);
    try {
      if (enabled) {
        await commands.startMeetingMode();
        setActive(true);
      } else {
        await commands.stopMeetingMode();
        setActive(false);
      }
    } catch (err) {
      console.error("Meeting mode toggle error:", err);
    }
    setLoading(false);
  };

  return (
    <ToggleSwitch
      checked={active}
      onChange={handleToggle}
      isUpdating={loading}
      label={t("settings.meetingMode.label")}
      description={t("settings.meetingMode.description")}
      descriptionMode="tooltip"
      grouped={true}
    />
  );
});
```

- [ ] **Step 2: Verify TypeScript compiles**

```powershell
bun run build 2>&1 | Select-String "error TS|Error:"
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/MeetingModeToggle.tsx
git commit -m "refactor: remove folder shortcut from MeetingModeToggle — now in History tab"
```

---

## Done

All 7 tasks complete. Manual smoke test on Windows:
1. Start meeting mode, speak for ~10 seconds, stop meeting mode.
2. Open History tab — new entry appears with date and "Open transcript" / "Open folder" buttons (no audio player, no text body).
3. Click "Open transcript" — the `.txt` file opens in Notepad.
4. Click "Open folder" (inline) or the global header button — `Documents\Handy\meetings\` opens in Explorer.
5. MeetingModeToggle in Settings shows only the toggle switch, no folder button.
