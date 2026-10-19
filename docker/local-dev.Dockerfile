# Local Linux toolchain, matching the version pinned by the release workflow.
FROM docker.1ms.run/library/rust:1.97.1-slim-bookworm
RUN http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY" apt-get update \
    && http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY" apt-get install -y --no-install-recommends \
        build-essential cmake pkg-config protobuf-compiler libprotobuf-dev clang lld libclang-dev zstd \
    && rm -rf /var/lib/apt/lists/*

RUN rustup component add rustfmt

RUN http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY" apt-get update \
    && http_proxy="$HTTP_PROXY" https_proxy="$HTTPS_PROXY" apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_BUILD_JOBS=2 \
    CARGO_PROFILE_DEV_DEBUG=0 \
    CARGO_PROFILE_TEST_DEBUG=0 \
    CARGO_INCREMENTAL=0 \
    CARGO_TARGET_DIR=/build/target \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=clang \
    RUSTFLAGS="-C link-arg=-fuse-ld=lld"
WORKDIR /workspace
CMD ["sleep", "infinity"]
