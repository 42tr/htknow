# Runtime image for htknow
# Binary should be compiled outside the container using: cargo build --release

FROM docker.1ms.run/debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    # SSL certificates for HTTPS requests
    ca-certificates \
    # Timezone data
    tzdata \
    # sqlite3
    sqlite3 \
    # LibreOffice runtime libraries
    libxinerama1 \
    libx11-6 \
    libxext6 \
    libxrender1 \
    libxrandr2 \
    libxcb1 \
    libxau6 \
    libxdmcp6 \
    libxfixes3 \
    libxcomposite1 \
    libxdamage1 \
    libxshmfence1 \
    libfontconfig1 \
    libfreetype6 \
    libcairo2 \
    libglib2.0-0 \
    libcups2 \
    libnss3 \
    libsm6 \
    libice6 \
    libgl1 \
    fonts-dejavu \
    fonts-noto \
    fontconfig \
    # Tools for installing LibreOffice from tarball
    tar \
    && rm -rf /var/lib/apt/lists/*

# Install LibreOffice from official tarball
ARG LO_VERSION=25.8.4
ARG LO_TARBALL=LibreOffice_25.8.4_Linux_x86-64_deb.tar.gz
COPY ${LO_TARBALL} /tmp/
RUN LO_DIR="$(tar -tzf /tmp/${LO_TARBALL} | head -1 | cut -d/ -f1)" \
    && tar -xzf /tmp/${LO_TARBALL} -C /tmp \
    && dpkg -i /tmp/${LO_DIR}/DEBS/*.deb \
    && rm -rf /tmp/${LO_TARBALL} /tmp/${LO_DIR}

# Ensure `soffice` is available on PATH
RUN ln -sf /opt/libreoffice25.8/program/soffice /usr/bin/soffice

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

# Copy frontend files if needed
COPY frontend /app/frontend

# Set permissions
RUN chmod +x /app/htknow

# Expose the application port (adjust based on your app's configuration)
EXPOSE 8080

# Set environment variables
ENV RUST_LOG=info

# Run the application
CMD ["/app/htknow"]
