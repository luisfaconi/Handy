# Meeting Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Windows-only "Meeting Mode" toggle that captures microphone + system audio simultaneously, mixes them, transcribes both sides, and saves the full session transcript to a `.txt` file on stop.

**Architecture:** A new `LoopbackRecorder` (WASAPI loopback on a dedicated thread) runs alongside the existing `AudioRecorder`. Both produce `Vec<f32>` at 16 kHz mono; `mix_samples()` combines them before Whisper. `AudioRecordingManager` gains meeting-mode state, an accumulator buffer, and three new Tauri commands. A `MeetingModeToggle` React component shows only on Windows.

**Tech Stack:** Rust/Tauri 2 backend, `windows = "0.61"` crate (WASAPI), React/TypeScript frontend, `FrameResampler` (existing), `chrono` (existing), i18next.

---

## File Map

| Action | Path |
|---|---|
| **Create** | `src-tauri/src/audio_toolkit/audio/loopback.rs` |
| **Create** | `src-tauri/src/audio_toolkit/audio/mixer.rs` |
| **Create** | `src/components/settings/MeetingModeToggle.tsx` |
| **Modify** | `src-tauri/Cargo.toml` |
| **Modify** | `src-tauri/src/audio_toolkit/audio/mod.rs` |
| **Modify** | `src-tauri/src/managers/audio.rs` |
| **Modify** | `src-tauri/src/commands/audio.rs` |
| **Modify** | `src-tauri/src/lib.rs` |
| **Modify** | `src-tauri/src/actions.rs` |
| **Modify** | `src/i18n/locales/en/translation.json` |
| **Modify** | `src/components/settings/general/GeneralSettings.tsx` |

---

## Task 1: Add WASAPI features to Cargo.toml

**Files:**
- Modify: `src-tauri/Cargo.toml`

- [ ] **Step 1.1: Add `Win32_Media_Audio` feature to the windows crate**

In `src-tauri/Cargo.toml`, find the `[target.'cfg(windows)'.dependencies]` section and update the `windows` entry to include `Win32_Media_Audio`:

```toml
[target.'cfg(windows)'.dependencies]
transcribe-rs = { version = "0.3.3", features = ["whisper-vulkan", "ort-directml"] }
windows = { version = "0.61.3", features = [
  "Win32_Media_Audio_Endpoints",
  "Win32_Media_Audio",
  "Win32_System_Com_StructuredStorage",
  "Win32_System_Variant",
  "Win32_Foundation",
  "Win32_UI_WindowsAndMessaging",
] }
winreg = "0.55"
```

- [ ] **Step 1.2: Verify the feature compiles**

```powershell
cd src-tauri; cargo check --target x86_64-pc-windows-msvc 2>&1 | Select-String "error"
```

Expected: no `error` lines (warnings are OK).

- [ ] **Step 1.3: Commit**

```bash
git add src-tauri/Cargo.toml
git commit -m "chore: add Win32_Media_Audio feature for WASAPI loopback"
```

---

## Task 2: Implement LoopbackRecorder (TDD)

**Files:**
- Create: `src-tauri/src/audio_toolkit/audio/loopback.rs`

- [ ] **Step 2.1: Write the failing tests**

Create `src-tauri/src/audio_toolkit/audio/loopback.rs` with the tests only (no implementation yet):

```rust
//! WASAPI loopback capture — Windows only.

#[cfg(target_os = "windows")]
pub use windows_impl::LoopbackRecorder;

#[cfg(target_os = "windows")]
mod windows_impl {
    use crate::audio_toolkit::audio::FrameResampler;
    use crate::audio_toolkit::constants;
    use std::sync::mpsc;
    use std::time::Duration;

    enum Cmd {
        Start,
        Stop(mpsc::Sender<Vec<f32>>),
        Shutdown,
    }

    pub struct LoopbackRecorder {
        cmd_tx: Option<mpsc::Sender<Cmd>>,
        worker_handle: Option<std::thread::JoinHandle<()>>,
    }

    impl LoopbackRecorder {
        pub fn new() -> Self {
            // TODO: implement
            unimplemented!()
        }

        pub fn open(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            // TODO: implement
            unimplemented!()
        }

        pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
            // TODO: implement
            unimplemented!()
        }

        pub fn stop(&self) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
            // TODO: implement
            unimplemented!()
        }

        pub fn close(&mut self) {
            // TODO: implement
            unimplemented!()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn loopback_recorder_new_does_not_panic() {
            // LoopbackRecorder::new() should construct without panicking.
            // We cannot call open() in a unit test (no audio hardware guaranteed),
            // so we only verify construction and that cmd_tx/worker_handle start as None.
            let rec = LoopbackRecorder {
                cmd_tx: None,
                worker_handle: None,
            };
            assert!(rec.cmd_tx.is_none());
            assert!(rec.worker_handle.is_none());
        }

        #[test]
        fn stop_without_open_returns_empty() {
            let rec = LoopbackRecorder {
                cmd_tx: None,
                worker_handle: None,
            };
            // stop() with no cmd_tx should return an empty vec (not panic)
            // This tests graceful degradation when loopback isn't available.
            let result = rec.stop_if_open();
            assert!(result.is_empty());
        }
    }
}
```

- [ ] **Step 2.2: Run tests to confirm they fail**

```powershell
cd src-tauri; cargo test audio_toolkit::audio::loopback 2>&1 | Select-String "FAILED|error|panicked"
```

Expected: compile errors or test failures — the `unimplemented!()` panics confirm the test hooks exist.

- [ ] **Step 2.3: Replace the stub with the real implementation**

Replace the entire content of `src-tauri/src/audio_toolkit/audio/loopback.rs` with:

```rust
//! WASAPI loopback capture — Windows only.

#[cfg(target_os = "windows")]
pub use windows_impl::LoopbackRecorder;

#[cfg(target_os = "windows")]
mod windows_impl {
    use crate::audio_toolkit::audio::FrameResampler;
    use crate::audio_toolkit::constants;
    use std::sync::mpsc;
    use std::time::Duration;

    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    // WAVE_FORMAT_EXTENSIBLE = 0xFFFE — samples are still f32 in modern Windows

    enum Cmd {
        Start,
        Stop(mpsc::Sender<Vec<f32>>),
        Shutdown,
    }

    pub struct LoopbackRecorder {
        cmd_tx: Option<mpsc::Sender<Cmd>>,
        worker_handle: Option<std::thread::JoinHandle<()>>,
    }

    impl LoopbackRecorder {
        pub fn new() -> Self {
            LoopbackRecorder {
                cmd_tx: None,
                worker_handle: None,
            }
        }

        /// Open the WASAPI loopback stream on a worker thread.
        /// Returns Ok(()) on success; the worker thread handles all COM calls.
        /// Returns Err if the loopback device cannot be opened (e.g., enterprise policy).
        pub fn open(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.worker_handle.is_some() {
                return Ok(());
            }

            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

            let worker = std::thread::spawn(move || {
                run_loopback_worker(cmd_rx, init_tx);
            });

            match init_rx.recv() {
                Ok(Ok(())) => {
                    self.cmd_tx = Some(cmd_tx);
                    self.worker_handle = Some(worker);
                    Ok(())
                }
                Ok(Err(e)) => {
                    let _ = worker.join();
                    Err(e.into())
                }
                Err(e) => {
                    let _ = worker.join();
                    Err(format!("Loopback worker init channel error: {e}").into())
                }
            }
        }

        /// Begin accumulating samples into the recording buffer.
        pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
            if let Some(tx) = &self.cmd_tx {
                tx.send(Cmd::Start)?;
            }
            Ok(())
        }

        /// Stop accumulating and return all accumulated samples at 16 kHz mono.
        pub fn stop(&self) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
            let (resp_tx, resp_rx) = mpsc::channel();
            if let Some(tx) = &self.cmd_tx {
                tx.send(Cmd::Stop(resp_tx))?;
            } else {
                return Ok(Vec::new());
            }
            Ok(resp_rx.recv()?)
        }

        /// stop() variant that never errors — returns empty on no connection.
        pub fn stop_if_open(&self) -> Vec<f32> {
            self.stop().unwrap_or_default()
        }

        /// Shut down the worker thread and release WASAPI resources.
        pub fn close(&mut self) {
            if let Some(tx) = self.cmd_tx.take() {
                let _ = tx.send(Cmd::Shutdown);
            }
            if let Some(h) = self.worker_handle.take() {
                let _ = h.join();
            }
        }
    }

    impl Drop for LoopbackRecorder {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn run_loopback_worker(
        cmd_rx: mpsc::Receiver<Cmd>,
        init_tx: mpsc::SyncSender<Result<(), String>>,
    ) {
        use windows::Win32::{
            Media::Audio::{
                eMultimedia, eRender, IAudioCaptureClient, IAudioClient,
                IMMDeviceEnumerator, MMDeviceEnumerator,
                AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
            },
            System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
        };

        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            macro_rules! or_fail {
                ($expr:expr, $msg:literal) => {
                    match $expr {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = init_tx.send(Err(format!("{}: {e}", $msg)));
                            return;
                        }
                    }
                };
            }

            let enumerator: IMMDeviceEnumerator =
                or_fail!(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL), "device enumerator");

            let device =
                or_fail!(enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia), "default render endpoint");

            let audio_client: IAudioClient =
                or_fail!(device.Activate::<IAudioClient>(CLSCTX_ALL, None), "IAudioClient activate");

            let pwfx = or_fail!(audio_client.GetMixFormat(), "GetMixFormat");
            let sample_rate = (*pwfx).nSamplesPerSec;
            let channels = (*pwfx).nChannels as usize;
            let bits_per_sample = (*pwfx).wBitsPerSample;
            let format_tag = (*pwfx).wFormatTag;

            or_fail!(
                audio_client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    0,
                    0,
                    pwfx,
                    None,
                ),
                "IAudioClient Initialize"
            );

            let capture_client: IAudioCaptureClient =
                or_fail!(audio_client.GetService::<IAudioCaptureClient>(), "IAudioCaptureClient");

            or_fail!(audio_client.Start(), "IAudioClient Start");

            let _ = init_tx.send(Ok(()));

            run_capture_loop(
                &capture_client,
                sample_rate,
                channels,
                bits_per_sample,
                format_tag,
                &cmd_rx,
            );

            let _ = audio_client.Stop();
        }
    }

    fn run_capture_loop(
        capture_client: &windows::Win32::Media::Audio::IAudioCaptureClient,
        sample_rate: u32,
        channels: usize,
        bits_per_sample: u16,
        format_tag: u16,
        cmd_rx: &mpsc::Receiver<Cmd>,
    ) {
        let mut resampler = FrameResampler::new(
            sample_rate as usize,
            constants::WHISPER_SAMPLE_RATE as usize,
            Duration::from_millis(30),
        );
        let mut recording = false;
        let mut buffer = Vec::<f32>::new();

        loop {
            // Non-blocking command check first
            while let Ok(cmd) = cmd_rx.try_recv() {
                match cmd {
                    Cmd::Start => {
                        recording = true;
                        buffer.clear();
                    }
                    Cmd::Stop(reply_tx) => {
                        recording = false;
                        resampler.finish(&mut |frame: &[f32]| buffer.extend_from_slice(frame));
                        let _ = reply_tx.send(std::mem::take(&mut buffer));
                    }
                    Cmd::Shutdown => return,
                }
            }

            // Read available audio packets
            unsafe {
                let mut packet_length: u32 = 0;
                if capture_client.GetNextPacketSize(&mut packet_length).is_err() {
                    break;
                }

                while packet_length > 0 {
                    let mut data_ptr: *mut u8 = std::ptr::null_mut();
                    let mut num_frames: u32 = 0;
                    let mut flags: u32 = 0;

                    if capture_client
                        .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                        .is_err()
                    {
                        break;
                    }

                    if recording && num_frames > 0 {
                        let num_samples = num_frames as usize * channels;
                        // AUDCLNT_BUFFERFLAGS_SILENT = 2 — fill with zeros if silent
                        let is_silent = (flags & 2) != 0;

                        let mono: Vec<f32> = if is_silent {
                            vec![0.0f32; num_frames as usize]
                        } else if format_tag == WAVE_FORMAT_IEEE_FLOAT
                            || (format_tag == 0xFFFE && bits_per_sample == 32)
                        {
                            // 32-bit float — most common on modern Windows
                            let slice =
                                std::slice::from_raw_parts(data_ptr as *const f32, num_samples);
                            if channels == 1 {
                                slice.to_vec()
                            } else {
                                slice
                                    .chunks(channels)
                                    .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                                    .collect()
                            }
                        } else {
                            // 16-bit PCM fallback
                            let slice =
                                std::slice::from_raw_parts(data_ptr as *const i16, num_samples);
                            if channels == 1 {
                                slice.iter().map(|&s| s as f32 / 32768.0).collect()
                            } else {
                                slice
                                    .chunks(channels)
                                    .map(|frame| {
                                        frame.iter().map(|&s| s as f32 / 32768.0).sum::<f32>()
                                            / channels as f32
                                    })
                                    .collect()
                            }
                        };

                        resampler.push(&mono, &mut |frame: &[f32]| {
                            buffer.extend_from_slice(frame)
                        });
                    }

                    let _ = capture_client.ReleaseBuffer(num_frames);

                    if capture_client.GetNextPacketSize(&mut packet_length).is_err() {
                        return;
                    }
                }
            }

            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn loopback_recorder_new_does_not_panic() {
            let rec = LoopbackRecorder::new();
            assert!(rec.cmd_tx.is_none());
            assert!(rec.worker_handle.is_none());
        }

        #[test]
        fn stop_without_open_returns_empty() {
            let rec = LoopbackRecorder::new();
            let result = rec.stop_if_open();
            assert!(result.is_empty());
        }
    }
}
```

- [ ] **Step 2.4: Run tests to confirm they pass**

```powershell
cd src-tauri; cargo test audio_toolkit::audio::loopback -- --nocapture 2>&1 | Select-String "test.*ok|FAILED|error\[" | Select-Object -First 20
```

Expected output:
```
test audio_toolkit::audio::loopback::windows_impl::tests::loopback_recorder_new_does_not_panic ... ok
test audio_toolkit::audio::loopback::windows_impl::tests::stop_without_open_returns_empty ... ok
```

- [ ] **Step 2.5: Commit**

```bash
git add src-tauri/src/audio_toolkit/audio/loopback.rs
git commit -m "feat: add LoopbackRecorder for WASAPI system audio capture"
```

---

## Task 3: Implement mix_samples (TDD)

**Files:**
- Create: `src-tauri/src/audio_toolkit/audio/mixer.rs`

- [ ] **Step 3.1: Write the failing tests first**

Create `src-tauri/src/audio_toolkit/audio/mixer.rs`:

```rust
/// Mix two 16 kHz mono f32 sample buffers.
/// Pads the shorter buffer with zeros. Clamps output to [-1.0, 1.0].
pub fn mix_samples(a: &[f32], b: &[f32]) -> Vec<f32> {
    // TODO: implement
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_equal_length() {
        let a = vec![0.5, 0.3, -0.1];
        let b = vec![0.2, 0.1, 0.4];
        let out = mix_samples(&a, &b);
        assert_eq!(out.len(), 3);
        assert!((out[0] - 0.7).abs() < 1e-5);
        assert!((out[1] - 0.4).abs() < 1e-5);
        assert!((out[2] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn mix_a_longer_than_b() {
        let a = vec![0.1, 0.2, 0.3, 0.4];
        let b = vec![0.1, 0.1];
        let out = mix_samples(&a, &b);
        assert_eq!(out.len(), 4);
        // b is padded with zeros for indices 2 and 3
        assert!((out[2] - 0.3).abs() < 1e-5);
        assert!((out[3] - 0.4).abs() < 1e-5);
    }

    #[test]
    fn mix_clamps_over_1() {
        let a = vec![0.8];
        let b = vec![0.8];
        let out = mix_samples(&a, &b);
        assert!((out[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn mix_clamps_under_neg_1() {
        let a = vec![-0.8];
        let b = vec![-0.8];
        let out = mix_samples(&a, &b);
        assert!((out[0] - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn mix_empty_inputs_returns_empty() {
        let out = mix_samples(&[], &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn mix_one_empty_returns_other() {
        let a = vec![0.5, 0.3];
        let out = mix_samples(&a, &[]);
        assert_eq!(out, a);
    }
}
```

- [ ] **Step 3.2: Run tests to confirm they fail**

```powershell
cd src-tauri; cargo test audio_toolkit::audio::mixer 2>&1 | Select-String "FAILED|panicked"
```

Expected: tests panic due to `unimplemented!()`.

- [ ] **Step 3.3: Implement mix_samples**

Replace the `mix_samples` function body (keep tests unchanged):

```rust
pub fn mix_samples(a: &[f32], b: &[f32]) -> Vec<f32> {
    let len = a.len().max(b.len());
    (0..len)
        .map(|i| {
            let sa = a.get(i).copied().unwrap_or(0.0);
            let sb = b.get(i).copied().unwrap_or(0.0);
            (sa + sb).clamp(-1.0, 1.0)
        })
        .collect()
}
```

- [ ] **Step 3.4: Run tests to confirm they pass**

```powershell
cd src-tauri; cargo test audio_toolkit::audio::mixer -- --nocapture 2>&1 | Select-String "test.*ok|FAILED"
```

Expected: all 6 tests pass.

- [ ] **Step 3.5: Commit**

```bash
git add src-tauri/src/audio_toolkit/audio/mixer.rs
git commit -m "feat: add mix_samples for combining mic and loopback audio streams"
```

---

## Task 4: Export new modules from audio/mod.rs

**Files:**
- Modify: `src-tauri/src/audio_toolkit/audio/mod.rs`

- [ ] **Step 4.1: Add the two new modules to mod.rs**

The current content of `src-tauri/src/audio_toolkit/audio/mod.rs` is:

```rust
// Re-export all audio components
mod device;
mod recorder;
mod resampler;
mod utils;
mod visualizer;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
pub use recorder::{is_microphone_access_denied, is_no_input_device_error, AudioRecorder};
pub use resampler::FrameResampler;
pub use utils::{read_wav_samples, save_wav_file, verify_wav_file};
pub use visualizer::AudioVisualiser;
```

Replace it with:

```rust
// Re-export all audio components
mod device;
mod loopback;
mod mixer;
mod recorder;
mod resampler;
mod utils;
mod visualizer;

pub use device::{list_input_devices, list_output_devices, CpalDeviceInfo};
#[cfg(target_os = "windows")]
pub use loopback::LoopbackRecorder;
pub use mixer::mix_samples;
pub use recorder::{is_microphone_access_denied, is_no_input_device_error, AudioRecorder};
pub use resampler::FrameResampler;
pub use utils::{read_wav_samples, save_wav_file, verify_wav_file};
pub use visualizer::AudioVisualiser;
```

- [ ] **Step 4.2: Verify compilation**

```powershell
cd src-tauri; cargo check 2>&1 | Select-String "error"
```

Expected: no error lines.

- [ ] **Step 4.3: Commit**

```bash
git add src-tauri/src/audio_toolkit/audio/mod.rs
git commit -m "chore: export LoopbackRecorder and mix_samples from audio module"
```

---

## Task 5: Add meeting mode to AudioRecordingManager

**Files:**
- Modify: `src-tauri/src/managers/audio.rs`

- [ ] **Step 5.1: Add meeting-mode fields to the struct**

In `src-tauri/src/managers/audio.rs`, find the `AudioRecordingManager` struct definition (around line 145) and add the new fields:

```rust
#[derive(Clone)]
pub struct AudioRecordingManager {
    state: Arc<Mutex<RecordingState>>,
    mode: Arc<Mutex<MicrophoneMode>>,
    app_handle: tauri::AppHandle,

    recorder: Arc<Mutex<Option<AudioRecorder>>>,
    is_open: Arc<Mutex<bool>>,
    is_recording: Arc<Mutex<bool>>,
    did_mute: Arc<Mutex<bool>>,
    close_generation: Arc<AtomicU64>,

    // Meeting mode — Windows only
    #[cfg(target_os = "windows")]
    meeting_mode: Arc<Mutex<bool>>,
    #[cfg(target_os = "windows")]
    loopback_recorder: Arc<Mutex<Option<crate::audio_toolkit::LoopbackRecorder>>>,
    #[cfg(target_os = "windows")]
    meeting_segments: Arc<Mutex<Vec<(chrono::DateTime<chrono::Local>, String)>>>,
    #[cfg(target_os = "windows")]
    meeting_start_time: Arc<Mutex<Option<chrono::DateTime<chrono::Local>>>>,
}
```

- [ ] **Step 5.2: Initialize the new fields in `AudioRecordingManager::new()`**

Find the `Self { ... }` block in `AudioRecordingManager::new()` (around line 169) and add initialization:

```rust
let manager = Self {
    state: Arc::new(Mutex::new(RecordingState::Idle)),
    mode: Arc::new(Mutex::new(mode.clone())),
    app_handle: app.clone(),

    recorder: Arc::new(Mutex::new(None)),
    is_open: Arc::new(Mutex::new(false)),
    is_recording: Arc::new(Mutex::new(false)),
    did_mute: Arc::new(Mutex::new(false)),
    close_generation: Arc::new(AtomicU64::new(0)),

    #[cfg(target_os = "windows")]
    meeting_mode: Arc::new(Mutex::new(false)),
    #[cfg(target_os = "windows")]
    loopback_recorder: Arc::new(Mutex::new(None)),
    #[cfg(target_os = "windows")]
    meeting_segments: Arc::new(Mutex::new(Vec::new())),
    #[cfg(target_os = "windows")]
    meeting_start_time: Arc::new(Mutex::new(None)),
};
```

- [ ] **Step 5.3: Skip muting in meeting mode**

In the `apply_mute` method (around line 244), add a guard at the very top:

```rust
pub fn apply_mute(&self) {
    // Muting output while meeting mode is active would silence the loopback capture.
    #[cfg(target_os = "windows")]
    if *self.meeting_mode.lock().unwrap() {
        return;
    }

    let settings = get_settings(&self.app_handle);
    let mut did_mute_guard = self.did_mute.lock().unwrap();
    // ... rest unchanged
```

- [ ] **Step 5.4: Start loopback in `try_start_recording()`**

In `try_start_recording()`, inside the `if rec.start().is_ok()` block (around line 401), add the loopback start AFTER setting the recording state:

```rust
if rec.start().is_ok() {
    *self.is_recording.lock().unwrap() = true;
    *state = RecordingState::Recording {
        binding_id: binding_id.to_string(),
    };

    // Also start loopback if meeting mode is active
    #[cfg(target_os = "windows")]
    if *self.meeting_mode.lock().unwrap() {
        if let Some(ref rec) = *self.loopback_recorder.lock().unwrap() {
            let _ = rec.start();
        }
    }

    debug!("Recording started for binding {binding_id}");
    return Ok(());
}
```

- [ ] **Step 5.5: Mix loopback into `stop_recording()` output**

In `stop_recording()`, after `let samples = ...` is collected (around line 447), add the mixing block:

```rust
let samples = if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
    match rec.stop() {
        Ok(buf) => buf,
        Err(e) => {
            error!("stop() failed: {e}");
            Vec::new()
        }
    }
} else {
    error!("Recorder not available");
    Vec::new()
};

// In meeting mode: stop loopback and mix with mic samples
#[cfg(target_os = "windows")]
let samples = {
    if *self.meeting_mode.lock().unwrap() {
        let loopback_samples = {
            let guard = self.loopback_recorder.lock().unwrap();
            if let Some(ref rec) = *guard {
                rec.stop_if_open()
            } else {
                Vec::new()
            }
        };
        if !loopback_samples.is_empty() {
            crate::audio_toolkit::mix_samples(&samples, &loopback_samples)
        } else {
            samples
        }
    } else {
        samples
    }
};
```

- [ ] **Step 5.6: Discard loopback in `cancel_recording()`**

In `cancel_recording()`, after `let _ = rec.stop();` (around line 500), add:

```rust
if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
    let _ = rec.stop(); // Discard the result
}

// Discard loopback samples too
#[cfg(target_os = "windows")]
if *self.meeting_mode.lock().unwrap() {
    let guard = self.loopback_recorder.lock().unwrap();
    if let Some(ref rec) = *guard {
        let _ = rec.stop_if_open();
    }
}
```

- [ ] **Step 5.7: Add meeting mode lifecycle methods**

Add these new public methods to `AudioRecordingManager`, after `cancel_recording()`:

```rust
/// Start meeting mode: open loopback recorder and reset the segment accumulator.
/// On loopback failure, logs a warning and proceeds without system audio.
#[cfg(target_os = "windows")]
pub fn start_meeting_mode(&self) -> Result<(), anyhow::Error> {
    let mut meeting = self.meeting_mode.lock().unwrap();
    if *meeting {
        return Ok(());
    }

    let vad_path = self
        .app_handle
        .path()
        .resolve(
            "resources/models/silero_vad_v4.onnx",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| anyhow::anyhow!("Failed to resolve VAD path: {}", e))?;
    let _ = vad_path; // unused — loopback has no VAD

    let mut loopback_opt = self.loopback_recorder.lock().unwrap();
    let mut new_loopback = crate::audio_toolkit::LoopbackRecorder::new();
    match new_loopback.open() {
        Ok(()) => {
            *loopback_opt = Some(new_loopback);
            info!("Meeting mode: loopback recorder opened");
        }
        Err(e) => {
            log::warn!("Meeting mode: failed to open loopback recorder — {e}; proceeding mic-only");
            *loopback_opt = None;
        }
    }

    self.meeting_segments.lock().unwrap().clear();
    *self.meeting_start_time.lock().unwrap() = Some(chrono::Local::now());
    *meeting = true;
    Ok(())
}

/// Stop meeting mode, close loopback, write transcript file.
/// Returns the path to the saved file, or None if no segments were recorded.
#[cfg(target_os = "windows")]
pub fn stop_meeting_mode(&self) -> Result<Option<String>, anyhow::Error> {
    let mut meeting = self.meeting_mode.lock().unwrap();
    if !*meeting {
        return Ok(None);
    }
    *meeting = false;

    if let Some(ref mut rec) = *self.loopback_recorder.lock().unwrap() {
        rec.close();
    }
    *self.loopback_recorder.lock().unwrap() = None;

    let segments = std::mem::take(&mut *self.meeting_segments.lock().unwrap());
    let start_time = self.meeting_start_time.lock().unwrap().take();

    if segments.is_empty() {
        return Ok(None);
    }

    let start = start_time.unwrap_or_else(chrono::Local::now);
    let end = chrono::Local::now();
    let duration = end.signed_duration_since(start);
    let file_path = write_meeting_transcript(&self.app_handle, start, duration, &segments)?;
    Ok(Some(file_path))
}

/// Append a transcription segment to the meeting accumulator (no-op outside meeting mode).
#[cfg(target_os = "windows")]
pub fn add_meeting_segment(&self, text: String) {
    if !*self.meeting_mode.lock().unwrap() {
        return;
    }
    self.meeting_segments
        .lock()
        .unwrap()
        .push((chrono::Local::now(), text));
}

/// Returns true if meeting mode is currently active.
#[cfg(target_os = "windows")]
pub fn is_meeting_mode(&self) -> bool {
    *self.meeting_mode.lock().unwrap()
}
```

- [ ] **Step 5.8: Add the file-writing helper function**

Add this free function at the bottom of `src-tauri/src/managers/audio.rs`, after the `AudioRecordingManager` impl block:

```rust
#[cfg(target_os = "windows")]
fn write_meeting_transcript(
    app: &tauri::AppHandle,
    start: chrono::DateTime<chrono::Local>,
    duration: chrono::Duration,
    segments: &[(chrono::DateTime<chrono::Local>, String)],
) -> Result<String, anyhow::Error> {
    let docs_dir = app
        .path()
        .document_dir()
        .map_err(|e| anyhow::anyhow!("Failed to resolve Documents directory: {e}"))?;

    let meetings_dir = docs_dir.join("Handy").join("meetings");
    std::fs::create_dir_all(&meetings_dir)
        .map_err(|e| anyhow::anyhow!("Failed to create meetings directory: {e}"))?;

    let filename = format!("meeting_{}.txt", start.format("%Y-%m-%d_%H-%M-%S"));
    let file_path = meetings_dir.join(&filename);

    let hours = duration.num_hours().abs();
    let minutes = (duration.num_minutes() % 60).abs();
    let seconds = (duration.num_seconds() % 60).abs();

    let mut content = format!(
        "Meeting: {}\nDuration: {:02}:{:02}:{:02}\n\n",
        start.format("%Y-%m-%d %H:%M"),
        hours,
        minutes,
        seconds
    );

    for (time, text) in segments {
        content.push_str(&format!("[{}] {}\n", time.format("%H:%M:%S"), text));
    }

    std::fs::write(&file_path, &content)
        .map_err(|e| anyhow::anyhow!("Failed to write transcript file: {e}"))?;

    info!("Meeting transcript saved to: {}", file_path.display());
    Ok(file_path.to_string_lossy().to_string())
}
```

- [ ] **Step 5.9: Verify compilation**

```powershell
cd src-tauri; cargo check 2>&1 | Select-String "error"
```

Expected: no error lines.

- [ ] **Step 5.10: Commit**

```bash
git add src-tauri/src/managers/audio.rs
git commit -m "feat: add meeting mode state, loopback integration, and transcript accumulator to AudioRecordingManager"
```

---

## Task 6: Hook accumulator into transcription result (actions.rs)

**Files:**
- Modify: `src-tauri/src/actions.rs`

- [ ] **Step 6.1: Add accumulator call after transcription**

In `src-tauri/src/actions.rs`, find the block that produces `processed.final_text` (around line 608):

```rust
let final_text = processed.final_text;
ah.run_on_main_thread(move || {
    match utils::paste(final_text, ah_clone.clone()) {
```

Replace with:

```rust
let final_text = processed.final_text;

// Accumulate in meeting mode transcript
#[cfg(target_os = "windows")]
if !final_text.is_empty() {
    if let Some(rm) = ah.try_state::<Arc<AudioRecordingManager>>() {
        rm.add_meeting_segment(final_text.clone());
    }
}

ah.run_on_main_thread(move || {
    match utils::paste(final_text, ah_clone.clone()) {
```

- [ ] **Step 6.2: Verify compilation**

```powershell
cd src-tauri; cargo check 2>&1 | Select-String "error"
```

Expected: no error lines.

- [ ] **Step 6.3: Commit**

```bash
git add src-tauri/src/actions.rs
git commit -m "feat: accumulate transcription segments for meeting transcript when meeting mode is active"
```

---

## Task 7: Add Tauri commands and register them

**Files:**
- Modify: `src-tauri/src/commands/audio.rs`
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 7.1: Add four new Tauri commands to commands/audio.rs**

Append at the end of `src-tauri/src/commands/audio.rs`:

```rust
#[tauri::command]
#[specta::specta]
pub fn is_meeting_mode_supported() -> bool {
    cfg!(target_os = "windows")
}

#[tauri::command]
#[specta::specta]
pub fn get_meeting_mode_state(app: AppHandle) -> bool {
    #[cfg(target_os = "windows")]
    {
        let rm = app.state::<Arc<AudioRecordingManager>>();
        rm.is_meeting_mode()
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        false
    }
}

#[tauri::command]
#[specta::specta]
pub fn start_meeting_mode(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let rm = app.state::<Arc<AudioRecordingManager>>();
        rm.start_meeting_mode()
            .map_err(|e| format!("Failed to start meeting mode: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err("Meeting mode is only supported on Windows".to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub fn stop_meeting_mode(app: AppHandle) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let rm = app.state::<Arc<AudioRecordingManager>>();
        rm.stop_meeting_mode()
            .map_err(|e| format!("Failed to stop meeting mode: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = app;
        Err("Meeting mode is only supported on Windows".to_string())
    }
}
```

- [ ] **Step 7.2: Register the four commands in lib.rs**

In `src-tauri/src/lib.rs`, find the `collect_commands!` block. After `commands::audio::is_recording,` add:

```rust
commands::audio::is_meeting_mode_supported,
commands::audio::get_meeting_mode_state,
commands::audio::start_meeting_mode,
commands::audio::stop_meeting_mode,
```

- [ ] **Step 7.3: Verify compilation and binding generation**

```powershell
cd src-tauri; cargo check 2>&1 | Select-String "error"
```

Expected: no error lines.

- [ ] **Step 7.4: Commit**

```bash
git add src-tauri/src/commands/audio.rs src-tauri/src/lib.rs
git commit -m "feat: add Tauri commands for meeting mode (start/stop/state/supported)"
```

---

## Task 8: Add i18n strings

**Files:**
- Modify: `src/i18n/locales/en/translation.json`

- [ ] **Step 8.1: Add meeting mode keys to English translation**

In `src/i18n/locales/en/translation.json`, find the `"settings"` object. Add a new `"meetingMode"` key at an appropriate place (e.g. after the `"sound"` group or next to `"debug"`). The exact JSON to add inside `"settings"`:

```json
"meetingMode": {
  "label": "Meeting Mode",
  "description": "Captures both your microphone and system audio (Teams, Meet, Zoom). Saves a full transcript when stopped.",
  "transcriptSaved": "Transcript saved",
  "systemAudioUnavailable": "System audio unavailable — capturing microphone only"
}
```

To find the right insertion point, look for `"debug"` inside the `"settings"` object and add the above after it (keeping valid JSON commas).

- [ ] **Step 8.2: Verify the JSON is valid**

```powershell
Get-Content "src\i18n\locales\en\translation.json" | ConvertFrom-Json | Select-Object -ExpandProperty settings | Select-Object -ExpandProperty meetingMode
```

Expected: prints the four keys without errors.

- [ ] **Step 8.3: Commit**

```bash
git add src/i18n/locales/en/translation.json
git commit -m "feat: add i18n keys for meeting mode toggle"
```

---

## Task 9: Create MeetingModeToggle component and add to settings

**Files:**
- Create: `src/components/settings/MeetingModeToggle.tsx`
- Modify: `src/components/settings/general/GeneralSettings.tsx`

- [ ] **Step 9.1: Create MeetingModeToggle.tsx**

Create `src/components/settings/MeetingModeToggle.tsx`:

```tsx
import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ToggleSwitch } from "../ui/ToggleSwitch";

export const MeetingModeToggle: React.FC = React.memo(() => {
  const { t } = useTranslation();
  const [supported, setSupported] = useState(false);
  const [active, setActive] = useState(false);
  const [loading, setLoading] = useState(false);
  const [savedPath, setSavedPath] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    invoke<boolean>("is_meeting_mode_supported").then(setSupported);
    invoke<boolean>("get_meeting_mode_state").then(setActive);
    return () => {
      if (toastTimer.current) clearTimeout(toastTimer.current);
    };
  }, []);

  if (!supported) return null;

  const handleToggle = async (enabled: boolean) => {
    setLoading(true);
    setSavedPath(null);
    try {
      if (enabled) {
        await invoke("start_meeting_mode");
        setActive(true);
      } else {
        const filePath = await invoke<string | null>("stop_meeting_mode");
        setActive(false);
        if (filePath) {
          setSavedPath(filePath);
          toastTimer.current = setTimeout(() => setSavedPath(null), 6000);
        }
      }
    } catch (err) {
      console.error("Meeting mode toggle error:", err);
    }
    setLoading(false);
  };

  return (
    <div>
      <ToggleSwitch
        checked={active}
        onChange={handleToggle}
        isUpdating={loading}
        label={t("settings.meetingMode.label")}
        description={t("settings.meetingMode.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      {savedPath && (
        <p className="text-xs text-green-600 mt-1 px-2 truncate">
          {t("settings.meetingMode.transcriptSaved")}: {savedPath}
        </p>
      )}
    </div>
  );
});
```

- [ ] **Step 9.2: Add MeetingModeToggle to GeneralSettings**

In `src/components/settings/general/GeneralSettings.tsx`, add the import and render the component inside the sound settings group, after `<MicrophoneSelector ... />`:

Add import at top:
```tsx
import { MeetingModeToggle } from "../MeetingModeToggle";
```

Inside the `<SettingsGroup title={t("settings.sound.title")}>` block, add after `<MicrophoneSelector .../>`:
```tsx
<MeetingModeToggle />
```

The full updated file should look like:

```tsx
import React from "react";
import { useTranslation } from "react-i18next";
import { type } from "@tauri-apps/plugin-os";
import { MicrophoneSelector } from "../MicrophoneSelector";
import { MeetingModeToggle } from "../MeetingModeToggle";
import { ShortcutInput } from "../ShortcutInput";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { OutputDeviceSelector } from "../OutputDeviceSelector";
import { PushToTalk } from "../PushToTalk";
import { AudioFeedback } from "../AudioFeedback";
import { useSettings } from "../../../hooks/useSettings";
import { VolumeSlider } from "../VolumeSlider";
import { MuteWhileRecording } from "../MuteWhileRecording";
import { ModelSettingsCard } from "./ModelSettingsCard";

export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();
  const { audioFeedbackEnabled, getSetting } = useSettings();
  const pushToTalk = getSetting("push_to_talk");
  const isLinux = type() === "linux";
  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.general.title")}>
        <ShortcutInput shortcutId="transcribe" grouped={true} />
        <PushToTalk descriptionMode="tooltip" grouped={true} />
        {!isLinux && !pushToTalk && (
          <ShortcutInput shortcutId="cancel" grouped={true} />
        )}
      </SettingsGroup>
      <ModelSettingsCard />
      <SettingsGroup title={t("settings.sound.title")}>
        <MicrophoneSelector descriptionMode="tooltip" grouped={true} />
        <MeetingModeToggle />
        <MuteWhileRecording descriptionMode="tooltip" grouped={true} />
        <AudioFeedback descriptionMode="tooltip" grouped={true} />
        <OutputDeviceSelector
          descriptionMode="tooltip"
          grouped={true}
          disabled={!audioFeedbackEnabled}
        />
        <VolumeSlider disabled={!audioFeedbackEnabled} />
      </SettingsGroup>
    </div>
  );
};
```

- [ ] **Step 9.3: TypeScript type check**

```powershell
bun run build 2>&1 | Select-String "error TS|Error"
```

Expected: no TypeScript errors. (Warnings about unused variables are OK.)

- [ ] **Step 9.4: Commit**

```bash
git add src/components/settings/MeetingModeToggle.tsx src/components/settings/general/GeneralSettings.tsx
git commit -m "feat: add MeetingModeToggle component to General settings"
```

---

## Task 10: Full build verification

- [ ] **Step 10.1: Run all Rust tests**

```powershell
cd src-tauri; cargo test 2>&1 | Select-String "test result|FAILED"
```

Expected: `test result: ok. N passed; 0 failed`.

- [ ] **Step 10.2: Run full Tauri build**

```powershell
bun run tauri build 2>&1 | Select-String "error|warning.*unused" | Select-Object -First 30
```

Expected: build completes with an installer at `src-tauri/target/release/bundle/`. Zero `error` lines.

- [ ] **Step 10.3: Manual smoke test on Windows**

1. Open Handy → Settings → General.
2. Confirm "Meeting Mode" toggle appears (Windows-only).
3. Toggle ON → confirm no crash, tray icon stays normal.
4. Press the transcription shortcut while in a Teams/Meet call → speak → release.
5. Confirm text is pasted to clipboard as usual.
6. Toggle OFF → confirm a `.txt` file appears in `Documents\Handy\meetings\`.
7. Open the file and verify it contains `[HH:MM:SS]` prefixed segments.
8. Toggle ON again, toggle OFF immediately (no segments) → confirm no file is created.
9. On a machine that blocks WASAPI loopback → confirm a warning toast appears and mic-only transcription still works.

- [ ] **Step 10.4: Final commit**

```bash
git add .
git commit -m "feat: Meeting Mode — WASAPI loopback + transcript export for meetings"
```

---

## Self-Review Checklist

### Spec Coverage
- [x] §3.1 Audio pipeline — loopback + mixer + existing VAD/transcription pipeline: Tasks 2–5
- [x] §3.2 LoopbackRecorder: Task 2
- [x] §3.2 StreamMixer (`mix_samples`): Task 3
- [x] §3.3 Cargo.toml WASAPI features: Task 1
- [x] §3.3 `managers/audio.rs` meeting state + accumulator: Task 5
- [x] §3.3 `mute_while_recording` bypass: Task 5.3
- [x] §3.4 Accumulator with timestamps: Tasks 5.7 + 6.1
- [x] §4 Tauri commands (start/stop/get_state/is_supported): Task 7
- [x] §5 File export with header + timestamps: Task 5.8
- [x] §5 No file if zero segments: Task 5.7
- [x] §6.1 MeetingModeToggle component: Task 9
- [x] §6.1 Windows-only guard: `is_meeting_mode_supported` command + conditional render
- [x] §6.1 File path notification on stop: Task 9.1 (savedPath toast)
- [x] §6.2 Active indicator: handled by toggle state in `ToggleSwitch` (checked=true)
- [x] §7 WASAPI access denied — graceful fallback to mic-only: Task 5.7 `warn!`
- [x] §7 Meetings dir creation failure — error propagated to command: Task 5.8

### Placeholder Scan
- No TBD or TODO remains in the implementation steps.

### Type Consistency
- `LoopbackRecorder::stop_if_open()` — defined in Task 2, used in Task 5.5 and 5.6 ✓
- `mix_samples(&[f32], &[f32]) -> Vec<f32>` — defined in Task 3, used in Task 5.5 ✓
- `add_meeting_segment(String)` — defined in Task 5.7, called in Task 6.1 ✓
- `start_meeting_mode()`, `stop_meeting_mode()`, `is_meeting_mode()` — defined in Task 5.7, exposed in Task 7 ✓
- i18n keys `settings.meetingMode.*` — defined in Task 8, used in Task 9 ✓
