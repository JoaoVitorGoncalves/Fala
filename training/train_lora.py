#!/usr/bin/env python3
"""
Fine-Tuning do Whisper (Large-v3-Turbo) com LoRA / QLoRA otimizado para NVIDIA RTX (CUDA).
Treina adaptadores de atenção com baixo consumo de VRAM (<6 GB) para a sua voz e termos técnicos.
"""

import argparse
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Union
import pandas as pd
import soundfile as sf
import torch
from datasets import Dataset
from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training
from transformers import (
    BitsAndBytesConfig,
    Seq2SeqTrainer,
    Seq2SeqTrainingArguments,
    WhisperForConditionalGeneration,
    WhisperProcessor,
)


@dataclass
class DataCollatorSpeechSeq2SeqWithPadding:
    processor: Any

    def __call__(
        self, features: List[Dict[str, Union[List[int], torch.Tensor]]]
    ) -> Dict[str, torch.Tensor]:
        input_features = [
            {"input_features": feature["input_features"]} for feature in features
        ]
        batch = self.processor.feature_extractor.pad(
            input_features, return_tensors="pt"
        )

        label_features = [{"input_ids": feature["labels"]} for feature in features]
        labels_batch = self.processor.tokenizer.pad(label_features, return_tensors="pt")

        # Substitui padding por -100 para ignorar no cálculo de loss
        labels = labels_batch["input_ids"].masked_fill(
            labels_batch.attention_mask.ne(1), -100
        )

        # Remove token bos se presente no início
        if (labels[:, 0] == self.processor.tokenizer.bos_token_id).all().cpu().item():
            labels = labels[:, 1:]

        batch["labels"] = labels
        return batch


def load_dataset_from_csv(csv_path: Path, processor: WhisperProcessor) -> Dataset:
    df = pd.read_csv(csv_path)

    def prepare_item(row):
        audio_path = row["audio_path"]
        sentence = str(row["sentence"]).strip()
        speech, sr = sf.read(audio_path)
        if sr != 16000:
            import torchaudio.transforms as T
            speech_tensor = torch.tensor(speech).float()
            resampler = T.Resample(sr, 16000)
            speech = resampler(speech_tensor).numpy()

        input_features = processor.feature_extractor(
            speech, sampling_rate=16000
        ).input_features[0]
        labels = processor.tokenizer(sentence).input_ids

        return {
            "input_features": input_features,
            "labels": labels,
        }

    items = [prepare_item(row) for _, row in df.iterrows()]
    return Dataset.from_list(items)


def parse_args():
    parser = argparse.ArgumentParser(description="Treino do Whisper com LoRA")
    parser.add_argument(
        "--model-id",
        type=str,
        default="openai/whisper-large-v3-turbo",
        help="Modelo base da Hugging Face (padrão: openai/whisper-large-v3-turbo)",
    )
    parser.add_argument(
        "--data-dir",
        type=Path,
        default=Path(__file__).parent / "data",
        help="Diretório com train.csv e val.csv (gerados pelo prepare_dataset.py)",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=Path(__file__).parent / "output" / "whisper-lora",
        help="Diretório de saída para os adaptadores LoRA",
    )
    parser.add_argument("--epochs", type=int, default=5, help="Número de épocas (padrão: 5)")
    parser.add_argument("--batch-size", type=int, default=4, help="Batch size por dispositivo (padrão: 4)")
    parser.add_argument("--grad-accum", type=int, default=2, help="Passos de acúmulo de gradiente (padrão: 2)")
    parser.add_argument("--lr", type=float, default=1e-4, help="Learning rate (padrão: 1e-4)")
    parser.add_argument("--lora-r", type=int, default=32, help="Posto (rank) do LoRA (padrão: 32)")
    parser.add_argument("--lora-alpha", type=int, default=64, help="Alpha do LoRA (padrão: 64)")
    parser.add_argument("--no-qlora", action="store_true", help="Desativar quantização 4-bit (usa FP16 completo)")
    return parser.parse_args()


def main():
    args = parse_args()
    train_csv = args.data_dir / "train.csv"
    val_csv = args.data_dir / "val.csv"

    if not train_csv.exists() or not val_csv.exists():
        print(f"[ERRO] Arquivos train.csv ou val.csv não encontrados em {args.data_dir}")
        print("Execute primeiro: python prepare_dataset.py")
        return

    device = "cuda" if torch.cuda.is_available() else "cpu"
    print(f"=== Treinamento Whisper LoRA ===")
    print(f"Dispositivo: {device}")
    if device == "cuda":
        print(f"GPU: {torch.cuda.get_device_name(0)}")
        print(f"VRAM Total: {torch.cuda.get_device_properties(0).total_memory / 1e9:.2f} GB")

    print(f"Carregando processador ({args.model_id})...")
    processor = WhisperProcessor.from_pretrained(
        args.model_id, language="portuguese", task="transcribe"
    )

    print("Carregando e processando dataset...")
    train_dataset = load_dataset_from_csv(train_csv, processor)
    val_dataset = load_dataset_from_csv(val_csv, processor)
    print(f"Treino: {len(train_dataset)} exemplos | Validação: {len(val_dataset)} exemplos")

    print(f"Carregando modelo base ({args.model_id})...")
    if not args.no_qlora and device == "cuda":
        print("Ativando QLoRA (quantização 4-bit NF4 para economizar VRAM)...")
        bnb_config = BitsAndBytesConfig(
            load_in_4bit=True,
            bnb_4bit_quant_type="nf4",
            bnb_4bit_compute_dtype=torch.float16,
            bnb_4bit_use_double_quant=True,
        )
        model = WhisperForConditionalGeneration.from_pretrained(
            args.model_id,
            quantization_config=bnb_config,
            device_map="auto",
        )
        model = prepare_model_for_kbit_training(model)
    else:
        model = WhisperForConditionalGeneration.from_pretrained(
            args.model_id,
            torch_dtype=torch.float16 if device == "cuda" else torch.float32,
            device_map="auto" if device == "cuda" else None,
        )

    # Configuração dos adaptadores LoRA
    lora_config = LoraConfig(
        r=args.lora_r,
        lora_alpha=args.lora_alpha,
        target_modules=["q_proj", "v_proj"],
        lora_dropout=0.05,
        bias="none",
    )

    model = get_peft_model(model, lora_config)
    print("\n--- Parâmetros do Modelo ---")
    model.print_trainable_parameters()

    # Desativa cache durante o treino (incompatível com gradient checkpointing)
    model.config.use_cache = False
    model.config.forced_decoder_ids = None
    model.config.suppress_tokens = []

    training_args = Seq2SeqTrainingArguments(
        output_dir=str(args.output_dir),
        per_device_train_batch_size=args.batch_size,
        per_device_eval_batch_size=args.batch_size,
        gradient_accumulation_steps=args.grad_accum,
        learning_rate=args.lr,
        warmup_steps=30,
        num_train_epochs=args.epochs,
        eval_strategy="epoch",
        save_strategy="epoch",
        logging_steps=5,
        fp16=(device == "cuda"),
        predict_with_generate=True,
        generation_max_length=225,
        save_total_limit=2,
        load_best_model_at_end=True,
        metric_for_best_model="eval_loss",
        greater_is_better=False,
        report_to=["tensorboard"],
    )

    data_collator = DataCollatorSpeechSeq2SeqWithPadding(processor=processor)

    trainer = Seq2SeqTrainer(
        args=training_args,
        model=model,
        train_dataset=train_dataset,
        eval_dataset=val_dataset,
        data_collator=data_collator,
        tokenizer=processor.feature_extractor,
    )

    print("\nIniciando treinamento com LoRA...")
    trainer.train()

    print(f"\nSalvando adaptadores LoRA finais em {args.output_dir}...")
    trainer.save_model(str(args.output_dir))
    processor.save_pretrained(str(args.output_dir))

    print("\n[SUCESSO] Treinamento concluído com sucesso!")
    print(f"Para converter o modelo para o Fala (formato GGML q5_0), execute:")
    print(f"python export_ggml.py --lora-path {args.output_dir}")


if __name__ == "__main__":
    main()
