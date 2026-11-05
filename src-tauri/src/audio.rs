use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Estado do detector de voz (energia), compartilhado entre o callback de áudio
/// e a thread de supervisão da gravação.
pub struct VadState {
    /// RMS do chunk mais recente (0..1)
    pub rms: Mutex<f32>,
    /// já houve fala nesta gravação?
    pub speech_seen: AtomicBool,
    /// instante da última fala detectada
    pub last_voice: Mutex<Option<Instant>>,
    pub started: Instant,
}

impl VadState {
    pub fn new() -> Self {
        Self {
            rms: Mutex::new(0.0),
            speech_seen: AtomicBool::new(false),
            last_voice: Mutex::new(None),
            started: Instant::now(),
        }
    }

    fn update(&self, rms: f32) {
        *self.rms.lock().unwrap() = rms;
        // limiar: ambiente típico ~0.004; fala de perto 0.05+. 0.015 cobre os dois.
        if rms > 0.015 {
            self.speech_seen.store(true, Ordering::Relaxed);
            *self.last_voice.lock().unwrap() = Some(Instant::now());
        }
    }

    pub fn current_rms(&self) -> f32 {
        *self.rms.lock().unwrap()
    }

    pub fn silence_since(&self) -> Option<Duration> {
        self.last_voice
            .lock()
            .unwrap()
            .map(|t| t.elapsed())
    }
}

use std::time::Duration;

/// Gravador: abre o stream do microfone, acumula amostras f32 (taxa nativa do
/// device) e, ao parar, devolve o áudio em 16 kHz mono. Alimenta o VAD.
pub struct Recorder {
    stream: cpal::Stream,
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl Recorder {
    /// Inicia a captura. `device_name = None` usa a entrada padrão do sistema.
    pub fn start(device_name: Option<&str>, vad: Arc<VadState>) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => {
                let found = host
                    .input_devices()
                    .map_err(|e| format!("input_devices: {e}"))?
                    .find(|d| {
                        d.description()
                            .map(|desc| desc.name() == name)
                            .unwrap_or(false)
                    });
                found.ok_or_else(|| format!("Dispositivo de entrada não encontrado: {name}"))?
            }
            None => host
                .default_input_device()
                .ok_or_else(|| "Nenhum dispositivo de entrada disponível".to_string())?,
        };
        let cfg_in = device
            .default_input_config()
            .map_err(|e| format!("default_input_config: {e}"))?;
        let rate = cfg_in.sample_rate(); // SampleRate é alias de u32 no cpal 0.18
        let channels = cfg_in.channels() as usize;
        let cfg: cpal::StreamConfig = cfg_in.into();

        let buffer: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
        let buf_cb = buffer.clone();
        let vad_cb = vad;

        let err_cb = move |err| eprintln!("[fala:audio] erro no stream: {err}");

        let stream = match cfg_in.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                cfg,
                move |data: &[f32], _| {
                    let f = downmix(data, channels);
                    vad_cb.update(rms_of(&f));
                    buf_cb.lock().unwrap().extend_from_slice(&f);
                },
                err_cb,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                cfg,
                move |data: &[i16], _| {
                    let f = downmix(
                        &data.iter().map(|&s| s as f32 / 32768.0).collect::<Vec<f32>>(),
                        channels,
                    );
                    vad_cb.update(rms_of(&f));
                    buf_cb.lock().unwrap().extend_from_slice(&f);
                },
                err_cb,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                cfg,
                move |data: &[u16], _| {
                    let f = downmix(
                        &data
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .collect::<Vec<f32>>(),
                        channels,
                    );
                    vad_cb.update(rms_of(&f));
                    buf_cb.lock().unwrap().extend_from_slice(&f);
                },
                err_cb,
                None,
            ),
            other => return Err(format!("Formato de amostra não suportado: {other:?}")),
        }
        .map_err(|e| format!("build_input_stream: {e}"))?;

        stream.play().map_err(|e| format!("stream.play: {e}"))?;
        Ok(Self {
            stream,
            buffer,
            rate,
        })
    }

    /// Para a captura e devolve as amostras prontas para o whisper (16 kHz mono f32).
    pub fn stop(self) -> Result<Vec<f32>, String> {
        drop(self.stream); // encerra o stream antes de consumir o buffer
        let raw = self
            .buffer
            .lock()
            .map_err(|e| e.to_string())?
            .drain(..)
            .collect::<Vec<f32>>();
        Ok(resample_to_16k(&raw, self.rate))
    }
}

/// downmix: média simples dos canais para mono
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// RMS (0..~1) de um trecho mono
fn rms_of(data: &[f32]) -> f32 {
    if data.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = data.iter().map(|s| s * s).sum();
    (sum_sq / data.len() as f32).sqrt()
}

/// Resumo de níveis por segmento (diagnóstico de mic que "morre" no meio da gravação).
/// Retorna o RMS de cada segmento de `segundos` da amostra 16 kHz.
pub fn level_map(samples: &[f32], rate: u32, segundos: u32) -> Vec<f32> {
    let step = (rate as usize * segundos as usize).max(1);
    samples
        .chunks(step)
        .map(|chunk| {
            let sum_sq: f32 = chunk.iter().map(|s| s * s).sum();
            (sum_sq / chunk.len().max(1) as f32).sqrt()
        })
        .collect()
}

/// Reamostragem linear para 16 kHz mono (suficiente para fala; o whisper é tolerante).
pub fn resample_to_16k(input: &[f32], from_rate: u32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if from_rate == 16_000 {
        return input.to_vec();
    }
    let ratio = f64::from(from_rate) / 16_000.0;
    let out_len = ((input.len() as f64) / ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * ratio;
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(input.len() - 1);
        let frac = (pos - i0 as f64) as f32;
        out.push(input[i0] * (1.0 - frac) + input[i1] * frac);
    }
    out
}

/// Lista os nomes dos dispositivos de entrada disponíveis.
pub fn list_input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devs| {
            devs.filter_map(|d| d.description().ok().map(|desc| desc.name().to_string()))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
}
