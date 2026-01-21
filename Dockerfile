# Runtime image for htknow
# Binary should be compiled outside the container using: cargo build --release

FROM docker.1ms.run/debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    # LibreOffice for Word to PDF conversion
    libreoffice-writer \
    libreoffice-core \
    # SSL certificates for HTTPS requests
    ca-certificates \
    # Timezone data
    tzdata \
    # Clean up
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
