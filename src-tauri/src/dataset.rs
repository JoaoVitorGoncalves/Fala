//! Coleta contínua de dataset para Fine-Tuning do Whisper.
//! Salva gravações reais do dia a dia em ~/.fala/dataset/wavs/*.wav (16 kHz mono 16-bit PCM)
//! acompanhadas de metadata.csv com a transcrição bruta e polida.

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Estatísticas do dataset coletado.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatasetStats {
    pub total_samples: usize,
    pub total_duration_secs: f32,
    pub dataset_dir: String,
}

/// Caminho raiz do dataset: ~/.fala/dataset
pub fn dataset_dir() -> Option<PathBuf> {
    crate::settings::dirs_home().map(|h| h.join(".fala").join("dataset"))
}

/// Cria o diretório raiz e a pasta de áudios wavs/ caso não existam.
pub fn ensure_dataset_dir() -> Result<PathBuf, String> {
    let dir = dataset_dir().ok_or_else(|| "diretório do usuário não encontrado".to_string())?;
    let wavs_dir = dir.join("wavs");
    fs::create_dir_all(&wavs_dir).map_err(|e| format!("create_dir_all dataset: {e}"))?;
    Ok(dir)
}

/// Escreve um arquivo de áudio WAV 16-bit PCM 16 kHz mono canônico (44 bytes de cabeçalho RIFF).
pub fn write_wav_16k_mono(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data_len = samples.len() * 2;
    let riff_size = (36 + data_len) as u32;

    let mut bytes = Vec::with_capacity(44 + data_len);
    // RIFF chunk descriptor
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&riff_size.to_le_bytes());
    bytes.extend_from_slice(b"WAVE");

    // "fmt " sub-chunk
    bytes.extend_from_slice(b"fmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes()); // subchunk size = 16 for PCM
    bytes.extend_from_slice(&1u16.to_le_bytes());  // AudioFormat 1 = PCM
    bytes.extend_from_slice(&1u16.to_le_bytes());  // NumChannels 1 = Mono
    bytes.extend_from_slice(&16000u32.to_le_bytes()); // SampleRate = 16000
    bytes.extend_from_slice(&32000u32.to_le_bytes()); // ByteRate = 16000 * 1 * 2
    bytes.extend_from_slice(&2u16.to_le_bytes());  // BlockAlign = 1 * 2
    bytes.extend_from_slice(&16u16.to_le_bytes()); // BitsPerSample = 16

    // "data" sub-chunk
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&(data_len as u32).to_le_bytes());

    // PCM 16-bit samples
    for &sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        let val = (clamped * 32767.0).round() as i16;
        bytes.extend_from_slice(&val.to_le_bytes());
    }

    fs::write(path, bytes).map_err(|e| format!("erro ao salvar wav: {e}"))
}

/// Salva uma amostra no dataset (WAV + linha no metadata.csv).
/// Executado de forma assíncrona; ignora áudios curtos ou vazios.
pub fn save_sample(
    samples: &[f32],
    sentence: &str,
    clean_sentence: Option<&str>,
) -> Result<(), String> {
    let sentence = sentence.trim();
    if sentence.is_empty() || samples.len() < 16_000 {
        // Ignora transcrições vazias ou com menos de 1 segundo de áudio
        return Ok(());
    }

    let root = ensure_dataset_dir()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let sec = now.as_secs();
    let cnt = COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = format!("fala_{sec}_{cnt}.wav");
    let wav_path = root.join("wavs").join(&filename);

    write_wav_16k_mono(&wav_path, samples)?;

    let duration_secs = samples.len() as f32 / 16_000.0;
    let meta_path = root.join("metadata.csv");

    let needs_header = !meta_path.exists() || fs::metadata(&meta_path).map(|m| m.len() == 0).unwrap_or(true);

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&meta_path)
        .map_err(|e| format!("abrir metadata.csv: {e}"))?;

    if needs_header {
        writeln!(file, "audio,sentence,clean_sentence,duration_secs")
            .map_err(|e| format!("escrever header metadata: {e}"))?;
    }

    // Escapa aspas no padrão CSV
    let s_esc = sentence.replace('"', "\"\"");
    let c_esc = clean_sentence.unwrap_or(sentence).replace('"', "\"\"");
    let rel_audio = format!("wavs/{filename}");

    writeln!(file, "\"{rel_audio}\",\"{s_esc}\",\"{c_esc}\",{duration_secs:.2}")
        .map_err(|e| format!("gravar linha metadata: {e}"))?;

    crate::diag(&format!(
        "dataset: amostra salva ({filename}, {duration_secs:.1}s)"
    ));

    Ok(())
}

/// Consulta o total de amostras e duração acumulada no metadata.csv.
pub fn get_stats() -> DatasetStats {
    let dir_str = dataset_dir()
        .map(|d| d.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut stats = DatasetStats {
        total_samples: 0,
        total_duration_secs: 0.0,
        dataset_dir: dir_str,
    };

    let meta_path = match dataset_dir() {
        Some(d) => d.join("metadata.csv"),
        None => return stats,
    };

    let content = match fs::read_to_string(&meta_path) {
        Ok(c) => c,
        Err(_) => return stats,
    };

    let mut lines = content.lines();
    // pula header
    let _ = lines.next();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        stats.total_samples += 1;
        // o último campo após a última vírgula é duration_secs
        if let Some(pos) = line.rfind(',') {
            if let Ok(dur) = line[pos + 1..].trim().parse::<f32>() {
                stats.total_duration_secs += dur;
            }
        }
    }

    stats
}
