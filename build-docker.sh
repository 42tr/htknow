#!/bin/bash

set -e

# Parse command line arguments
BUILD_MODE="release"
PROFILING_FLAG=""
CUSTOM_IMAGE_TAG="${IMAGE_TAG:-}"
if [ "${1:-}" = "debug" ] || [ "${1:-}" = "--debug" ]; then
    BUILD_MODE="debug"
    PROFILING_FLAG="--features profiling"
fi

# Build frontend
cd frontend
npm install
npm run build
cd ..

if [ "$BUILD_MODE" = "debug" ]; then
    echo "Building htknow for debug with profiling..."
    cargo build ${PROFILING_FLAG}
    if [ -n "$CUSTOM_IMAGE_TAG" ]; then
        IMAGE_TAG="${CUSTOM_IMAGE_TAG}-debug"
    else
        IMAGE_TAG="$(date +%Y%m%d%H%M)-debug"
    fi
    DOCKERFILE="Dockerfile.debug"
    echo "Using Dockerfile.debug with jemalloc profiling tools"
else
    echo "Building htknow for release (profiling disabled)..."
    cargo build --release
    if [ -n "$CUSTOM_IMAGE_TAG" ]; then
        IMAGE_TAG="${CUSTOM_IMAGE_TAG}"
    else
        IMAGE_TAG="$(date +%Y%m%d%H%M)"
    fi
    DOCKERFILE="Dockerfile"
fi

# Build Docker image
echo "Building Docker image with ${DOCKERFILE}..."
docker build -f "${DOCKERFILE}" -t "htknow:${IMAGE_TAG}" -t htknow:latest .

echo "Build complete!"
echo ""
echo "Image tag: htknow:${IMAGE_TAG}"
echo "Latest tag: htknow:latest"
echo "BUILT_IMAGE=htknow:${IMAGE_TAG}"
echo "BUILT_LATEST_IMAGE=htknow:latest"
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
echo "  IMAGE_TAG=v1.2.3 ./build-docker.sh  # Use custom image tag"
