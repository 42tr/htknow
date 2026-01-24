#!/bin/bash

set -e

echo "Building htknow for release..."
cargo build --release

echo "Building Docker image..."
IMAGE_TAG="$(date +%Y%m%d%H%M)"
docker build -t htknow:${IMAGE_TAG} .

echo "Build complete!"
echo ""
echo "To run the container:"
echo "  docker-compose up -d"
echo ""
echo "Or manually:"
echo "  docker run -d -p 8080:8080 -v \$(pwd)/data:/app/data --name htknow htknow:${IMAGE_TAG}"
