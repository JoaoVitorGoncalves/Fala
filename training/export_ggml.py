#!/usr/bin/env python3
"""
Funde os adaptadores LoRA de volta ao modelo base Whisper e converte para GGML (formato do Fala).
Gera o modelo quantizado em q5_0 pronto para rodar com aceleração máxima (CUDA no Windows / Metal no Mac).
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path
import torch
from peft import PeftModel
from transformers import WhisperForConditionalGeneration, WhisperProcessor


def default_fala_models_dir() -> Path:
    home = os.environ.get("USERPROFILE") or os.environ.get("HOME") or "~"
    return Path(home).expanduser() / ".fala" / "models"


def parse_args():
    parser = argparse.ArgumentParser(description="Exporta modelo LoRA do Whisper para formato GGML")
    parser.add_argument(
        "--lora-path",
        type=Path,
        default=Path(__file__).parent / "output" / "whisper-lora",
        help="Diretório onde os adaptadores LoRA foram salvos pelo train_lora.py",
    )
    parser.add_argument(
        "--base-model",
        type=str,
        default="openai/whisper-large-v3-turbo",
        help="Identificador do modelo base Hugging Face",
    )
    parser.add_argument(
        "--merged-dir",
        type=Path,
        default=Path(__file__).parent / "output" / "whisper-merged",
        help="Diretório temporário para salvar o modelo Hugging Face com pesos fundidos",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent / "output" / "ggml",
        help="Diretório de saída para os arquivos .bin GGML",
    )
    parser.add_argument(
        "--install",
        action="store_true",
        help="Copia automaticamente o modelo gerado para ~/.fala/models/",
    )
    return parser.parse_args()


def merge_lora_to_base(base_model_id: str, lora_path: Path, output_dir: Path):
    print(f"\n1. Fundindo adaptadores LoRA ao modelo base...")
    print(f"Base: {base_model_id}")
    print(f"LoRA: {lora_path}")

    model = WhisperForConditionalGeneration.from_pretrained(
        base_model_id,
        torch_dtype=torch.float16,
        low_cpu_mem_usage=True,
    )
    model = PeftModel.from_pretrained(model, str(lora_path))
    model = model.merge_and_unload()

    processor = WhisperProcessor.from_pretrained(str(lora_path))

    output_dir.mkdir(parents=True, exist_ok=True)
    print(f"Salvando modelo fundido em {output_dir}...")
    model.save_pretrained(str(output_dir))
    processor.save_pretrained(str(output_dir))
    print("Fusão concluída com sucesso!")


def ensure_whisper_cpp_repo(cache_dir: Path) -> Path:
    repo_dir = cache_dir / "whisper.cpp"
    if not (repo_dir / "models" / "convert-h5-to-ggml.py").exists():
        print(f"\n2. Baixando scripts de conversão do whisper.cpp em {repo_dir}...")
        repo_dir.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "clone", "--depth", "1", "https://github.com/ggerganov/whisper.cpp.git", str(repo_dir)],
            check=True,
        )
    return repo_dir


def convert_to_ggml(whisper_cpp_dir: Path, hf_model_dir: Path, output_dir: Path) -> Path:
    print(f"\n3. Convertendo modelo Hugging Face para formato GGML...")
    output_dir.mkdir(parents=True, exist_ok=True)
    convert_script = whisper_cpp_dir / "models" / "convert-h5-to-ggml.py"

    cmd = [
        sys.executable,
        str(convert_script),
        str(hf_model_dir),
        str(whisper_cpp_dir),
        str(output_dir),
    ]
    subprocess.run(cmd, check=True)

    ggml_model = output_dir / "ggml-model.bin"
    if not ggml_model.exists():
        # Algumas versões salvam no próprio diretório do modelo
        candidates = list(output_dir.glob("ggml-*.bin")) + list(hf_model_dir.glob("ggml-*.bin"))
        if candidates:
            ggml_model = candidates[0]

    print(f"Arquivo GGML gerado: {ggml_model} ({ggml_model.stat().st_size / 1e6:.1f} MB)")
    return ggml_model


def quantize_model(whisper_cpp_dir: Path, ggml_path: Path, output_dir: Path) -> Path:
    print(f"\n4. Quantizando modelo para q5_0 (alta velocidade e fidelidade)...")
    q5_path = output_dir / "ggml-large-v3-turbo-custom-q5_0.bin"

    # Procura quantize.exe compilado
    quantize_bins = [
        whisper_cpp_dir / "build" / "bin" / "quantize.exe",
        whisper_cpp_dir / "build" / "bin" / "Release" / "quantize.exe",
        whisper_cpp_dir / "build" / "bin" / "quantize",
        whisper_cpp_dir / "quantize.exe",
        whisper_cpp_dir / "quantize",
    ]
    quantize_cmd = None
    for b in quantize_bins:
        if b.exists():
            quantize_cmd = str(b)
            break

    if not quantize_cmd and shutil.which("quantize"):
        quantize_cmd = "quantize"

    if not quantize_cmd:
        print("\n[INFO] Binário 'quantize' do whisper.cpp não encontrado compilado.")
        print("Compilando ferramenta de quantização via cmake...")
        try:
            build_dir = whisper_cpp_dir / "build"
            build_dir.mkdir(exist_ok=True)
            subprocess.run(["cmake", "-B", str(build_dir), str(whisper_cpp_dir)], check=True)
            subprocess.run(["cmake", "--build", str(build_dir), "--config", "Release", "--target", "quantize"], check=True)
            for b in quantize_bins:
                if b.exists():
                    quantize_cmd = str(b)
                    break
        except Exception as e:
            print(f"[AVISO] Não foi possível compilar quantize automaticamente: {e}")
            print(f"O modelo em precisão total (FP16) segue disponível em: {ggml_path}")
            return ggml_path

    if quantize_cmd:
        cmd = [quantize_cmd, str(ggml_path), str(q5_path), "q5_0"]
        subprocess.run(cmd, check=True)
        print(f"Modelo quantizado com sucesso: {q5_path} ({q5_path.stat().st_size / 1e6:.1f} MB)")
        return q5_path

    return ggml_path


def main():
    args = parse_args()
    if not args.lora_path.exists():
        print(f"[ERRO] Diretório LoRA {args.lora_path} não encontrado!")
        print("Execute primeiro: python train_lora.py")
        sys.exit(1)

    merge_lora_to_base(args.base_model, args.lora_path, args.merged_dir)

    tools_cache = Path(__file__).parent / ".tools"
    whisper_cpp_dir = ensure_whisper_cpp_repo(tools_cache)

    ggml_raw = convert_to_ggml(whisper_cpp_dir, args.merged_dir, args.output_dir)
    final_model = quantize_model(whisper_cpp_dir, ggml_raw, args.output_dir)

    print("\n=== Conclusão da Exportação ===")
    print(f"Modelo pronto: {final_model}")

    if args.install:
        dest_dir = default_fala_models_dir()
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest_file = dest_dir / "ggml-large-v3-turbo-custom-q5_0.bin"
        shutil.copy2(final_model, dest_file)
        print(f"[INSTALADO] Modelo copiado para o Fala: {dest_file}")
        print("\nPara ativá-lo no Fala:")
        print("1. Abra as Configurações do Fala")
        print(f"2. No campo 'Modelo local' em 'Modelo Whisper', insira:")
        print(f"   {dest_file}")
        print("3. Clique em Salvar alterações.")


if __name__ == "__main__":
    main()
