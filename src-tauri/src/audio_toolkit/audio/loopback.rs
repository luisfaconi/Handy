//! WASAPI loopback capture — Windows only.

#[cfg(target_os = "windows")]
pub use windows_impl::LoopbackRecorder;

#[cfg(target_os = "windows")]
mod windows_impl {
    use crate::audio_toolkit::audio::{AudioVisualiser, FrameResampler};
    use crate::audio_toolkit::constants;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;

    enum Cmd {
        Start,
        Stop(mpsc::Sender<Vec<f32>>),
        Shutdown,
    }

    pub struct LoopbackRecorder {
        cmd_tx: Option<mpsc::Sender<Cmd>>,
        worker_handle: Option<std::thread::JoinHandle<()>>,
        level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    }

    impl LoopbackRecorder {
        pub fn new() -> Self {
            LoopbackRecorder {
                cmd_tx: None,
                worker_handle: None,
                level_cb: None,
            }
        }

        pub fn set_level_cb<F>(&mut self, cb: F)
        where
            F: Fn(Vec<f32>) + Send + Sync + 'static,
        {
            self.level_cb = Some(Arc::new(cb));
        }

        pub fn open(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            if self.worker_handle.is_some() {
                return Ok(());
            }

            let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
            let (init_tx, init_rx) = mpsc::sync_channel::<Result<(), String>>(1);

            let level_cb = self.level_cb.clone();

            let worker = std::thread::spawn(move || {
                run_loopback_worker(cmd_rx, init_tx, level_cb);
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

        pub fn start(&self) -> Result<(), Box<dyn std::error::Error>> {
            if let Some(tx) = &self.cmd_tx {
                tx.send(Cmd::Start)?;
            }
            Ok(())
        }

        pub fn stop(&self) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
            let (resp_tx, resp_rx) = mpsc::channel();
            if let Some(tx) = &self.cmd_tx {
                tx.send(Cmd::Stop(resp_tx))?;
            } else {
                return Ok(Vec::new());
            }
            Ok(resp_rx.recv()?)
        }

        pub fn stop_if_open(&self) -> Vec<f32> {
            self.stop().unwrap_or_default()
        }

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
        level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
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

            let device = or_fail!(
                enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia),
                "default render endpoint"
            );

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

            run_capture_loop(&capture_client, sample_rate, channels, bits_per_sample, format_tag, &cmd_rx, level_cb);

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
        level_cb: Option<Arc<dyn Fn(Vec<f32>) + Send + Sync + 'static>>,
    ) {
        let mut resampler = FrameResampler::new(
            sample_rate as usize,
            constants::WHISPER_SAMPLE_RATE as usize,
            Duration::from_millis(30),
        );
        let mut recording = false;
        let mut buffer = Vec::<f32>::new();

        const BUCKETS: usize = 16;
        const WINDOW_SIZE: usize = 512;
        let mut visualizer = level_cb.as_ref().map(|_| {
            AudioVisualiser::new(sample_rate, WINDOW_SIZE, BUCKETS, 400.0, 4000.0)
        });

        loop {
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

            unsafe {
                let mut packet_length = match capture_client.GetNextPacketSize() {
                    Ok(size) => size,
                    Err(_) => break,
                };

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
                        // AUDCLNT_BUFFERFLAGS_SILENT = 2
                        let is_silent = (flags & 2) != 0;

                        let mono: Vec<f32> = if is_silent {
                            vec![0.0f32; num_frames as usize]
                        } else if format_tag == WAVE_FORMAT_IEEE_FLOAT
                            || (format_tag == 0xFFFE && bits_per_sample == 32)
                        {
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

                        if let (Some(vis), Some(cb)) = (&mut visualizer, &level_cb) {
                            if let Some(buckets) = vis.feed(&mono) {
                                cb(buckets);
                            }
                        }

                        resampler.push(&mono, &mut |frame: &[f32]| {
                            buffer.extend_from_slice(frame)
                        });
                    }

                    let _ = capture_client.ReleaseBuffer(num_frames);

                    packet_length = match capture_client.GetNextPacketSize() {
                        Ok(size) => size,
                        Err(_) => return,
                    };
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
