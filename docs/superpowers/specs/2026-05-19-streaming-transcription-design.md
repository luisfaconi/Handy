# Streaming Transcription for Meeting Mode — Design Spec

**Date:** 2026-05-19
**Status:** Approved

## Summary

Add progressive (streaming) transcription to Meeting Mode. Instead of waiting until the session ends to transcribe all audio at once, each completed speech segment is transcribed immediately and written to the `.txt` file as it arrives. The recording overlay shows the last transcribed segment briefly so the user can confirm capture is working.

---

## 1. Problem

In the current Meeting Mode, transcription only happens after the user stops the session — all accumulated audio is sent to the model in one batch. For long meetings this means:

- No feedback that transcription is working until the end
- A potentially large audio buffer in memory
- If the app crashes, everything is lost

---

## 2. Scope

- **In scope:** Meeting Mode only (Windows); VAD-driven chunk dispatch; forced chunk boundary at 30s; progressive file write; overlay feedback.
- **Out of scope:** Streaming in normal (non-meeting) recording mode; token-level streaming (Moonshine Streaming engine); speaker diarization; configurable chunk duration (hardcoded at 30s for now).

---

## 3. Architecture

### 3.1 Chunk Emission Pipeline

```
AudioRecorder (VAD boundary OR 30s timeout)
    → segment_callback(Vec<f32>)
    → mpsc::Sender<Vec<f32>>  (meeting_chunk_tx)
    → Worker thread
    → tm.transcribe(chunk)
    → append to .txt file
    → emit "meeting-segment-transcribed" Tauri event
    → update meeting_segments accumulator
```

Recording and transcription run in parallel — the next chunk is captured while the previous one is being transcribed.

### 3.2 Chunk Boundary Conditions

| Condition                                                     | Action                                             |
| ------------------------------------------------------------- | -------------------------------------------------- |
| VAD detects silence after voice                               | Emit accumulated speech buffer as chunk            |
| Buffer reaches ≥ 30s of audio (16 000 × 30 = 480 000 samples) | Force-emit chunk regardless of VAD state           |
| Meeting Mode stopped                                          | Emit any remaining buffered audio as a final chunk |

The 30s ceiling aligns with Whisper's training window and keeps memory bounded.

---

## 4. Backend Changes

### 4.1 `src-tauri/src/audio_toolkit/audio/recorder.rs`

Add an optional segment callback to `AudioRecorder`:

```rust
segment_callback: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
segment_buffer: Vec<f32>,          // accumulates voice samples between VAD events
max_segment_samples: usize,        // = 16_000 * 30
```

**Builder method:**

```rust
pub fn with_segment_callback(
    mut self,
    cb: impl Fn(Vec<f32>) + Send + Sync + 'static,
    max_samples: usize,
) -> Self
```

**Trigger logic (called on each audio frame):**

- When VAD transitions `voice → silence`: call callback with `segment_buffer`, clear buffer.
- When `segment_buffer.len() >= max_segment_samples`: call callback with buffer, clear buffer (forced boundary).
- When VAD is `voice`: accumulate samples into `segment_buffer`.
- Normal (non-meeting) mode: `segment_callback` is `None`, existing behavior unchanged.

### 4.2 `src-tauri/src/managers/audio.rs`

**New fields (behind `#[cfg(target_os = "windows")]`, inside the meeting mode block):**

```rust
meeting_chunk_tx: Arc<Mutex<Option<mpsc::SyncSender<Vec<f32>>>>>,
meeting_worker_handle: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
```

**`start_meeting_mode` changes:**

1. Create `mpsc::sync_channel::<Vec<f32>>(8)` — bounded to 8 pending chunks to avoid unbounded queuing.
2. Store `tx` in `meeting_chunk_tx`.
3. Spawn worker thread (see §4.3).
4. Register `segment_callback` on the `AudioRecorder` that sends to `tx`.

**`stop_meeting_mode` changes:**

1. Drop `meeting_chunk_tx` (closes the channel sender).
2. Join `meeting_worker_handle` — waits for worker to drain the queue and finish.
3. File is already fully written; emit the "Transcript saved" notification as before.
4. Clear `meeting_chunk_tx` and `meeting_worker_handle`.

### 4.3 Meeting Mode Worker Thread

```rust
thread::spawn(move || {
    let file = OpenOptions::new()
        .create(true).append(true)
        .open(&transcript_path)?;
    let mut writer = BufWriter::new(file);
    let mut index: u32 = 0;

    while let Ok(chunk) = rx.recv() {
        match tm.transcribe(chunk) {
            Ok(text) if !text.is_empty() => {
                let timestamp = chrono::Local::now();
                let line = format!("[{}] {}\n", timestamp.format("%H:%M:%S"), text);

                // Progressive file write
                let _ = writer.write_all(line.as_bytes());
                let _ = writer.flush();

                // Update in-memory accumulator
                meeting_segments.lock().unwrap()
                    .push((timestamp, text.clone()));

                // Emit Tauri event
                let _ = app_handle.emit("meeting-segment-transcribed", MeetingSegmentEvent {
                    text,
                    timestamp: format!("[{}]", timestamp.format("%H:%M:%S")),
                    index,
                });
                index += 1;
            }
            Ok(_) => {} // empty transcription, skip
            Err(e) => error!("Chunk transcription failed: {}", e),
        }
    }
    // rx closed — worker exits cleanly
});
```

### 4.4 New Event Type

```rust
#[derive(Clone, Serialize, Type)]
pub struct MeetingSegmentEvent {
    pub text: String,
    pub timestamp: String,   // "[HH:MM:SS]"
    pub index: u32,
}
```

Emitted on: `"meeting-segment-transcribed"`

---

## 5. Frontend Changes

### 5.1 `src/overlay/RecordingOverlay.tsx`

Add state and event listener:

```typescript
const [lastSegment, setLastSegment] = useState<string | null>(null);

useEffect(() => {
  const unlisten = listen<MeetingSegmentEvent>(
    "meeting-segment-transcribed",
    (event) => {
      setLastSegment(event.payload.text);
    },
  );
  return () => {
    unlisten.then((f) => f());
  };
}, []);

// Auto-clear after 3s
useEffect(() => {
  if (!lastSegment) return;
  const timer = setTimeout(() => setLastSegment(null), 3000);
  return () => clearTimeout(timer);
}, [lastSegment]);
```

**UI:** when `lastSegment` is set, render a small text element below the existing overlay content with fade-in / fade-out CSS transition. If a new segment arrives before 3s, the state update resets the timer, replacing the text.

### 5.2 No changes to `MeetingModeToggle.tsx`

The `start_meeting_mode` / `stop_meeting_mode` command signatures are unchanged. The toggle behavior is identical from the frontend's perspective.

---

## 6. Error Handling

| Scenario                               | Behavior                                                                                                                              |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| Worker queue full (8 chunks backed up) | `SyncSender::try_send` returns `Full` — chunk is dropped with a warning log. Prevents memory growth if model is very slow.            |
| Chunk transcription error              | Log error, skip segment, continue worker loop                                                                                         |
| File write fails mid-session           | Log error, continue transcription — in-memory accumulator still works; user gets the file path notification with whatever was written |
| App crash mid-session                  | All segments written before the crash are already on disk (progressive append)                                                        |
| Meeting stopped with 0 segments        | No file written, no notification (unchanged from current behavior)                                                                    |

---

## 7. Files Changed

| File                                            | Type of change                                               |
| ----------------------------------------------- | ------------------------------------------------------------ |
| `src-tauri/src/audio_toolkit/audio/recorder.rs` | Add `segment_callback`, `segment_buffer`, forced chunk logic |
| `src-tauri/src/managers/audio.rs`               | Worker thread, channel, progressive file write               |
| `src-tauri/src/managers/audio.rs`               | New `MeetingSegmentEvent` struct                             |
| `src/overlay/RecordingOverlay.tsx`              | Listen for event, show last segment with fade                |
| `src/bindings.ts`                               | Auto-regenerated (new event type via tauri-specta)           |

---

## 8. Out-of-Scope Decisions (deferred)

- Configurable max segment duration (hardcoded at 30s)
- Streaming in normal (non-meeting) recording mode
- Token-level streaming via `MoonshineStreaming`
- macOS/Linux meeting mode support (blocked on loopback capture, per existing spec)
- Displaying full running transcript in a dedicated window
