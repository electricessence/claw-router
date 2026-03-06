#Requires -Version 7
<#
.SYNOPSIS
    Tests Ollama connectivity and model responsiveness on LXC 200 via Proxmox.

.DESCRIPTION
    Runs a series of checks against Ollama on the language LXC:
    1. HTTP connectivity (root endpoint)
    2. Loaded models (api/ps)
    3. Available models (api/tags)
    4. Simple generate request with timeout

    Uses base64-encoded JSON to avoid SSH escaping issues.

.PARAMETER SshAlias
    SSH alias for the Proxmox host. Default: cortex.

.PARAMETER LxcId
    LXC container ID. Default: 200.

.PARAMETER Model
    Ollama model to test. Default: qwen3:1.7b.

.PARAMETER TimeoutSec
    Timeout for the generate request. Default: 30.

.EXAMPLE
    .\Test-OllamaHealth.ps1
    Checks Ollama health and tests qwen3:1.7b on LXC 200.
#>
[CmdletBinding()]
param(
    [string]$SshAlias   = 'cortex',
    [int]$LxcId         = 200,
    [string]$Model      = 'qwen3:1.7b',
    [int]$TimeoutSec    = 30
)

$ErrorActionPreference = 'Stop'

function Invoke-LxcCommand {
    param([string]$Command, [string]$Label)
    Write-Host "`n[$Label]" -ForegroundColor Cyan
    $result = ssh $SshAlias "pct exec $LxcId -- bash -c '$Command'" 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  FAILED (exit $LASTEXITCODE)" -ForegroundColor Red
        Write-Host "  $result" -ForegroundColor DarkYellow
        return $null
    }
    return $result
}

# 1. Check Ollama is running
$health = Invoke-LxcCommand "curl -sf --max-time 5 http://localhost:11434/" "Ollama HTTP check"
if ($health) {
    Write-Host "  $health" -ForegroundColor Green
} else {
    Write-Host "  Ollama not responding on port 11434" -ForegroundColor Red
    exit 1
}

# 2. Loaded models
$ps = Invoke-LxcCommand "curl -sf --max-time 5 http://localhost:11434/api/ps" "Loaded models"
if ($ps) {
    try {
        $parsed = $ps | ConvertFrom-Json
        foreach ($m in $parsed.models) {
            Write-Host "  $($m.name) — VRAM: $([math]::Round($m.size_vram / 1GB, 2)) GB" -ForegroundColor Green
        }
        if (-not $parsed.models -or $parsed.models.Count -eq 0) {
            Write-Host "  No models loaded in VRAM" -ForegroundColor Yellow
        }
    } catch {
        Write-Host "  Raw: $ps" -ForegroundColor DarkGray
    }
}

# 3. Available models
$tags = Invoke-LxcCommand "curl -sf --max-time 5 http://localhost:11434/api/tags" "Available models"
if ($tags) {
    try {
        $parsed = $tags | ConvertFrom-Json
        foreach ($m in $parsed.models) {
            Write-Host "  $($m.name) — $($m.details.parameter_size) $($m.details.quantization_level)" -ForegroundColor White
        }
    } catch {
        Write-Host "  Raw: $tags" -ForegroundColor DarkGray
    }
}

# 4. Test generate
Write-Host "`n[Generate test: $Model]" -ForegroundColor Cyan
$json = @{ model = $Model; prompt = 'Say hello in one word'; stream = $false } | ConvertTo-Json -Compress
$b64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($json))

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$genResult = Invoke-LxcCommand "echo $b64 | base64 -d | curl -sf --max-time $TimeoutSec http://localhost:11434/api/generate -d @-" "Generate: $Model"
$sw.Stop()

if ($genResult) {
    try {
        $parsed = $genResult | ConvertFrom-Json
        Write-Host "  Response: $($parsed.response)" -ForegroundColor Green
        Write-Host "  Elapsed: $($sw.Elapsed.TotalSeconds.ToString('N1'))s  eval_count=$($parsed.eval_count)" -ForegroundColor White
    } catch {
        Write-Host "  Raw ($($sw.Elapsed.TotalSeconds.ToString('N1'))s): $genResult" -ForegroundColor DarkGray
    }
} else {
    Write-Host "  No response after $($sw.Elapsed.TotalSeconds.ToString('N1'))s" -ForegroundColor Red
}
