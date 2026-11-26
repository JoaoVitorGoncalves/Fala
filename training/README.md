# Fine-Tuning do Whisper (LoRA) para o Fala

Pipeline completo para treinar uma versão exclusiva do **Whisper Large-v3-Turbo** adaptada à sua voz, ao seu microfone e ao seu vocabulário técnico, com quantização final para **GGML `q5_0`** (compatível tanto com a RTX no Windows quanto com Metal no Mac).

---

## Como Funciona

```text
1. Seu uso diário no Fala (áudio 16kHz + transcrição)
   └── Salvo em ~/.fala/dataset/ (wavs/ + metadata.csv)
        ↓
2. Validação e divisão (prepare_dataset.py)
   └── Gera data/train.csv e data/val.csv
        ↓
3. Treinamento com LoRA / QLoRA (train_lora.py)
   └── Ajusta os pesos de atenção do Whisper na sua GPU (RTX 4060)
        ↓
4. Fusão e quantização GGML (export_ggml.py)
   └── Gera ggml-large-v3-turbo-custom-q5_0.bin
        ↓
5. Ativação no Fala (Windows e Mac)
   └── Modelo exclusivo rodando local em ~1 segundo por ditado!
```

---

## 1. Coleta de Dados no App

1. Abra as Configurações do **Fala** (pelo ícone na bandeja).
2. Na seção **Treinamento do Modelo (Dataset)**, marque a opção:
   **"Coletar áudios para Fine-Tuning"**.
3. Use o aplicativo normalmente no seu dia a dia.
4. **Recomendação de volume:** Colete entre **50 e 200 gravações** (cerca de 15 a 45 minutos de áudio real) para obter uma adaptação consistente da sua voz e dos seus termos técnicos.
5. Você pode acompanhar a contagem de amostras diretamente na tela de Configurações ou clicando em **"Abrir pasta"** (`~/.fala/dataset`).

---

## 2. Preparação do Ambiente de Treinamento

No Windows (PowerShell ou terminal):

```powershell
cd D:\React\Fala\training

# Cria um ambiente virtual Python isolado
python -m venv venv
.\venv\Scripts\Activate.ps1

# Instala PyTorch com suporte a CUDA e as dependências
pip install torch torchvision torchaudio --index-url https://download.pytorch.org/whl/cu121
pip install -r requirements.txt
```

No macOS (Apple Silicon / MPS):

```bash
cd ~/Projects/fala/training

python3 -m venv venv
source venv/bin/activate

# PyTorch com suporte a MPS (Metal) — bitsandbytes não é necessário no Mac
pip install torch torchaudio transformers peft datasets accelerate soundfile evaluate jiwer tqdm pandas
```

---

## 3. Validação do Dataset

Antes de treinar, verifique a saúde das amostras coletadas:

```bash
python prepare_dataset.py --check-only
```

Ele exibirá quantas amostras estão válidas, o tempo total de áudio acumulado e se há algum arquivo corrompido.

Quando estiver pronto para treinar, gere as divisões de treino (90%) e validação (10%):

```bash
python prepare_dataset.py
```

*Dica:* Se você usa a IA do servidor (Ollama) e quer que o modelo aprenda diretamente o texto já pontuado e formatado pela LLM, use a flag `--use-clean`:
```bash
python prepare_dataset.py --use-clean
```

---

## 4. Treinamento com LoRA na GPU

Inicie o treinamento com QLoRA (quantização 4-bit NF4 para manter o consumo de VRAM abaixo de 6 GB na sua RTX 4060):

```bash
python train_lora.py --epochs 5 --batch-size 4 --grad-accum 2
```

### O que acontece durante o treino:
- O modelo base `openai/whisper-large-v3-turbo` é baixado da Hugging Face (~1.6 GB, apenas na primeira vez).
- Apenas as matrizes de atenção (`q_proj` e `v_proj`) são atualizadas (~1% dos parâmetros do modelo).
- Duração estimada na RTX 4060: entre **10 e 25 minutos** dependendo da quantidade de amostras.
- Os adaptadores treinados serão salvos em `training/output/whisper-lora/`.

---

## 5. Conversão e Quantização para GGML (`q5_0`)

Converta os adaptadores LoRA de volta para o formato de alto desempenho do Fala:

```bash
python export_ggml.py --install
```

Esse comando realiza:
1. A fusão das matrizes LoRA nos pesos originais do Whisper.
2. A conversão da arquitetura para formato GGML (`whisper.cpp`).
3. A quantização para `q5_0` (~550 MB), mantendo a fidelidade acústica.
4. Com a flag `--install`, copia o arquivo final diretamente para:
   `C:\Users\senho\.fala\models\ggml-large-v3-turbo-custom-q5_0.bin`

---

## 6. Ativação no Fala (Windows e Mac)

### No Windows:
1. Abra as Configurações do **Fala**.
2. Na seção **Modelo Whisper**, no campo **Modelo local**, insira:
   ```text
   C:\Users\senho\.fala\models\ggml-large-v3-turbo-custom-q5_0.bin
   ```
3. Clique em **Salvar alterações**.

### No Mac:
1. Copie o arquivo `ggml-large-v3-turbo-custom-q5_0.bin` gerado para o seu Mac na pasta `~/.fala/models/`.
2. Nas Configurações do Fala no Mac, aponte para:
   ```text
   /Users/<seu-usuario>/.fala/models/ggml-large-v3-turbo-custom-q5_0.bin
   ```
3. O modelo rodará nativamente na GPU Apple Silicon (Metal) com a sua voz personalizada!
