[CmdletBinding()]
param([int]$PeriodicMinutes = 5)

$ErrorActionPreference = 'Stop'
$syncScript = Join-Path $PSScriptRoot 'Sync-IbctlTime.ps1'
if (-not (Test-Path -LiteralPath $syncScript)) { throw "Missing $syncScript" }

$action = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument (
    '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{0}" -PeriodicMinutes {1}' -f $syncScript, $PeriodicMinutes
)
$trigger = New-ScheduledTaskTrigger -Once -At (Get-Date).AddMinutes(1) -RepetitionInterval (New-TimeSpan -Minutes 1) -RepetitionDuration (New-TimeSpan -Days 3650)
$settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfNetworkAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Minutes 2)
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = ([System.Security.Principal.WindowsPrincipal]::new($identity)).IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
$runLevel = if ($isAdmin) { 'Highest' } else { 'Limited' }
$principal = New-ScheduledTaskPrincipal -UserId $identity.Name -LogonType Interactive -RunLevel $runLevel

Register-ScheduledTask -TaskName 'ibctl-TimeSync' -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Description 'Every minute checks ibctl 2FA sync requests; forces and verifies W32Time at least every five minutes.' -Force | Out-Null
Get-ScheduledTask -TaskName 'ibctl-TimeSync' -ErrorAction Stop | Out-Null
Start-ScheduledTask -TaskName 'ibctl-TimeSync'
if ($isAdmin) {
    Write-Output 'Installed and started elevated scheduled task ibctl-TimeSync.'
} else {
    Write-Output 'Installed and started current-user task ibctl-TimeSync. Host resync is best-effort; public-NTP offset verification remains enforced.'
}
