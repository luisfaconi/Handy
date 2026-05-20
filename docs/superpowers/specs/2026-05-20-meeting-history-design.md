# Meeting History — Design Spec

## Goal

Show meeting mode transcriptions in the History tab with a distinct card layout (title + date + "Open file" button), driven by an explicit `entry_type` DB field instead of the fragile `.txt` filename heuristic.

## Problem Statement

`stop_meeting_mode` calls `save_entry` but entries never appear in the History tab. Root causes to fix:

1. No `entry_type` field — the frontend hides or misrenders meeting entries because it relies on `file_name.endsWith('.txt')` for every branch of the render logic, and the card layout for meeting entries is not implemented.
2. `stop_meeting_mode` may silently return early (empty segments, missing path) without saving.
3. "Open meetings folder" button lives in `MeetingModeToggle` settings — should live in the History tab.

---

## Architecture

### Data Layer (Rust + SQLite)

**New migration** appended to the `MIGRATIONS` slice in `src-tauri/src/managers/history.rs`:

```rust
M::up("ALTER TABLE transcription_history ADD COLUMN entry_type TEXT NOT NULL DEFAULT 'normal';"),
```

This is migration index 4 (0-indexed). The existing `rusqlite_migration` mechanism handles it idempotently.

**New enum** added to `history.rs`:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntryType {
    Normal,
    Meeting,
}
```

**`HistoryEntry` struct** gets a new field:

```rust
pub entry_type: EntryType,
```

**`map_history_entry`** reads the new column:

```rust
entry_type: row.get::<_, String>("entry_type")
    .map(|s| if s == "meeting" { EntryType::Meeting } else { EntryType::Normal })
    .unwrap_or(EntryType::Normal),
```

(Uses string deserialization manually to handle legacy rows that predate the column gracefully.)

**`save_entry` signature** gains a parameter:

```rust
pub fn save_entry(
    &self,
    file_name: String,
    transcription_text: String,
    post_process_requested: bool,
    post_processed_text: Option<String>,
    post_process_prompt: Option<String>,
    entry_type: EntryType,
) -> Result<HistoryEntry>
```

The `INSERT` statement adds `entry_type` to the column list and `?9` to the values. The constructed `HistoryEntry` sets `entry_type` from the parameter.

All existing callers of `save_entry` pass `EntryType::Normal`. The call in `stop_meeting_mode` passes `EntryType::Meeting`.

**Test helper `setup_conn`** in `history.rs` adds `entry_type TEXT NOT NULL DEFAULT 'normal'` to the in-memory schema so existing tests keep compiling.

---

### `stop_meeting_mode` Fix

The existing call in `src-tauri/src/managers/audio.rs` is structurally correct but may return early via the `segments.is_empty()` guard before segments are fully drained. Ensure:

1. The worker `join()` completes **before** reading `self.meeting_segments`.
2. Add a log line after `try_state` to confirm `HistoryManager` is available.
3. Pass `EntryType::Meeting` to the updated `save_entry`.

The corrected call:

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
        error!("Meeting mode: HistoryManager state not available");
    }
}
```

---

### New Tauri Command: `open_meeting_file`

Added to `src-tauri/src/commands/audio.rs`:

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

Registered in `collect_commands![]` in `lib.rs`.

---

### Frontend

#### `src/bindings.ts`

`HistoryEntry` type gains:

```ts
entry_type: "normal" | "meeting";
```

New command added:

```ts
async openMeetingFile(file_name: string): Promise<Result<null, string>>
```

#### `src/components/settings/history/HistorySettings.tsx`

**Global "Open meetings folder" button**: Already has `OpenRecordingsButton` in the header. Add a second button next to it that only renders when `entries.some(e => e.entry_type === 'meeting')`:

```tsx
{entries.some((e) => e.entry_type === "meeting") && (
  <OpenRecordingsButton
    onClick={openMeetingsFolder}
    label={t("settings.meetingMode.openFolder")}
  />
)}
```

`openMeetingsFolder` calls `commands.openMeetingsFolder()` (already exists).

**`HistoryEntryComponent`**: Replace `entry.file_name.endsWith(".txt")` heuristic with `entry.entry_type === "meeting"`. When meeting:

- Render `MeetingHistoryEntry` instead of the normal layout.
- Do NOT render transcription text, audio player, copy/retranscribe/star buttons.
- Only render: formatted date (title), "Open file" button, "Open folder" button, delete button.

**New component `MeetingHistoryEntry`** (inline in `HistorySettings.tsx`):

```tsx
const MeetingHistoryEntry: React.FC<{
  entry: HistoryEntry;
  onDelete: () => void;
}> = ({ entry, onDelete }) => {
  const { t, i18n } = useTranslation();

  const handleOpenFile = async () => {
    await commands.openMeetingFile(entry.file_name);
  };

  const handleOpenFolder = async () => {
    await commands.openMeetingsFolder();
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

`FileText` imported from `lucide-react`.

#### `MeetingModeToggle.tsx`

Remove: `handleOpenFolder`, `FolderOpen` import, the `<div className="flex items-center gap-2 ...">` block containing the folder button and saved path toast. Keep only the `ToggleSwitch` and the saved-path toast (`savedPath && <p>...`) if desired — or remove the toast too since the entry now appears in history.

Actually: remove the entire secondary row (folder button + toast). The `savedPath` state, `toastTimer`, and `handleOpenFolder` can all be deleted. The `FolderOpen` import can be removed.

#### `src/i18n/locales/en/translation.json`

Add under `settings.history`:

```json
"openTranscript": "Open transcript"
```

(`settings.meetingMode.openFolder` already exists.)

---

## File Manifest

| File | Change |
|---|---|
| `src-tauri/src/managers/history.rs` | Add `EntryType` enum, migration, `entry_type` field on struct, `map_history_entry`, `save_entry` param, test schema |
| `src-tauri/src/managers/audio.rs` | Update `stop_meeting_mode` to pass `EntryType::Meeting`, add error log |
| `src-tauri/src/commands/audio.rs` | Add `open_meeting_file` command; remove `open_meetings_folder` folder-button from being relied on from MeetingModeToggle |
| `src-tauri/src/lib.rs` | Register `open_meeting_file` in `collect_commands![]` |
| `src/bindings.ts` | Add `entry_type` to `HistoryEntry`, add `openMeetingFile` command |
| `src/components/settings/history/HistorySettings.tsx` | Add `MeetingHistoryEntry` component, global meeting folder button, replace heuristic with `entry_type` |
| `src/components/settings/MeetingModeToggle.tsx` | Remove folder button, `handleOpenFolder`, `savedPath` toast, `FolderOpen` import |
| `src/i18n/locales/en/translation.json` | Add `settings.history.openTranscript` |

---

## Error Handling

- `open_meeting_file`: if the `.txt` file no longer exists on disk, the OS opener will show its own error dialog. No special handling needed in the app.
- `delete_entry` for meeting entries: the current code calls `get_audio_file_path(file_name)` which looks in `recordings_dir`. Meeting `.txt` files live in `Documents/Handy/meetings/`, so `file_path.exists()` returns false and the file is silently skipped — correct behavior, the transcript persists on disk even after the history entry is deleted.
- Meeting entries with `entry_type = 'meeting'` in the DB but missing `.txt` file: handled gracefully by OS opener; the delete button still works to clean up the DB entry.

---

## Testing

- Unit: `history.rs` tests updated to include `entry_type` in `setup_conn` schema and `insert_entry` helper; new test confirms `save_entry` with `EntryType::Meeting` stores `"meeting"` in the column and `map_history_entry` round-trips it correctly.
- Manual: Start meeting mode on Windows, record for a few seconds, stop — entry appears in History tab with meeting card layout. Clicking "Open transcript" opens the `.txt` in the default text editor. Clicking "Open folder" opens the meetings folder. The "Open meetings folder" global header button appears. MeetingModeToggle no longer has a folder button.
