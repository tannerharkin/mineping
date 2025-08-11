# PowerShell build script for Windows
Write-Host "Building mineping API..." -ForegroundColor Green

# Check if Rust is installed
if (!(Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust is not installed. Please install from https://rustup.rs" -ForegroundColor Red
    exit 1
}

# Build release version
Write-Host "Building optimized release build..." -ForegroundColor Yellow
cargo build --release

if ($LASTEXITCODE -eq 0) {
    Write-Host "Build successful!" -ForegroundColor Green
    Write-Host "Binary location: .\target\release\mineping.exe" -ForegroundColor Cyan
    
    # Optional: Create a standalone directory with the exe
    $outputDir = ".\dist"
    if (!(Test-Path $outputDir)) {
        New-Item -ItemType Directory -Path $outputDir | Out-Null
    }
    
    Copy-Item ".\target\release\mineping.exe" -Destination $outputDir -Force
    Write-Host "Executable copied to: $outputDir\mineping.exe" -ForegroundColor Cyan
} else {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}