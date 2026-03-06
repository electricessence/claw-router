#Requires -Version 7
<#
.SYNOPSIS
    Diagnoses and optionally restarts a stuck Ollama instance on LXC 200.

.DESCRIPTION
    Checks Ollama process state, looks for stuck requests, and can restart
    the service to clear a jammed queue. Also checks lm-gateway state.

.PARAMETER SshAlias
    SSH alias for the Proxmox host. Default: cortex.

.PARAMETER LxcId
    LXC container ID. Default: 200.

.PARAMETER Restart
    If set, restarts the Ollama service after diagnosis.

.PARAMETER RestartGateway
    If set, also restarts lm-gateway after Ollama restart.

.EXAMPLE
    .\Restart-OllamaService.ps1 -Restart -RestartGateway
    Diagnose, restart Ollama, then restart lm-gateway.
#>
[CmdletBinding()]
param(
    [string]$SshAlias      = 'cortex',
    [int]$LxcId            = 200,
    [switch]$Restart,
    [switch]$RestartGateway
)

$ErrorActionPreference = 'Stop'

function Invoke-Lxc {
    param([string]$Cmd, [string]$Label)
    Write-Host "[$Label]" -ForegroundColor Cyan
    $b64 = [Convert]::ToBase64String([System.Text.Encoding]::UTF8.GetBytes($Cmd))
    $out = ssh $SshAlias "pct exec $LxcId -- bash -c `"echo $b64 | base64 -d | bash`"" 2>&1
    Write-Host $out -ForegroundColor White
    Write-Host ""
    return $out
}

# Check Ollama process state
Invoke-Lxc "systemctl is-active ollama" "Ollama service status"
Invoke-Lxc "ps aux | grep -E 'ollama' | grep -v grep | head -10" "Ollama process"

# Check for pending/stuck HTTP connections to Ollama
Invoke-Lxc "ss -tnp | grep ':11434' | head -20" "Active connections to Ollama"

# Check recent Ollama logs for errors
Invoke-Lxc "journalctl -u ollama --since '5 minutes ago' --no-pager 2>&1 | tail -20" "Recent Ollama logs"

# Check lm-gateway pending requests
Invoke-Lxc "ss -tnp | grep ':8080' | head -20" "Active connections to lm-gateway"

if ($Restart) {
    Write-Host "=== Restarting Ollama ===" -ForegroundColor Yellow
    ssh $SshAlias "pct exec $LxcId -- systemctl restart ollama"
    Start-Sleep -Seconds 3
    Invoke-Lxc "systemctl is-active ollama" "Ollama status after restart"

    Write-Host "Waiting 5s for Ollama to settle..." -ForegroundColor DarkGray
    Start-Sleep -Seconds 5
    Invoke-Lxc "curl -sf --max-time 5 http://localhost:11434/" "Ollama health after restart"
}

if ($RestartGateway) {
    Write-Host "=== Restarting lm-gateway ===" -ForegroundColor Yellow
    ssh $SshAlias "pct exec $LxcId -- systemctl restart lm-gateway"
    Start-Sleep -Seconds 2
    Invoke-Lxc "systemctl is-active lm-gateway" "lm-gateway status after restart"
}
