# Runtime image for htknow
# Binary should be compiled outside the container using: cargo build --release

FROM docker.hjkl01.cn/debian:bookworm-20260406-slim

ARG DEBIAN_FRONTEND=noninteractive
ARG APT_MIRROR=http://mirrors.aliyun.com

COPY docker/apt-fast /usr/local/sbin/apt-fast

# Switch Debian APT to Aliyun mirror and bootstrap vendored apt-fast.
RUN set -eux; \
    for sources in /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list; do \
        if [ -f "${sources}" ]; then \
            sed -i \
                -e "s|http://deb.debian.org/debian|${APT_MIRROR}/debian|g" \
                -e "s|https://deb.debian.org/debian|${APT_MIRROR}/debian|g" \
                -e "s|http://deb.debian.org/debian-security|${APT_MIRROR}/debian-security|g" \
                -e "s|https://deb.debian.org/debian-security|${APT_MIRROR}/debian-security|g" \
                -e "s|http://security.debian.org/debian-security|${APT_MIRROR}/debian-security|g" \
                -e "s|https://security.debian.org/debian-security|${APT_MIRROR}/debian-security|g" \
                "${sources}"; \
        fi; \
    done; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates aria2; \
    chmod +x /usr/local/sbin/apt-fast; \
    printf '%s\n' \
        '_APTMGR=apt-get' \
        'DOWNLOADBEFORE=true' \
        '_MAXNUM=8' \
        '_MAXCONPERSRV=8' \
        '_SPLITCON=8' \
        "MIRRORS=(" \
        "  '${APT_MIRROR}/debian,http://mirrors.tuna.tsinghua.edu.cn/debian,http://mirrors.ustc.edu.cn/debian'" \
        "  '${APT_MIRROR}/debian-security,http://mirrors.tuna.tsinghua.edu.cn/debian-security,http://mirrors.ustc.edu.cn/debian-security'" \
        ')' \
        > /etc/apt-fast.conf; \
    rm -rf /var/lib/apt/lists/*

# Install runtime dependencies
RUN apt-fast update && apt-fast install -y \
    # Timezone data
    tzdata \
    # sqlite3
    sqlite3 \
    && rm -rf /var/lib/apt/lists/*

# Configure timezone
ENV TZ=Asia/Shanghai
RUN ln -snf /usr/share/zoneinfo/${TZ} /etc/localtime && echo ${TZ} > /etc/timezone

# Create application directory
WORKDIR /app

# Create data directories
RUN mkdir -p /app/data/images /app/data/temp /app/data/db

# Copy the compiled binary from host
# Make sure to build with: cargo build --release
COPY target/release/htknow /app/htknow

# Set permissions
RUN chmod +x /app/htknow

# Expose the application port (adjust based on your app's configuration)
EXPOSE 8080

# Set environment variables
ENV RUST_LOG=info

# Run the application
CMD ["/app/htknow"]
