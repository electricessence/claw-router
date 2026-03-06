#Requires -Version 7
<#
.SYNOPSIS
    Sends a diverse set of prompts to the gateway and reports how each was classified/routed.

.DESCRIPTION
    Uses the admin traffic log to show tier decisions. Designed for quick
    smoke-testing after a deploy or classifier change.

.PARAMETER GatewayUrl
    Base URL of the lm-gateway client endpoint. Defaults to LMG_URL env var.

.PARAMETER AdminUrl
    Base URL of the admin endpoint. Defaults to LMG_ADMIN_URL env var,
    or GatewayUrl with port 8081 if 8080 detected.

.PARAMETER ClientKey
    Client API key. Defaults to LMG_CLIENT_KEY env var.

.PARAMETER AdminToken
    Admin bearer token. Defaults to LMG_ADMIN_TOKEN env var.

.PARAMETER ProfileModel
    The model/profile to test through. Defaults to "auto:latest".

.PARAMETER TrafficLimit
    How many recent traffic entries to fetch after each batch. Default 20.
#>
[CmdletBinding()]
param(
    [string]$GatewayUrl    = $env:LMG_URL,
    [string]$AdminUrl      = $env:LMG_ADMIN_URL,
    [string]$ClientKey     = $env:LMG_CLIENT_KEY,
    [string]$AdminToken    = $env:LMG_ADMIN_TOKEN,
    [string]$ProfileModel  = 'auto:latest',
    [int]   $TrafficLimit  = 20
)
$ErrorActionPreference = 'Stop'

if (-not $GatewayUrl)  { throw 'Set LMG_URL env var or pass -GatewayUrl' }
if (-not $ClientKey)   { throw 'Set LMG_CLIENT_KEY env var or pass -ClientKey' }
if (-not $AdminToken)  { throw 'Set LMG_ADMIN_TOKEN env var or pass -AdminToken' }

# Derive admin URL from client URL when not explicitly set
if (-not $AdminUrl) {
    $AdminUrl = $GatewayUrl -replace ':8080', ':8081' -replace '/v1.*', ''
    $AdminUrl = $AdminUrl.TrimEnd('/')
}

$clientBase = $GatewayUrl.TrimEnd('/')
$adminBase  = $AdminUrl.TrimEnd('/')

Write-Host ""
Write-Host "=== LM Gateway — Classification Smoke Test ===" -ForegroundColor Cyan
Write-Host "  Client : $clientBase"
Write-Host "  Admin  : $adminBase"
Write-Host "  Profile: $ProfileModel"
Write-Host ""

$prompts = [ordered]@{
    # Expected tier: instant  (trivial / low-stakes)
    "Greet me"                                                        = "instant"
    "What's 12 times 8?"                                             = "instant"
    "Turn on the living room lights"                                  = "instant"
    # Expected tier: fast  (moderate reasoning)
    "What does TCP/IP mean?"                                          = "fast"
    "Summarise the French Revolution in 3 sentences"                 = "fast"
    "Why does my Home Assistant automation trigger twice on startup?" = "fast"
    # Expected tier: deep  (substantial reasoning / long output)
    "Write a PowerShell function to recursively diff two directory trees and output a summary report" = "deep"
    "Design a distributed rate-limiter that handles clock skew across nodes"                         = "deep"
    "Explain transformer attention mechanism with maths"                                             = "deep"
}

function Send-Prompt {
    param([string]$Query, [string]$Model, [string]$Key, [string]$Uri)
    $body = @{
        model    = $Model
        messages = @(@{ role = 'user'; content = $Query })
        stream   = $false
    } | ConvertTo-Json -Depth 5 -Compress
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $r = Invoke-RestMethod -Method Post -Uri "$Uri/v1/chat/completions" `
            -Headers @{ Authorization = "Bearer $Key"; 'Content-Type' = 'application/json' } `
            -Body $body -TimeoutSec 120
        $sw.Stop()
        [PSCustomObject]@{ Ok = $true; Ms = $sw.ElapsedMilliseconds; Tier = $r.model; Error = $null }
    } catch {
        $sw.Stop()
        [PSCustomObject]@{ Ok = $false; Ms = $sw.ElapsedMilliseconds; Tier = '?'; Error = $_.Exception.Message }
    }
}

function Get-Traffic {
    param([string]$Uri, [string]$Token, [int]$Limit)
    try {
        $r = Invoke-RestMethod -Uri "$Uri/admin/traffic?limit=$Limit" `
            -Headers @{ Authorization = "Bearer $Token" } -TimeoutSec 10
        return $r.entries
    } catch {
        Write-Warning "Admin traffic fetch failed: $_"
        return @()
    }
}

# Capture baseline traffic count
$baseline = (Get-Traffic -Uri $adminBase -Token $AdminToken -Limit 1 | Select-Object -First 1).id

$results = [System.Collections.Generic.List[PSCustomObject]]::new()

foreach ($kv in $prompts.GetEnumerator()) {
    $q        = $kv.Key
    $expected = $kv.Value
    Write-Host "  Sending: $($q.Substring(0, [Math]::Min(60, $q.Length)))…" -ForegroundColor DarkGray
    $res = Send-Prompt -Query $q -Model $ProfileModel -Key $ClientKey -Uri $clientBase
    $results.Add([PSCustomObject]@{
        Query    = if ($q.Length -gt 55) { $q.Substring(0, 55) + '…' } else { $q }
        Expected = $expected
        Tier     = $res.Tier
        Ms       = $res.Ms
        OK       = $res.Ok
        Error    = $res.Error
    })
}

Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
$col = @{ Expression = { $_.Query }; Label = 'Prompt'; Width = 58 },
       @{ Expression = { $_.Expected }; Label = 'Want'; Width = 8 },
       @{ Expression = { $_.Tier }; Label = 'Got'; Width = 18 },
       @{ Expression = { '{0}ms' -f $_.Ms }; Label = 'ms'; Width = 7 }
$results | Format-Table $col -AutoSize

# Compare expected vs actual
$tierMatches = $results | Where-Object {
    $_.OK -and ($_.Tier -like "*$($_.Expected)*")
}
$errors = $results | Where-Object { -not $_.OK }
Write-Host ""
Write-Host ("Routing accuracy: {0}/{1}" -f $tierMatches.Count, ($results.Count - $errors.Count)) `
    -ForegroundColor $(if ($tierMatches.Count -eq ($results.Count - $errors.Count)) { 'Green' } else { 'Yellow' })
if ($errors.Count -gt 0) {
    Write-Host "Errors: $($errors.Count)" -ForegroundColor Red
    $errors | ForEach-Object { Write-Host "  [$($_.Query)] $($_.Error)" -ForegroundColor Red }
}
Write-Host ""
