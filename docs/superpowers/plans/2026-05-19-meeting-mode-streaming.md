# Meeting Mode Streaming Transcription — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Transcribe each speech segment progressively during a Meeting Mode recording, writing to the `.txt` file as each segment completes and showing it briefly in the recording overlay.

**Architecture:** Add a `SetMeetingTx` command to `AudioRecorder` that activates VAD-boundary detection inside `run_consumer`. When meeting mode is on and the user starts a recording via shortcut, each VAD-detected speech segment (or 30s forced chunk) is sent through a `SyncSender` to a worker thread that transcribes, appends to the transcript file, and emits a Tauri event. On stop, the recorder returns an empty Vec so `actions.rs` skips its normal transcription pass.

**Tech Stack:** Rust (std::sync::mpsc, std::io::BufWriter, std::thread), Tauri Emitter, React useState/useEffect/useRef, CSS transitions.

---

## File Map

| File | Change |
|---|---|
| `src-tauri/src/audio_toolkit/audio/recorder.rs` | New `Cmd::SetMeetingTx`, `set_meeting_tx()` method, VAD boundary detection in `run_consumer` |
| `src-tauri/src/managers/audio.rs` | `MeetingSegmentEvent`, new fields, streaming pipeline in `start_meeting_mode` / `stop_meeting_mode` |
| `src/overlay/RecordingOverlay.tsx` | Event listener, `lastSegment` state, fade display |
| `src/overlay/RecordingOverlay.css` | `.segment-preview` fade animation |

---

## Task 1: Extend `AudioRecorder` with `SetMeetingTx` command

**Files:**
- Modify: `src-tauri/src/audio_toolkit/audio/recorder.rs`

- [ ] **Step 1: Add the new `Cmd` variant**

In `recorder.rs`, find the `enum Cmd` block (line 22) and add the new variant:

```rust
enum Cmd {
    Start,
    Stop(mpsc::Sender<Vec<f32>>),
    Shutdown,
    SetMeetingTx(Option<mpsc::SyncSender<Vec<f32>>>), // enable/disable VAD chunk dispatch
}
```

- [ ] **Step 2: Add `set_meeting_tx` method to `AudioRecorder`**

After the `with_level_callback` method (~line 63), add:

```rust
/// Enable or disable VAD-driven segment dispatch for meeting mode.
/// Pass `Some(tx)` to activate; `None` to deactivate.
pub fn set_meeting_tx(&self, tx: Option<mpsc::SyncSender<Vec<f32>>>) {
    if let Some(cmd_tx) = &self.cmd_tx {
        let _ = cmd_tx.send(Cmd::SetMeetingTx(tx));
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run:
```bash
cargo check --manifest-path src-tauri/Cargo.toml
```
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/audio_toolkit/audio/recorder.rs
git commit -m "feat(recorder): add SetMeetingTx command for VAD segment dispatch"
```

---

## Task 2: VAD boundary detection in `run_consumer`

**Files:**
- Modify: `src-tauri/src/audio_toolkit/audio/recorder.rs`

- [ ] **Step 1: Add state variables at the top of `run_consumer`**

Find `run_consumer` (~line 395). After the `let mut recording = false;` line, add:

```rust
// Meeting mode: track VAD transitions and accumulate speech chunks
let mut meeting_tx: Option<mpsc::SyncSender<Vec<f32>>> = None;
let mut speech_buffer: Vec<f32> = Vec::new();
let mut was_speech: bool = false;
const MAX_SEGMENT_SAMPLES: usize = 16_000 * 30; // 30s at 16kHz
```

- [ ] **Step 2: Replace `handle_frame` call with inline VAD tracking**

The current code uses `handle_frame` as a closure argument to `frame_resampler.push`. We need to track VAD results outside that closure to detect speech→noise transitions.

Replace the existing block:
```rust
frame_resampler.push(&raw, &mut |frame: &[f32]| {
    handle_frame(frame, recording, &vad, &mut processed_samples)
});
```

With:
```rust
frame_resampler.push(&raw, &mut |frame: &[f32]| {
    if !recording {
        return;
    }

    let vad_result = if let Some(vad_arc) = &vad {
        let mut det = vad_arc.lock().unwrap();
        det.push_frame(frame).unwrap_or(VadFrame::Speech(frame))
    } else {
        VadFrame::Speech(frame)
    };

    match vad_result {
        VadFrame::Speech(buf) => {
            if meeting_tx.is_some() {
                // Meeting mode: accumulate in speech_buffer, not processed_samples
                speech_buffer.extend_from_slice(buf);
                was_speech = true;
                // Force boundary every 30s to bound memory and guarantee progress
                if speech_buffer.len() >= MAX_SEGMENT_SAMPLES {
                    if let Some(ref tx) = meeting_tx {
                        let chunk = std::mem::take(&mut speech_buffer);
                        if tx.try_send(chunk).is_err() {
                            log::warn!("Meeting segment queue full, dropping 30s chunk");
                        }
                    }
                    was_speech = false;
                }
            } else {
                // Normal mode: accumulate in processed_samples
                processed_samples.extend_from_slice(buf);
            }
        }
        VadFrame::Noise => {
            if meeting_tx.is_some() {
                // Detect speech→noise transition: emit the completed segment
                if was_speech && !speech_buffer.is_empty() {
                    if let Some(ref tx) = meeting_tx {
                        let chunk = std::mem::take(&mut speech_buffer);
                        if tx.try_send(chunk).is_err() {
                            log::warn!("Meeting segment queue full, dropping VAD-boundary chunk");
                        }
                    }
                }
                was_speech = false;
            }
        }
    }
});
```

- [ ] **Step 3: Handle `Cmd::SetMeetingTx` in the command loop**

Find the `while let Ok(cmd) = cmd_rx.try_recv()` block and add a new arm after `Cmd::Start`:

```rust
Cmd::SetMeetingTx(tx) => {
    meeting_tx = tx;
    speech_buffer.clear();
    was_speech = false;
}
```

- [ ] **Step 4: Update `frame_resampler.finish` inside `Cmd::Stop` for meeting mode**

Find the existing `frame_resampler.finish(...)` call inside `Cmd::Stop`. Replace it with:

```rust
frame_resampler.finish(&mut |frame: &[f32]| {
    if meeting_tx.is_some() {
        // Meeting mode: accumulate tail frames directly (VAD not needed at stop)
        speech_buffer.extend_from_slice(frame);
        was_speech = true;
    } else {
        handle_frame(frame, true, &vad, &mut processed_samples);
    }
});
```

- [ ] **Step 5: Flush remainder on `Cmd::Stop` when meeting mode is active**

After the `frame_resampler.finish(...)` call and before `let _ = reply_tx.send(...)`, add:

```rust
// Meeting mode: flush any buffered tail, return empty Vec to caller
// so actions.rs skips its normal transcription pass.
if let Some(ref tx) = meeting_tx {
    if !speech_buffer.is_empty() {
        let chunk = std::mem::take(&mut speech_buffer);
        if tx.try_send(chunk).is_err() {
            log::warn!("Meeting segment queue full, dropping stop-flush chunk");
        }
    }
    was_speech = false;
    // Signal to caller: meeting mode consumed this recording
    let _ = reply_tx.send(Vec::new());
} else {
    let _ = reply_tx.send(std::mem::take(&mut processed_samples));
}
```

Remove the original `let _ = reply_tx.send(std::mem::take(&mut processed_samples));` line that was there before.

- [ ] **Step 6: Clear speech state on `Cmd::Start`**

Find `Cmd::Start =>` and add after `processed_samples.clear();`:

```rust
speech_buffer.clear();
was_speech = false;
```

- [ ] **Step 7: Remove orphaned `handle_frame` function**

`handle_frame` is now only used in the non-meeting `frame_resampler.finish` path. Keep it — it's still valid for the else branch. No action needed.

- [ ] **Step 8: Write unit test for VAD transition logic**

At the bottom of `recorder.rs`, inside the `#[cfg(test)] mod tests` block, add:

```rust
#[test]
fn speech_to_noise_transition_emits_chunk() {
    use std::sync::mpsc;

    // Simulate: run_consumer state variables
    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(8);
    let mut meeting_tx: Option<mpsc::SyncSender<Vec<f32>>> = Some(tx);
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut was_speech: bool = false;

    // Simulate speech frame arriving
    let speech_samples = vec![0.1f32; 100];
    if meeting_tx.is_some() {
        speech_buffer.extend_from_slice(&speech_samples);
        was_speech = true;
    }

    // Simulate noise frame arriving (transition)
    if meeting_tx.is_some() && was_speech && !speech_buffer.is_empty() {
        if let Some(ref t) = meeting_tx {
            let chunk = std::mem::take(&mut speech_buffer);
            t.try_send(chunk).unwrap();
        }
        was_speech = false;
    }

    let received = rx.try_recv().unwrap();
    assert_eq!(received.len(), 100);
    assert!(speech_buffer.is_empty());
    assert!(!was_speech);
}

#[test]
fn forced_30s_boundary_emits_chunk() {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::sync_channel::<Vec<f32>>(8);
    let mut meeting_tx: Option<mpsc::SyncSender<Vec<f32>>> = Some(tx);
    let mut speech_buffer: Vec<f32> = Vec::new();
    let mut was_speech: bool = false;
    const MAX_SEGMENT_SAMPLES: usize = 16_000 * 30;

    // Fill buffer beyond 30s limit
    let big_chunk = vec![0.2f32; MAX_SEGMENT_SAMPLES + 100];
    if meeting_tx.is_some() {
        speech_buffer.extend_from_slice(&big_chunk);
        was_speech = true;
        if speech_buffer.len() >= MAX_SEGMENT_SAMPLES {
            if let Some(ref t) = meeting_tx {
                let chunk = std::mem::take(&mut speech_buffer);
                t.try_send(chunk).unwrap();
            }
            was_speech = false;
        }
    }

    let received = rx.try_recv().unwrap();
    assert!(received.len() >= MAX_SEGMENT_SAMPLES);
    assert!(speech_buffer.is_empty());
    assert!(!was_speech);
}
```

- [ ] **Step 9: Run tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml audio_toolkit::audio::recorder::tests
```

Expected:
```
test audio_toolkit::audio::recorder::tests::speech_to_noise_transition_emits_chunk ... ok
test audio_toolkit::audio::recorder::tests::forced_30s_boundary_emits_chunk ... ok
test audio_toolkit::audio::recorder::tests::detects_access_is_denied ... ok
test audio_toolkit::audio::recorder::tests::detects_permission_denied ... ok
test audio_toolkit::audio::recorder::tests::detects_windows_error_code ... ok
test audio_toolkit::audio::recorder::tests::does_not_match_unrelated_errors ... ok
test audio_toolkit::audio::recorder::tests::detects_no_input_device ... ok
test audio_toolkit::audio::recorder::tests::detects_coreaudio_config_error ... ok
test audio_toolkit::audio::recorder::tests::does_not_match_other_errors_for_no_device ... ok
```

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/audio_toolkit/audio/recorder.rs
git commit -m "feat(recorder): VAD boundary detection dispatches meeting mode segments"
```

---

## Task 3: Add `MeetingSegmentEvent` and new fields to `AudioRecordingManager`

**Files:**
- Modify: `src-tauri/src/managers/audio.rs`

- [ ] **Step 1: Add imports**

At the top of `audio.rs`, add to the existing `use` block:

```rust
use std::io::{BufWriter, Write};
use std::fs::OpenOptions;
```

- [ ] **Step 2: Add `MeetingSegmentEvent` struct**

After the existing imports, before the first `const`, add:

```rust
#[cfg(target_os = "windows")]
#[derive(Clone, serde::Serialize)]
pub struct MeetingSegmentEvent {
    pub text: String,
    pub timestamp: String, // "[HH:MM:SS]"
    pub index: u32,
}
```

- [ ] **Step 3: Add new fields to `AudioRecordingManager`**

Inside the `pub struct AudioRecordingManager` block, inside the `#[cfg(target_os = "windows")]` section, add three new fields after the existing meeting mode fields:

```rust
#[cfg(target_os = "windows")]
meeting_chunk_tx: Arc<Mutex<Option<mpsc::SyncSender<Vec<f32>>>>>,
#[cfg(target_os = "windows")]
meeting_worker_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
#[cfg(target_os = "windows")]
transcript_path: Arc<Mutex<Option<std::path::PathBuf>>>,
```

- [ ] **Step 4: Initialize the new fields in `AudioRecordingManager::new()`**

Find the `let manager = Self {` block. Inside the `#[cfg(target_os = "windows")]` section, add after `meeting_start_time`:

```rust
#[cfg(target_os = "windows")]
meeting_chunk_tx: Arc::new(Mutex::new(None)),
#[cfg(target_os = "windows")]
meeting_worker_handle: Arc::new(Mutex::new(None)),
#[cfg(target_os = "windows")]
transcript_path: Arc::new(Mutex::new(None)),
```

- [ ] **Step 5: Verify compilation**

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/managers/audio.rs
git commit -m "feat(audio): add MeetingSegmentEvent and streaming pipeline fields"
```

---

## Task 4: Streaming pipeline in `start_meeting_mode`

**Files:**
- Modify: `src-tauri/src/managers/audio.rs`

- [ ] **Step 1: Add `resolve_transcript_path` helper**

Before `write_meeting_transcript` (near the bottom of the file), add a new private helper:

```rust
#[cfg(target_os = "windows")]
fn resolve_transcript_path(
    app: &tauri::AppHandle,
    start: chrono::DateTime<chrono::Local>,
) -> Result<std::path::PathBuf, anyhow::Error> {
    let docs_dir = app
        .path()
        .document_dir()
        .map_err(|e| anyhow::anyhow!("Failed to resolve Documents directory: {e}"))?;
    let meetings_dir = docs_dir.join("Handy").join("meetings");
    std::fs::create_dir_all(&meetings_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create meetings directory: {e}"))?;
    let filename = format!("meeting_{}.txt", start.format("%Y-%m-%d_%H-%M-%S"));
    Ok(meetings_dir.join(filename))
}
```

- [ ] **Step 2: Rewrite `start_meeting_mode`**

Replace the existing `start_meeting_mode` implementation with:

```rust
#[cfg(target_os = "windows")]
pub fn start_meeting_mode(&self) -> Result<(), anyhow::Error> {
    if *self.meeting_mode.lock().unwrap() {
        return Ok(());
    }

    // Open loopback recorder (unchanged from before)
    let mut new_loopback = crate::audio_toolkit::LoopbackRecorder::new();
    match new_loopback.open() {
        Ok(()) => {
            *self.loopback_recorder.lock().unwrap() = Some(new_loopback);
            info!("Meeting mode: loopback recorder opened");
        }
        Err(e) => {
            log::warn!("Meeting mode: failed to open loopback recorder — {e}; proceeding mic-only");
            *self.loopback_recorder.lock().unwrap() = None;
        }
    }

    let start_time = chrono::Local::now();
    self.meeting_segments.lock().unwrap().clear();
    *self.meeting_start_time.lock().unwrap() = Some(start_time);

    // Create transcript file and write header immediately
    let path = resolve_transcript_path(&self.app_handle, start_time)?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("Failed to create transcript file: {e}"))?;
        writeln!(file, "Meeting: {}", start_time.format("%Y-%m-%d %H:%M"))?;
        writeln!(file)?; // blank line before segments
    }
    *self.transcript_path.lock().unwrap() = Some(path.clone());

    // Create bounded channel for audio chunks (8 slots = ~4 minutes of buffered work)
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(8);

    // Register the segment callback on the recorder
    {
        let recorder = self.recorder.lock().unwrap();
        if let Some(r) = recorder.as_ref() {
            r.set_meeting_tx(Some(tx.clone()));
        }
    }
    *self.meeting_chunk_tx.lock().unwrap() = Some(tx);

    // Spawn worker thread: transcribes chunks and writes to file progressively
    let app_handle = self.app_handle.clone();
    let segments_arc = self.meeting_segments.clone();
    let worker = std::thread::spawn(move || {
        let tm = match app_handle.try_state::<Arc<crate::managers::transcription::TranscriptionManager>>() {
            Some(s) => s.inner().clone(),
            None => {
                error!("Meeting mode worker: TranscriptionManager not available");
                return;
            }
        };

        let file = match OpenOptions::new().append(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                error!("Meeting mode worker: failed to open transcript file: {e}");
                return;
            }
        };
        let mut writer = BufWriter::new(file);
        let mut index: u32 = 0;

        while let Ok(chunk) = rx.recv() {
            match tm.transcribe(chunk) {
                Ok(text) if !text.is_empty() => {
                    let ts = chrono::Local::now();
                    let line = format!("[{}] {}\n", ts.format("%H:%M:%S"), text);

                    // Write to file immediately (crash-resilient)
                    if let Err(e) = writer.write_all(line.as_bytes()) {
                        error!("Meeting mode worker: failed to write segment: {e}");
                    } else {
                        let _ = writer.flush();
                    }

                    // Update in-memory accumulator (for stop_meeting_mode duration calc)
                    segments_arc.lock().unwrap().push((ts, text.clone()));

                    // Notify frontend
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
                Ok(_) => {} // empty result — silence or noise, skip
                Err(e) => error!("Meeting mode worker: transcription error: {e}"),
            }
        }
        // rx closed — meeting mode stopped, worker exits cleanly
        debug!("Meeting mode worker thread finished");
    });

    *self.meeting_worker_handle.lock().unwrap() = Some(worker);
    *self.meeting_mode.lock().unwrap() = true;
    Ok(())
}
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```
Expected: no errors (may need to add `use crate::managers::transcription::TranscriptionManager;` at top of audio.rs if the compiler asks).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/managers/audio.rs
git commit -m "feat(audio): streaming pipeline in start_meeting_mode"
```

---

## Task 5: Drain and finalize in `stop_meeting_mode`

**Files:**
- Modify: `src-tauri/src/managers/audio.rs`

- [ ] **Step 1: Rewrite `stop_meeting_mode`**

Replace the existing `stop_meeting_mode` implementation with:

```rust
#[cfg(target_os = "windows")]
pub fn stop_meeting_mode(&self) -> Result<Option<String>, anyhow::Error> {
    let mut meeting = self.meeting_mode.lock().unwrap();
    if !*meeting {
        return Ok(None);
    }
    *meeting = false;
    drop(meeting);

    // Stop loopback recorder (unchanged)
    {
        let mut guard = self.loopback_recorder.lock().unwrap();
        if let Some(ref mut rec) = *guard {
            rec.close();
        }
        *guard = None;
    }

    // Disconnect the segment callback on the recorder (stops new chunks arriving)
    {
        let recorder = self.recorder.lock().unwrap();
        if let Some(r) = recorder.as_ref() {
            r.set_meeting_tx(None);
        }
    }

    // Close the sender — this signals the worker to drain and exit
    *self.meeting_chunk_tx.lock().unwrap() = None;

    // Wait for the worker to finish (drains queue, writes all pending segments)
    if let Some(handle) = self.meeting_worker_handle.lock().unwrap().take() {
        if let Err(e) = handle.join() {
            error!("Meeting mode worker panicked: {:?}", e);
        }
    }

    // Retrieve state for finalization
    let segments = self.meeting_segments.lock().unwrap().clone();
    let start_time = self.meeting_start_time.lock().unwrap().take();
    let path = self.transcript_path.lock().unwrap().take();

    // Nothing transcribed: don't report a file path
    if segments.is_empty() {
        return Ok(None);
    }

    let path = match path {
        Some(p) => p,
        None => return Ok(None),
    };

    // Append duration footer to the file
    let start = start_time.unwrap_or_else(chrono::Local::now);
    let end = chrono::Local::now();
    let duration = end.signed_duration_since(start);
    let hours = duration.num_hours().abs();
    let minutes = (duration.num_minutes() % 60).abs();
    let seconds = (duration.num_seconds() % 60).abs();

    if let Ok(mut file) = OpenOptions::new().append(true).open(&path) {
        let _ = writeln!(file);
        let _ = writeln!(file, "Duration: {:02}:{:02}:{:02}", hours, minutes, seconds);
    }

    info!("Meeting transcript saved to: {}", path.display());
    Ok(Some(path.to_string_lossy().to_string()))
}
```

- [ ] **Step 2: Remove `write_meeting_transcript` (no longer used)**

Find and delete the entire `fn write_meeting_transcript(...)` function at the bottom of the file (it was the batch writer). Replace any remaining call sites with `Ok(None)` if any exist outside what we've already replaced (search with grep first).

```bash
grep -n "write_meeting_transcript" src-tauri/src/managers/audio.rs
```

If the output is empty, the function was only called from `stop_meeting_mode` and is now safe to delete.

- [ ] **Step 3: Verify compilation**

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```
Expected: no errors.

- [ ] **Step 4: Run all Rust tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```
Expected: all tests pass, no failures.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/managers/audio.rs
git commit -m "feat(audio): drain worker and write duration footer in stop_meeting_mode"
```

---

## Task 6: Overlay segment feedback (frontend)

**Files:**
- Modify: `src/overlay/RecordingOverlay.tsx`
- Modify: `src/overlay/RecordingOverlay.css`

- [ ] **Step 1: Add `lastSegment` state and event listener to `RecordingOverlay.tsx`**

Open `src/overlay/RecordingOverlay.tsx`. Add a `useRef` for the timer and a `useState` for the last segment. After the existing `const direction = ...` line, add:

```tsx
const [lastSegment, setLastSegment] = useState<string | null>(null);
const segmentTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
```

- [ ] **Step 2: Add event listener inside `setupEventListeners`**

Inside the `setupEventListeners` async function, after the `unlistenLevel` declaration, add:

```tsx
const unlistenSegment = await listen<{
  text: string;
  timestamp: string;
  index: number;
}>("meeting-segment-transcribed", (event) => {
  setLastSegment(event.payload.text);
  if (segmentTimerRef.current) clearTimeout(segmentTimerRef.current);
  segmentTimerRef.current = setTimeout(() => setLastSegment(null), 3000);
});
```

Update the cleanup return to also call `unlistenSegment()`:

```tsx
return () => {
  unlistenShow();
  unlistenHide();
  unlistenLevel();
  unlistenSegment();
  if (segmentTimerRef.current) clearTimeout(segmentTimerRef.current);
};
```

- [ ] **Step 3: Add segment display to the JSX**

Find the `<div className="overlay-middle">` block. After the closing `</div>` of `overlay-middle` and before `<div className="overlay-right">`, add:

```tsx
{lastSegment && (
  <div className="segment-preview">{lastSegment}</div>
)}
```

This renders outside the 3-column grid, below the existing content.

- [ ] **Step 4: Update overlay container to allow overflow for segment text**

The overlay is currently fixed at `height: 36px`. We need to allow it to grow when a segment is showing. In `RecordingOverlay.tsx` find the outer `<div className="recording-overlay ...">` and change its className logic:

```tsx
<div
  dir={direction}
  className={`recording-overlay ${isVisible ? "fade-in" : ""} ${lastSegment ? "has-segment" : ""}`}
>
```

- [ ] **Step 5: Add CSS for segment preview**

Open `src/overlay/RecordingOverlay.css`. Add at the end:

```css
.recording-overlay.has-segment {
  height: auto;
  min-height: 36px;
}

.segment-preview {
  grid-column: 1 / -1;
  color: #ffffffcc;
  font-size: 10px;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  padding: 2px 4px 4px 4px;
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  animation: segment-fade-in 200ms ease-out;
}

@keyframes segment-fade-in {
  from { opacity: 0; transform: translateY(-4px); }
  to   { opacity: 1; transform: translateY(0); }
}
```

- [ ] **Step 6: Run frontend type-check**

```bash
bun run build
```
Expected: no TypeScript errors, build succeeds.

- [ ] **Step 7: Commit**

```bash
git add src/overlay/RecordingOverlay.tsx src/overlay/RecordingOverlay.css
git commit -m "feat(overlay): show last meeting segment briefly with fade animation"
```

---

## Task 7: Push to origin

- [ ] **Step 1: Final check — run all tests**

```bash
cargo test --manifest-path src-tauri/Cargo.toml
bun run build
```

Expected: all Rust tests pass, TypeScript build succeeds.

- [ ] **Step 2: Push**

```bash
git push origin main
```

---

## Manual Verification Steps

These require running the app (`bun run tauri dev`) and cannot be automated:

1. **VAD chunking during recording:** Toggle meeting mode on → press shortcut → speak a sentence → pause → the overlay briefly shows the transcribed text.

2. **30s forced boundary:** Hold the shortcut and speak continuously for >30s — the overlay should show partial segments before you stop.

3. **Remainder on stop:** Speak, then immediately press the shortcut to stop mid-sentence — the last partial segment should still appear in the overlay and file.

4. **File written progressively:** Open the `.txt` file in a text editor while a meeting is running — new lines should appear as segments are transcribed without waiting for stop.

5. **Duration footer:** After stopping meeting mode, open the transcript file — the last line should be `Duration: HH:MM:SS`.

6. **No double transcription:** Confirm the clipboard is NOT updated on shortcut stop during meeting mode (only overlay feedback via events).
