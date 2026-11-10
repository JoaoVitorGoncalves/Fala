//! Diagnóstico: verificação de travamento do whisper.cpp no Windows.
//! Uso: cargo test --test whisper_hang -- --nocapture
//! WHISPER_TEST_GPU=0 desativa use_gpu (padrão: 1, como o app usa).

use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

fn params() -> FullParams<'static, 'static> {
    let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    p.set_print_progress(false);
    p.set_print_special(false);
    p.set_print_realtime(false);
    p.set_print_timestamps(false);
    p.set_suppress_blank(true);
    p.set_suppress_nst(true);
    p.set_language(Some("pt"));
    p
}

fn seno_4s() -> Vec<f32> {
    (0..16_000 * 4)
        .map(|i| {
            let env = 0.5 * (1.0 - (((i as f32) / 16_000.0 - 2.0).abs() / 2.0).min(1.0));
            (2.0 * std::f32::consts::PI * 440.0 * (i as f32) / 16_000.0).sin() * 0.3 * env
        })
        .collect()
}

fn rodar(nome: &str, samples: &[f32]) {
    let model = std::env::var("WHISPER_TEST_MODEL")
        .unwrap_or_else(|_| "C:/Users/senho/.fala/models/ggml-large-v3-turbo-q5_0.bin".into());
    let use_gpu = std::env::var("WHISPER_TEST_GPU").map(|v| v != "0").unwrap_or(true);
    println!("{nome}: modelo={model} use_gpu={use_gpu}");

    let t = Instant::now();
    let ctx = WhisperContext::new_with_params(
        &model,
        WhisperContextParameters { use_gpu, ..Default::default() },
    )
    .expect("carregar modelo");
    println!("[{:.1}s] modelo carregado", t.elapsed().as_secs_f32());

    let mut state = ctx.create_state().expect("create_state");
    println!("[{:.1}s] estado criado", t.elapsed().as_secs_f32());

    println!("[{:.1}s] chamando full() ({} amostras)...", t.elapsed().as_secs_f32(), samples.len());
    state.full(params(), samples).expect("whisper full");
    println!("[{:.1}s] full() RETORNOU", t.elapsed().as_secs_f32());
    println!("  segmentos: {}", state.full_n_segments());
}

#[test]
fn seno_limpo() {
    rodar("seno_limpo", &seno_4s());
}

#[test]
fn seno_com_nan_e_inf() {
    // simula driver USB entregando amostras não-finitas no meio da fala
    let mut s = seno_4s();
    for i in 0..s.len() / 8 {
        s[i * 2] = f32::NAN;
    }
    for i in 0..s.len() / 16 {
        s[i * 8 + 3] = f32::INFINITY;
    }
    // pipeline corrigida: a captura (audio.rs) sanitiza antes de chegar ao whisper
    let s: Vec<f32> = s.iter().map(|&v| if v.is_finite() { v } else { 0.0 }).collect();
    rodar("seno_com_nan_e_inf", &s);
}
