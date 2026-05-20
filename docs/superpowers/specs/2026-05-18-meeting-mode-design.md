# Meeting Mode — Design Spec

**Date:** 2026-05-18
**Status:** Approved

## Summary

Add a "Meeting Mode" to Handy that captures both microphone and system audio simultaneously on Windows, feeding a mixed stream into the existing transcription pipeline. Transcription continues to work as today (clipboard/paste per segment), and at the end of the session the full transcript is saved as a `.txt` file with timestamps.

---

## 1. Problem

Handy currently transcribes only microphone input. During video meetings (Teams, Google Meet, Zoom), the user also needs to capture what other participants say through the computer's speakers. The goal is a full, two-sided meeting transcript.

---

## 2. Scope

- **In scope:** Windows only; WASAPI loopback capture; dual-stream mixing; Meeting Mode toggle in UI; `.txt` file export on session end; conflict resolution with `mute_while_recording`.
- **Out of scope:** macOS/Linux support (no loopback API abstraction in this iteration); speaker diarization (who said what); cloud transcription changes; per-app audio routing.

---

## 3. Architecture

### 3.1 Audio Pipeline in Meeting Mode

```
Microphone  → CPAL input stream  → Resampler → f32 16kHz mono ─┐
                                                                  ├→ StreamMixer → VAD → Transcription → Clipboard + Accumulator
System audio → WASAPI Loopback   → Resampler → f32 16kHz mono ─┘
```

The VAD, transcription engine, and clipboard/paste behavior are **unchanged**. Meeting Mode only adds an upstream mixing step.

### 3.2 New Rust Modules

**`src-tauri/src/audio_toolkit/audio/loopback.rs`** — `LoopbackRecorder`

- Uses the `windows` crate (already a dependency) with added features: `Win32_Media_Audio`, `Win32_System_Com`.
- Enumerates the default audio render endpoint via `IMMDeviceEnumerator`.
- Activates `IAudioClient` on that endpoint with `AUDCLNT_STREAMFLAGS_LOOPBACK`.
- Reads samples from `IAudioCaptureClient`, converts to f32 mono, and resamples to 16 kHz using the existing `FrameResampler`.
- Runs on a dedicated thread; pushes samples into a `crossbeam` channel, mirroring the pattern of `AudioRecorder`.

**`src-tauri/src/audio_toolkit/audio/mixer.rs`** — `StreamMixer`

- Receives two `Receiver<Vec<f32>>` channels (mic and loopback), both at 16 kHz mono.
- Combines them: `out = (mic_sample + loopback_sample).clamp(-1.0, 1.0)`.
- Emits a single `Vec<f32>` stream to the VAD input, replacing the mic-only stream.

### 3.3 Changes to Existing Files

| File                | Change                                                                                                                                                                              |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`        | Add `Win32_Media_Audio`, `Win32_System_Com` features to `windows` crate                                                                                                             |
| `managers/audio.rs` | Add `meeting_mode: bool` state; when true, instantiate `LoopbackRecorder` + `StreamMixer` alongside the existing `AudioRecorder`; accumulate transcription segments with timestamps |
| `managers/audio.rs` | Auto-disable `mute_while_recording` when Meeting Mode is active (muting output would silence the loopback)                                                                          |

### 3.4 Transcription Accumulator

`AudioRecordingManager` gains a `Vec<(DateTime<Local>, String)>` buffer. Each transcription segment produced during a meeting session is appended with its wall-clock timestamp. The buffer is cleared on `start_meeting_mode` and flushed to disk on `stop_meeting_mode`.

---

## 4. Tauri Commands

| Command                  | Description                                                                                     |
| ------------------------ | ----------------------------------------------------------------------------------------------- |
| `start_meeting_mode`     | Starts `LoopbackRecorder` + `StreamMixer`, resets accumulator, returns `Ok(())` or error string |
| `stop_meeting_mode`      | Stops extra streams, writes transcript file, returns the file path as `String`                  |
| `get_meeting_mode_state` | Returns `bool` — used by frontend to sync UI on app restart                                     |

---

## 5. File Export

**Location:** `{Documents}/Handy/meetings/meeting_YYYY-MM-DD_HH-MM-SS.txt`

The `meetings/` directory is created automatically if it does not exist.

**Format:**

```
Meeting: 2026-05-18 14:30
Duration: 00:42:17

[14:30:05] Hello everyone, let's get started
[14:30:12] Thanks for joining, let me share my screen
[14:31:03] ...
```

No file is written if the session produced zero transcription segments.

---

## 6. Frontend (React/TypeScript)

### 6.1 New Component: `MeetingModeToggle.tsx`

- Placed in `src/components/settings/` alongside `AlwaysOnMicrophone.tsx`.
- Only rendered on Windows. The backend exposes a `is_meeting_mode_supported` Tauri command that returns `true` only on `cfg(target_os = "windows")`; the component hides itself when this returns `false`.
- Toggle calls `start_meeting_mode` / `stop_meeting_mode` via Tauri invoke.
- On stop: displays a dismissible notification — "Transcript saved: `<path>`" — clicking it opens the containing folder via Tauri's `shell.open`.

### 6.2 Active Indicator

When Meeting Mode is active, the main window header shows a badge or color change to make the mode visually distinct from normal recording.

---

## 7. Error Handling

| Scenario                                      | Behavior                                                                                               |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| WASAPI access denied (enterprise policy)      | Meeting Mode activates mic-only; toast warning: "System audio unavailable — capturing microphone only" |
| No default output device                      | Same as above                                                                                          |
| `Documents/Handy/meetings/` cannot be created | Toast error; transcript not saved, but clipboard behavior continues normally                           |
| Meeting stopped with 0 segments               | No file written; no notification shown                                                                 |

---

## 8. Out-of-Scope Decisions (deferred)

- macOS aggregate device or PipeWire monitor source support
- Speaker diarization / labeling who said what
- Per-application audio capture (capturing only Teams audio, not all system audio)
- `.md` or `.srt` export formats (only `.txt` in this iteration)
- Configurable save location (fixed to Documents/Handy/meetings/ for now)
