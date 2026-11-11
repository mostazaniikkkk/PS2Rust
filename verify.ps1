# Verificación completa del compilador de Rust para PlayStation 2.
#
#   powershell -ExecutionPolicy Bypass -File verify.ps1 -Emulator C:uta\pcsx2-qt.exe
#
# Compila todo, ejecuta los programas en PCSX2 y comprueba los resultados.
# Necesita PCSX2 con BIOS: -Emulator <pcsx2-qt.exe> o la variable PCSX2_EXE.

param([string]$Emulator = $env:PCSX2_EXE)

$ErrorActionPreference = 'Stop'
if (-not $Emulator -or -not (Test-Path $Emulator)) {
    Write-Host 'Indica PCSX2: verify.ps1 -Emulator C:uta\pcsx2-qt.exe (o define PCSX2_EXE)' -ForegroundColor Red
    exit 1
}
$env:PCSX2_EXE = (Resolve-Path $Emulator).Path
Set-Location $PSScriptRoot
$out = Join-Path $PSScriptRoot 'target\ps2'
New-Item -ItemType Directory -Force $out | Out-Null
$results = @()

function Step([string]$name, [scriptblock]$body) {
    Write-Host "`n=== $name" -ForegroundColor Cyan
    $ok = $false
    try { $ok = [bool](& $body) } catch { Write-Host "  $_" -ForegroundColor Red }
    $script:results += [pscustomobject]@{ Paso = $name; Resultado = $(if ($ok) { 'OK' } else { 'FALLO' }) }
    Write-Host ("  -> " + $(if ($ok) { 'OK' } else { 'FALLO' })) -ForegroundColor $(if ($ok) { 'Green' } else { 'Red' })
}

# Ejecuta `cargo <argumentos>` y devuelve su codigo de salida; stdout/stderr a archivos.
function Run([string]$arguments, [string]$stdout, [string]$stderr) {
    $p = Start-Process -FilePath 'cargo' -ArgumentList $arguments -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    return $p.ExitCode
}

Step 'Pruebas unitarias del legalizador R5900' {
    $code = Run 'test --release --manifest-path toolchain\cargo-ps2\Cargo.toml' "$out\unit.txt" "$out\unit.err"
    Get-Content "$out\unit.txt" | Select-String 'test result' | ForEach-Object { Write-Host "  $_" }
    $code -eq 0
}

Step 'Compilar los cuatro ejemplos para PS2' {
    $ok = $true
    foreach ($p in 'hello', 'selftest', 'cube', 'panic-demo') {
        $code = Run "ps2 build -p $p" "$out\build-$p.txt" "$out\build-$p.err"
        Get-Content "$out\build-$p.err" | Select-String 'R5900:' | ForEach-Object { Write-Host "  $p`: $($_.Line.Trim())" }
        if ($code -ne 0) { $ok = $false; Get-Content "$out\build-$p.err" | Select-Object -Last 15 | Write-Host }
    }
    $ok
}

Step 'El verificador rechaza el ELF sin legalizar y acepta el legalizado' {
    $raw = Run 'ps2 check target\mipsel-sony-ps2\release\selftest' "$out\check-raw.txt" "$out\check-raw.err"
    $fixed = Run 'ps2 check target\ps2\selftest.elf target\ps2\hello.elf target\ps2\cube.elf target\ps2\panic-demo.elf' "$out\check-ok.txt" "$out\check-ok.err"
    Get-Content "$out\check-raw.err" | Select-Object -First 1 | ForEach-Object { Write-Host "  $_" }
    Get-Content "$out\check-ok.txt" | ForEach-Object { Write-Host "  $_" }
    ($raw -ne 0) -and ($fixed -eq 0)
}

Step 'Bateria de pruebas: salida de la PS2 identica a la del PC' {
    $hostCode = Run 'run --release -q --manifest-path toolchain\selftest-host\Cargo.toml -- --escaped' "$out\selftest.expected.txt" "$out\selftest-host.err"
    $ps2Code = Run "ps2 run -p selftest --timeout 180 --log `"$out\selftest.actual.txt`"" "$out\selftest.stdout.txt" "$out\selftest.err"
    $expected = @(Get-Content "$out\selftest.expected.txt" -Encoding UTF8)
    $actual = @(Get-Content "$out\selftest.actual.txt" -Encoding UTF8)
    $start = [array]::IndexOf($actual, $expected[0])
    if ($start -lt 0) { Write-Host '  la PS2 no imprimio el inicio de la bateria'; return $false }
    $actual = @($actual[$start..($actual.Count - 1)])
    $diff = @(Compare-Object $expected $actual -SyncWindow 0)
    Write-Host "  PC: $($expected.Count) lineas (exit $hostCode); PS2: $($actual.Count) lineas (exit $ps2Code); diferencias: $($diff.Count)"
    Write-Host "  $($actual[-1])"
    $diff | Select-Object -First 10 | ForEach-Object { Write-Host "  $($_.SideIndicator) $($_.InputObject)" }
    ($hostCode -eq 0) -and ($ps2Code -eq 0) -and ($diff.Count -eq 0) -and ($expected.Count -eq $actual.Count)
}

Step 'Panico: mensaje por consola y codigo de salida 101' {
    $code = Run 'ps2 run -p panic-demo --timeout 90' "$out\panic.txt" "$out\panic.err"
    $text = Get-Content "$out\panic.txt" -Raw
    Get-Content "$out\panic.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 101) -and ($text -match 'index out of bounds: the len is 4 but the index is 7')
}

Step 'hello: consola + texto en pantalla (captura)' {
    $code = Run "ps2 run -p hello --timeout 90 --until `"texto en pantalla`" --screenshot `"$out\hello.png`"" "$out\hello.txt" "$out\hello.err"
    $text = Get-Content "$out\hello.txt" -Raw
    Get-Content "$out\hello.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 0) -and ($text -match 'suma = 385') -and ($text -match '0\.3333333333333333') -and (Test-Path "$out\hello.png")
}

Step 'cube: 240 frames de graficos en el GS (captura)' {
    $code = Run "ps2 run -p cube --timeout 120 --until `"240 frames`" --screenshot `"$out\cube.png`"" "$out\cube.txt" "$out\cube.err"
    Get-Content "$out\cube.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 0) -and (Test-Path "$out\cube.png")
}

Write-Host "`n=== Resumen" -ForegroundColor Cyan
$results | Format-Table -AutoSize | Out-String | Write-Host
$failed = @($results | Where-Object { $_.Resultado -ne 'OK' }).Count
if ($failed -eq 0) { Write-Host 'TODO OK' -ForegroundColor Green } else { Write-Host "$failed paso(s) fallaron" -ForegroundColor Red }
exit $failed
