#!/usr/bin/env python3
"""
Prepara e valida o dataset coletado pelo Fala (~/.fala/dataset) para o treino com LoRA.
Verifica os arquivos de áudio, limpa o texto e gera as divisões de treino e validação (train.csv / val.csv).
Funciona com a biblioteca padrão do Python (sem depender de pacotes externos para validação).
"""

import argparse
import csv
import os
import random
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
        default=30.0,
        help="Duração máxima em segundos (padrão: 30.0s)",
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


def get_audio_duration(audio_file: Path) -> float:
    try:
        with wave.open(str(audio_file), "rb") as w:
            frames = w.getnframes()
            rate = w.getframerate()
            return frames / float(rate)
    except Exception:
        # Se soundfile estiver instalado, tenta como fallback
        try:
            import soundfile as sf
            info = sf.info(str(audio_file))
            return info.duration
        except Exception:
            return -1.0


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
    skipped_duration = 0
    skipped_empty = 0
    total_audio_secs = 0.0

    target_col = "clean_sentence" if (args.use_clean and rows and "clean_sentence" in rows[0]) else "sentence"
    print(f"Coluna alvo para texto: '{target_col}'")

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

        dur = get_audio_duration(audio_file)
        if dur < 0:
            skipped_not_found += 1
            continue

        if dur < args.min_duration or dur > args.max_duration:
            skipped_duration += 1
            continue

        total_audio_secs += dur
        valid_records.append({
            "audio_path": str(audio_file.resolve()),
            "sentence": text,
            "duration": f"{dur:.2f}",
        })

    valid_count = len(valid_records)
    print(f"\n--- Estatísticas de Qualidade ---")
    print(f"Amostras válidas: {valid_count} de {total_rows}")
    print(f"Áudio total válido: {total_audio_secs / 60.0:.2f} minutos ({total_audio_secs:.1f}s)")
    if skipped_not_found:
        print(f"Descartados (áudio não encontrado/ilegível): {skipped_not_found}")
    if skipped_duration:
        print(f"Descartados (fora do limite {args.min_duration}s-{args.max_duration}s): {skipped_duration}")
    if skipped_empty:
        print(f"Descartados (texto vazio): {skipped_empty}")

    if valid_count < 10:
        print("\n[RECOMENDAÇÃO] Você possui menos de 10 amostras válidas no momento.")
        print("Recomenda-se coletar pelo menos 50 a 100 amostras (cerca de 15 a 30 minutos de fala) antes de iniciar o treino com LoRA.")

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
