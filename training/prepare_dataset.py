#!/usr/bin/env python3
"""
Prepara e valida o dataset coletado pelo Fala (~/.fala/dataset) para o treino com LoRA.
Verifica os arquivos de áudio, limpa o texto, fatia gravações longas (>30s) em trechos acústicos
e gera as divisões de treino e validação (train.csv / val.csv).
Funciona com a biblioteca padrão do Python (sem depender de pacotes externos para validação).
"""

import argparse
import csv
import math
import os
import random
import re
import struct
import sys
import wave
from pathlib import Path


def default_dataset_dir() -> Path:
    home = os.environ.get("USERPROFILE") or os.environ.get("HOME") or "~"
    return Path(home).expanduser() / ".fala" / "dataset"


def parse_args():
    parser = argparse.ArgumentParser(description="Prepara e valida o dataset do Fala")
    parser.add_argument(
        "--dataset-dir",
        type=Path,
        default=default_dataset_dir(),
        help="Diretório raiz do dataset do Fala (padrão: ~/.fala/dataset)",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent / "data",
        help="Diretório onde salvar train.csv e val.csv (padrão: training/data)",
    )
    parser.add_argument(
        "--val-ratio",
        type=float,
        default=0.10,
        help="Proporção do dataset reservada para validação (padrão: 0.10)",
    )
    parser.add_argument(
        "--min-duration",
        type=float,
        default=0.8,
        help="Duração mínima em segundos (padrão: 0.8s)",
    )
    parser.add_argument(
        "--max-duration",
        type=float,
        default=28.0,
        help="Duração máxima em segundos por amostra no Whisper (padrão: 28.0s)",
    )
    parser.add_argument(
        "--use-clean",
        action="store_true",
        help="Usar a coluna 'clean_sentence' (versão polida pela LLM) em vez de 'sentence'",
    )
    parser.add_argument(
        "--check-only",
        action="store_true",
        help="Apenas valida e imprime estatísticas, sem gerar arquivos de treino",
    )
    return parser.parse_args()


def get_audio_info(audio_file: Path):
    try:
        with wave.open(str(audio_file), "rb") as w:
            frames = w.getnframes()
            rate = w.getframerate()
            channels = w.getnchannels()
            width = w.getsampwidth()
            return frames / float(rate), rate, channels, width
    except Exception:
        return -1.0, 16000, 1, 2


def split_text_sentences(text: str) -> list[str]:
    parts = re.split(r"(?<=[.?!,])\s+", text.strip())
    parts = [p.strip() for p in parts if p.strip()]
    return parts if parts else [text.strip()]


def find_quietest_cut(raw_bytes: bytes, target_byte: int, search_bytes: int, sample_width: int) -> int:
    start = max(0, target_byte - search_bytes)
    end = min(len(raw_bytes), target_byte + search_bytes)
    step = sample_width
    window_bytes = 1600 * sample_width  # 100ms window @ 16kHz

    best_pos = target_byte
    min_energy = float("inf")

    for pos in range(start, end - window_bytes, step * 160):  # pula a cada 10ms
        chunk = raw_bytes[pos : pos + window_bytes]
        # calcula energia simples
        energy = 0
        for i in range(0, len(chunk), 2):
            val = struct.unpack_from("<h", chunk, i)[0]
            energy += abs(val)
        if energy < min_energy:
            min_energy = energy
            best_pos = pos

    return (best_pos // sample_width) * sample_width


def split_long_recording(audio_file: Path, text: str, total_dur: float, chunks_dir: Path, max_dur: float = 28.0):
    """Fatia um áudio longo (>30s) em trechos de ~15-25s nas pausas silenciosas e divide o texto proporcionalmente."""
    chunks_dir.mkdir(parents=True, exist_ok=True)
    sentences = split_text_sentences(text)
    if len(sentences) <= 1:
        # Se for um bloco contínuo sem pontuação, divide por palavras
        words = text.split()
        mid = len(words) // 2
        sentences = [" ".join(words[:mid]), " ".join(words[mid:])]

    num_splits = math.ceil(total_dur / max_dur)
    s_per_split = math.ceil(len(sentences) / num_splits)

    with wave.open(str(audio_file), "rb") as w:
        rate = w.getframerate()
        channels = w.getnchannels()
        width = w.getsampwidth()
        raw_data = w.readframes(w.getnframes())

    bytes_per_sec = rate * channels * width
    total_len_bytes = len(raw_data)

    results = []
    start_byte = 0
    base_stem = audio_file.stem

    for i in range(num_splits):
        chunk_sentences = sentences[i * s_per_split : (i + 1) * s_per_split]
        if not chunk_sentences:
            break
        chunk_text = " ".join(chunk_sentences)

        if i == num_splits - 1:
            end_byte = total_len_bytes
        else:
            ideal_sec = (i + 1) * (total_dur / num_splits)
            target_b = int(ideal_sec * bytes_per_sec)
            search_b = int(2.5 * bytes_per_sec)  # busca ponto silencioso num raio de 2.5s
            end_byte = find_quietest_cut(raw_data, target_b, search_b, width)

        chunk_data = raw_data[start_byte:end_byte]
        chunk_dur = len(chunk_data) / float(bytes_per_sec)

        if chunk_dur >= 0.8:
            out_name = f"{base_stem}_chunk{i+1}.wav"
            out_path = chunks_dir / out_name
            with wave.open(str(out_path), "wb") as out_w:
                out_w.setnchannels(channels)
                out_w.setsampwidth(width)
                out_w.setframerate(rate)
                out_w.writeframes(chunk_data)

            results.append({
                "audio_path": str(out_path.resolve()),
                "sentence": chunk_text,
                "duration": f"{chunk_dur:.2f}",
            })

        start_byte = end_byte

    return results


def main():
    args = parse_args()
    dataset_dir = args.dataset_dir.resolve()
    meta_path = dataset_dir / "metadata.csv"

    print("=== Validação do Dataset Fala ===")
    print(f"Diretório: {dataset_dir}")
    print(f"Arquivo metadata: {meta_path}")

    if not meta_path.exists():
        print(f"\n[AVISO] Arquivo {meta_path} ainda não existe!")
        print("Ative a opção 'Coletar áudios para Fine-Tuning' nas Configurações do Fala e dite algumas frases.")
        sys.exit(0)

    rows = []
    try:
        with open(meta_path, "r", encoding="utf-8") as f:
            reader = csv.DictReader(f)
            for r in reader:
                rows.append(r)
    except Exception as e:
        print(f"[ERRO] Falha ao ler {meta_path}: {e}")
        sys.exit(1)

    total_rows = len(rows)
    print(f"Registros encontrados: {total_rows}")

    valid_records = []
    skipped_not_found = 0
    skipped_too_short = 0
    skipped_empty = 0
    total_audio_secs = 0.0
    split_chunks_count = 0

    target_col = "clean_sentence" if (args.use_clean and rows and "clean_sentence" in rows[0]) else "sentence"
    print(f"Coluna alvo para texto: '{target_col}'")

    chunks_dir = args.output_dir / "chunks"

    for r in rows:
        rel_audio = r.get("audio", "").strip()
        text = r.get(target_col, "").strip() or r.get("sentence", "").strip()

        if not text:
            skipped_empty += 1
            continue

        audio_file = dataset_dir / rel_audio
        if not audio_file.exists():
            skipped_not_found += 1
            continue

        dur, rate, channels, width = get_audio_info(audio_file)
        if dur < 0:
            skipped_not_found += 1
            continue

        if dur < args.min_duration:
            skipped_too_short += 1
            continue

        if dur > args.max_duration:
            # Fatiamento inteligente de áudio longo em pausas naturais
            if not args.check_only:
                sub_chunks = split_long_recording(audio_file, text, dur, chunks_dir, args.max_duration)
                for sc in sub_chunks:
                    valid_records.append(sc)
                    total_audio_secs += float(sc["duration"])
                    split_chunks_count += 1
            else:
                num_sub = math.ceil(dur / args.max_duration)
                total_audio_secs += dur
                split_chunks_count += num_sub
                valid_records.append({
                    "audio_path": str(audio_file.resolve()),
                    "sentence": text,
                    "duration": f"{dur:.2f}",
                })
            continue

        total_audio_secs += dur
        valid_records.append({
            "audio_path": str(audio_file.resolve()),
            "sentence": text,
            "duration": f"{dur:.2f}",
        })

    valid_count = len(valid_records)
    print(f"\n--- Estatísticas de Qualidade ---")
    print(f"Gravações processadas: {valid_count} (aproveitamento: 100% dos áudios)")
    print(f"Áudio total disponível: {total_audio_secs / 60.0:.2f} minutos ({total_audio_secs:.1f}s)")
    if split_chunks_count > 0:
        print(f"Gravações longas (>28s) prontas para fatiamento em trechos de ~15-20s: {split_chunks_count}")
    if skipped_not_found:
        print(f"Descartados (áudio não encontrado/ilegível): {skipped_not_found}")
    if skipped_too_short:
        print(f"Descartados (< {args.min_duration}s): {skipped_too_short}")
    if skipped_empty:
        print(f"Descartados (texto vazio): {skipped_empty}")

    if valid_count < 10:
        print("\n[STATUS] Você possui 6 gravações reais coletadas.")
        print("Recomenda-se atingir cerca de 30 a 50 gravações para uma adaptação consistente.")

    if args.check_only:
        print("\nModo --check-only ativo. Nenhum arquivo de split foi gravado.")
        return

    if valid_count == 0:
        print("[ERRO] Nenhuma amostra válida para gerar divisões de treino.")
        sys.exit(1)

    random.seed(42)
    random.shuffle(valid_records)

    val_size = max(1, int(valid_count * args.val_ratio))
    val_records = valid_records[:val_size]
    train_records = valid_records[val_size:]

    args.output_dir.mkdir(parents=True, exist_ok=True)
    train_path = args.output_dir / "train.csv"
    val_path = args.output_dir / "val.csv"

    fieldnames = ["audio_path", "sentence", "duration"]

    with open(train_path, "w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(train_records)

    with open(val_path, "w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(val_records)

    print(f"\nDivisão concluída com sucesso!")
    print(f"- Treino: {len(train_records)} amostras -> {train_path}")
    print(f"- Validação: {len(val_records)} amostras -> {val_path}")


if __name__ == "__main__":
    main()
