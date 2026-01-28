#!/bin/bash

set -e

# Parse command line arguments
BUILD_MODE="release"
if [ "$1" = "debug" ] || [ "$1" = "--debug" ]; then
    BUILD_MODE="debug"
fi

if [ "$BUILD_MODE" = "debug" ]; then
    echo "Building htknow for debug..."
    cargo build
    IMAGE_TAG="$(date +%Y%m%d%H%M)-debug"
    BINARY_PATH="target/debug/htknow"
else
    echo "Building htknow for release..."
    cargo build --release
    IMAGE_TAG="$(date +%Y%m%d%H%M)"
    BINARY_PATH="target/release/htknow"
fi

# Create a temporary Dockerfile with the correct binary path
echo "Building Docker image..."
sed "s|target/release/htknow|${BINARY_PATH}|g" Dockerfile > Dockerfile.tmp
docker build -f Dockerfile.tmp -t htknow:${IMAGE_TAG} .
rm -f Dockerfile.tmp

echo "Build complete!"
echo ""
echo "Image tag: htknow:${IMAGE_TAG}"
echo "Build mode: ${BUILD_MODE}"
echo ""
echo "To run the container:"
echo "  docker-compose up -d"
echo ""
echo "Or manually:"
echo "  docker run -d -p 8080:8080 -v \$(pwd)/data:/app/data --name htknow htknow:${IMAGE_TAG}"
echo ""
echo "Usage:"
echo "  ./build-docker.sh          # Build in release mode (default)"
echo "  ./build-docker.sh debug    # Build in debug mode"
