# ibctl — IBC replacement for IB Gateway/TWS automation
# Self-contained build following gnzsnz/ib-gateway-docker's proven process,
# but without IBC. ibctl replaces it entirely.
#
# Two build modes:
#   Fast (pre-built release):
#     docker build --build-arg IBCTL_VERSION=v0.1.0 -t ibctl .
#   From source (no release available):
#     docker build -t ibctl .

ARG IB_GATEWAY_VERSION=latest
ARG IB_GATEWAY_CHANNEL=latest
ARG IBCTL_VERSION=""
# Docker's ubuntu:latest tag tracks the latest LTS release; use
# --build-arg UBUNTU_IMAGE_TAG=24.04 to pin a specific LTS for repeatability.
ARG UBUNTU_IMAGE_TAG=latest
ARG UBUNTU_APT_MIRROR=mirror+http://mirrors.ubuntu.com/mirrors.txt
ARG UBUNTU_APT_FALLBACK_MIRROR=https://archive.ubuntu.com/ubuntu
ARG UBUNTU_APT_PORTS_FALLBACK_MIRROR=https://ports.ubuntu.com/ubuntu-ports

##############################################################################
# Stage 1: Setup — download and install IB Gateway
# Follows gnzsnz/ib-gateway-docker's exact process (minus IBC)
##############################################################################
FROM ubuntu:${UBUNTU_IMAGE_TAG} AS setup

ARG IB_GATEWAY_VERSION
ARG IB_GATEWAY_CHANNEL
ARG TARGETARCH
ARG DEBIAN_FRONTEND=noninteractive
ARG IB_GATEWAY_REPO="https://github.com/gnzsnz/ib-gateway-docker"
ARG IB_GATEWAY_API_REPO="https://api.github.com/repos/gnzsnz/ib-gateway-docker"
ARG UBUNTU_APT_MIRROR
ARG UBUNTU_APT_FALLBACK_MIRROR
ARG UBUNTU_APT_PORTS_FALLBACK_MIRROR
# aarch64 JDK (only used on ARM)
ARG ZULU_NAME=zulu17.60.17-ca-fx-jre17.0.16-linux_aarch64
ARG ZULU_FILE=${ZULU_NAME}.tar.gz
ARG ZULU_URL=https://cdn.azul.com/zulu/bin/${ZULU_FILE}

WORKDIR /tmp/setup

# Official Ubuntu mirror setup:
#  1) Bootstrap ca-certificates from HTTP-compatible sources because the
#     minimal Ubuntu image has no CA trust store yet.
#  2) Use Ubuntu's official mirror service plus HTTPS fallbacks after trust is
#     installed. Apt still verifies package authenticity via Ubuntu signatures.
# Security updates stay on the official security.ubuntu.com source. Mirror
# choice is delegated to Ubuntu, not custom speed tests or regional pins.
RUN set -eux; \
    configure_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        ports_arch=0; \
        case "${TARGETARCH:-}" in arm|arm64) ports_arch=1 ;; esac; \
        if [ "${ports_arch}" = 1 ] && grep -q 'URIs: .*ports.ubuntu.com/ubuntu-ports' "${sources}"; then \
            sed -i \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports/|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports/|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                "${sources}"; \
        else \
            sed -i \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu/|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu/|URIs: ${archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu|URIs: ${archive_uris}|g" \
                "${sources}"; \
        fi; \
    }; \
    harden_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_uris}|g" \
            -e "s|URIs: ${archive_bootstrap}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            -e "s|URIs: ${ports_bootstrap}|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu/|URIs: https://security.ubuntu.com/ubuntu|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu|URIs: https://security.ubuntu.com/ubuntu|g" \
            "${sources}"; \
    }; \
    use_ubuntu_apt_fallback_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_bootstrap}|g" \
            -e "s|URIs: ${archive_uris}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            "${sources}"; \
    }; \
    apt_get_update_with_fallback() { \
        if ! apt-get update "$@"; then \
            echo "Ubuntu mirror list unavailable; retrying apt update with ${UBUNTU_APT_FALLBACK_MIRROR}" >&2; \
            use_ubuntu_apt_fallback_sources; \
            apt-get update "$@"; \
        fi; \
    }; \
    configure_ubuntu_apt_sources \
    && apt_get_update_with_fallback -y \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && harden_ubuntu_apt_sources \
    && apt_get_update_with_fallback -y \
    && apt-get install --no-install-recommends --yes curl \
    && apt-get clean && rm -rf /var/lib/apt/lists/* \
    # Validate supported architectures
    && if [ "${TARGETARCH}" != "amd64" ] && [ "${TARGETARCH}" != "arm64" ]; then \
        echo "Unsupported Docker target architecture: ${TARGETARCH}" >&2; \
        exit 1; \
    fi \
    # arm64: download Zulu JDK
    && if [ "${TARGETARCH}" = "arm64" ]; then \
        curl -sSLO ${ZULU_URL} && \
        tar -xzf ${ZULU_FILE} -C /usr/local/ && \
        ln -s /usr/local/${ZULU_NAME} /usr/local/zulu17; \
    fi \
    # Resolve the current upstream Gateway version for the selected channel.
    # Passing --build-arg IB_GATEWAY_VERSION=10.xx.yz still pins an exact build.
    && ib_gateway_version="${IB_GATEWAY_VERSION}" \
    && if [ "${ib_gateway_version}" = "latest" ] || [ "${ib_gateway_version}" = "auto" ]; then \
        ib_gateway_version="$(curl -fsSL "${IB_GATEWAY_API_REPO}/releases?per_page=100" \
            | sed -n "s/.*\"tag_name\": \"ibgateway-${IB_GATEWAY_CHANNEL}@\\([^\"]*\\)\".*/\\1/p" \
            | head -n 1)"; \
        if [ -z "${ib_gateway_version}" ]; then \
            echo "Could not resolve latest IB Gateway version for channel '${IB_GATEWAY_CHANNEL}'" >&2; \
            exit 1; \
        fi; \
        echo "Resolved IB Gateway ${IB_GATEWAY_CHANNEL} channel to ${ib_gateway_version}"; \
    fi \
    && ib_gateway_file="ibgateway-${ib_gateway_version}-standalone-linux-x64.sh" \
    && ib_gateway_url="${IB_GATEWAY_REPO}/releases/download/ibgateway-${IB_GATEWAY_CHANNEL}%40${ib_gateway_version}/${ib_gateway_file}" \
    # Download and verify IB Gateway installer
    && curl -fsSLO "${ib_gateway_url}" \
    && curl -fsSLO "${ib_gateway_url}.sha256" \
    && sha256sum --check "./${ib_gateway_file}.sha256" \
    && chmod a+x "./${ib_gateway_file}" \
    # Install IB Gateway
    && if [ "${TARGETARCH}" = "arm64" ]; then \
        app_java_home=/usr/local/zulu17 "./${ib_gateway_file}" -q -dir "/root/Jts/ibgateway/${ib_gateway_version}"; \
    else \
        "./${ib_gateway_file}" -q -dir "/root/Jts/ibgateway/${ib_gateway_version}"; \
    fi

# jts.ini template (ibctl's version, includes ReadOnlyApi=no)
COPY docker/jts.ini.tmpl /root/Jts/jts.ini.tmpl

##############################################################################
# Stage 2a: Download pre-built ibctl binaries (if IBCTL_VERSION is set)
##############################################################################
FROM ubuntu:${UBUNTU_IMAGE_TAG} AS prebuilt-downloader
ARG IBCTL_VERSION
ARG TARGETARCH
ARG UBUNTU_APT_MIRROR
ARG UBUNTU_APT_FALLBACK_MIRROR
ARG UBUNTU_APT_PORTS_FALLBACK_MIRROR
# Official Ubuntu mirror setup (see Stage 1 for bootstrap/signature rationale)
RUN set -eux; \
    configure_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        ports_arch=0; \
        case "${TARGETARCH:-}" in arm|arm64) ports_arch=1 ;; esac; \
        if [ "${ports_arch}" = 1 ] && grep -q 'URIs: .*ports.ubuntu.com/ubuntu-ports' "${sources}"; then \
            sed -i \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports/|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports/|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                "${sources}"; \
        else \
            sed -i \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu/|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu/|URIs: ${archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu|URIs: ${archive_uris}|g" \
                "${sources}"; \
        fi; \
    }; \
    harden_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_uris}|g" \
            -e "s|URIs: ${archive_bootstrap}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            -e "s|URIs: ${ports_bootstrap}|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu/|URIs: https://security.ubuntu.com/ubuntu|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu|URIs: https://security.ubuntu.com/ubuntu|g" \
            "${sources}"; \
    }; \
    use_ubuntu_apt_fallback_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_bootstrap}|g" \
            -e "s|URIs: ${archive_uris}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            "${sources}"; \
    }; \
    apt_get_update_with_fallback() { \
        if ! apt-get update "$@"; then \
            echo "Ubuntu mirror list unavailable; retrying apt update with ${UBUNTU_APT_FALLBACK_MIRROR}" >&2; \
            use_ubuntu_apt_fallback_sources; \
            apt-get update "$@"; \
        fi; \
    }; \
    configure_ubuntu_apt_sources \
    && apt_get_update_with_fallback -qq \
    && apt-get install -y -qq --no-install-recommends ca-certificates \
    && harden_ubuntu_apt_sources \
    && apt_get_update_with_fallback -qq \
    && apt-get install -y -qq --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /prebuilt \
    && if [ -n "${IBCTL_VERSION}" ]; then \
        echo "Downloading pre-built ibctl ${IBCTL_VERSION}" \
        && curl -sL -o /prebuilt/ibctl "https://github.com/Lcstyle/ibctl/releases/download/${IBCTL_VERSION}/ibctl" \
        && curl -sL -o /prebuilt/ibctl-agent.jar "https://github.com/Lcstyle/ibctl/releases/download/${IBCTL_VERSION}/ibctl-agent.jar" \
        && chmod +x /prebuilt/ibctl; \
    else \
        echo "No IBCTL_VERSION — will build from source" \
        && touch /prebuilt/.build-from-source; \
    fi

##############################################################################
# Stage 2b: Build Rust binary from source (fallback)
##############################################################################
FROM rust:1.83-bookworm AS rust-builder
ARG IBCTL_BUILD_VERSION=""
COPY Cargo.toml Cargo.lock /build/
COPY .cargo/ /build/.cargo/
COPY ibctl/ /build/ibctl/
WORKDIR /build
# Use thin LTO for Docker source builds (fast). Release workflow uses fat LTO.
# IBCTL_BUILD_VERSION is read by build.rs to embed the git tag version.
RUN sed -i 's/lto = "fat"/lto = "thin"/' /build/.cargo/config.toml \
    && sed -i 's/codegen-units = 1/codegen-units = 16/' /build/.cargo/config.toml \
    && IBCTL_BUILD_VERSION="${IBCTL_BUILD_VERSION}" cargo build --release && strip target/release/ibctl

##############################################################################
# Stage 2c: Build Java agent from source (fallback)
##############################################################################
FROM eclipse-temurin:17-jdk-jammy AS java-builder
COPY agent/src/ /build/agent/src/
COPY agent/pom.xml /build/agent/pom.xml
WORKDIR /build/agent
RUN mkdir -p target/classes \
    && javac --release 17 -d target/classes src/main/java/ibctl/agent/*.java \
    && jar cfm target/ibctl-agent.jar src/main/resources/META-INF/MANIFEST.MF -C target/classes .

##############################################################################
# Stage 3: Production image
# Same base + packages as gnzsnz, minus IBC
##############################################################################
FROM ubuntu:${UBUNTU_IMAGE_TAG}

ARG IB_GATEWAY_VERSION
ARG USER_ID=1000
ARG USER_GID=1000
ARG DEBIAN_FRONTEND=noninteractive
ARG TARGETARCH
ARG UBUNTU_APT_MIRROR
ARG UBUNTU_APT_FALLBACK_MIRROR
ARG UBUNTU_APT_PORTS_FALLBACK_MIRROR

# Environment (matching gnzsnz conventions)
ENV HOME=/home/ibgateway \
    IB_GATEWAY_VERSION=${IB_GATEWAY_VERSION} \
    TWS_PATH=/home/ibgateway/Jts \
    GATEWAY_OR_TWS=gateway \
    JAVA_PATH=/usr/local/zulu17 \
    NO_AT_BRIDGE=1

# Copy Gateway + JRE from setup stage (same as gnzsnz)
COPY --from=setup /usr/local/ /usr/local/
COPY --from=setup /root/Jts /home/ibgateway/Jts

# Install runtime packages + Python for dashboard.
# Official Ubuntu mirror setup (see Stage 1 for bootstrap/signature rationale)
RUN set -eux; \
    configure_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        ports_arch=0; \
        case "${TARGETARCH:-}" in arm|arm64) ports_arch=1 ;; esac; \
        if [ "${ports_arch}" = 1 ] && grep -q 'URIs: .*ports.ubuntu.com/ubuntu-ports' "${sources}"; then \
            sed -i \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports/|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: http://ports.ubuntu.com/ubuntu-ports|URIs: ${ports_bootstrap}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports/|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                -e "s|URIs: https://ports.ubuntu.com/ubuntu-ports|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
                "${sources}"; \
        else \
            sed -i \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu/|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: http://archive.ubuntu.com/ubuntu|URIs: ${bootstrap_archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu/|URIs: ${archive_uris}|g" \
                -e "s|URIs: https://archive.ubuntu.com/ubuntu|URIs: ${archive_uris}|g" \
                "${sources}"; \
        fi; \
    }; \
    harden_ubuntu_apt_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        ports_bootstrap="$(printf '%s' "${UBUNTU_APT_PORTS_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_uris}|g" \
            -e "s|URIs: ${archive_bootstrap}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            -e "s|URIs: ${ports_bootstrap}|URIs: ${UBUNTU_APT_PORTS_FALLBACK_MIRROR}|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu/|URIs: https://security.ubuntu.com/ubuntu|g" \
            -e "s|URIs: http://security.ubuntu.com/ubuntu|URIs: https://security.ubuntu.com/ubuntu|g" \
            "${sources}"; \
    }; \
    use_ubuntu_apt_fallback_sources() { \
        sources=/etc/apt/sources.list.d/ubuntu.sources; \
        bootstrap_mirror="$(printf '%s' "${UBUNTU_APT_MIRROR}" | sed 's|^https://|http://|')"; \
        archive_bootstrap="$(printf '%s' "${UBUNTU_APT_FALLBACK_MIRROR}" | sed 's|^https://|http://|')"; \
        bootstrap_archive_uris="${bootstrap_mirror}"; \
        if [ "${bootstrap_mirror}" != "${archive_bootstrap}" ]; then bootstrap_archive_uris="${bootstrap_archive_uris} ${archive_bootstrap}"; fi; \
        archive_uris="${UBUNTU_APT_MIRROR}"; \
        if [ "${UBUNTU_APT_MIRROR}" != "${UBUNTU_APT_FALLBACK_MIRROR}" ]; then archive_uris="${archive_uris} ${UBUNTU_APT_FALLBACK_MIRROR}"; fi; \
        sed -i \
            -e "s|URIs: ${bootstrap_archive_uris}|URIs: ${archive_bootstrap}|g" \
            -e "s|URIs: ${archive_uris}|URIs: ${UBUNTU_APT_FALLBACK_MIRROR}|g" \
            "${sources}"; \
    }; \
    apt_get_update_with_fallback() { \
        if ! apt-get update "$@"; then \
            echo "Ubuntu mirror list unavailable; retrying apt update with ${UBUNTU_APT_FALLBACK_MIRROR}" >&2; \
            use_ubuntu_apt_fallback_sources; \
            apt-get update "$@"; \
        fi; \
    }; \
    configure_ubuntu_apt_sources \
    && apt_get_update_with_fallback -y \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && harden_ubuntu_apt_sources \
    && apt_get_update_with_fallback -y \
    && apt-get upgrade -y \
    && apt-get install --no-install-recommends --yes \
        gettext-base socat xvfb x11vnc sshpass openssh-client telnet iputils-ping \
        oathtool python3 python3-pip python3-venv websockify \
    && apt-get clean && rm -rf /var/lib/apt/lists/* \
    # Remove default ubuntu user if present
    && if id ubuntu 2>/dev/null; then userdel -rf ubuntu; fi \
    # Create ibgateway user (matching gnzsnz)
    && groupadd --gid ${USER_GID} ibgateway \
    && useradd -ms /bin/bash --uid ${USER_ID} --gid ${USER_GID} ibgateway \
    && mkdir -p /tmp/.X11-unix && chmod 1777 /tmp/.X11-unix \
    && mkdir -p /opt/ibctl \
    && mkdir -p /opt/ibctl/persist/config \
    && mkdir -p /opt/ibctl/persist/logs \
    && mkdir -p /run/ibctl && chmod 700 /run/ibctl

# Copy ibctl binaries — prefer pre-built, fall back to source
COPY --from=prebuilt-downloader /prebuilt/ /tmp/prebuilt/
COPY --from=rust-builder /build/target/release/ibctl /tmp/source/ibctl
COPY --from=java-builder /build/agent/target/ibctl-agent.jar /tmp/source/ibctl-agent.jar
RUN if [ -f /tmp/prebuilt/ibctl ]; then \
        echo "Using pre-built ibctl release" \
        && cp /tmp/prebuilt/ibctl /opt/ibctl/ibctl \
        && cp /tmp/prebuilt/ibctl-agent.jar /opt/ibctl/ibctl-agent.jar; \
    else \
        echo "Using source-built ibctl" \
        && cp /tmp/source/ibctl /opt/ibctl/ibctl \
        && cp /tmp/source/ibctl-agent.jar /opt/ibctl/ibctl-agent.jar; \
    fi && rm -rf /tmp/prebuilt /tmp/source

# Install dashboard Python dependencies in a venv
COPY dashboard/pyproject.toml /opt/ibctl/dashboard/pyproject.toml
RUN python3 -m venv /opt/ibctl/dashboard/.venv \
    && /opt/ibctl/dashboard/.venv/bin/pip install --no-cache-dir \
        fastapi uvicorn jinja2 sse-starlette requests beautifulsoup4 httpx pyzmq

# Copy dashboard source
COPY dashboard/app /opt/ibctl/dashboard/app

# Copy ibctl config and entrypoint
COPY docker/entrypoint.sh /opt/ibctl/entrypoint.sh
COPY docker/ibctl.toml /opt/ibctl/ibctl.toml
RUN chmod +x /opt/ibctl/ibctl /opt/ibctl/entrypoint.sh \
    && chown -R ibgateway:ibgateway /home/ibgateway /opt/ibctl /run/ibctl

USER ${USER_ID}:${USER_GID}
WORKDIR /home/ibgateway

# No Docker HEALTHCHECK — ibctl manages its own lifecycle, liveness checks,
# and notifications. A Docker healthcheck with restart: unless-stopped is
# destructive: it kills the container during legitimate 2FA waits, destroying
# authenticated sessions and forcing re-authentication.

ENTRYPOINT ["/opt/ibctl/entrypoint.sh"]

LABEL org.opencontainers.image.source=https://github.com/Lcstyle/ibctl
LABEL org.opencontainers.image.description="IBC replacement for automated IB Gateway/TWS login and session management"
LABEL org.opencontainers.image.licenses="MIT"
LABEL org.opencontainers.image.version=${IB_GATEWAY_VERSION}
