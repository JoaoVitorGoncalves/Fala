# Assina os binários do Fala com o certificado de código local (autoassinado).
# O certificado precisa existir em Cert:\CurrentUser\My ("Fala Code Signing (local)")
# e estar confiado na máquina (LocalMachine\Root + TrustedPublisher) — ver README.
$ErrorActionPreference = "Continue"
$signtool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $signtool) { Write-Output "signtool não encontrado; pulando assinatura"; exit 0 }
$files = @(
    "D:\React\Fala\src-tauri\target\release\fala.exe",
    "D:\React\Fala\src-tauri\target\release\bundle\nsis\Fala_0.1.0_x64-setup.exe",
    "D:\React\Fala\src-tauri\target\release\bundle\msi\Fala_0.1.0_x64_en-US.msi"
)
foreach ($f in $files) {
    if (Test-Path $f) {
        $null = & $signtool sign /fd SHA256 /a /n "Fala Desenvolvimento" $f 2>&1
        Write-Output "assinado: $(Split-Path $f -Leaf)"
    }
}
