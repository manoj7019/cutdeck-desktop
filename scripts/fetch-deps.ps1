# Downloads the native pieces Cutdeck bundles (Windows):
#   - FFmpeg + FFprobe  -> src-tauri\binaries\cutdeck-{ffmpeg,ffprobe}-<target-triple>.exe
#   - ONNX Runtime 1.22 -> src-tauri\resources\onnxruntime\
#   - whisper.cpp CLI   -> src-tauri\resources\whisper\ (auto captions; models download on first use)
# Usage (PowerShell):  .\scripts\fetch-deps.ps1
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"   # makes Invoke-WebRequest much faster
$root = Split-Path $PSScriptRoot -Parent
$triple = ((rustc -vV) | Select-String "^host:").ToString().Split(" ")[1].Trim()
$ortVer = "1.22.0"
$whisperVer = "b5454"
$bin = Join-Path $root "src-tauri\binaries"
$ortDir = Join-Path $root "src-tauri\resources\onnxruntime"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("cutdeck-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $bin, $ortDir, $tmp | Out-Null

switch ($triple) {
  "x86_64-pc-windows-msvc"  { $ff = "win64";    $ort = "win-x64" }
  "aarch64-pc-windows-msvc" { $ff = "winarm64"; $ort = "win-arm64" }
  default { throw "Unsupported target $triple" }
}
Write-Host "Target: $triple"

Write-Host "FFmpeg (BtbN build with NVENC / Quick Sync / AMF)"
$ffZip = Join-Path $tmp "ff.zip"
Invoke-WebRequest "https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-$ff-gpl.zip" -OutFile $ffZip
Expand-Archive $ffZip -DestinationPath (Join-Path $tmp "ff")
$ffBin = Get-ChildItem -Recurse -Path (Join-Path $tmp "ff") -Filter ffmpeg.exe | Select-Object -First 1
Copy-Item $ffBin.FullName (Join-Path $bin "cutdeck-ffmpeg-$triple.exe") -Force
Copy-Item (Join-Path $ffBin.DirectoryName "ffprobe.exe") (Join-Path $bin "cutdeck-ffprobe-$triple.exe") -Force

Write-Host "ONNX Runtime $ortVer"
$ortZip = Join-Path $tmp "ort.zip"
Invoke-WebRequest "https://github.com/microsoft/onnxruntime/releases/download/v$ortVer/onnxruntime-$ort-$ortVer.zip" -OutFile $ortZip
Expand-Archive $ortZip -DestinationPath (Join-Path $tmp "ort")
$lib = Get-ChildItem -Recurse -Path (Join-Path $tmp "ort") -Filter onnxruntime.dll | Select-Object -First 1
Copy-Item $lib.FullName $ortDir -Force
$shared = Join-Path $lib.DirectoryName "onnxruntime_providers_shared.dll"
if (Test-Path $shared) { Copy-Item $shared $ortDir -Force }

Write-Host "whisper.cpp $whisperVer (auto captions)"
$whDir = Join-Path $root "src-tauri\resources\whisper"
New-Item -ItemType Directory -Force -Path $whDir | Out-Null
$whZip = Join-Path $tmp "whisper.zip"
Invoke-WebRequest "https://github.com/ggml-org/whisper.cpp/releases/download/$whisperVer/whisper-bin-x64.zip" -OutFile $whZip
Expand-Archive $whZip -DestinationPath (Join-Path $tmp "whisper")
$cli = Get-ChildItem -Recurse -Path (Join-Path $tmp "whisper") -Filter whisper-cli.exe | Select-Object -First 1
Copy-Item $cli.FullName $whDir -Force
Get-ChildItem $cli.DirectoryName -Filter *.dll | Where-Object { $_.Name -match '^(whisper|ggml.*)\.dll$' } | Copy-Item -Destination $whDir -Force

Remove-Item -Recurse -Force $tmp
& (Join-Path $bin "cutdeck-ffmpeg-$triple.exe") -hide_banner -version | Select-Object -First 1
Write-Host "Done."
