#!/bin/bash
set -euo pipefail

PERSIST_ROOT="${IBCTL_PERSIST_ROOT:-/opt/ibctl/persist}"
BACKUP_ROOT="${PERSIST_ROOT}/settings-backups"
KEY_FILE="${IBCTL_SETTINGS_BACKUP_KEY_FILE:-${PERSIST_ROOT}/settings-backup.key}"
GOOD_MARKER="${PERSIST_ROOT}/maintenance/settings-good"
RECYCLE_MARKER="${PERSIST_ROOT}/maintenance/container-recycle"
RETENTION_DAYS="${IBCTL_LOG_RETENTION_DAYS:-45}"
BACKUP_RETENTION_DAYS="${IBCTL_SETTINGS_BACKUP_RETENTION_DAYS:-30}"

mkdir -p "${BACKUP_ROOT}" "${PERSIST_ROOT}/maintenance" "${PERSIST_ROOT}/logs"
chmod 700 "${BACKUP_ROOT}" "${PERSIST_ROOT}/maintenance" 2>/dev/null || true
if [ ! -s "${KEY_FILE}" ]; then
    umask 077
    openssl rand -hex 32 > "${KEY_FILE}"
fi
chmod 600 "${KEY_FILE}" 2>/dev/null || true

settings_dirs() {
    local root="${TWS_SETTINGS_PATH:-/home/ibgateway/Jts}"
    for dir in "${root}" "${root}_live" "${root}_paper"; do
        [ -d "${dir}" ] && printf '%s\n' "${dir}"
    done
}

backup_settings() {
    local stamp base output
    stamp=$(date -u +%Y%m%dT%H%M%SZ)
    while IFS= read -r dir; do
        base=$(basename "${dir}")
        output="${BACKUP_ROOT}/${base}-${stamp}.tar.enc"
        (cd "${dir}" && tar \
            --exclude='./ibgateway' --exclude='./launcher.log' \
            --exclude='*.log' --exclude='language.jar' -cf - .) \
            | openssl enc -aes-256-cbc -salt -pbkdf2 -pass "file:${KEY_FILE}" -out "${output}.tmp"
        mv "${output}.tmp" "${output}"
        chmod 600 "${output}" 2>/dev/null || true
        echo "Backed up known-good Gateway settings: ${output}"
    done < <(settings_dirs)
}

restore_settings() {
    local base newest
    while IFS= read -r dir; do
        base=$(basename "${dir}")
        newest=$(find "${BACKUP_ROOT}" -maxdepth 1 -type f -name "${base}-*.tar.enc" -printf '%T@ %p\n' 2>/dev/null \
            | sort -nr | head -1 | cut -d' ' -f2- || true)
        [ -n "${newest}" ] || continue
        openssl enc -d -aes-256-cbc -pbkdf2 -pass "file:${KEY_FILE}" -in "${newest}" \
            | tar -xf - -C "${dir}"
        echo "Restored last-known-good Gateway settings from ${newest}"
    done < <(settings_dirs)
}

cleanup_storage() {
    find "${PERSIST_ROOT}/logs" -type f -mtime "+${RETENTION_DAYS}" -delete 2>/dev/null || true
    find "${TWS_SETTINGS_PATH:-/home/ibgateway/Jts}"* -type f \( -name '*.log' -o -name '*.trace' \) \
        -mtime "+${RETENTION_DAYS}" -delete 2>/dev/null || true
    find "${BACKUP_ROOT}" -type f -name '*.tar.enc' -mtime "+${BACKUP_RETENTION_DAYS}" -delete 2>/dev/null || true

    local used
    used=$(df -P "${PERSIST_ROOT}" | awk 'NR==2 {gsub(/%/, "", $5); print $5}')
    if [ "${used:-0}" -ge 85 ]; then
        echo "Persistent disk ${used}% full — removing logs older than 7 days"
        find "${PERSIST_ROOT}/logs" -type f -mtime +7 -delete 2>/dev/null || true
    fi
}

if [ "${1:-}" = "--startup" ]; then
    if [ -f "${RECYCLE_MARKER}" ]; then
        restore_settings
        rm -f "${RECYCLE_MARKER}"
    fi
    cleanup_storage
    exit 0
fi

last_good_mtime=0
last_cleanup=0
while true; do
    now=$(date +%s)
    if [ $((now - last_cleanup)) -ge 21600 ]; then
        cleanup_storage
        last_cleanup="${now}"
    fi
    if [ -f "${GOOD_MARKER}" ]; then
        current=$(stat -c %Y "${GOOD_MARKER}" 2>/dev/null || echo 0)
        if [ "${current}" -gt "${last_good_mtime}" ]; then
            backup_settings || true
            last_good_mtime="${current}"
        fi
    fi
    sleep 60
done
