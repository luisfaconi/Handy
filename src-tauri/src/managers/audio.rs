use crate::audio_toolkit::{list_input_devices, vad::SmoothedVad, AudioRecorder, SileroVad};
use crate::helpers::clamshell;
use crate::settings::{get_settings, AppSettings};
use crate::utils;
use log::{debug, error, info};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::Manager;

#[cfg(target_os = "windows")]
#[derive(Clone, serde::Serialize)]
pub struct MeetingSegmentEvent {
    pub text: String,
    pub timestamp: String, // "[HH:MM:SS]"
    pub index: u32,
}

const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

fn set_mute(mute: bool) {
    // Expected behavior:
    // - Windows: works on most systems using standard audio drivers.
    // - Linux: works on many systems (PipeWire, PulseAudio, ALSA),
    //   but some distros may lack the tools used.
    // - macOS: works on most standard setups via AppleScript.
    // If unsupported, fails silently.

    #[cfg(target_os = "windows")]
    {
        unsafe {
            use windows::Win32::{
                Media::Audio::{
                    eMultimedia, eRender, Endpoints::IAudioEndpointVolume, IMMDeviceEnumerator,
                    MMDeviceEnumerator,
                },
                System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED},
            };

            macro_rules! unwrap_or_return {
                ($expr:expr) => {
                    match $expr {
                        Ok(val) => val,
                        Err(_) => return,
                    }
                };
            }

            // Initialize the COM library for this thread.
            // If already initialized (e.g., by another library like Tauri), this does nothing.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            let all_devices: IMMDeviceEnumerator =
                unwrap_or_return!(CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL));
            let default_device =
                unwrap_or_return!(all_devices.GetDefaultAudioEndpoint(eRender, eMultimedia));
            let volume_interface = unwrap_or_return!(
                default_device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
            );

            let _ = volume_interface.SetMute(mute, std::ptr::null());
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;

        let mute_val = if mute { "1" } else { "0" };
        let amixer_state = if mute { "mute" } else { "unmute" };

        // Try multiple backends to increase compatibility
        // 1. PipeWire (wpctl)
        if Command::new("wpctl")
            .args(["set-mute", "@DEFAULT_AUDIO_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        // 2. PulseAudio (pactl)
        if Command::new("pactl")
            .args(["set-sink-mute", "@DEFAULT_SINK@", mute_val])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return;
        }

        // 3. ALSA (amixer)
        let _ = Command::new("amixer")
            .args(["set", "Master", amixer_state])
            .output();
    }

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let script = format!(
            "set volume output muted {}",
            if mute { "true" } else { "false" }
        );
        let _ = Command::new("osascript").args(["-e", &script]).output();
    }
}

const WHISPER_SAMPLE_RATE: usize = 16000;

/* ──────────────────────────────────────────────────────────────── */

#[derive(Clone, Debug)]
pub enum RecordingState {
    Idle,
    Recording { binding_id: String },
}

#[derive(Clone, Debug)]
pub enum MicrophoneMode {
    AlwaysOn,
    OnDemand,
}

/* ──────────────────────────────────────────────────────────────── */

fn create_audio_recorder(
    vad_path: &str,
    app_handle: &tauri::AppHandle,
) -> Result<AudioRecorder, anyhow::Error> {
    let silero = SileroVad::new(vad_path, 0.3)
        .map_err(|e| anyhow::anyhow!("Failed to create SileroVad: {}", e))?;
    let smoothed_vad = SmoothedVad::new(Box::new(silero), 15, 15, 2);

    // Recorder with VAD plus a spectrum-level callback that forwards updates to
    // the frontend.
    let recorder = AudioRecorder::new()
        .map_err(|e| anyhow::anyhow!("Failed to create AudioRecorder: {}", e))?
        .with_vad(Box::new(smoothed_vad))
        .with_level_callback({
            let app_handle = app_handle.clone();
            move |levels| {
                utils::emit_levels(&app_handle, &levels);
            }
        });

    Ok(recorder)
}

/* ──────────────────────────────────────────────────────────────── */

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
    #[cfg(target_os = "windows")]
    meeting_chunk_tx: Arc<Mutex<Option<mpsc::SyncSender<Vec<f32>>>>>,
    #[cfg(target_os = "windows")]
    meeting_worker_handle: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    #[cfg(target_os = "windows")]
    transcript_path: Arc<Mutex<Option<std::path::PathBuf>>>,
}

impl AudioRecordingManager {
    /* ---------- construction ------------------------------------------------ */

    pub fn new(app: &tauri::AppHandle) -> Result<Self, anyhow::Error> {
        let settings = get_settings(app);
        let mode = if settings.always_on_microphone {
            MicrophoneMode::AlwaysOn
        } else {
            MicrophoneMode::OnDemand
        };

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
            #[cfg(target_os = "windows")]
            meeting_chunk_tx: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "windows")]
            meeting_worker_handle: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "windows")]
            transcript_path: Arc::new(Mutex::new(None)),
        };

        // Always-on?  Open immediately.
        if matches!(mode, MicrophoneMode::AlwaysOn) {
            manager.start_microphone_stream()?;
        }

        Ok(manager)
    }

    /* ---------- helper methods --------------------------------------------- */

    fn get_effective_microphone_device(&self, settings: &AppSettings) -> Option<cpal::Device> {
        // Check if we're in clamshell mode and have a clamshell microphone configured
        let use_clamshell_mic = if let Ok(is_clamshell) = clamshell::is_clamshell() {
            is_clamshell && settings.clamshell_microphone.is_some()
        } else {
            false
        };

        let device_name = if use_clamshell_mic {
            settings.clamshell_microphone.as_ref().unwrap()
        } else {
            settings.selected_microphone.as_ref()?
        };

        // Find the device by name
        match list_input_devices() {
            Ok(devices) => devices
                .into_iter()
                .find(|d| d.name == *device_name)
                .map(|d| d.device),
            Err(e) => {
                debug!("Failed to list devices, using default: {}", e);
                None
            }
        }
    }

    fn schedule_lazy_close(&self) {
        let gen = self.close_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let app = self.app_handle.clone();
        std::thread::spawn(move || {
            std::thread::sleep(STREAM_IDLE_TIMEOUT);
            let rm = app.state::<Arc<AudioRecordingManager>>();
            // Hold state lock across the check AND close to serialize against
            // try_start_recording, preventing a race where the stream is closed
            // under an active recording.
            let state = rm.state.lock().unwrap();
            if rm.close_generation.load(Ordering::SeqCst) == gen
                && matches!(*state, RecordingState::Idle)
            {
                // stop_microphone_stream does not acquire the state lock,
                // so holding it here is safe (no deadlock).
                info!(
                    "Closing idle microphone stream after {:?}",
                    STREAM_IDLE_TIMEOUT
                );
                rm.stop_microphone_stream();
            }
        });
    }

    /* ---------- microphone life-cycle -------------------------------------- */

    /// Applies mute if mute_while_recording is enabled and stream is open
    pub fn apply_mute(&self) {
        #[cfg(target_os = "windows")]
        if *self.meeting_mode.lock().unwrap() {
            return;
        }

        let settings = get_settings(&self.app_handle);
        let mut did_mute_guard = self.did_mute.lock().unwrap();

        if settings.mute_while_recording && *self.is_open.lock().unwrap() {
            set_mute(true);
            *did_mute_guard = true;
            debug!("Mute applied");
        }
    }

    /// Removes mute if it was applied
    pub fn remove_mute(&self) {
        let mut did_mute_guard = self.did_mute.lock().unwrap();
        if *did_mute_guard {
            set_mute(false);
            *did_mute_guard = false;
            debug!("Mute removed");
        }
    }

    pub fn preload_vad(&self) -> Result<(), anyhow::Error> {
        let mut recorder_opt = self.recorder.lock().unwrap();
        if recorder_opt.is_none() {
            let vad_path = self
                .app_handle
                .path()
                .resolve(
                    "resources/models/silero_vad_v4.onnx",
                    tauri::path::BaseDirectory::Resource,
                )
                .map_err(|e| anyhow::anyhow!("Failed to resolve VAD path: {}", e))?;
            *recorder_opt = Some(create_audio_recorder(
                vad_path.to_str().unwrap(),
                &self.app_handle,
            )?);
        }
        Ok(())
    }

    pub fn start_microphone_stream(&self) -> Result<(), anyhow::Error> {
        let mut open_flag = self.is_open.lock().unwrap();
        if *open_flag {
            debug!("Microphone stream already active");
            return Ok(());
        }

        let start_time = Instant::now();

        // Don't mute immediately - caller will handle muting after audio feedback
        let mut did_mute_guard = self.did_mute.lock().unwrap();
        *did_mute_guard = false;

        // Get the selected device from settings, considering clamshell mode
        let settings = get_settings(&self.app_handle);
        let selected_device = self.get_effective_microphone_device(&settings);

        // Pre-flight check: if no device was selected/configured AND no devices
        // exist at all, fail early with a clear error instead of letting cpal
        // produce a cryptic backend-specific message.
        if selected_device.is_none() {
            let has_any_device = list_input_devices()
                .map(|devices| !devices.is_empty())
                .unwrap_or(false);
            if !has_any_device {
                return Err(anyhow::anyhow!("No input device found"));
            }
        }

        // Ensure VAD is loaded if it wasn't for whatever reason
        self.preload_vad()?;

        let mut recorder_opt = self.recorder.lock().unwrap();
        if let Some(rec) = recorder_opt.as_mut() {
            rec.open(selected_device)
                .map_err(|e| anyhow::anyhow!("Failed to open recorder: {}", e))?;
        }

        *open_flag = true;
        // This timing covers through cpal's stream.play() returning — i.e. the
        // point cpal surfaces as "stream running." It does NOT guarantee the
        // host audio device is producing samples yet; the first input callback
        // fires asynchronously one buffer period later (hardware dependent,
        // typically ~10–200ms on macOS, longer on Bluetooth/USB).
        info!(
            "Microphone stream initialized in {:?}",
            start_time.elapsed()
        );
        Ok(())
    }

    pub fn stop_microphone_stream(&self) {
        let mut open_flag = self.is_open.lock().unwrap();
        if !*open_flag {
            return;
        }

        let mut did_mute_guard = self.did_mute.lock().unwrap();
        if *did_mute_guard {
            set_mute(false);
        }
        *did_mute_guard = false;

        if let Some(rec) = self.recorder.lock().unwrap().as_mut() {
            // If still recording, stop first.
            if *self.is_recording.lock().unwrap() {
                let _ = rec.stop();
                *self.is_recording.lock().unwrap() = false;
            }
            let _ = rec.close();
        }

        *open_flag = false;
        debug!("Microphone stream stopped");
    }

    /* ---------- mode switching --------------------------------------------- */

    pub fn update_mode(&self, new_mode: MicrophoneMode) -> Result<(), anyhow::Error> {
        let cur_mode = self.mode.lock().unwrap().clone();

        match (cur_mode, &new_mode) {
            (MicrophoneMode::AlwaysOn, MicrophoneMode::OnDemand) => {
                if matches!(*self.state.lock().unwrap(), RecordingState::Idle) {
                    self.close_generation.fetch_add(1, Ordering::SeqCst);
                    self.stop_microphone_stream();
                }
            }
            (MicrophoneMode::OnDemand, MicrophoneMode::AlwaysOn) => {
                self.close_generation.fetch_add(1, Ordering::SeqCst);
                self.start_microphone_stream()?;
            }
            _ => {}
        }

        *self.mode.lock().unwrap() = new_mode;
        Ok(())
    }

    /* ---------- recording --------------------------------------------------- */

    pub fn try_start_recording(&self, binding_id: &str) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();

        if let RecordingState::Idle = *state {
            // Ensure microphone is open in on-demand mode
            if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                // Cancel any pending lazy close
                self.close_generation.fetch_add(1, Ordering::SeqCst);
                if let Err(e) = self.start_microphone_stream() {
                    let msg = format!("{e}");
                    error!("Failed to open microphone stream: {msg}");
                    return Err(msg);
                }
            }

            if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                if rec.start().is_ok() {
                    *self.is_recording.lock().unwrap() = true;
                    *state = RecordingState::Recording {
                        binding_id: binding_id.to_string(),
                    };
                    #[cfg(target_os = "windows")]
                    if *self.meeting_mode.lock().unwrap() {
                        if let Some(ref rec) = *self.loopback_recorder.lock().unwrap() {
                            let _ = rec.start();
                        }
                    }
                    debug!("Recording started for binding {binding_id}");
                    return Ok(());
                }
            }
            Err("Recorder not available".to_string())
        } else {
            Err("Already recording".to_string())
        }
    }

    pub fn update_selected_device(&self) -> Result<(), anyhow::Error> {
        // If currently open, restart the microphone stream to use the new device
        if *self.is_open.lock().unwrap() {
            self.close_generation.fetch_add(1, Ordering::SeqCst);
            self.stop_microphone_stream();
            self.start_microphone_stream()?;
        }
        Ok(())
    }

    pub fn stop_recording(&self, binding_id: &str) -> Option<Vec<f32>> {
        let mut state = self.state.lock().unwrap();

        match *state {
            RecordingState::Recording {
                binding_id: ref active,
            } if active == binding_id => {
                *state = RecordingState::Idle;
                drop(state);

                // Optionally keep recording for a bit longer to capture trailing audio
                let settings = get_settings(&self.app_handle);
                if settings.extra_recording_buffer_ms > 0 {
                    debug!(
                        "Extra recording buffer: sleeping {}ms before stopping",
                        settings.extra_recording_buffer_ms
                    );
                    std::thread::sleep(Duration::from_millis(settings.extra_recording_buffer_ms));
                }

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

                *self.is_recording.lock().unwrap() = false;

                // In on-demand mode, close the mic (lazily if the setting is enabled)
                if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                    if get_settings(&self.app_handle).lazy_stream_close {
                        self.schedule_lazy_close();
                    } else {
                        self.stop_microphone_stream();
                    }
                }

                // Pad if very short
                let s_len = samples.len();
                // debug!("Got {} samples", s_len);
                if s_len < WHISPER_SAMPLE_RATE && s_len > 0 {
                    let mut padded = samples;
                    padded.resize(WHISPER_SAMPLE_RATE * 5 / 4, 0.0);
                    Some(padded)
                } else {
                    Some(samples)
                }
            }
            _ => None,
        }
    }
    pub fn is_recording(&self) -> bool {
        matches!(
            *self.state.lock().unwrap(),
            RecordingState::Recording { .. }
        )
    }

    /* ---------- meeting mode ----------------------------------------------- */

    #[cfg(target_os = "windows")]
    pub fn start_meeting_mode(&self) -> Result<(), anyhow::Error> {
        use std::io::Write;

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
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
                .map_err(|e| anyhow::anyhow!("Failed to create transcript file: {e}"))?;
            writeln!(file, "Meeting: {}", start_time.format("%Y-%m-%d %H:%M"))?;
            writeln!(file)?; // blank line before segments
        }
        *self.transcript_path.lock().unwrap() = Some(path.clone());

        // Create bounded channel for audio chunks
        // 8 slots ≈ ~30-40 seconds of speech backlog at typical VAD chunk sizes
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(8);

        // Register the segment callback on the recorder
        {
            let recorder = self.recorder.lock().unwrap();
            if let Some(r) = recorder.as_ref() {
                r.set_meeting_tx(Some(tx.clone()));
            } else {
                log::warn!("Meeting mode: recorder not available, segment dispatch disabled");
            }
        }
        *self.meeting_chunk_tx.lock().unwrap() = Some(tx);

        // Spawn worker thread: transcribes chunks and writes to file progressively
        let app_handle = self.app_handle.clone();
        let segments_arc = self.meeting_segments.clone();
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

    #[cfg(target_os = "windows")]
    pub fn stop_meeting_mode(&self) -> Result<Option<String>, anyhow::Error> {
        let mut meeting = self.meeting_mode.lock().unwrap();
        if !*meeting {
            return Ok(None);
        }
        *meeting = false;
        drop(meeting);

        {
            let mut guard = self.loopback_recorder.lock().unwrap();
            if let Some(ref mut rec) = *guard {
                rec.close();
            }
            *guard = None;
        }

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

    #[cfg(target_os = "windows")]
    pub fn is_meeting_mode(&self) -> bool {
        *self.meeting_mode.lock().unwrap()
    }

    /// Cancel any ongoing recording without returning audio samples
    pub fn cancel_recording(&self) {
        let mut state = self.state.lock().unwrap();

        if let RecordingState::Recording { .. } = *state {
            *state = RecordingState::Idle;
            drop(state);

            if let Some(rec) = self.recorder.lock().unwrap().as_ref() {
                let _ = rec.stop(); // Discard the result
            }

            #[cfg(target_os = "windows")]
            if *self.meeting_mode.lock().unwrap() {
                let guard = self.loopback_recorder.lock().unwrap();
                if let Some(ref rec) = *guard {
                    let _ = rec.stop_if_open();
                }
            }

            *self.is_recording.lock().unwrap() = false;

            // In on-demand mode, close the mic (lazily if the setting is enabled)
            if matches!(*self.mode.lock().unwrap(), MicrophoneMode::OnDemand) {
                if get_settings(&self.app_handle).lazy_stream_close {
                    self.schedule_lazy_close();
                } else {
                    self.stop_microphone_stream();
                }
            }
        }
    }
}

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
