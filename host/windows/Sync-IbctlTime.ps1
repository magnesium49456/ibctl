[CmdletBinding()]
param(
    [string]$ContainerName = 'ibctl-gateway',
    [int]$PeriodicMinutes = 5
)

$ErrorActionPreference = 'Stop'
$stateRoot = Join-Path $env:LOCALAPPDATA 'ibctl-time-sync'
$statePath = Join-Path $stateRoot 'state.json'
New-Item -ItemType Directory -Path $stateRoot -Force | Out-Null

$state = @{ lastRequest = ''; lastSyncUtc = '1970-01-01T00:00:00Z' }
if (Test-Path -LiteralPath $statePath) {
    try {
        $loaded = Get-Content -LiteralPath $statePath -Raw | ConvertFrom-Json
        $state = @{ lastRequest = [string]$loaded.lastRequest; lastSyncUtc = [string]$loaded.lastSyncUtc }
    } catch {}
}

$request = ''
try {
    $request = (& docker exec $ContainerName sh -lc 'cat /opt/ibctl/persist/time-sync/request 2>/dev/null || true' 2>$null | Out-String).Trim()
} catch {}

$lastSync = [datetime]::Parse($state.lastSyncUtc).ToUniversalTime()
$periodicDue = ([datetime]::UtcNow - $lastSync).TotalMinutes -ge $PeriodicMinutes
$requestDue = $request -and $request -ne $state.lastRequest
if (-not ($periodicDue -or $requestDue)) { exit 0 }

function Get-NtpOffsetMilliseconds([string]$Server) {
    $client = [System.Net.Sockets.UdpClient]::new()
    try {
        $client.Client.ReceiveTimeout = 2500
        $packet = New-Object byte[] 48
        $packet[0] = 0x23
        $unixEpoch = [datetime]::SpecifyKind([datetime]'1970-01-01', [DateTimeKind]::Utc)
        $t1 = ([datetime]::UtcNow - $unixEpoch).TotalSeconds
        $client.Connect($Server, 123)
        [void]$client.Send($packet, $packet.Length)
        $remote = New-Object System.Net.IPEndPoint ([System.Net.IPAddress]::Any, 0)
        $response = $client.Receive([ref]$remote)
        $t4 = ([datetime]::UtcNow - $unixEpoch).TotalSeconds
        if ($response.Length -lt 48 -or $response[1] -eq 0) { throw 'Invalid NTP response' }
        function Read-NtpTimestamp([byte[]]$Bytes, [int]$Start) {
            $sec = [byte[]]$Bytes[$Start..($Start + 3)]
            $frac = [byte[]]$Bytes[($Start + 4)..($Start + 7)]
            [array]::Reverse($sec); [array]::Reverse($frac)
            [double][BitConverter]::ToUInt32($sec, 0) - 2208988800.0 +
                ([double][BitConverter]::ToUInt32($frac, 0) / 4294967296.0)
        }
        $t2 = Read-NtpTimestamp $response 32
        $t3 = Read-NtpTimestamp $response 40
        return [math]::Round(((($t2 - $t1) + ($t3 - $t4)) / 2.0) * 1000.0, 1)
    } finally {
        $client.Dispose()
    }
}

# Use Windows' signed built-in COM time task, then ask W32Time to resync as a
# second best-effort path. Either may be sufficient depending on host policy.
try {
    Start-ScheduledTask -TaskPath '\Microsoft\Windows\Time Synchronization\' -TaskName 'ForceSynchronizeTime'
} catch {}
$resyncRequested = $false
try {
    & "$env:SystemRoot\System32\w32tm.exe" /resync /nowait | Out-Null
    $resyncRequested = $LASTEXITCODE -eq 0
} catch {}
Start-Sleep -Seconds 3

$statusText = (& "$env:SystemRoot\System32\w32tm.exe" /query /status 2>&1 | Out-String).Trim()
$w32StatusVerified = $LASTEXITCODE -eq 0 -and $statusText -match 'Source:' -and $statusText -match 'Last Successful Sync Time:'
$offsets = @()
foreach ($server in @('time.cloudflare.com', 'time.google.com', 'pool.ntp.org')) {
    try { $offsets += Get-NtpOffsetMilliseconds $server } catch {}
}
$authoritativeOffsetMs = $null
if ($offsets.Count) {
    $sorted = @($offsets | Sort-Object)
    $authoritativeOffsetMs = $sorted[[math]::Floor($sorted.Count / 2)]
}
$ntpOffsetVerified = $null -ne $authoritativeOffsetMs -and [math]::Abs($authoritativeOffsetMs) -le 2000
$verified = $ntpOffsetVerified -or ($resyncRequested -and $w32StatusVerified)
$verificationMethod = if ($ntpOffsetVerified) { 'public_ntp_offset' } elseif ($verified) { 'w32time_resync_and_status' } else { 'unverified' }
$now = [datetime]::UtcNow.ToString('o')
$state = @{ lastRequest = $request; lastSyncUtc = $now; verified = $verified; verificationMethod = $verificationMethod; authoritativeOffsetMs = $authoritativeOffsetMs; samples = $offsets.Count; status = $statusText }
$state | ConvertTo-Json -Depth 3 | Set-Content -LiteralPath $statePath -Encoding utf8

try {
    $response = @{ verified = $verified; verificationMethod = $verificationMethod; checkedAtUtc = $now; request = $request; authoritativeOffsetMs = $authoritativeOffsetMs; samples = $offsets.Count; status = $statusText } | ConvertTo-Json -Compress
    $response | & docker exec -i $ContainerName sh -lc 'mkdir -p /opt/ibctl/persist/time-sync; tee /opt/ibctl/persist/time-sync/response.json >/dev/null' 2>$null
} catch {}

if (-not $verified) { throw "Host time could not be verified after a W32Time resync request or within 2 seconds of public NTP: $statusText" }
