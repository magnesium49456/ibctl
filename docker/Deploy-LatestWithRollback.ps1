[CmdletBinding()]
param(
    [int]$ReadyTimeoutSeconds = 1800,
    [int]$StableSeconds = 90,
    [switch]$UseExistingCandidate
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$candidateDeployed = $false
Push-Location $repoRoot
try {
    $currentId = (& docker image inspect ibctl:latest --format '{{.Id}}' 2>$null | Out-String).Trim()
    if ($currentId) { & docker tag ibctl:latest ibctl:last-known-good }

    if (-not $UseExistingCandidate) {
        $refreshToken = [datetime]::UtcNow.ToString('yyyyMMddHHmmss')
        & docker build --pull --build-arg "IB_GATEWAY_REFRESH=$refreshToken" -t ibctl:candidate -f Dockerfile .
        if ($LASTEXITCODE -ne 0) { throw 'Candidate image build failed' }
    } elseif (-not (& docker image inspect ibctl:candidate --format '{{.Id}}' 2>$null)) {
        throw 'ibctl:candidate does not exist'
    }

    & docker tag ibctl:candidate ibctl:latest
    & docker compose up -d --no-build --force-recreate
    if ($LASTEXITCODE -ne 0) { throw 'Candidate deployment failed' }
    $candidateDeployed = $true

    $deadline = [datetime]::UtcNow.AddSeconds($ReadyTimeoutSeconds)
    $stableSince = $null
    do {
        Start-Sleep -Seconds 5
        try {
            $status = Invoke-RestMethod -Uri 'http://127.0.0.1:8080/api/v1/status/raw?mode=live' -TimeoutSec 4
            $dockerHealth = (& docker inspect ibctl-gateway --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}').Trim()
            $healthy = $status.state -eq 'Connected' -and $status.ready -eq $true -and $status.jvm.alive -eq $true -and $status.socat.running -eq $true -and $dockerHealth -eq 'healthy'
            if ($healthy) {
                if (-not $stableSince) { $stableSince = [datetime]::UtcNow }
                if (([datetime]::UtcNow - $stableSince).TotalSeconds -ge $StableSeconds) {
                    & docker tag ibctl:latest ibctl:last-known-good
                    Write-Output "Candidate promoted after $StableSeconds stable seconds."
                    exit 0
                }
            } else {
                $stableSince = $null
            }
        } catch {
            $stableSince = $null
        }
    } while ([datetime]::UtcNow -lt $deadline)

    throw "Candidate did not remain API-ready for $StableSeconds seconds within timeout"
} catch {
    $failure = $_
    $lkg = (& docker image inspect ibctl:last-known-good --format '{{.Id}}' 2>$null | Out-String).Trim()
    if ($candidateDeployed -and $lkg) {
        & docker tag ibctl:last-known-good ibctl:latest
        & docker compose up -d --no-build --force-recreate
        Write-Warning "Rolled back to ibctl:last-known-good: $failure"
    } elseif (-not $candidateDeployed) {
        Write-Warning "Candidate failed before deployment; the running container was not touched: $failure"
    }
    throw
} finally {
    Pop-Location
}
