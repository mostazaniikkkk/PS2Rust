# Verificación completa del compilador de Rust para PlayStation 2 (EE e IOP).
#
#   powershell -ExecutionPolicy Bypass -File verify.ps1 -Emulator C:\PCSX2\pcsx2-qt.exe
#
# Compila todo, ejecuta los programas en PCSX2 y comprueba los resultados.
# Necesita PCSX2 con BIOS: -Emulator <pcsx2-qt.exe> o la variable PCSX2_EXE.

param([string]$Emulator = $env:PCSX2_EXE)

$ErrorActionPreference = 'Stop'
if (-not $Emulator -or -not (Test-Path $Emulator)) {
    Write-Host 'Indica PCSX2: verify.ps1 -Emulator C:\PCSX2\pcsx2-qt.exe (o define PCSX2_EXE)' -ForegroundColor Red
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
function Run([string]$arguments, [string]$stdout, [string]$stderr, [string]$dir = $PSScriptRoot) {
    $p = Start-Process -FilePath 'cargo' -ArgumentList $arguments -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -WorkingDirectory $dir
    return $p.ExitCode
}

# Compara la salida del PC con la de la PS2 desde la primera linea esperada.
function SameOutput([string]$expectedFile, [string]$actualFile) {
    $expected = @(Get-Content $expectedFile -Encoding UTF8)
    $actual = @(Get-Content $actualFile -Encoding UTF8)
    $start = [array]::IndexOf($actual, $expected[0])
    if ($start -lt 0) { Write-Host '  la PS2 no imprimio el inicio de la bateria'; return $false }
    $actual = @($actual[$start..($actual.Count - 1)])
    $diff = @(Compare-Object $expected $actual -SyncWindow 0)
    Write-Host "  PC: $($expected.Count) lineas; PS2: $($actual.Count) lineas; diferencias: $($diff.Count)"
    Write-Host "  $($actual[-1])"
    $diff | Select-Object -First 10 | ForEach-Object { Write-Host "  $($_.SideIndicator) $($_.InputObject)" }
    ($diff.Count -eq 0) -and ($expected.Count -eq $actual.Count)
}

Step 'Pruebas unitarias del legalizador R5900' {
    $code = Run 'test --release --manifest-path toolchain\cargo-ps2\Cargo.toml' "$out\unit.txt" "$out\unit.err"
    Get-Content "$out\unit.txt" | Select-String 'test result' | ForEach-Object { Write-Host "  $_" }
    $code -eq 0
}

$examples = 'hello', 'selftest', 'cube', 'panic-demo', 'std-hello', 'std-selftest', 'std-panic', 'iop-demo', 'iop-selftest-runner'
$modules = 'rustiop', 'iop-selftest'

Step 'Compilar los nueve programas del EE (cuatro sin std, cinco con std)' {
    $ok = $true
    foreach ($p in $examples) {
        $code = Run "ps2 build -p $p" "$out\build-$p.txt" "$out\build-$p.err"
        Get-Content "$out\build-$p.err" | Select-String 'R5900:' | ForEach-Object { Write-Host "  $p`: $($_.Line.Trim())" }
        if ($code -ne 0) { $ok = $false; Get-Content "$out\build-$p.err" | Select-Object -Last 15 | Write-Host }
    }
    $ok
}

Step 'IOP: compilar los dos modulos IRX; reubicados como la BIOS, identicos al enlazador' {
    $ok = $true
    foreach ($p in $modules) {
        $code = Run "ps2 build --iop --verify-relocs -p $p" "$out\build-$p.txt" "$out\build-$p.err"
        $log = @(Get-Content "$out\build-$p.err" -Encoding UTF8)
        $log | Select-String 'IOP:|R3000:|verificada' | ForEach-Object { Write-Host "  $p`: $($_.Line.Trim())" }
        if (($code -ne 0) -or -not ($log -match 'verificada: cargado en')) { $ok = $false; $log | Select-Object -Last 15 | Write-Host }
    }
    $ok
}

Step 'El verificador rechaza el ELF sin legalizar y acepta los legalizados y los IRX' {
    $raw = Run 'ps2 check target\mipsel-sony-ps2\release\selftest' "$out\check-raw.txt" "$out\check-raw.err"
    $elfs = (@($examples | ForEach-Object { "target\ps2\$_.elf" }) + @($modules | ForEach-Object { "target\ps2\$_.irx" })) -join ' '
    $fixed = Run "ps2 check $elfs" "$out\check-ok.txt" "$out\check-ok.err"
    Get-Content "$out\check-raw.err" | Select-Object -First 1 | ForEach-Object { Write-Host "  $_" }
    Get-Content "$out\check-ok.txt" | ForEach-Object { Write-Host "  $_" }
    ($raw -ne 0) -and ($fixed -eq 0)
}

Step 'Bateria core/alloc (no_std): salida de la PS2 identica a la del PC' {
    $hostCode = Run 'run --release -q --manifest-path toolchain\selftest-host\Cargo.toml -- --escaped' "$out\selftest.expected.txt" "$out\selftest-host.err"
    $ps2Code = Run "ps2 run -p selftest --timeout 180 --log `"$out\selftest.actual.txt`"" "$out\selftest.stdout.txt" "$out\selftest.err"
    $same = SameOutput "$out\selftest.expected.txt" "$out\selftest.actual.txt"
    Write-Host "  exit PC $hostCode, PS2 $ps2Code"
    ($hostCode -eq 0) -and ($ps2Code -eq 0) -and $same
}

Step 'Bateria std (hilos, sync, tiempo, archivos, entorno): PS2 identica al PC' {
    # El PC usa el mismo directorio que ve la PS2 como host: (el del ELF).
    $manifest = Join-Path $PSScriptRoot 'toolchain\selftest-host\Cargo.toml'
    $hostCode = Run "run --release -q --manifest-path `"$manifest`" -- --std" "$out\std-selftest.expected.txt" "$out\std-selftest-host.err" $out
    $ps2Code = Run "ps2 run -p std-selftest --timeout 240 --log `"$out\std-selftest.actual.txt`"" "$out\std-selftest.stdout.txt" "$out\std-selftest.err"
    $same = SameOutput "$out\std-selftest.expected.txt" "$out\std-selftest.actual.txt"
    Write-Host "  exit PC $hostCode, PS2 $ps2Code"
    ($hostCode -eq 0) -and ($ps2Code -eq 0) -and $same -and -not (Test-Path "$out\std_selftest_tmp")
}

Step 'IOP: bateria core/alloc en el R3000A: salida del IOP identica a la del PC' {
    $hostCode = Run 'run --release -q --manifest-path toolchain\selftest-host\Cargo.toml -- --iop' "$out\iop-selftest.expected.txt" "$out\iop-selftest-host.err"
    $ps2Code = Run "ps2 run -p iop-selftest-runner --timeout 240 --log `"$out\iop-selftest.actual.txt`"" "$out\iop-selftest.stdout.txt" "$out\iop-selftest.err"
    $same = SameOutput "$out\iop-selftest.expected.txt" "$out\iop-selftest.actual.txt"
    Write-Host "  exit PC $hostCode, PS2 $ps2Code"
    ($hostCode -eq 0) -and ($ps2Code -eq 0) -and $same
}

Step 'IOP: modulo en Rust sirviendo RPC a un programa del EE (captura)' {
    $code = Run "ps2 run -p iop-demo --timeout 120 --until `"pantalla lista`" --screenshot `"$out\iop-demo.png`"" "$out\iop-demo.txt" "$out\iop-demo.err"
    $text = Get-Content "$out\iop-demo.txt" -Raw
    Get-Content "$out\iop-demo.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 0) -and ($text -match 'rustiop: Rust en el IOP') -and ($text -match 'strlen = 7') -and ($text -match 'demo IOP: 7/7 comprobaciones correctas') -and (Test-Path "$out\iop-demo.png")
}

Step 'Panico: mensaje por consola y codigo de salida 101' {
    $code = Run 'ps2 run -p panic-demo --timeout 90' "$out\panic.txt" "$out\panic.err"
    $text = Get-Content "$out\panic.txt" -Raw
    Get-Content "$out\panic.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 101) -and ($text -match 'index out of bounds: the len is 4 but the index is 7')
}

Step 'Panico en un hilo con std: mensaje y codigo 101' {
    $code = Run 'ps2 run -p std-panic --timeout 90' "$out\std-panic.txt" "$out\std-panic.err"
    $text = Get-Content "$out\std-panic.txt" -Raw
    Get-Content "$out\std-panic.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 101) -and ($text -match "thread 'calculo'") -and ($text -match 'index out of bounds: the len is 4 but the index is 7')
}

Step 'std-hello: hilos + canal + HashMap + archivo + reloj + GS (captura)' {
    $code = Run "ps2 run -p std-hello --timeout 120 --until `"pantalla lista`" --screenshot `"$out\std-hello.png`"" "$out\std-hello.txt" "$out\std-hello.err"
    $text = Get-Content "$out\std-hello.txt" -Raw
    Get-Content "$out\std-hello.txt" | ForEach-Object { Write-Host "  $_" }
    ($code -eq 0) -and ($text -match '-> 9592') -and ($text -match 'llaves=1') -and ($text -match 'reloj de la consola: 20') -and (Test-Path "$out\std-hello.png")
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
