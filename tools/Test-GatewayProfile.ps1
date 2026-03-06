#Requires -Version 7
<#
.SYNOPSIS
    Tests a gateway profile by sending a chat completion request and reporting the result.

.DESCRIPTION
    Sends a non-streaming POST to /v1/chat/completions and displays the response
    or error. Useful for verifying that a specific profile routes correctly and
    that Ollama responds. Includes timeout handling so hangs are surfaced clearly.

.PARAMETER GatewayUrl
    Base URL of the lm-gateway client endpoint (no trailing /v1).
    Defaults to LMG_URL env var or http://language.cortex.lan:8080.

.PARAMETER Model
    The model field to send in the request body. This is resolved through
    aliases by the gateway. Default: "auto".

.PARAMETER Prompt
    The user message to send. Default: "hello".

.PARAMETER ClientKey
    Optional Bearer token. If omitted, the request uses the public profile.

.PARAMETER TimeoutSec
    HTTP request timeout in seconds. Default: 30.

.PARAMETER ShowBody
    If set, dumps the raw request body before sending.

.EXAMPLE
    .\Test-GatewayProfile.ps1
    Tests the public profile with model=auto, prompt="hello".

.EXAMPLE
    .\Test-GatewayProfile.ps1 -Model "ha-auto:latest" -ClientKey $env:LMG_HA_KEY -Prompt "Turn on the light"
    Tests the ha-auto profile with authentication.

.EXAMPLE
    .\Test-GatewayProfile.ps1 -Model "hint:instant" -Prompt "Hi" -TimeoutSec 10
    Quick test targeting the instant tier directly.
#>
[CmdletBinding()]
param(
    [string]$GatewayUrl = ($env:LMG_URL ?? 'http://language.cortex.lan:8080'),
    [string]$Model      = 'auto',
    [string]$Prompt     = 'hello',
    [string]$ClientKey  = '',
    [int]$TimeoutSec    = 30,
    [switch]$ShowBody
)

$ErrorActionPreference = 'Stop'

$url = "$($GatewayUrl.TrimEnd('/'))/v1/chat/completions"

$body = @{
    model    = $Model
    messages = @(
        @{ role = 'user'; content = $Prompt }
    )
    stream   = $false
} | ConvertTo-Json -Depth 4 -Compress

if ($ShowBody) {
    Write-Host "`nRequest body:" -ForegroundColor DarkGray
    Write-Host ($body | ConvertFrom-Json | ConvertTo-Json -Depth 4) -ForegroundColor DarkGray
}

$headers = @{ 'Content-Type' = 'application/json' }
if ($ClientKey) {
    $headers['Authorization'] = "Bearer $ClientKey"
}

Write-Host "`nPOST $url" -ForegroundColor Cyan
Write-Host "  model=$Model  timeout=${TimeoutSec}s" -ForegroundColor Cyan
if ($ClientKey) {
    Write-Host "  auth=Bearer ***" -ForegroundColor Cyan
} else {
    Write-Host "  auth=none (public profile)" -ForegroundColor Cyan
}
Write-Host ""

$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

try {
    $response = Invoke-RestMethod -Uri $url -Method Post -Headers $headers -Body $body -TimeoutSec $TimeoutSec

    $stopwatch.Stop()
    $elapsed = $stopwatch.Elapsed

    Write-Host "OK — ${elapsed.TotalSeconds:N1}s" -ForegroundColor Green

    $choice = $response.choices[0]
    $content = $choice.message.content
    $model = $response.model
    $finish = $choice.finish_reason

    Write-Host "  model:  $model" -ForegroundColor White
    Write-Host "  finish: $finish" -ForegroundColor White
    Write-Host "  tokens: prompt=$($response.usage.prompt_tokens) completion=$($response.usage.completion_tokens)" -ForegroundColor White
    Write-Host ""
    Write-Host "Response:" -ForegroundColor Yellow
    Write-Host $content
} catch {
    $stopwatch.Stop()
    $elapsed = $stopwatch.Elapsed

    $err = $_.Exception
    if ($err.Message -match 'Timeout|timed out|HttpClient\.Timeout') {
        Write-Host "TIMEOUT after ${elapsed.TotalSeconds:N1}s — gateway or backend hung" -ForegroundColor Red
        Write-Host "  Check: journalctl -u lm-gateway --since '2 minutes ago' --no-pager" -ForegroundColor DarkYellow
    } elseif ($_.Exception.Response) {
        $status = [int]$_.Exception.Response.StatusCode
        $detail = $_.ErrorDetails.Message
        Write-Host "HTTP $status after ${elapsed.TotalSeconds:N1}s" -ForegroundColor Red
        if ($detail) { Write-Host "  $detail" -ForegroundColor DarkYellow }
    } else {
        Write-Host "ERROR after ${elapsed.TotalSeconds:N1}s — $($err.Message)" -ForegroundColor Red
    }
    exit 1
}
