use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Gravador push-to-talk: abre o stream do microfone, acumula amostras f32
/// (taxa nativa do device) e, ao parar, devolve o áudio em 16 kHz mono.
pub struct Recorder {
    stream: cpal::Stream,
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl Recorder {
    /// Inicia a captura. `device_name = None` usa a entrada padrão do sistema.
    pub fn start(device_name: Option<&str>) -> Result<Self, String> {
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

        let err_cb = move |err| eprintln!("[fala:audio] erro no stream: {err}");

        let stream = match cfg_in.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                cfg,
                move |data: &[f32], _| push(&mut buf_cb.lock().unwrap(), data, channels),
                err_cb,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                cfg,
                move |data: &[i16], _| {
                    let f: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                    push(&mut buf_cb.lock().unwrap(), &f, channels)
                },
                err_cb,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                cfg,
                move |data: &[u16], _| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    push(&mut buf_cb.lock().unwrap(), &f, channels)
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

fn push(buffer: &mut Vec<f32>, data: &[f32], channels: usize) {
    if channels <= 1 {
        buffer.extend_from_slice(data);
    } else {
        // downmix: média simples dos canais
        for frame in data.chunks(channels) {
            let sum: f32 = frame.iter().sum();
            buffer.push(sum / channels as f32);
        }
    }
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
