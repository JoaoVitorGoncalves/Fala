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

/// Acesso somente-leitura ao buffer de gravação para outras threads (worker de
/// transcrição em tempo real) enquanto o stream continua capturando. O cpal
/// Stream não atravessa threads com segurança, por isso o handle é separado.
#[derive(Clone)]
pub struct RecorderHandle {
    buffer: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

impl RecorderHandle {
    /// Amostras novas (taxa nativa) desde o índice `from`, SEM drenar o buffer —
    /// o consumo final (stop) continua intocado.
    pub fn snapshot_since(&self, from: usize) -> Vec<f32> {
        self.buffer
            .lock()
            .map(|b| b[from.min(b.len())..].to_vec())
            .unwrap_or_default()
    }

    /// Taxa nativa do device (para reamostrar as parciais para 16 kHz).
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
}

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

    /// Handle compartilhável para o worker de transcrição em tempo real.
    pub fn handle(&self) -> RecorderHandle {
        RecorderHandle {
            buffer: self.buffer.clone(),
            rate: self.rate,
        }
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

/// downmix: média simples dos canais para mono.
/// Drivers USB podem entregar NaN/Inf em glitchs; o whisper.cpp aborta com
/// entrada não-finita (assert no ggml), então substituímos por silêncio.
fn downmix(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data
            .iter()
            .map(|&v| if v.is_finite() { v } else { 0.0 })
            .collect();
    }
    data.chunks(channels)
        .map(|frame| {
            frame
                .iter()
                .fold(0.0f32, |acc, &v| if v.is_finite() { acc + v } else { acc })
                / channels as f32
        })
        .collect()
}

/// RMS (0..~1) de um trecho mono
pub(crate) fn rms_of(data: &[f32]) -> f32 {
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
        let v = input[i0] * (1.0 - frac) + input[i1] * frac;
        out.push(if v.is_finite() { v } else { 0.0 });
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

// ---------------------------------------------------------------- conflito HFP

/// Conflito de áudio Bluetooth: o microfone selecionado é BT e a saída padrão
/// também — abrir o mic comuta o fone para HFP (perfil de ligação) e degrada a
/// música ("efeito copo"). `suggestion` = entrada não-BT alternativa, se houver.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, serde::Serialize)]
pub struct BtConflict {
    pub input_name: String,
    pub suggestion: Option<String>,
}

#[cfg(target_os = "macos")]
pub fn bt_conflict(selected_input: Option<&str>) -> Option<BtConflict> {
    macos::bt_conflict_impl(selected_input)
}

#[cfg(not(target_os = "macos"))]
pub fn bt_conflict(_selected_input: Option<&str>) -> Option<()> {
    None
}

/// Consulta direta ao CoreAudio: cpal não expõe o tipo de transporte do
/// dispositivo, e é ele que revela se a entrada é Bluetooth.
#[cfg(target_os = "macos")]
mod macos {
    use coreaudio_sys::*;
    use std::ffi::c_void;

    // kAudioObjectSystemObject
    const SYSTEM_OBJECT: AudioObjectID = 1;
    // kAudioObjectPropertyElementMaster/Main == 0 (constante definida à mão
    // para não depender do nome que o SDK binding escolheu)
    const ELEM: AudioObjectPropertyElement = 0;
    // 'blue' / 'blle' / 'bltn' (AudioHardwareBase.h — builtin é 'bltn', não 'buil')
    const TRANSPORT_BT: u32 = 0x626C_7565;
    const TRANSPORT_BTLE: u32 = 0x626C_6C65;
    const TRANSPORT_BUILTIN: u32 = 0x626C_746E;

    fn addr(selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope) -> AudioObjectPropertyAddress {
        AudioObjectPropertyAddress { mSelector: selector, mScope: scope, mElement: ELEM }
    }

    pub(crate) fn get<T: Copy>(obj: AudioObjectID, selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope) -> Option<T> {
        let a = addr(selector, scope);
        let mut val = unsafe { std::mem::zeroed::<T>() };
        let mut size = std::mem::size_of::<T>() as u32;
        let st = unsafe {
            AudioObjectGetPropertyData(obj, &a, 0, std::ptr::null(), &mut size, &mut val as *mut T as *mut c_void)
        };
        (st == 0 && size as usize >= std::mem::size_of::<T>()).then_some(val)
    }

    pub(crate) fn device_ids() -> Vec<AudioObjectID> {
        let a = addr(kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal);
        let mut size: u32 = 0;
        let st = unsafe { AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size) };
        if st != 0 || size == 0 {
            return vec![];
        }
        let n = size as usize / std::mem::size_of::<AudioObjectID>();
        let mut ids = vec![AudioObjectID::default(); n];
        let st = unsafe {
            AudioObjectGetPropertyData(SYSTEM_OBJECT, &a, 0, std::ptr::null(), &mut size, ids.as_mut_ptr() as *mut c_void)
        };
        if st != 0 { vec![] } else { ids }
    }

    pub(crate) fn name_of(obj: AudioObjectID) -> String {
        let a = addr(kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal);
        let mut cf: *mut c_void = std::ptr::null_mut();
        let mut size = std::mem::size_of::<*mut c_void>() as u32;
        let st = unsafe { AudioObjectGetPropertyData(obj, &a, 0, std::ptr::null(), &mut size, &mut cf as *mut _ as *mut c_void) };
        if st != 0 || cf.is_null() {
            return String::new();
        }
        unsafe {
            let cf = cf as CFStringRef;
            let mut buf = [0u8; 256];
            let ok = CFStringGetCString(cf, buf.as_mut_ptr() as *mut i8, buf.len() as i64, kCFStringEncodingUTF8);
            CFRelease(cf as *const c_void);
            if ok == 0 {
                return String::new();
            }
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        }
    }

    pub(crate) fn has_input_stream(obj: AudioObjectID) -> bool {
        let a = addr(kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput);
        let mut size: u32 = 0;
        let st = unsafe { AudioObjectGetPropertyDataSize(obj, &a, 0, std::ptr::null(), &mut size) };
        st == 0 && size > 0
    }

    fn is_bt(transport: u32) -> bool {
        transport == TRANSPORT_BT || transport == TRANSPORT_BTLE
    }

    pub(crate) fn bt_conflict_impl(selected_input: Option<&str>) -> Option<super::BtConflict> {
        let mut info: Vec<(String, u32, bool)> = device_ids()
            .into_iter()
            .map(|id| {
                (
                    name_of(id),
                    get::<u32>(id, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal)
                        .unwrap_or(0),
                    has_input_stream(id),
                )
            })
            .collect();

        // sem fone BT na saída não há "efeito copo" para prevenir
        let out_id = get::<AudioObjectID>(SYSTEM_OBJECT, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal)?;
        let out_transport = get::<u32>(out_id, kAudioDevicePropertyTransportType, kAudioObjectPropertyScopeGlobal).unwrap_or(0);
        if !is_bt(out_transport) {
            return None;
        }

        // microfone efetivo: o selecionado nas settings ou o padrão do sistema
        let in_name = match selected_input {
            Some(n) => n.to_string(),
            None => {
                let in_id = get::<AudioObjectID>(SYSTEM_OBJECT, kAudioHardwarePropertyDefaultInputDevice, kAudioObjectPropertyScopeGlobal)?;
                name_of(in_id)
            }
        };
        let (_, in_transport, _) = info.iter().find(|(n, _, _)| *n == in_name)?;
        if !is_bt(*in_transport) {
            return None;
        }

        // sugestão: microfone embutido primeiro; senão qualquer entrada não-BT
        info.sort_by_key(|(_, t, _)| if *t == TRANSPORT_BUILTIN { 0 } else { 1 });
        let suggestion = info
            .iter()
            .find(|(n, t, has_in)| *has_in && n != &in_name && !is_bt(*t))
            .map(|(n, _, _)| n.clone());

        Some(super::BtConflict { input_name: in_name, suggestion })
    }
}

#[cfg(test)]
mod tests {
    /// Sonda ao vivo (depende do hardware do momento): `cargo test bt_conflict -- --nocapture`
    #[test]
    #[cfg(target_os = "macos")]
    fn bt_conflict_probe() {
        use super::macos;
        eprintln!("bt_conflict(default) = {:?}", super::bt_conflict(None));
        eprintln!("bt_conflict(builtin) = {:?}", super::bt_conflict(Some("MacBook Air Microphone")));
        eprintln!("--- dispositivos (nome | transporte hex | tem entrada) ---");
        for id in macos::device_ids() {
            let t = macos::get::<u32>(
                id,
                coreaudio_sys::kAudioDevicePropertyTransportType,
                coreaudio_sys::kAudioObjectPropertyScopeGlobal,
            )
            .unwrap_or(0);
            eprintln!(
                "{:?} | {t:#010x} | entrada={}",
                macos::name_of(id),
                macos::has_input_stream(id)
            );
        }
    }
}
