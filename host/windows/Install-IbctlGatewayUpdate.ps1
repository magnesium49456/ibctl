[CmdletBinding()]
param(
    [string]$At = '11:00',
    [ValidateSet('Sunday','Monday','Tuesday','Wednesday','Thursday','Friday','Saturday')]
    [string]$Day = 'Sunday'
)

$ErrorActionPreference = 'Stop'
$deployScript = Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) 'docker\Deploy-LatestWithRollback.ps1'
if (-not (Test-Path -LiteralPath $deployScript)) { throw "Missing $deployScript" }
$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument ('-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}"' -f $deployScript)
$trigger = New-ScheduledTaskTrigger -Weekly -WeeksInterval 1 -DaysOfWeek $Day -At ([datetime]::ParseExact($At, 'HH:mm', $null))
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfNetworkAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 2)
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = ([System.Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
$runLevel = if ($isAdmin) { 'Highest' } else { 'Limited' }
$principal = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel $runLevel
Register-ScheduledTask -TaskName 'ibctl-LatestGatewayCanary' -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description 'Build latest IB Gateway, deploy as a canary, and automatically roll back unless API-ready.' -Force | Out-Null
Get-ScheduledTask -TaskName 'ibctl-LatestGatewayCanary' -ErrorAction Stop | Out-Null
Write-Output 'Installed weekly ibctl latest-Gateway canary task.'
